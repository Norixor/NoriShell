<script setup lang="ts">
import { Network, RotateCw, Unplug } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import {
  attachPluginTerminalSession,
  detachPluginTerminalSession,
  disconnectPluginTerminalSession,
  heartbeatPluginAttachment,
  openPluginTerminalSession,
  claimPluginProtocolLaunch,
  fetchPluginTerminalSessionSnapshot,
  reconnectPluginTerminalSession,
  renewPluginInputLease,
  resizePluginTerminal,
  sendPluginInput,
} from "../../core-api/plugin-terminal";
import type {
  PluginTerminalProfile,
  PluginProtocolLaunchSummary,
  PluginInputLease,
  PluginTerminalSessionAttachment,
  PluginTerminalSessionEvent,
  PluginTerminalSessionOutputItem,
  PluginTerminalSessionState,
  PluginTerminalSessionSummary,
} from "../../core-api/plugin-terminal";
import { createUuidV7 } from "../../core-api/ids";
import { parseCoreApiError } from "../../core-api/client";
import type { TerminalInputLease } from "../../core-api/generated/core-api";
import {
  focusTerminalInputTarget,
  isTerminalInputTargetFocused,
  registerTerminalInputTarget,
} from "../../terminal-input-target";
import {
  NvxButton,
  NvxIcon,
  NvxInlineNotice,
  NvxStatusLabel,
} from "../ui";
import NvxTerminalPaneControls from "./NvxTerminalPaneControls.vue";
import NvxTerminalView from "./NvxTerminalView.vue";
import { createFencedTerminalResize } from "./fencedTerminalResize";
import NvxTerminalTools from "./NvxTerminalTools.vue";
import type { ShortcutCommandId } from "../../shortcuts";
import { useTipsStore } from "../../stores/tips";

interface TerminalViewExpose {
  writeBytes(bytes: readonly number[]): void;
  writeGap(): void;
  dimensions(): { rows: number; cols: number };
  focus(): void;
  fit(): void;
  clear(): void;
  selection(): string;
  findNext(term: string, incremental?: boolean): boolean;
  findPrevious(term: string): boolean;
  clearSearch(): void;
  pasteFromClipboard(): Promise<void> | undefined;
}

const props = withDefaults(defineProps<{
  paneId: string;
  label: string;
  profile: PluginTerminalProfile | null;
  launch?: PluginProtocolLaunchSummary | null;
  tabId: string;
  existingSession: PluginTerminalSessionSummary | null;
  deferredStart?: boolean;
  active: boolean;
  canSplitHorizontal: boolean;
  canSplitVertical: boolean;
  canSplitWorkspaceRight: boolean;
}>(), { deferredStart: false, launch: null });

const emit = defineEmits<{
  activate: [paneId: string];
  state: [state: PluginTerminalSessionState, summary: PluginTerminalSessionSummary | null];
  bellAttention: [active: boolean];
  split: [direction: "horizontal" | "vertical"];
  splitWorkspaceRight: [];
  close: [];
}>();

const { t, te } = useI18n();
const terminalView = ref<TerminalViewExpose | null>(null);
const session = ref<PluginTerminalSessionSummary | null>(props.existingSession);
const attachment = ref<PluginTerminalSessionAttachment | null>(null);
const lease = ref<PluginInputLease | null>(null);
const opening = ref(false);
const openFailed = ref(false);
let openUncertain = false;
let openOperationId = createUuidV7();
const disconnecting = ref(false);
let closeOwnsCleanup = false;
let closeWork: Promise<void> | null = null;
const pendingEvents: PluginTerminalSessionEvent[] = [];
let binding = false;
let pendingEventsDropped = false;
let released = false;
let clientSequence = 0n;
let lastEventSequence = 0n;
let lastOutputSequence = 0n;
let attachmentHeartbeatTimer: number | null = null;
let leaseTimer: number | null = null;
let unregisterInputTarget: (() => void) | null = null;

const fencedResize = createFencedTerminalResize((dimensions) => {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.streamId || !currentAttachment || !currentLease || !ownsLease.value) return null;
  const streamId = current.streamId;
  return {
    key: [
      current.sessionId,
      current.generation,
      currentAttachment.attachmentId,
      dimensions.rows,
      dimensions.cols,
    ].join(":"),
    send: (resizeSeq: string) => resizePluginTerminal({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      streamId,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
      leaseId: currentLease.leaseId,
      focusEpoch: currentLease.focusEpoch,
      inputEpoch: currentLease.inputEpoch,
      resizeSeq,
      rows: dimensions.rows,
      cols: dimensions.cols,
    }),
  };
});
const resize = fencedResize.resize;
const flushPendingResize = fencedResize.flush;

const state = computed<PluginTerminalSessionState>(() => session.value?.state
  ?? (openFailed.value ? "failed" : props.deferredStart ? "closed" : "connecting"));
const stateLabel = computed(() => t(`pluginTerminal.states.${state.value}`));
const failureMessage = computed(() => {
  if (session.value?.cleanupBlocked) return t("pluginTerminal.cleanupBlocked");
  const reason = session.value?.failureReason;
  if (!reason) return openFailed.value ? t("pluginTerminal.failureFallback") : null;
  return te(reason.messageKey) ? t(reason.messageKey) : t("pluginTerminal.failureFallback");
});
const ownsLease = computed(() => {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  return Boolean(
    current?.streamId
    && currentAttachment?.streamId === current.streamId
    && currentLease
    && currentLease.sessionId === current.sessionId
    && currentLease.generation === current.generation
    && currentLease.streamId === current.streamId
    && currentLease.attachmentId === currentAttachment.attachmentId
    && currentLease.viewId === props.paneId
    && currentLease.expiresAtUnixMs > Date.now(),
  );
});
const writable = computed(() => props.active
  && state.value === "running"
  && ownsLease.value
  && isTerminalInputTargetFocused(props.paneId, lease.value?.focusEpoch ?? null));

function updateSummary(next: PluginTerminalSessionSummary) {
  session.value = next;
  emit("state", next.state, next);
}

function applyFocusLease(value: TerminalInputLease | null) {
  lease.value = value?.kind === "plugin" ? value.lease : null;
  if (lease.value) void nextTick(() => flushPendingResize());
}

function focusTarget() {
  if (!props.active) return null;
  const current = session.value;
  const currentAttachment = attachment.value;
  if (
    current?.state !== "running"
    || !current.streamId
    || !currentAttachment
    || currentAttachment.streamId !== current.streamId
  ) return null;
  return {
    kind: "plugin" as const,
    target: {
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      streamId: current.streamId,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
    },
  };
}

function applyOutput(item: PluginTerminalSessionOutputItem) {
  const current = session.value;
  if (!current?.streamId) return;
  if (item.kind === "frame") {
    const frame = item.payload;
    if (
      frame.sessionId !== current.sessionId
      || frame.generation !== current.generation
      || frame.streamId !== current.streamId
    ) return;
    const sequence = BigInt(frame.outputSeq);
    if (sequence <= lastOutputSequence) return;
    if (sequence > lastOutputSequence + 1n) {
      terminalView.value?.writeGap();
    }
    terminalView.value?.writeBytes(frame.bytes);
    lastOutputSequence = sequence;
    return;
  }
  const gap = item.payload;
  if (
    gap.sessionId !== current.sessionId
    || gap.generation !== current.generation
    || gap.streamId !== current.streamId
  ) return;
  const resumesAt = BigInt(gap.resumesAtOutputSeq);
  if (resumesAt > lastOutputSequence) {
    terminalView.value?.writeGap();
    lastOutputSequence = resumesAt - 1n;
  }
}

function applyEvent(event: PluginTerminalSessionEvent) {
  if (binding || !session.value) {
    if (pendingEvents.length < 256) pendingEvents.push(event);
    else pendingEventsDropped = true;
    return;
  }
  if (event.sessionId !== session.value.sessionId || event.generation !== session.value.generation) return;
  const sequence = BigInt(event.eventSeq);
  if (sequence <= lastEventSequence) return;
  lastEventSequence = sequence;
  if (BigInt(event.session.stateRevision) >= BigInt(session.value.stateRevision)) updateSummary(event.session);
  switch (event.payload.kind) {
    case "stateChanged":
      if (BigInt(event.payload.session.stateRevision) < BigInt(session.value.stateRevision)) return;
      updateSummary(event.payload.session);
      if (event.payload.session.state !== "running") lease.value = null;
      if (event.payload.session.state === "running" && props.active) activateFromTab();
      break;
    case "attachmentAttached":
      if (event.payload.attachment.viewId === props.paneId
        && event.payload.attachment.generation === session.value.generation) {
        attachment.value = event.payload.attachment;
      }
      break;
    case "attachmentDetached":
      if (event.payload.attachmentId === attachment.value?.attachmentId) {
        attachment.value = null;
        lease.value = null;
      }
      break;
    case "inputLeaseRevoked":
      lease.value = null;
      break;
    case "output":
      applyOutput({ kind: "frame", payload: event.payload.frame });
      break;
    case "outputGap":
      if (event.payload.gap.reason === "eventOverrun") {
        lease.value = null;
        void attachExisting();
        break;
      }
      applyOutput({ kind: "gap", payload: event.payload.gap });
      break;
  }
}

function replayPendingEvents() {
  if (pendingEventsDropped) {
    terminalView.value?.writeGap();
    pendingEventsDropped = false;
  }
  pendingEvents
    .splice(0)
    .sort((left, right) => Number(BigInt(left.eventSeq) - BigInt(right.eventSeq)))
    .forEach(applyEvent);
}


let openingWork: Promise<void> | null = null;
function open(force = false) {
  openingWork ??= performOpen(force).finally(() => { openingWork = null; });
  return openingWork;
}

async function performOpen(force = false) {
  if (opening.value || (props.deferredStart && !force)) return;
  opening.value = true;
  openFailed.value = false;
  binding = true;
  try {
    await nextTick();
    const dimensions = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
    openUncertain = true;
    const response = props.launch
      ? await claimPluginProtocolLaunch(props.launch)
      : props.profile
        ? await openPluginTerminalSession({
          ...props.profile, label: props.label, tabId: props.tabId, paneId: props.paneId,
          rows: dimensions.rows, cols: dimensions.cols,
        }, openOperationId)
        : null;
    if (!response) { openUncertain = false; throw new Error("Plugin terminal profile unavailable"); }
    openUncertain = false;
    updateSummary(response.session);
    binding = false;
    if (!released) await attachExisting();
  } catch (error) {
    binding = false;
    openFailed.value = true;
    if (parseCoreApiError(error)) { openUncertain = false; openOperationId = createUuidV7(); }
    const recovered = await fetchPluginTerminalSessionSnapshot().catch(() => null);
    const found = recovered?.sessions.find((item) => item.paneId === props.paneId && item.tabId === props.tabId);
    if (found) {
      openUncertain = false;
      updateSummary(found);
      if (!released) await attachExisting();
    } else {
      // Lost IPC does not prove Core has no resource; keep close confirmation to avoid hiding a possibly live session.
      emit("state", openUncertain ? "connecting" : "failed", null);
    }
  } finally {
    opening.value = false;
  }
}

let attachingWork: Promise<void> | null = null;
function attachExisting() {
  attachingWork ??= performAttach().finally(() => { attachingWork = null; });
  return attachingWork;
}

async function performAttach() {
  const current = session.value;
  if (!current || current.state === "closed" || current.state === "failed") return;
  binding = true;
  try {
    const response = await attachPluginTerminalSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      streamId: current.streamId!,
      viewId: props.paneId,
      afterOutputSeq: lastOutputSequence > 0n ? lastOutputSequence.toString() : null,
    }, applyEvent);
    if (released && !closeOwnsCleanup) {
      await detachPluginTerminalSession({
        sessionId: response.session.sessionId, expectedGeneration: response.session.generation,
        expectedStateRevision: response.session.stateRevision, expectedAttachmentRevision: response.session.attachmentRevision,
        attachmentId: response.attachment.attachmentId, viewId: props.paneId,
        intent: "rendererUnavailable", disconnectIfLast: false,
      });
      binding = false;
      return;
    }
    openFailed.value = false;
    updateSummary(response.session);
    lastEventSequence = BigInt(response.session.eventSeq);
    attachment.value = response.attachment;
    // A Tab close controller can outlive this renderer while attach is still in progress.
    // Preserve the attachment for the original close operation so it can submit atomic userClose instead of racing
    // rendererUnavailable from a different revision.
    if (closeOwnsCleanup) {
      binding = false;
      return;
    }
    response.replay.forEach(applyOutput);
    binding = false;
    replayPendingEvents();
    startTimers();
    if (props.active) activateFromTab();
  } catch {
    binding = false;
    openFailed.value = true;
    emit("state", current.state, current);
  }
}

function startTimers() {
  stopTimers();
  attachmentHeartbeatTimer = window.setInterval(() => void heartbeatAttachment(), 10_000);
  leaseTimer = window.setInterval(() => void renewLease(), 5_000);
}

function stopTimers() {
  if (attachmentHeartbeatTimer !== null) window.clearInterval(attachmentHeartbeatTimer);
  if (leaseTimer !== null) window.clearInterval(leaseTimer);
  attachmentHeartbeatTimer = null;
  leaseTimer = null;
}

async function heartbeatAttachment() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment) return;
  try {
    const next = await heartbeatPluginAttachment({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedAttachmentRevision: current.attachmentRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
    });
    if (attachment.value === currentAttachment && session.value?.generation === current.generation) attachment.value = next;
  } catch {
    attachment.value = null;
    lease.value = null;
  }
}

async function renewLease() {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.streamId || !currentAttachment || !currentLease || !writable.value) return;
  try {
    const next = await renewPluginInputLease({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      streamId: current.streamId,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
      leaseId: currentLease.leaseId,
      focusEpoch: currentLease.focusEpoch,
      inputEpoch: currentLease.inputEpoch,
    });
    if (lease.value === currentLease && writable.value) lease.value = next;
  } catch {
    lease.value = null;
  }
}

async function send(value: string) {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.streamId || !currentAttachment || !currentLease || !writable.value) return;
  clientSequence += 1n;
  try { await sendPluginInput({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    expectedStateRevision: current.stateRevision,
    streamId: current.streamId,
    attachmentId: currentAttachment.attachmentId,
    viewId: props.paneId,
    leaseId: currentLease.leaseId,
    focusEpoch: currentLease.focusEpoch,
    inputEpoch: currentLease.inputEpoch,
    clientSeq: clientSequence.toString(),
    bytes: [...new TextEncoder().encode(value)],
  }); } catch (error) { lease.value = null; openFailed.value = true; throw error; }
}


function activateFromTab() {
  if (!props.active) return;
  registerInputTarget();
  terminalView.value?.fit();
  terminalView.value?.focus();
  void focusTerminalInputTarget(props.paneId);
}

function activateTerminalSurface(event: Event) {
  const target = event.target;
  if (!(target instanceof Element) || !target.closest(".nvx-terminal-view")) return;
  emit("activate", props.paneId);
  if (props.active) activateFromTab();
}

function deactivateFromTab() {
  lease.value = null;
  unregisterInputTarget?.();
  unregisterInputTarget = null;
}

function registerInputTarget() {
  if (unregisterInputTarget) return;
  unregisterInputTarget = registerTerminalInputTarget({
    id: props.paneId,
    label: () => props.label,
    focusTarget,
    applyFocusLease,
    // A provider protocol may not be a Shell; expose only user input on the terminal surface.
    canAcceptInput: () => false,
    canAcceptRawInput: () => writable.value,
    send,
  });
}

function disconnectForClose(disconnectAll = false) {
  if (closeWork) return closeWork;
  // SshTerminalView removes a confirmed-closed Tab before awaiting its captured controller. Acquire cleanup ownership before the first
  // await so Vue unmount cannot send rendererUnavailable before userClose.
  closeOwnsCleanup = true;
  disconnecting.value = true;
  const work = performDisconnectForClose(disconnectAll);
  closeWork = work;
  void work.then(
    () => {
      closeWork = null;
      closeOwnsCleanup = false;
      disconnecting.value = false;
    },
    () => {
      closeWork = null;
      closeOwnsCleanup = false;
      disconnecting.value = false;
    },
  );
  return work;
}

async function performDisconnectForClose(disconnectAll: boolean) {
  await openingWork;
  await attachingWork;
  if (openUncertain && !session.value) {
    const snapshot = await fetchPluginTerminalSessionSnapshot();
    const found = snapshot.sessions.find((item) => item.paneId === props.paneId && item.tabId === props.tabId);
    if (!found) throw new Error("Plugin terminal launch outcome is unresolved");
    openUncertain = false; updateSummary(found);
  }
  const current = session.value;
  if (!current || (["closed", "failed"].includes(current.state) && !current.cleanupBlocked)) return;
  const bound = attachment.value;
  if (bound && !current.cleanupBlocked && !disconnectAll) {
    const result = await detachPluginTerminalSession({
      sessionId: current.sessionId, expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision, expectedAttachmentRevision: current.attachmentRevision,
      attachmentId: bound.attachmentId, viewId: props.paneId, intent: "userClose", disconnectIfLast: true,
    });
    attachment.value = null; lease.value = null;
    updateSummary(result.session);
    if (result.remainingAttachmentCount > 0) emit("state", "closed", result.session);
    return;
  }
  if (!disconnectAll && !current.cleanupBlocked) {
    throw new Error("Plugin terminal attachment is unavailable for close");
  }
  attachment.value = null;
  lease.value = null;
  updateSummary(await disconnectPluginTerminalSession({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    expectedStateRevision: current.stateRevision,
  }));
}

async function requestReconnect() {
  if (opening.value || session.value?.cleanupBlocked) return;
  const current = session.value;
  if (current && !["closed", "failed"].includes(current.state)) { await reconcileAfterForeground(); return; }
  if (!current) { await open(true); return; }
  opening.value = true;
  try {
    const dimensions = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
    const next = await reconnectPluginTerminalSession({
      sessionId: current.sessionId, expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision, ...dimensions,
    });
    lastEventSequence = 0n;
    lastOutputSequence = 0n;
    clientSequence = 0n;
    lease.value = null;
    attachment.value = null;
    updateSummary(next);
    await attachExisting();
  } catch { openFailed.value = true; }
  finally { opening.value = false; }
}

let reconciling: Promise<void> | null = null;
function reconcileAfterForeground() {
  if (reconciling || released || binding) return reconciling;
  reconciling = (async () => {
    const current = session.value;
    if (!current) return;
    try {
      const snapshot = await fetchPluginTerminalSessionSnapshot();
      const next = snapshot.sessions.find((item) => item.sessionId === current.sessionId);
      if (!next || released) { lease.value = null; return; }
      if (next.generation !== current.generation) {
        lastOutputSequence = 0n; lastEventSequence = 0n; attachment.value = null; lease.value = null;
      }
      updateSummary(next);
      if (["closed", "failed"].includes(next.state)) { lease.value = null; return; }
      if (attachment.value) await heartbeatAttachment();
      if (!attachment.value) await attachExisting();
      else if (props.active) activateFromTab();
    } catch { lease.value = null; }
  })().finally(() => { reconciling = null; });
  return reconciling;
}

async function release() {
  stopTimers();
  unregisterInputTarget?.();
  unregisterInputTarget = null;
  if (released || closeOwnsCleanup) return;
  released = true;
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment) return;
  await detachPluginTerminalSession({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    expectedStateRevision: current.stateRevision,
    expectedAttachmentRevision: current.attachmentRevision,
    attachmentId: currentAttachment.attachmentId,
    viewId: props.paneId,
    intent: "rendererUnavailable",
    disconnectIfLast: false,
  }).catch(() => undefined);
}

const terminalTools = ref<InstanceType<typeof NvxTerminalTools> | null>(null);
const hasSelection = ref(false);
const tips = useTipsStore();
function runShortcut(commandId: ShortcutCommandId) {
  if (!props.active) return;
  if (commandId === "terminal.search") terminalTools.value?.openSearch();
  else if (commandId === "terminal.copy") void terminalTools.value?.copySelection();
  else if (commandId === "terminal.paste") void terminalView.value?.pasteFromClipboard();
  else if (commandId === "terminal.clear") terminalView.value?.clear();
  else if (commandId === "terminal.reconnect" && ["closed", "failed"].includes(state.value)) requestReconnect();
  else if (commandId === "terminal.disconnect" && !["closed", "failed"].includes(state.value)) void disconnectForClose(true);
  else if (commandId === "terminal.history-suggestions") tips.show({ tone: "info", title: t("nativeTerminal.unsupportedSession") });
}
defineExpose({ disconnectForClose, activateFromTab, deactivateFromTab, runShortcut, reconcileAfterForeground });

onMounted(async () => {
  if (props.active) registerInputTarget();
  if (session.value) await attachExisting();
  else await open();
});

watch(() => props.active, (active) => {
  if (!active) deactivateFromTab();
});

onBeforeUnmount(() => {
  deactivateFromTab();
  void release();
});
</script>

<template>
  <section
    class="plugin-terminal-pane"
    @pointerdown="activateTerminalSurface"
  >
    <header class="plugin-terminal-pane__toolbar">
      <span class="plugin-terminal-pane__identity">
        <NvxIcon
          :icon="Network"
          :size="16"
        />
        <strong>{{ label }}</strong>
        <small>{{ session?.providerId ?? profile?.providerId ?? launch?.providerId }}</small>
      </span>
      <span class="plugin-terminal-pane__actions">
        <NvxTerminalTools
          ref="terminalTools"
          :terminal="terminalView"
          :has-selection="hasSelection"
        />
        <NvxStatusLabel :tone="state === 'running' ? 'success' : state === 'failed' ? 'danger' : 'neutral'">
          {{ stateLabel }}
        </NvxStatusLabel>
        <NvxButton
          v-if="(openFailed || state === 'closed' || state === 'failed') && !session?.cleanupBlocked"
          size="sm"
          variant="secondary"
          @click.stop="requestReconnect"
        >
          <NvxIcon
            :icon="RotateCw"
            :size="16"
          />
          {{ t("pluginTerminal.reconnect") }}
        </NvxButton>
        <NvxButton
          v-else
          size="sm"
          variant="ghost"
          :loading="disconnecting"
          @click.stop="disconnectForClose(true)"
        >
          <NvxIcon
            :icon="Unplug"
            :size="16"
          />
          {{ t("pluginTerminal.disconnect") }}
        </NvxButton>
        <NvxTerminalPaneControls
          :plugin-context-key="paneId"
          :can-split-horizontal="canSplitHorizontal"
          :can-split-vertical="canSplitVertical"
          :can-split-workspace-right="canSplitWorkspaceRight"
          :show-layout-actions="active"
          @split="emit('split', $event)"
          @split-workspace-right="emit('splitWorkspaceRight')"
          @close="emit('close')"
        />
      </span>
    </header>
    <NvxInlineNotice
      v-if="failureMessage"
      tone="error"
      :title="failureMessage"
    />
    <NvxTerminalView
      ref="terminalView"
      :pane-id="paneId"
      class="plugin-terminal-pane__terminal"
      :read-only="!writable"
      :terminal-label="label"
      :gap-label="t('pluginTerminal.outputGap')"
      @input="(value) => { void send(value).catch(() => undefined); }"
      @resize="resize"
      @selection-change="hasSelection = $event"
      @search-request="terminalTools?.openSearch()"
      @bell-attention="emit('bellAttention', $event)"
    />
  </section>
</template>

<style scoped>
.plugin-terminal-pane {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  background: var(--nvx-color-terminal-bg);
}

.plugin-terminal-pane:has(> .nvx-inline-notice) { grid-template-rows: auto auto minmax(0, 1fr); }

.plugin-terminal-pane__toolbar,
.plugin-terminal-pane__identity,
.plugin-terminal-pane__actions {
  display: flex;
  align-items: center;
}

.plugin-terminal-pane__toolbar {
  justify-content: space-between;
  gap: 12px;
  min-height: 42px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--nvx-color-border-subtle);
  background: var(--nvx-color-surface-raised);
}

.plugin-terminal-pane__identity,
.plugin-terminal-pane__actions { gap: 8px; }
.plugin-terminal-pane__identity { min-width: 0; }
.plugin-terminal-pane__identity strong,
.plugin-terminal-pane__identity small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.plugin-terminal-pane__identity small { color: var(--nvx-color-text-muted); }
.plugin-terminal-pane__risk { margin: 8px 8px 0; }
.plugin-terminal-pane__terminal { min-height: 0; }
</style>
