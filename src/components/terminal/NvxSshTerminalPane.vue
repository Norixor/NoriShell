<script setup lang="ts">
import { KeyRound, RotateCcw, Server, Unplug } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import {
  attachSshSession,
  detachSshSession,
  disconnectSshSession,
  confirmLoginAutomation,
  getHostConnectionConfig,
  getSshSession,
  heartbeatSshAttachment,
  openSshSession,
  parseCoreApiError,
  reconnectSshSession,
  renewSshInputLease,
  resizeSshTerminal,
  sendSshInput,
  takeoverSshLoginAutomation,
} from "../../core-api/client";
import { requestSecureSshChallenge } from "../../core-api/secure-ssh-challenge-client";
import type {
  SshHostKeyChallenge,
  SshKeyboardInteractiveChallenge,
  SshLoginAutomationProgress,
  SshSessionAttachment,
  SshSessionEvent,
  SshSessionFailureReason,
  SshSessionHeartbeatStatus,
  SshSessionInputLease,
  SshSessionOutputItem,
  SshSessionState,
  SshSessionSummary,
  SshSessionTarget,
  LoginAutomationSummary,
  TerminalInputLease,
} from "../../core-api/generated/core-api";
import {
  focusTerminalInputTarget,
  isTerminalInputTargetFocused,
  registerTerminalInputTarget,
  takeoverTerminalInputTarget,
} from "../../terminal-input-target";
import { NvxPluginExtensionTarget } from "../plugins";
import NvxHostMarker from "../hosts/NvxHostMarker.vue";
import { useHostMarkersStore } from "../../stores/hostMarkers";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxDialog, NvxIcon, NvxInlineNotice, NvxStatusLabel } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import NvxTerminalPaneControls from "./NvxTerminalPaneControls.vue";
import NvxTerminalPaneOverflowMenu from "./NvxTerminalPaneOverflowMenu.vue";
import NvxTerminalTools from "./NvxTerminalTools.vue";
import NvxTerminalView from "./NvxTerminalView.vue";
import { createFencedTerminalResize } from "./fencedTerminalResize";
import NvxNativeTerminalTools from "./NvxNativeTerminalTools.vue";
import { recordNativeTerminalHistory } from "../../core-api/native-terminal";
import { useNativeTerminalStore } from "../../stores/nativeTerminal";
import { posixDirectoryCommand } from "../../views/sftpTerminalLaunch";
import type { NativeTerminalSessionScope } from "../../core-api/generated/core-api";
import type { ShortcutCommandId } from "../../shortcuts";

interface TerminalViewExpose {
  writeBytes(bytes: readonly number[]): void;
  writeGap(): void;
  dimensions(): { rows: number; cols: number };
  focus(): void;
  findNext(term: string, incremental?: boolean): boolean;
  findPrevious(term: string): boolean;
  clearSearch(): void;
  selection(): string;
  clear(): void;
  pasteFromClipboard(): Promise<void> | undefined;
  currentDraft(): string | null;
  invalidateDraft(): void;
}

interface TerminalToolsExpose {
  openSearch(): void;
  copySelection(): Promise<void>;
}

const props = withDefaults(defineProps<{
  paneId: string;
  label: string;
  target: SshSessionTarget;
  credentialRefId: string | null;
  pluginAuthorizationToken?: string | null;
  existingSession: SshSessionSummary | null;
  deferredStart?: boolean;
  deferredRecovery?: "reconnect" | "credential" | "vaultUnlock";
  active: boolean;
  visible: boolean;
  initialDirectory?: string | null;
  canSplitHorizontal: boolean;
  canSplitVertical: boolean;
  canSplitWorkspaceRight: boolean;
}>(), {
  deferredStart: false,
  deferredRecovery: "reconnect",
  pluginAuthorizationToken: null,
  initialDirectory: null,
});

const emit = defineEmits<{
  activate: [paneId: string];
  state: [state: SshSessionState, summary: SshSessionSummary | null];
  bellAttention: [active: boolean];
  requestCredential: [paneId: string, target: SshSessionTarget];
  requestAuthenticationRecovery: [paneId: string, target: SshSessionTarget];
  requestVaultUnlock: [paneId: string, target: SshSessionTarget];
  initialDirectoryHandled: [directory: string];
  split: [direction: "horizontal" | "vertical"];
  splitWorkspaceRight: [];
  close: [];
}>();

const { t, te } = useI18n();
const hostMarkers = useHostMarkersStore();
const terminalPreferences = useTerminalPreferencesStore();
const tips = useTipsStore();
const nativeHistory = useNativeTerminalStore();
const nativeTools = ref<InstanceType<typeof NvxNativeTerminalTools> | null>(null);
const inputDraft = ref<string | null>(null);
const ghostSuggestion = ref<{ draft: string; suffix: string } | null>(null);
const pluginAuthorizationToken = ref(props.pluginAuthorizationToken);
// paneId is already a UUIDv7 and is the stable renderer view identity. Keeping
// it across component HMR lets a fresh attach attempt atomically replace only
// this Pane's stale Channel.
const viewId = props.paneId;
const terminalView = ref<TerminalViewExpose | null>(null);
const terminalTools = ref<TerminalToolsExpose | null>(null);
const hasSelection = ref(false);
const session = ref<SshSessionSummary | null>(props.existingSession);
const pluginContextKey = computed(() => [
  props.target.kind === "host" ? props.target.hostId : "",
  props.paneId,
  "ssh",
  session.value?.sessionId ?? "none",
  session.value?.generation ?? "0",
].join("|"));
const attachment = ref<SshSessionAttachment | null>(null);
const lease = ref<SshSessionInputLease | null>(null);
const challenge = ref<SshHostKeyChallenge | null>(null);
const keyboardInteractiveChallenge = ref<SshKeyboardInteractiveChallenge | null>(null);
const loginAutomationProgress = ref<SshLoginAutomationProgress | null>(null);
const loginAutomationTakingOver = ref(false);
const loginAutomationTakeoverFailed = ref(false);
const heartbeat = ref<SshSessionHeartbeatStatus>({
  policyRevision: null,
  mode: "disabled",
  transports: [],
  shell: null,
});
const {
  rootRef: latencyRootRef,
  triggerRef: latencyTriggerRef,
  viewportPanelRef: latencyPanelRef,
  viewportPanelStyle: latencyPanelStyle,
  open: latencyOpen,
  closeMenu: closeLatency,
  toggleMenu: toggleLatency,
} = usePopoverMenu();
const failure = ref<SshSessionFailureReason | null>(null);
const opening = ref(false);
const openFailed = ref(false);
const disconnecting = ref(false);
const reconnecting = ref(false);
const reconnectErrorKey = ref<string | null>(null);
const pendingAutomationConfirmation = ref<LoginAutomationSummary | null>(null);
const pendingAutomationCredentialRefId = ref<string | null>(null);
const pendingAutomationReconnect = ref(false);
const automationConfirmationBusy = ref(false);
const automationConfirmationFailed = ref(false);
const automationConfirmationOpen = computed({
  get: () => pendingAutomationConfirmation.value !== null,
  set: (open: boolean) => {
    if (!open && !automationConfirmationBusy.value) pendingAutomationConfirmation.value = null;
  },
});
let inputSequence = 0n;
let leaseTimer: number | null = null;
let attachmentHeartbeatTimer: number | null = null;
let unregisterInputTarget: (() => void) | null = null;
const pendingEvents: SshSessionEvent[] = [];
const pendingEventGaps = new Set<string>();
const PENDING_EVENT_MAX_COUNT = 256;
const PENDING_EVENT_MAX_BYTES = 1024 * 1024;
let pendingEventBytes = 0;
let binding = false;
let released = false;
let lastAppliedEventSeq = 0n;
let lastAppliedOutputSeq = 0n;
let foregroundReconcilePromise: Promise<void> | null = null;

const fencedResize = createFencedTerminalResize((dimensions) => {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!props.visible || !current || !currentAttachment?.channelId || current.state !== "running") return null;
  const channelId = currentAttachment.channelId;
  return {
    key: [
      current.sessionId,
      current.generation,
      currentAttachment.attachmentId,
      dimensions.rows,
      dimensions.cols,
    ].join(":"),
    send: (resizeSeq: string) => resizeSshTerminal({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      channelId,
      attachmentId: currentAttachment.attachmentId,
      viewId,
      resizeSeq,
      rows: dimensions.rows,
      cols: dimensions.cols,
    }),
  };
});
const resize = fencedResize.resize;
const flushPendingResize = fencedResize.flush;

function reassertCurrentResize() {
  if (!props.visible) return;
  const size = terminalView.value?.dimensions();
  if (size) fencedResize.reassert(size.rows, size.cols);
}

const state = computed<SshSessionState>(
  () => session.value?.state
    ?? (openFailed.value ? "failed" : props.deferredStart ? "closed" : "resolving"),
);
const ownsLease = computed(() => {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  return current !== null
    && currentAttachment !== null
    && currentLease !== null
    && currentLease.sessionId === current.sessionId
    && currentLease.generation === current.generation
    && currentLease.attachmentId === currentAttachment.attachmentId
    && currentLease.viewId === viewId
    && currentLease.expiresAtUnixMs > Date.now();
});
const inputReady = computed(() => props.active
  && state.value === "running"
  && ownsLease.value
  && isTerminalInputTargetFocused(props.paneId, lease.value?.focusEpoch ?? null));
const writable = computed(() => inputReady.value && !props.initialDirectory);
const nativeScope = computed<NativeTerminalSessionScope | null>(() => session.value && attachment.value?.channelId ? {
  kind: "ssh", sessionId: session.value.sessionId, generation: session.value.generation,
  channelId: attachment.value.channelId, paneId: viewId,
} : null);

function runShortcut(commandId: ShortcutCommandId) {
  if (!props.active) return;
  if (commandId === "terminal.search") openSearch();
  else if (commandId === "terminal.copy") copySelection();
  else if (commandId === "terminal.paste") void terminalView.value?.pasteFromClipboard();
  else if (commandId === "terminal.clear") terminalView.value?.clear();
  else if (commandId === "terminal.history-suggestions") void nativeTools.value?.openHistory();
  else if (commandId === "terminal.reconnect" && ["closed", "failed"].includes(state.value)) void reconnect();
  else if (commandId === "terminal.disconnect" && !["closed", "failed"].includes(state.value)) void disconnect();
}
const stateLabel = computed(() => t(`sshSession.states.${state.value}`));
const endpointLabel = computed(() => {
  const endpoint = session.value?.endpoint;
  if (endpoint) return `${endpoint.address}:${endpoint.port}`;
  return props.target.kind === "quickConnect"
    ? `${props.target.endpoint.address}:${props.target.endpoint.port}`
    : props.label;
});
const failureMessage = computed(() => {
  if (!failure.value) return null;
  const message = te(failure.value.messageKey)
    ? t(failure.value.messageKey)
    : t("sshSession.failureFallback");
  const negotiation = failure.value.algorithmNegotiation;
  if (negotiation) {
    const categoryKey = {
      keyExchange: "sshHosts.algorithms.categoryKeyExchange",
      hostKey: "sshHosts.algorithms.categoryHostKey",
      cipher: "sshHosts.algorithms.categoryCipher",
      mac: "sshHosts.algorithms.categoryMac",
    }[negotiation.category];
    const unavailable = t("sshSession.algorithms.noCandidates");
    return `${message} ${t("sshSession.algorithms.negotiationFailureDetails", {
      category: t(categoryKey),
      clientCandidates: negotiation.clientCandidates.join(", ") || unavailable,
      serverCandidates: negotiation.serverCandidates.join(", ") || unavailable,
    })}`;
  }
  const routeStage = failure.value.routeStage;
  if (routeStage?.kind !== "jumpHost") return message;
  return `${t("sshSession.routeJumpContext", {
    hop: routeStage.hopIndex + 1,
    endpoint: `${routeStage.endpoint.address}:${routeStage.endpoint.port}`,
  })} ${message}`;
});
const loginAutomationTitle = computed(() => {
  const progress = loginAutomationProgress.value;
  if (!progress) return null;
  if (progress.status === "failed") {
    return t(`sshSession.loginAutomation.failures.${progress.failureCode ?? "channelUnavailable"}`);
  }
  return t("sshSession.loginAutomation.running", {
    current: progress.currentStepIndex + 1,
    total: progress.totalSteps,
  });
});
const heartbeatLabel = computed(() => {
  if (heartbeat.value.mode === "disabled") return null;
  if (heartbeat.value.mode === "transportKeepalive") {
    if (session.value?.state !== "running" && session.value?.state !== "automatingLogin") {
      return null;
    }
    const failures = heartbeat.value.transports.reduce(
      (total, transport) => total + transport.consecutiveFailures,
      0,
    );
    return failures > 0
      ? t("sshSession.heartbeat.transportFailures", { count: failures })
      : t("sshSession.heartbeat.transportActive", { count: heartbeat.value.transports.length });
  }
  if (session.value?.state !== "running") return null;
  const skipReason = heartbeat.value.shell?.skipReason;
  return skipReason
    ? t(`sshSession.heartbeat.skips.${skipReason}`)
    : t("sshSession.heartbeat.shellActive");
});
const heartbeatTitle = computed(() => {
  if (heartbeat.value.mode === "transportKeepalive") {
    const lastAck = Math.max(
      ...heartbeat.value.transports.map((status) => status.lastAckAtUnixMs ?? 0),
    );
    return lastAck > 0
      ? t("sshSession.heartbeat.lastAck", { time: new Date(lastAck).toLocaleTimeString() })
      : t("sshSession.heartbeat.awaitingAck");
  }
  const lastSent = heartbeat.value.shell?.lastSentAtUnixMs;
  return lastSent
    ? t("sshSession.heartbeat.lastSent", { time: new Date(lastSent).toLocaleTimeString() })
    : t("sshSession.heartbeat.awaitingSend");
});
function transportLatency(status: SshSessionHeartbeatStatus["transports"][number] | undefined) {
  if (!status || status.consecutiveFailures > 0) return null;
  const sent = status.lastSentAtUnixMs;
  const ack = status.lastAckAtUnixMs;
  return sent !== null && ack !== null && ack >= sent ? ack - sent : null;
}
const latencyRows = computed(() => {
  const hops = new Map<number, { endpoint: string; latency: number | null }>();
  for (const result of session.value?.negotiatedAlgorithms ?? []) {
    if (result.routeStage.kind !== "jumpHost") continue;
    const { hopIndex, endpoint } = result.routeStage;
    hops.set(hopIndex, { endpoint: `${endpoint.address}:${endpoint.port}`, latency: null });
  }
  for (const status of heartbeat.value.transports) {
    if (status.routeStage.kind !== "jumpHost") continue;
    const { hopIndex, endpoint } = status.routeStage;
    hops.set(hopIndex, { endpoint: `${endpoint.address}:${endpoint.port}`, latency: transportLatency(status) });
  }
  const rows = [...hops.entries()]
    .sort(([left], [right]) => left - right)
    .map(([hopIndex, value]) => ({ key: `jump-${hopIndex}`, label: t("sshSession.heartbeat.jumpHost", { hop: hopIndex + 1 }), ...value }));
  rows.push({
    key: "target",
    label: t("sshSession.heartbeat.target"),
    endpoint: session.value?.endpoint ? `${session.value.endpoint.address}:${session.value.endpoint.port}` : "",
    latency: transportLatency(heartbeat.value.transports.find((status) => status.routeStage.kind === "target")),
  });
  return rows;
});
const latencyMs = computed(() => latencyRows.value.at(-1)?.latency ?? null);
function latencyText(value: number | null) {
  return value === null ? "—" : `${value} ms`;
}
function closeLatencyOnBlur(event: FocusEvent) {
  if (!(event.relatedTarget instanceof Node) || !(event.currentTarget instanceof Node)
    || !event.currentTarget.contains(event.relatedTarget)) closeLatency();
}
watch(() => [state.value, session.value?.generation], () => closeLatency());
const stateDisplayLabel = computed(() => {
  if (state.value !== "running") return stateLabel.value;
  return latencyMs.value === null ? "— ms" : `${latencyMs.value}ms`;
});
const stateTitle = computed(() => {
  if (state.value !== "running") return stateLabel.value;
  const details = [stateLabel.value];
  if (heartbeatLabel.value) details.push(heartbeatLabel.value, heartbeatTitle.value);
  if (latencyMs.value === null) details.push(t("sshSession.heartbeat.latencyUnavailable"));
  return details.join(" · ");
});

function openSearch() {
  terminalTools.value?.openSearch();
}

function copySelection() {
  void terminalTools.value?.copySelection();
}
const reconnectErrorMessage = computed(() => {
  if (!reconnectErrorKey.value) return null;
  return te(reconnectErrorKey.value)
    ? t(reconnectErrorKey.value)
    : t("sshSession.reconnectFailed");
});
const needsCredentialInput = computed(
  () => failure.value?.retryStrategy === "chooseCredential"
    || (props.deferredStart
      && session.value === null
      && props.deferredRecovery === "credential"
      && props.credentialRefId === null),
);
const needsVaultUnlock = computed(
  () => failure.value?.retryStrategy === "unlockVault"
    || (props.deferredStart
      && session.value === null
      && props.deferredRecovery === "vaultUnlock"),
);
const sessionActionReconnects = computed(() => ["closed", "failed"].includes(state.value));
const reconnectOnInput = computed(() => terminalPreferences.preferences.interaction.sshReconnectOnInput
  && props.active && sessionActionReconnects.value
  && !opening.value && !reconnecting.value && !disconnecting.value);
let inputReconnectPending = false;

async function requestReconnectFromInput() {
  if (!reconnectOnInput.value || inputReconnectPending || released
    || document.querySelector('[role="dialog"][aria-modal="true"]')) return;
  inputReconnectPending = true;
  try {
    await reconnect();
  } finally {
    inputReconnectPending = false;
  }
}
const sessionActionLabel = computed(() => {
  if (!sessionActionReconnects.value) return t("sshSession.disconnect");
  if (needsVaultUnlock.value) return t("sshSession.unlockVault");
  return needsCredentialInput.value
    ? t("sshSession.enterCredential")
    : t("sshSession.reconnect");
});
const sessionActionIcon = computed(() => {
  if (!sessionActionReconnects.value) return Unplug;
  return needsCredentialInput.value || needsVaultUnlock.value ? KeyRound : RotateCcw;
});

function runSessionAction() {
  if (sessionActionReconnects.value) void reconnect();
  else void disconnect();
}

function updateSummary(next: SshSessionSummary) {
  session.value = next;
  failure.value = next.failureReason;
  emit("state", next.state, next);
}

async function reconcileAfterForegroundOnce() {
  const current = session.value;
  if (!current || opening.value || reconnecting.value || disconnecting.value) return;
  let details: Awaited<ReturnType<typeof getSshSession>>;
  try {
    details = await getSshSession(current.sessionId);
  } catch {
    return;
  }

  const generationChanged = details.session.generation !== current.generation;
  const currentAttachment = details.attachments.find(
    (candidate) => candidate.viewId === viewId,
  ) ?? null;
  challenge.value = details.activeHostKeyChallenge;
  setKeyboardInteractiveChallenge(details.activeKeyboardInteractiveChallenge);
  loginAutomationProgress.value = details.activeLoginAutomation;
  heartbeat.value = details.heartbeat;
  attachment.value = currentAttachment;
  applyFocusLease(
    details.inputLease?.viewId === viewId
      ? { kind: "ssh", lease: details.inputLease }
      : null,
  );
  updateSummary(details.session);

  if (!currentAttachment || generationChanged) {
    await attachExistingSession(!generationChanged);
    return;
  }
  if (details.session.state === "running" && props.active) activateFromTab();
}

function reconcileAfterForeground() {
  foregroundReconcilePromise ??= reconcileAfterForegroundOnce().finally(() => {
    foregroundReconcilePromise = null;
  });
  return foregroundReconcilePromise;
}

function setKeyboardInteractiveChallenge(next: SshKeyboardInteractiveChallenge | null) {
  keyboardInteractiveChallenge.value = next;
}

function pendingEventKey(event: SshSessionEvent) {
  return `${event.sessionId}:${event.generation}`;
}

function pendingEventSize(event: SshSessionEvent) {
  return event.payload.kind === "outputFrame"
    ? event.payload.frame.bytes.length
    : 256;
}

function bufferPendingEvent(event: SshSessionEvent) {
  pendingEvents.push(event);
  pendingEventBytes += pendingEventSize(event);
  while (
    pendingEvents.length > PENDING_EVENT_MAX_COUNT
    || pendingEventBytes > PENDING_EVENT_MAX_BYTES
  ) {
    const dropped = pendingEvents.shift();
    if (!dropped) break;
    pendingEventBytes = Math.max(0, pendingEventBytes - pendingEventSize(dropped));
    pendingEventGaps.add(pendingEventKey(dropped));
  }
}

function clearPendingEvents() {
  pendingEvents.splice(0);
  pendingEventBytes = 0;
  pendingEventGaps.clear();
}

function applyOutputItem(item: SshSessionOutputItem) {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment?.channelId) return;
  if (item.kind === "frame") {
    const frame = item.payload;
    if (
      frame.sessionId !== current.sessionId
      || frame.generation !== current.generation
      || frame.channelId !== currentAttachment.channelId
    ) return;
    const outputSeq = BigInt(frame.outputSeq);
    if (outputSeq <= lastAppliedOutputSeq) return;
    if (outputSeq > lastAppliedOutputSeq + 1n && lastAppliedOutputSeq > 0n) {
      terminalView.value?.writeGap();
    }
    terminalView.value?.writeBytes(frame.bytes);
    lastAppliedOutputSeq = outputSeq;
    return;
  }
  const gap = item.payload;
  if (
    gap.sessionId !== current.sessionId
    || gap.generation !== current.generation
    || gap.channelId !== currentAttachment.channelId
  ) return;
  const resumesAt = BigInt(gap.resumesAtOutputSeq);
  if (resumesAt <= lastAppliedOutputSeq) return;
  terminalView.value?.writeGap();
  lastAppliedOutputSeq = resumesAt - 1n;
}

function applyEvent(event: SshSessionEvent) {
  if (!session.value) {
    bufferPendingEvent(event);
    return;
  }
  if (session.value && event.sessionId !== session.value.sessionId) return;
  if (session.value && event.generation !== session.value.generation) {
    if (
      reconnecting.value
      && BigInt(event.generation) > BigInt(session.value.generation)
    ) {
      bufferPendingEvent(event);
    }
    return;
  }
  if (binding) {
    bufferPendingEvent(event);
    return;
  }
  const eventSeq = BigInt(event.eventSeq);
  if (eventSeq <= lastAppliedEventSeq) return;
  lastAppliedEventSeq = eventSeq;
  switch (event.payload.kind) {
    case "stateChanged":
      if (session.value) {
        updateSummary({
          ...session.value,
          state: event.payload.state,
          stateRevision: event.stateRevision,
          eventSeq: event.eventSeq,
          closeReason: event.payload.closeReason,
          failureReason: event.payload.failureReason,
          updatedAtUnixMs: event.occurredAtUnixMs,
        });
      }
      if (event.payload.state === "running" && props.active) activateFromTab();
      break;
    case "hostKeyChallenge":
      challenge.value = event.payload.challenge;
      break;
    case "keyboardInteractiveChallengeChanged":
      updateSummary({
        ...session.value,
        stateRevision: event.stateRevision,
        eventSeq: event.eventSeq,
        updatedAtUnixMs: event.occurredAtUnixMs,
      });
      setKeyboardInteractiveChallenge(event.payload.challenge);
      break;
    case "loginAutomationProgressChanged":
      updateSummary({
        ...session.value,
        stateRevision: event.stateRevision,
        eventSeq: event.eventSeq,
        updatedAtUnixMs: event.occurredAtUnixMs,
      });
      loginAutomationProgress.value = event.payload.progress;
      loginAutomationTakeoverFailed.value = false;
      break;
    case "negotiatedAlgorithmsChanged":
      updateSummary({
        ...session.value,
        negotiatedAlgorithms: event.payload.algorithms,
        eventSeq: event.eventSeq,
        updatedAtUnixMs: event.occurredAtUnixMs,
      });
      break;
    case "heartbeatChanged":
      heartbeat.value = event.payload.heartbeat;
      break;
    case "inputLeaseChanged":
      applyFocusLease(event.payload.lease ? { kind: "ssh", lease: event.payload.lease } : null);
      break;
    case "outputFrame":
      applyOutputItem({ kind: "frame", payload: event.payload.frame });
      break;
    case "outputGap":
      applyOutputItem({ kind: "gap", payload: event.payload.gap });
      break;
    case "attachmentChanged":
      if (
        event.payload.change === "attached"
        && event.payload.attachment.viewId === viewId
      ) {
        attachment.value = event.payload.attachment;
        if (
          session.value
          && event.payload.attachment.sessionId === session.value.sessionId
          && event.payload.attachment.generation === session.value.generation
        ) {
          session.value = {
            ...session.value,
            channelId: event.payload.attachment.channelId,
            attachmentRevision: event.payload.attachmentRevision,
          };
        }
      } else if (
        event.payload.change === "detached"
        && event.payload.attachment.attachmentId === attachment.value?.attachmentId
      ) {
        attachment.value = null;
        lease.value = null;
      }
      break;
  }
}

function replayPendingEvents(sessionId: string, generation: string) {
  const key = `${sessionId}:${generation}`;
  const matching = pendingEvents
    .splice(0)
    .filter((event) => event.sessionId === sessionId && event.generation === generation)
    .sort((left, right) => {
      const leftSeq = BigInt(left.eventSeq);
      const rightSeq = BigInt(right.eventSeq);
      return leftSeq < rightSeq ? -1 : leftSeq > rightSeq ? 1 : 0;
    });
  pendingEventBytes = 0;
  if (pendingEventGaps.has(key)) terminalView.value?.writeGap();
  pendingEventGaps.clear();
  matching.forEach(applyEvent);
}

async function open(credentialRefId = props.credentialRefId) {
  if (opening.value) return;
  opening.value = true;
  openFailed.value = false;
  reconnectErrorKey.value = null;
  binding = true;
  released = false;
  lastAppliedEventSeq = 0n;
  lastAppliedOutputSeq = 0n;
  try {
    await nextTick();
    const size = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
    const authorizationToken = pluginAuthorizationToken.value;
    pluginAuthorizationToken.value = null;
    const response = await openSshSession({
      target: props.target,
      credentialRefId,
      pluginAuthorizationToken: authorizationToken,
      viewId,
      rows: size.rows,
      cols: size.cols,
    }, applyEvent);
    attachment.value = response.attachment;
    updateSummary(response.session);
    binding = false;
    replayPendingEvents(response.session.sessionId, response.session.generation);
    startAttachmentHeartbeat();
    if (response.session.state === "running" && props.active) activateFromTab();
    if (props.active) terminalView.value?.focus();
  } catch (error) {
    openFailed.value = true;
    const coreError = parseCoreApiError(error);
    if (coreError?.code === "ssh_terminal.login_automation_confirmation_required") {
      await showLoginAutomationConfirmation(credentialRefId, false);
    } else {
      reconnectErrorKey.value = coreError?.messageKey ?? "sshSession.reconnectFailed";
    }
    emit("state", "failed", session.value);
    if (coreError?.code === "ssh_terminal.credential_unavailable") {
      emit("requestAuthenticationRecovery", props.paneId, props.target);
    }
  } finally {
    binding = false;
    opening.value = false;
  }
}

async function showLoginAutomationConfirmation(credentialRefId: string | null, reconnectAttempt: boolean) {
  const target = session.value?.target ?? props.target;
  if (target.kind !== "host") return;
  try {
    const summary = (await getHostConnectionConfig(target.hostId)).loginAutomation;
    if (summary.hostId !== target.hostId
      || !summary.enabled
      || summary.confirmedRevision === summary.revision
      || summary.steps.length === 0) {
      reconnectErrorKey.value = "sshSession.loginAutomation.confirmationUnavailable";
      return;
    }
    pendingAutomationCredentialRefId.value = credentialRefId;
    pendingAutomationReconnect.value = reconnectAttempt;
    automationConfirmationFailed.value = false;
    reconnectErrorKey.value = null;
    pendingAutomationConfirmation.value = summary;
  } catch {
    reconnectErrorKey.value = "sshSession.loginAutomation.confirmationUnavailable";
  }
}

async function confirmPendingLoginAutomation() {
  const summary = pendingAutomationConfirmation.value;
  if (!summary || automationConfirmationBusy.value) return;
  automationConfirmationBusy.value = true;
  automationConfirmationFailed.value = false;
  try {
    await confirmLoginAutomation({ hostId: summary.hostId, expectedRevision: summary.revision });
    const credentialRefId = pendingAutomationCredentialRefId.value;
    const reconnectAttempt = pendingAutomationReconnect.value;
    pendingAutomationConfirmation.value = null;
    if (reconnectAttempt) await reconnect(credentialRefId, true);
    else await open(credentialRefId);
  } catch {
    automationConfirmationFailed.value = true;
  } finally {
    automationConfirmationBusy.value = false;
  }
}

async function attachExistingSession(resumeRenderedOutput = false) {
  if (opening.value || !session.value) return;
  const renderedGeneration = session.value.generation;
  const renderedOutputSeq = lastAppliedOutputSeq;
  opening.value = true;
  binding = true;
  released = false;
  clearPendingEvents();
  if (!resumeRenderedOutput) {
    lastAppliedEventSeq = 0n;
    lastAppliedOutputSeq = 0n;
  }
  try {
    let details = await getSshSession(session.value.sessionId);
    let response;
    let canResumeRenderedOutput = resumeRenderedOutput
      && details.session.generation === renderedGeneration;
    if (!canResumeRenderedOutput) {
      lastAppliedEventSeq = 0n;
      lastAppliedOutputSeq = 0n;
    }
    try {
      response = await attachSshSession({
        sessionId: details.session.sessionId,
        expectedGeneration: details.session.generation,
        expectedStateRevision: details.session.stateRevision,
        viewId,
        afterOutputSeq: canResumeRenderedOutput && renderedOutputSeq > 0n
          ? renderedOutputSeq.toString()
          : null,
      }, applyEvent);
    } catch {
      // A state transition between get and attach is harmless; refresh once
      // and issue a fresh operation/attempt rather than replaying a Channel.
      details = await getSshSession(session.value.sessionId);
      canResumeRenderedOutput = resumeRenderedOutput
        && details.session.generation === renderedGeneration;
      if (!canResumeRenderedOutput) {
        lastAppliedEventSeq = 0n;
        lastAppliedOutputSeq = 0n;
      }
      response = await attachSshSession({
        sessionId: details.session.sessionId,
        expectedGeneration: details.session.generation,
        expectedStateRevision: details.session.stateRevision,
        viewId,
        afterOutputSeq: canResumeRenderedOutput && renderedOutputSeq > 0n
          ? renderedOutputSeq.toString()
          : null,
      }, applyEvent);
    }
    attachment.value = response.attachment;
    const replacesExistingView = details.attachments.some(
      (candidate) => candidate.viewId === viewId,
    );
    updateSummary({
      ...details.session,
      stateRevision: response.stateRevision,
      attachmentRevision: response.attachmentRevision,
      attachmentCount: details.session.attachmentCount + (replacesExistingView ? 0 : 1),
    });
    challenge.value = details.activeHostKeyChallenge;
    setKeyboardInteractiveChallenge(details.activeKeyboardInteractiveChallenge);
    loginAutomationProgress.value = details.activeLoginAutomation;
    heartbeat.value = details.heartbeat;
    lease.value = null;
    // The attach response's replay and the already-registered live Channel are
    // one ordered stream. A later get snapshot may include eventSeq values for
    // live output that is still buffered here, so it must never advance the
    // renderer's consumed-event watermark.
    const snapshotEventSeq = BigInt(details.session.eventSeq);
    if (snapshotEventSeq > lastAppliedEventSeq) lastAppliedEventSeq = snapshotEventSeq;
    response.replay.forEach(applyOutputItem);
    binding = false;
    replayPendingEvents(details.session.sessionId, details.session.generation);
    startAttachmentHeartbeat();
    if (details.session.state === "running" && props.active) activateFromTab();
    if (props.active) terminalView.value?.focus();
  } catch {
    emit("state", "failed", session.value);
  } finally {
    binding = false;
    opening.value = false;
  }
}

function currentFocusTarget() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (
    !props.active
    || !current
    || current.state !== "running"
    || !currentAttachment?.channelId
  ) return null;
  return {
    kind: "ssh" as const,
    target: {
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      channelId: currentAttachment.channelId,
      attachmentId: currentAttachment.attachmentId,
      viewId,
    },
  };
}

function applyFocusLease(inputLease: TerminalInputLease | null) {
  const nextLease = inputLease?.kind === "ssh" ? inputLease.lease : null;
  const current = session.value;
  const currentAttachment = attachment.value;
  lease.value = nextLease
    && current
    && currentAttachment
    && nextLease.sessionId === current.sessionId
    && nextLease.generation === current.generation
    && nextLease.attachmentId === currentAttachment.attachmentId
    && nextLease.viewId === viewId
    ? nextLease
    : null;
  if (lease.value) startLeaseHeartbeat();
  else stopLeaseHeartbeat();
  if (props.visible) void flushPendingResize();
}

function startLeaseHeartbeat() {
  stopLeaseHeartbeat();
  leaseTimer = window.setInterval(() => void renewLease(), 5_000);
}

function stopLeaseHeartbeat() {
  if (leaseTimer !== null) window.clearInterval(leaseTimer);
  leaseTimer = null;
}

function startAttachmentHeartbeat() {
  stopAttachmentHeartbeat();
  attachmentHeartbeatTimer = window.setInterval(() => void heartbeatAttachment(), 10_000);
}

function stopAttachmentHeartbeat() {
  if (attachmentHeartbeatTimer !== null) window.clearInterval(attachmentHeartbeatTimer);
  attachmentHeartbeatTimer = null;
}

async function heartbeatAttachment() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment || binding || released) return;
  try {
    await heartbeatSshAttachment({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedAttachmentRevision: currentAttachment.attachmentRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId,
    });
  } catch {
    if (
      session.value?.sessionId === current.sessionId
      && session.value.generation === current.generation
      && attachment.value?.attachmentId === currentAttachment.attachmentId
    ) {
      stopAttachmentHeartbeat();
      stopLeaseHeartbeat();
      attachment.value = null;
      lease.value = null;
      void attachExistingSession(true);
    }
  }
}

async function renewLease() {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current || !currentAttachment || !currentLease || current.state !== "running") return;
  try {
    const renewed = await renewSshInputLease({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      attachmentId: currentAttachment.attachmentId,
      viewId,
      focusEpoch: currentLease.focusEpoch,
      leaseId: currentLease.leaseId,
      inputEpoch: currentLease.inputEpoch,
    });
    if (
      session.value?.sessionId === current.sessionId
      && session.value.generation === current.generation
      && attachment.value?.attachmentId === currentAttachment.attachmentId
      && lease.value?.leaseId === currentLease.leaseId
    ) {
      lease.value = renewed;
    }
  } catch {
    if (
      session.value?.sessionId === current.sessionId
      && session.value.generation === current.generation
      && attachment.value?.attachmentId === currentAttachment.attachmentId
      && lease.value?.leaseId === currentLease.leaseId
    ) {
      lease.value = null;
      stopLeaseHeartbeat();
      if (props.active) void recoverActiveInputLease();
    }
  }
}

async function send(value: string) {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current || !currentAttachment?.channelId || !currentLease || !inputReady.value) {
    throw new Error("SSH terminal input is unavailable");
  }
  inputSequence += 1n;
  // Network delay can deliver the prompt after typed-ahead input; do not mistake this line for empty input.
  await sendSshInput({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    channelId: currentAttachment.channelId,
    attachmentId: currentAttachment.attachmentId,
    viewId,
    focusEpoch: currentLease.focusEpoch,
    leaseId: currentLease.leaseId,
    inputEpoch: currentLease.inputEpoch,
    clientSeq: inputSequence.toString(),
    bytes: Array.from(new TextEncoder().encode(value)),
  });
}

async function handleTerminalInput(value: string, observedCommand?: string) {
  if (props.initialDirectory) return;
  const current = session.value;
  const attached = attachment.value;
  const inputLease = lease.value;
  const previousSequence = inputSequence;
  try {
    await send(value);
  } catch {
    // Input is non-idempotent: a rejected IPC result may still have reached
    // the remote Shell. Revoke this renderer's local write path and require an
    // explicit reconnect/focus recovery after the user checks terminal output.
    lease.value = null;
    stopLeaseHeartbeat();
    const current = session.value;
    tips.show({
      scope: `ssh-terminal-input:${current?.sessionId ?? props.paneId}:${current?.generation ?? "none"}`,
      tone: "error",
      title: t("quickCommands.runFailed"),
    });
    return;
  }
  if (!observedCommand || value !== "\r" || props.target.kind !== "host"
    || !current || !attached?.channelId || !inputLease || inputSequence !== previousSequence + 1n) return;
  try {
    const recorded = await recordNativeTerminalHistory({ command: observedCommand, inputFence: { kind: "ssh", payload: {
      sessionId: current.sessionId, expectedGeneration: current.generation,
      channelId: attached.channelId, attachmentId: attached.attachmentId, viewId,
      focusEpoch: inputLease.focusEpoch, leaseId: inputLease.leaseId,
      inputEpoch: inputLease.inputEpoch, clientSeq: inputSequence.toString(),
    } } });
    if (recorded) nativeHistory.historyChanged();
  } catch {
    /* Uncertain history capture must not change terminal input or retry its Enter. */
  }
}


let secureChallengePending = false;
const presentedChallenges = new Set<string>();
async function presentSecureChallenge() {
  const current = session.value;
  const owner = attachment.value;
  const hostKey = challenge.value;
  const keyboard = keyboardInteractiveChallenge.value;
  const currentChallenge = hostKey ?? keyboard;
  if (!current || !owner || !currentChallenge || secureChallengePending) return;
  const key = currentChallenge.challengeId;
  if (presentedChallenges.has(key)) return;
  presentedChallenges.add(key);
  secureChallengePending = true;
  try {
    const request = {
      sessionId: current.sessionId,
      expectedGeneration: currentChallenge.generation,
      challengeId: currentChallenge.challengeId,
      expectedStateRevision: currentChallenge.stateRevision,
      attachmentId: owner.attachmentId,
      viewId,
    };
    if (hostKey) await requestSecureSshChallenge({ kind: "sshHostKey", request });
    else if (keyboard) await requestSecureSshChallenge({ kind: "sshKeyboard", request: { ...request, roundIndex: keyboard.roundIndex } });
    if (session.value?.sessionId !== current.sessionId || session.value?.generation !== current.generation || attachment.value?.attachmentId !== owner.attachmentId) return;
    const details = await getSshSession(current.sessionId);
    challenge.value = details.activeHostKeyChallenge;
    setKeyboardInteractiveChallenge(details.activeKeyboardInteractiveChallenge);
    heartbeat.value = details.heartbeat;
    loginAutomationProgress.value = details.activeLoginAutomation;
    updateSummary(details.session);
  } catch (error) {
    // Expired or already owned challenges must not reopen or create a second approval surface.
    if (error !== "secureChallengeExpired" && error !== "secureChallengeAlreadyOpen") {
      reconnectErrorKey.value = "sshSession.keyboardInteractive.failed";
    }
  } finally {
    secureChallengePending = false;
    void presentSecureChallenge();
  }
}
watch([challenge, keyboardInteractiveChallenge, attachment], () => { void presentSecureChallenge(); });

async function performDisconnect(propagateFailure: boolean) {
  const current = session.value;
  if (!current || disconnecting.value) return;
  disconnecting.value = true;
  try {
    const details = await disconnectSshSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
    });
    setKeyboardInteractiveChallenge(details.activeKeyboardInteractiveChallenge);
    loginAutomationProgress.value = details.activeLoginAutomation;
    heartbeat.value = details.heartbeat;
    updateSummary(details.session);
    if (
      propagateFailure
      && !["disconnecting", "closed", "failed"].includes(details.session.state)
    ) {
      throw new Error("SSH disconnect was not accepted");
    }
  } catch (disconnectError) {
    // A stale fence or an IPC response loss does not prove that the Core-side
    // disconnect was rejected. Refresh the authoritative projection without
    // reissuing this non-idempotent user action; if Core is unreachable, keep
    // the current actionable state so the user can retry explicitly.
    let disconnectAccepted = false;
    try {
      const details = await getSshSession(current.sessionId);
      attachment.value = details.attachments.find(
        (candidate) => candidate.viewId === viewId,
      ) ?? null;
      lease.value = details.inputLease?.viewId === viewId
        ? details.inputLease
        : null;
      challenge.value = details.activeHostKeyChallenge;
      setKeyboardInteractiveChallenge(details.activeKeyboardInteractiveChallenge);
      loginAutomationProgress.value = details.activeLoginAutomation;
      heartbeat.value = details.heartbeat;
      updateSummary(details.session);
      disconnectAccepted = ["disconnecting", "closed", "failed"].includes(details.session.state);
    } catch {
      // Preserve the last acknowledged projection and re-enable the action.
    }
    if (propagateFailure && !disconnectAccepted) throw disconnectError;
  } finally {
    disconnecting.value = false;
  }
}

function disconnect() {
  return performDisconnect(false);
}

function disconnectForClose() {
  return performDisconnect(true);
}

async function reconnect(
  credentialRefId: string | null = null,
  vaultAlreadyUnlocked = false,
  propagateFailure = false,
) {
  const current = session.value;
  if (credentialRefId === null && needsVaultUnlock.value && !vaultAlreadyUnlocked) {
    emit("requestVaultUnlock", props.paneId, current?.target ?? props.target);
    return;
  }
  if (!current && credentialRefId === null && needsCredentialInput.value) {
    emit("requestCredential", props.paneId, props.target);
    return;
  }
  if (!current && props.deferredStart && !openFailed.value) {
    await open(credentialRefId);
    return;
  }
  if (!current && openFailed.value) {
    await open(credentialRefId);
    return;
  }
  if (
    !current
    || !attachment.value
    || reconnecting.value
    || !["closed", "failed"].includes(current.state)
  ) return;
  const currentAttachment = attachment.value;
  if (credentialRefId === null && needsCredentialInput.value) {
    emit("requestCredential", props.paneId, current.target);
    return;
  }

  reconnecting.value = true;
  reconnectErrorKey.value = null;
  clearPendingEvents();
  stopLeaseHeartbeat();
  lease.value = null;
  challenge.value = null;
  setKeyboardInteractiveChallenge(null);
  loginAutomationProgress.value = null;
  try {
    await nextTick();
    const size = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
    const details = await reconnectSshSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId,
      credentialRefId,
      rows: size.rows,
      cols: size.cols,
    });
    if (details.session.generation !== current.generation) {
      lastAppliedEventSeq = 0n;
      lastAppliedOutputSeq = 0n;
    }
    inputSequence = 0n;
    fencedResize.reset();
    attachment.value = details.attachments.find(
      (candidate) => candidate.viewId === viewId,
    ) ?? null;
    lease.value = details.inputLease?.viewId === viewId ? details.inputLease : null;
    challenge.value = details.activeHostKeyChallenge;
    setKeyboardInteractiveChallenge(details.activeKeyboardInteractiveChallenge);
    loginAutomationProgress.value = details.activeLoginAutomation;
    heartbeat.value = details.heartbeat;
    updateSummary(details.session);
    replayPendingEvents(details.session.sessionId, details.session.generation);
    if (session.value?.state === "running" && props.active) activateFromTab();
    if (props.active) terminalView.value?.focus();
  } catch (error) {
    const coreError = parseCoreApiError(error);
    if (coreError?.code === "ssh_terminal.login_automation_confirmation_required") {
      await showLoginAutomationConfirmation(credentialRefId, true);
    } else if (
      coreError?.code === "ssh_terminal.credential_unavailable"
      && credentialRefId === null
    ) {
      emit("requestCredential", props.paneId, current.target);
    } else {
      reconnectErrorKey.value = coreError?.messageKey ?? "sshSession.reconnectFailed";
      if (credentialRefId !== null || propagateFailure) throw error;
    }
  } finally {
    reconnecting.value = false;
  }
}

async function takeoverLoginAutomation() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (
    !current
    || current.state !== "automatingLogin"
    || !currentAttachment?.channelId
    || loginAutomationTakingOver.value
  ) return;
  loginAutomationTakingOver.value = true;
  loginAutomationTakeoverFailed.value = false;
  try {
    if (!props.active) {
      emit("activate", props.paneId);
      await nextTick();
    }
    registerInputTarget();
    const response = await takeoverTerminalInputTarget(
      props.paneId,
      (expectedFocusEpoch) => takeoverSshLoginAutomation({
        sessionId: current.sessionId,
        expectedGeneration: current.generation,
        expectedStateRevision: current.stateRevision,
        expectedFocusEpoch,
        channelId: currentAttachment.channelId as string,
        attachmentId: currentAttachment.attachmentId,
        viewId,
      }),
    );
    attachment.value = response.details.attachments.find(
      (candidate) => candidate.viewId === viewId,
    ) ?? null;
    challenge.value = response.details.activeHostKeyChallenge;
    setKeyboardInteractiveChallenge(response.details.activeKeyboardInteractiveChallenge);
    loginAutomationProgress.value = response.details.activeLoginAutomation;
    heartbeat.value = response.details.heartbeat;
    updateSummary(response.details.session);
    terminalView.value?.focus();
  } catch {
    loginAutomationTakeoverFailed.value = true;
  } finally {
    loginAutomationTakingOver.value = false;
  }
}

async function releaseRendererBinding() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment || released) return;
  released = true;
  try {
    await detachSshSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId,
      intent: "rendererUnavailable",
      confirmation: null,
    });
  } catch {
    // A concurrent rebind may already have replaced this attachment. If IPC is
    // unavailable, Core's bounded liveness timeout performs the same cleanup.
  }
}

function handleBeforeUnload() {
  void releaseRendererBinding();
}

defineExpose({
  runShortcut,
  disconnectForClose,
  reconcileAfterForeground,
  reconnectWithCredential: (credentialRefId: string) => reconnect(credentialRefId),
  reconnectSavedCredential: () => reconnect(null, true, true),
  activateFromTab,
  deactivateFromTab,
});

function registerInputTarget() {
  if (unregisterInputTarget) return;
  unregisterInputTarget = registerTerminalInputTarget({
    id: props.paneId,
    label: () => props.label,
    focusTarget: currentFocusTarget,
    applyFocusLease,
    canAcceptInput: () => writable.value,
    send,
  });
}

function recoverActiveInputLease() {
  if (!props.active) return Promise.resolve(false);
  registerInputTarget();
  // The global broker serializes these requests and fences stale results. A
  // later call must still enqueue because attach/reconnect may have replaced a
  // null or old Channel while an earlier focus request was in flight.
  return focusTerminalInputTarget(props.paneId);
}

function activateFromTab() {
  if (!props.active) return;
  void recoverActiveInputLease();
  if (session.value?.attachmentCount && session.value.attachmentCount > 1) reassertCurrentResize();
  else if (props.visible) void flushPendingResize();
  terminalView.value?.focus();
}

let initialDirectoryAttempted = false;
watch(
  () => [props.initialDirectory, state.value, props.active, attachment.value?.channelId],
  () => {
    const directory = props.initialDirectory;
    if (!directory || initialDirectoryAttempted || state.value !== "running"
      || !props.active || !attachment.value?.channelId) return;
    const originalSessionId = session.value?.sessionId;
    const originalGeneration = session.value?.generation;
    const originalAttachmentId = attachment.value.attachmentId;
    const originalChannelId = attachment.value.channelId;
    initialDirectoryAttempted = true;
    void (async () => {
      try {
        const focused = await recoverActiveInputLease();
        if (!focused || released || !inputReady.value
          || session.value?.sessionId !== originalSessionId
          || session.value?.generation !== originalGeneration
          || attachment.value?.attachmentId !== originalAttachmentId
          || attachment.value.channelId !== originalChannelId) {
          throw new Error("SSH input target changed");
        }
        await send(posixDirectoryCommand(directory));
      } catch {
        lease.value = null;
        stopLeaseHeartbeat();
        tips.show({ scope: `ssh-initial-directory:${props.paneId}`, tone: "error", title: t("sshSession.initialDirectoryFailed") });
      } finally {
        emit("initialDirectoryHandled", directory);
      }
    })();
  },
  { flush: "post", immediate: true },
);

watch(
  () => [props.visible, session.value?.sessionId, session.value?.generation, session.value?.state, attachment.value?.attachmentId],
  reassertCurrentResize,
);

function deactivateFromTab() {
  unregisterInputTarget?.();
  unregisterInputTarget = null;
}

function activate() {
  emit("activate", props.paneId);
  if (props.active) activateFromTab();
}

function activateTerminalSurface(event: Event) {
  const target = event.target;
  if (!(target instanceof Element) || !target.closest(".nvx-terminal-view")) return;
  activate();
}

onMounted(() => {
  if (props.active) activateFromTab();
  window.addEventListener("beforeunload", handleBeforeUnload);
  if (session.value) void attachExistingSession();
  else if (!props.deferredStart) void open();
});

watch(() => props.active, (active) => {
  if (!active) deactivateFromTab();
});

onBeforeUnmount(() => {
  window.removeEventListener("beforeunload", handleBeforeUnload);
  stopLeaseHeartbeat();
  stopAttachmentHeartbeat();
  clearPendingEvents();
  deactivateFromTab();
  void releaseRendererBinding();
});
</script>

<template>
  <section
    class="ssh-terminal-pane"
    @pointerdown="activateTerminalSurface"
    @focusin="activateTerminalSurface"
  >
    <header class="ssh-terminal-pane__status">
      <div class="ssh-terminal-pane__identity">
        <div
          v-if="state === 'running'"
          :ref="latencyRootRef"
          class="ssh-terminal-pane__latency"
          @focusout="closeLatencyOnBlur"
        >
          <button
            :ref="latencyTriggerRef"
            class="ssh-terminal-pane__latency-trigger"
            type="button"
            :title="stateTitle"
            :aria-label="t('sshSession.heartbeat.showHopLatency', { latency: stateDisplayLabel })"
            :aria-expanded="latencyOpen"
            :aria-controls="`${paneId}-latency`"
            @click="toggleLatency"
          >
            <NvxStatusLabel
              class="ssh-terminal-pane__state"
              tone="success"
            >
              {{ stateDisplayLabel }}
            </NvxStatusLabel>
          </button>
          <Teleport to="body">
            <div
              v-if="latencyOpen"
              :id="`${paneId}-latency`"
              :ref="latencyPanelRef"
              class="ssh-terminal-pane__latency-popover"
              :style="latencyPanelStyle"
              role="region"
              :aria-label="t('sshSession.heartbeat.hopLatency')"
            >
              <div
                v-for="row in latencyRows"
                :key="row.key"
                class="ssh-terminal-pane__latency-row"
              >
                <span
                  class="ssh-terminal-pane__latency-host"
                  :title="row.endpoint"
                >
                  <span>{{ row.label }}</span>
                  <small v-if="row.endpoint">{{ row.endpoint }}</small>
                </span>
                <span class="ssh-terminal-pane__latency-value">{{ latencyText(row.latency) }}</span>
              </div>
            </div>
          </Teleport>
        </div>
        <NvxStatusLabel
          v-else
          class="ssh-terminal-pane__state"
          :tone="state === 'failed' ? 'danger' : 'neutral'"
          :title="stateTitle"
        >
          {{ stateDisplayLabel }}
        </NvxStatusLabel>
        <span
          class="ssh-terminal-pane__endpoint"
          :title="endpointLabel"
        >
          <NvxIcon
            :icon="Server"
            :size="16"
          />
          {{ endpointLabel }}
        </span>
        <NvxHostMarker
          v-if="target.kind === 'host'"
          :marker="hostMarkers.visibleMarker(target.hostId)"
        />
      </div>
      <div class="ssh-terminal-pane__actions">
        <NvxNativeTerminalTools
          ref="nativeTools"
          :pane-id="paneId"
          :label="label"
          :host-id="target.kind === 'host' ? target.hostId : null"
          :session="nativeScope"
          :active="active"
          :writable="writable"
          :draft="inputDraft"
          :current-draft="() => terminalView?.currentDraft() ?? null"
          @invalidate-draft="terminalView?.invalidateDraft()"
          @suggestion-change="ghostSuggestion = $event"
          @focus="terminalView?.focus()"
        />
        <NvxTerminalTools
          ref="terminalTools"
          :terminal="terminalView"
          :has-selection="hasSelection"
        />
        <NvxTerminalPaneControls
          :plugin-context-key="pluginContextKey"
          :can-split-horizontal="canSplitHorizontal"
          :can-split-vertical="canSplitVertical"
          :can-split-workspace-right="canSplitWorkspaceRight"
          :show-layout-actions="active"
          @split="emit('split', $event)"
          @split-workspace-right="emit('splitWorkspaceRight')"
          @close="emit('close')"
        >
          <NvxButton
            v-if="!['closed', 'failed'].includes(state)"
            class="ssh-terminal-pane__disconnect"
            variant="ghost"
            size="sm"
            :loading="disconnecting"
            @click="disconnect"
          >
            <NvxIcon
              :icon="Unplug"
              :size="16"
            />
            {{ t("sshSession.disconnect") }}
          </NvxButton>
          <NvxButton
            v-else
            class="ssh-terminal-pane__reconnect"
            variant="ghost"
            size="sm"
            :loading="reconnecting"
            @click="reconnect()"
          >
            <NvxIcon
              :icon="sessionActionIcon"
              :size="16"
            />
            {{ sessionActionLabel }}
          </NvxButton>
        </NvxTerminalPaneControls>
        <NvxTerminalPaneOverflowMenu
          :plugin-context-key="pluginContextKey"
          class="ssh-terminal-pane__overflow"
          :can-split-horizontal="canSplitHorizontal"
          :can-split-vertical="canSplitVertical"
          :can-split-workspace-right="canSplitWorkspaceRight"
          :has-selection="hasSelection"
          :show-layout-actions="active"
          show-session-action
          :session-action-label="sessionActionLabel"
          :session-action-icon="sessionActionIcon"
          :session-action-disabled="disconnecting || reconnecting"
          :session-action-danger="!sessionActionReconnects"
          @search="openSearch"
          @copy="copySelection"
          @split="emit('split', $event)"
          @split-workspace-right="emit('splitWorkspaceRight')"
          @session="runSessionAction"
          @close="emit('close')"
        />
      </div>
    </header>

    <NvxPluginExtensionTarget
      class="ssh-terminal-pane__annotation"
      target-id="terminal.annotation"
      :instance-key="pluginContextKey"
      :display-label="label"
    />

    <NvxInlineNotice
      v-if="loginAutomationProgress"
      class="ssh-terminal-pane__failure"
      :tone="loginAutomationProgress.status === 'failed' ? 'error' : 'info'"
      :title="loginAutomationTitle ?? undefined"
    >
      <div class="ssh-login-automation__body">
        <span>{{ t("sshSession.loginAutomation.description") }}</span>
        <div class="ssh-login-automation__actions">
          <NvxButton
            size="sm"
            :loading="loginAutomationTakingOver"
            :disabled="disconnecting || !attachment?.channelId"
            @click.stop="takeoverLoginAutomation"
          >
            {{ t("sshSession.loginAutomation.takeover") }}
          </NvxButton>
          <NvxButton
            variant="ghost"
            size="sm"
            :loading="disconnecting"
            :disabled="loginAutomationTakingOver"
            @click.stop="disconnect"
          >
            {{ t("sshSession.disconnect") }}
          </NvxButton>
        </div>
        <small v-if="loginAutomationTakeoverFailed">
          {{ t("sshSession.loginAutomation.takeoverFailed") }}
        </small>
      </div>
    </NvxInlineNotice>
    <NvxInlineNotice
      v-else-if="reconnectErrorMessage"
      class="ssh-terminal-pane__failure"
      tone="error"
      :title="reconnectErrorMessage"
    />
    <NvxInlineNotice
      v-else-if="failureMessage"
      class="ssh-terminal-pane__failure"
      tone="error"
      :title="failureMessage"
    />
    <NvxTerminalView
      ref="terminalView"
      :pane-id="paneId"
      :host-id="target.kind === 'host' ? target.hostId : null"
      :ghost-suggestion="ghostSuggestion"
      :read-only="!writable"
      :reconnect-on-input="reconnectOnInput"
      :terminal-label="t('sshSession.terminalLabel', { label })"
      :gap-label="t('sshSession.outputGap')"
      @input="handleTerminalInput"
      @reconnect-request="requestReconnectFromInput"
      @resize="resize"
      @selection-change="hasSelection = $event"
      @search-request="openSearch"
      @draft-change="inputDraft = $event"
      @accept-suggestion="nativeTools?.acceptSuggestion()"
      @bell-attention="emit('bellAttention', $event)"
    />

    <NvxDialog
      v-model="automationConfirmationOpen"
      :title="t('sshSession.loginAutomation.confirmationTitle', { host: label })"
      :description="t('sshSession.loginAutomation.confirmationDescription')"
      :close-label="t('sshSession.loginAutomation.confirmationCancel')"
      :dismissible="!automationConfirmationBusy"
    >
      <ol class="ssh-login-automation__review">
        <li
          v-for="(step, index) in pendingAutomationConfirmation?.steps ?? []"
          :key="index"
        >
          <strong>{{ t(`sshHosts.loginAutomation.stepTypes.${step.kind}`) }}</strong>
          <code v-if="step.kind === 'expect'">{{ step.literalText }}</code>
          <code v-else-if="step.kind === 'sendText'">{{ step.text }}</code>
          <span v-else>{{ step.secretLabel }}</span>
          <small>{{ t('sshHosts.loginAutomation.timeout') }}: {{ step.timeoutSeconds }}</small>
          <small v-if="step.kind !== 'expect' && step.appendEnter">
            {{ t('sshHosts.loginAutomation.appendEnter') }}
          </small>
        </li>
      </ol>
      <NvxInlineNotice
        v-if="automationConfirmationFailed"
        tone="error"
        :title="t('sshSession.loginAutomation.confirmationFailed')"
      />
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="automationConfirmationBusy"
          @click="automationConfirmationOpen = false"
        >
          {{ t('sshSession.loginAutomation.confirmationCancel') }}
        </NvxButton>
        <NvxButton
          :loading="automationConfirmationBusy"
          @click="confirmPendingLoginAutomation"
        >
          {{ t('sshSession.loginAutomation.confirmationAccept') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.ssh-terminal-pane {
  position: relative;
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  background: var(--nvx-color-terminal-bg);
}

.ssh-terminal-pane__status {
  container-name: ssh-terminal-pane-toolbar;
  container-type: inline-size;
  display: flex;
  min-height: 36px;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--nvx-space-2);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  background: var(--nvx-color-terminal-bg);
  color: var(--nvx-color-terminal-fg);
}

.ssh-terminal-pane__identity {
  display: flex;
  flex: 1;
  gap: var(--nvx-space-3);
  align-items: center;
  min-width: 0;
  overflow: hidden;
}

.ssh-terminal-pane__state {
  flex: none;
  white-space: nowrap;
}

.ssh-terminal-pane__latency {
  flex: none;
}

.ssh-terminal-pane__latency-trigger {
  display: inline-flex;
  align-items: center;
  padding: 0;
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  cursor: pointer;
}

.ssh-terminal-pane__latency-trigger:hover .ssh-terminal-pane__state {
  text-decoration: underline;
}

.ssh-terminal-pane__latency-trigger:focus-visible {
  outline: 2px solid var(--nvx-color-accent);
  outline-offset: 2px;
}

.ssh-terminal-pane__latency-popover {
  position: fixed;
  z-index: var(--nvx-z-popover);
  display: grid;
  box-sizing: border-box;
  width: min(264px, calc(100vw - 16px));
  max-height: calc(100dvh - 16px);
  overflow-y: auto;
  padding: var(--nvx-space-1);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  box-shadow: var(--nvx-shadow-overlay);
}

.ssh-terminal-pane__latency-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--nvx-space-3);
  padding: var(--nvx-space-1) var(--nvx-space-2);
  font-size: var(--nvx-font-size-xs);
}

.ssh-terminal-pane__latency-host {
  display: grid;
  min-width: 0;
}

.ssh-terminal-pane__latency-host small {
  overflow: hidden;
  color: var(--nvx-color-text-secondary);
  font-size: inherit;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ssh-terminal-pane__latency-value {
  flex: none;
  font-family: var(--nvx-font-mono);
}

.ssh-terminal-pane__actions {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: var(--nvx-space-1);
}

.ssh-terminal-pane__overflow {
  display: none;
}

.ssh-terminal-pane__endpoint {
  display: inline-flex;
  flex: 1;
  gap: var(--nvx-space-1);
  align-items: center;
  min-width: 0;
  overflow: hidden;
  color: var(--nvx-color-terminal-muted);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ssh-terminal-pane__disconnect {
  color: var(--nvx-color-terminal-muted);
  padding-inline: var(--nvx-space-2);
}

.ssh-terminal-pane__disconnect:hover:not(:disabled) {
  background: color-mix(in srgb, var(--nvx-color-danger) 9%, transparent);
}

@container ssh-terminal-pane-toolbar (max-width: 560px) {
  .ssh-terminal-pane__actions :deep(.terminal-tools > .nvx-icon-button),
  .ssh-terminal-pane__actions :deep(.terminal-pane-controls) {
    display: none;
  }

  .ssh-terminal-pane__overflow {
    display: inline-flex;
  }
}

.ssh-terminal-pane__failure {
  position: absolute;
  z-index: var(--nvx-z-sticky);
  top: 44px;
  right: var(--nvx-space-3);
  left: var(--nvx-space-3);
}

.ssh-login-automation__body {
  display: grid;
  gap: var(--nvx-space-2);
}

.ssh-login-automation__actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--nvx-space-2);
}

.ssh-login-automation__body small {
  color: var(--nvx-color-danger);
}

.ssh-login-automation__review {
  display: grid;
  gap: var(--nvx-space-3);
  max-height: 45vh;
  overflow-y: auto;
  padding-left: var(--nvx-space-5);
}

.ssh-login-automation__review li {
  display: grid;
  gap: var(--nvx-space-1);
  overflow-wrap: anywhere;
}

.ssh-login-automation__review code {
  white-space: pre-wrap;
}

.ssh-host-key {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: var(--nvx-space-4);
  align-items: start;
}

.ssh-keyboard-interactive {
  display: grid;
  gap: var(--nvx-space-4);
}

.ssh-keyboard-interactive__context {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: var(--nvx-space-3);
  align-items: start;
}

.ssh-keyboard-interactive__context p {
  margin: var(--nvx-space-2) 0;
  color: var(--nvx-color-text-secondary);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.ssh-keyboard-interactive__context small {
  color: var(--nvx-color-text-tertiary);
}

.ssh-keyboard-interactive__prompts {
  display: grid;
  gap: var(--nvx-space-3);
}

.ssh-host-key dl,
.ssh-host-key dd {
  margin: 0;
}

.ssh-host-key dl {
  display: grid;
  gap: var(--nvx-space-3);
}

.ssh-host-key dt {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.ssh-host-key dd {
  overflow-wrap: anywhere;
}
</style>
