<script setup lang="ts">
import { SquareTerminal, Unplug } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import {
  attachLocalSession,
  detachLocalSession,
  getLocalSession,
  heartbeatLocalAttachment,
  openLocalSession,
  renewLocalInputLease,
  resizeLocalTerminal,
  sendLocalInput,
  terminateLocalSession,
} from "../../core-api/client";
import type {
  LocalSessionAttachment,
  LocalSessionEvent,
  LocalSessionFailureReason,
  LocalSessionInputLease,
  LocalSessionOutputItem,
  LocalSessionState,
  LocalSessionSummary,
  TerminalInputLease,
} from "../../core-api/generated/core-api";
import {
  focusTerminalInputTarget,
  isTerminalInputTargetFocused,
  registerTerminalInputTarget,
} from "../../terminal-input-target";
import { NvxPluginExtensionTarget } from "../plugins";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxIcon, NvxInlineNotice, NvxStatusLabel } from "../ui";
import NvxTerminalPaneControls from "./NvxTerminalPaneControls.vue";
import NvxTerminalPaneOverflowMenu from "./NvxTerminalPaneOverflowMenu.vue";
import NvxTerminalTools from "./NvxTerminalTools.vue";
import NvxTerminalView from "./NvxTerminalView.vue";
import { createFencedTerminalResize } from "./fencedTerminalResize";
import NvxNativeTerminalTools from "./NvxNativeTerminalTools.vue";
import { enableNativeTerminal } from "../../core-api/native-terminal";
import type { NativeTerminalSessionScope, NativeTerminalSessionStatus, NativeTerminalShellKind } from "../../core-api/generated/core-api";
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
  existingSession: LocalSessionSummary | null;
  deferredStart?: boolean;
  active: boolean;
  canSplitHorizontal: boolean;
  canSplitVertical: boolean;
}>(), { deferredStart: false });

const emit = defineEmits<{
  activate: [paneId: string];
  state: [state: LocalSessionState, summary: LocalSessionSummary | null];
  bellAttention: [active: boolean];
  split: [direction: "horizontal" | "vertical"];
  close: [];
}>();

const { t, te } = useI18n();
const tips = useTipsStore();
const nativeTools = ref<InstanceType<typeof NvxNativeTerminalTools> | null>(null);
const inputDraft = ref<string | null>(null);
const shellPromptKey = ref<string | null>(null);
const viewId = props.paneId;
const terminalView = ref<TerminalViewExpose | null>(null);
const terminalTools = ref<TerminalToolsExpose | null>(null);
const hasSelection = ref(false);
const session = ref<LocalSessionSummary | null>(props.existingSession);
const pluginContextKey = computed(() => [
  "",
  props.paneId,
  "local",
  session.value?.sessionId ?? "none",
  session.value?.generation ?? "0",
].join("|"));
const attachment = ref<LocalSessionAttachment | null>(null);
const lease = ref<LocalSessionInputLease | null>(null);
const failure = ref<LocalSessionFailureReason | null>(props.existingSession?.failureReason ?? null);
const opening = ref(false);
const openFailed = ref(false);
const terminating = ref(false);
let inputSequence = 0n;
let promptBoundaryInputSequence: bigint | null = null;
let leaseTimer: number | null = null;
let attachmentHeartbeatTimer: number | null = null;
let unregisterInputTarget: (() => void) | null = null;
let binding = false;
let released = false;
let lastAppliedEventSeq = 0n;
let lastAppliedOutputSeq = 0n;
const pendingEvents: LocalSessionEvent[] = [];
const pendingEventGaps = new Set<string>();
const PENDING_EVENT_MAX_COUNT = 256;
const PENDING_EVENT_MAX_BYTES = 1024 * 1024;
let pendingEventBytes = 0;

const fencedResize = createFencedTerminalResize((dimensions) => {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.ptyId || !currentAttachment || !currentLease || current.state !== "running") return null;
  const ptyId = current.ptyId;
  return {
    key: [
      current.sessionId,
      current.generation,
      currentAttachment.attachmentId,
      dimensions.rows,
      dimensions.cols,
    ].join(":"),
    send: (resizeSeq: string) => resizeLocalTerminal({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      ptyId,
      attachmentId: currentAttachment.attachmentId,
      viewId,
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

const state = computed<LocalSessionState>(
  () => session.value?.state
    ?? (openFailed.value ? "failed" : props.deferredStart ? "closed" : "starting"),
);
const ownsLease = computed(() => {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  return current !== null
    && currentAttachment !== null
    && currentLease !== null
    && current.ptyId !== null
    && currentAttachment.ptyId === current.ptyId
    && currentLease.sessionId === current.sessionId
    && currentLease.generation === current.generation
    && currentLease.attachmentId === currentAttachment.attachmentId
    && currentLease.viewId === viewId
    && currentLease.expiresAtUnixMs > Date.now();
});
const writable = computed(() => props.active
  && state.value === "running"
  && ownsLease.value
  && isTerminalInputTargetFocused(props.paneId, lease.value?.focusEpoch ?? null));
const nativeScope = computed<NativeTerminalSessionScope | null>(() => session.value?.ptyId ? {
  kind: "local", sessionId: session.value.sessionId, generation: session.value.generation,
  ptyId: session.value.ptyId, paneId: viewId,
} : null);
function acceptNativePrompt(status: NativeTerminalSessionStatus | null) {
  shellPromptKey.value = status && status.promptInputSequence === inputSequence.toString()
    && promptBoundaryInputSequence === inputSequence
    && status.promptInputEpoch === lease.value?.inputEpoch
    ? `${status.session.sessionId}:${status.session.generation}:${status.promptSequence}` : null;
}
async function enableNativeShell(shellKind: NativeTerminalShellKind) {
  const current = session.value;
  const attached = attachment.value;
  const inputLease = lease.value;
  if (!current?.ptyId || !attached || !inputLease || !writable.value) throw new Error("Terminal unavailable");
  inputSequence++;
  promptBoundaryInputSequence = inputSequence;
  return enableNativeTerminal({ shellKind, confirmedEmptyPrompt: true, inputFence: { kind: "local", payload: {
    sessionId: current.sessionId, expectedGeneration: current.generation, expectedStateRevision: current.stateRevision,
    ptyId: current.ptyId, attachmentId: attached.attachmentId, viewId,
    focusEpoch: inputLease.focusEpoch, leaseId: inputLease.leaseId,
    inputEpoch: inputLease.inputEpoch, clientSeq: inputSequence.toString(),
  } } });
}
function runShortcut(commandId: ShortcutCommandId) {
  if (!props.active) return;
  if (commandId === "terminal.search") openSearch();
  else if (commandId === "terminal.copy") copySelection();
  else if (commandId === "terminal.paste") void terminalView.value?.pasteFromClipboard();
  else if (commandId === "terminal.clear") terminalView.value?.clear();
  else if (commandId === "terminal.history-suggestions") void nativeTools.value?.openHistory();
  else if (commandId === "terminal.reconnect" && ["exited", "failed", "closed"].includes(state.value) && !cleanupPending.value) void open();
  else if (commandId === "terminal.disconnect" && (!['exited', 'failed', 'closed'].includes(state.value) || cleanupPending.value)) void terminateForClose();
}
const stateLabel = computed(() => t(`localSession.states.${state.value}`));
const shellLabel = computed(() => session.value?.shellName || props.label);
const cleanupPending = computed(
  () => failure.value?.code === "processCleanupFailed",
);
const failureMessage = computed(() => {
  if (!failure.value) return openFailed.value ? t("localSession.failureFallback") : null;
  return te(failure.value.messageKey)
    ? t(failure.value.messageKey)
    : t("localSession.failureFallback");
});

function updateSummary(next: LocalSessionSummary) {
  session.value = next;
  failure.value = next.failureReason;
  emit("state", next.state, next);
}

function openSearch() {
  terminalTools.value?.openSearch();
}

function copySelection() {
  void terminalTools.value?.copySelection();
}

function pendingEventKey(event: LocalSessionEvent) {
  return `${event.sessionId}:${event.generation}`;
}

function pendingEventSize(event: LocalSessionEvent) {
  return event.payload.kind === "outputFrame"
    ? event.payload.frame.bytes.length
    : 256;
}

function bufferPendingEvent(event: LocalSessionEvent) {
  pendingEvents.push(event);
  pendingEventBytes += pendingEventSize(event);
  while (
    pendingEvents.length > PENDING_EVENT_MAX_COUNT
    || pendingEventBytes > PENDING_EVENT_MAX_BYTES
  ) {
    const outputIndex = pendingEvents.findIndex((candidate) =>
      candidate.payload.kind === "outputFrame" || candidate.payload.kind === "outputGap");
    const dropped = outputIndex >= 0
      ? pendingEvents.splice(outputIndex, 1)[0]
      : pendingEvents.shift();
    if (!dropped) break;
    pendingEventBytes = Math.max(0, pendingEventBytes - pendingEventSize(dropped));
    if (dropped.payload.kind === "outputFrame" || dropped.payload.kind === "outputGap") {
      pendingEventGaps.add(pendingEventKey(dropped));
    }
  }
}

function clearPendingEvents() {
  pendingEvents.splice(0);
  pendingEventBytes = 0;
  pendingEventGaps.clear();
}

function applyOutputItem(item: LocalSessionOutputItem) {
  const current = session.value;
  if (!current?.ptyId) return;
  if (item.kind === "frame") {
    const frame = item.payload;
    if (
      frame.sessionId !== current.sessionId
      || frame.generation !== current.generation
      || frame.ptyId !== current.ptyId
    ) return;
    const outputSeq = BigInt(frame.outputSeq);
    if (outputSeq <= lastAppliedOutputSeq) return;
    if (lastAppliedOutputSeq > 0n && outputSeq > lastAppliedOutputSeq + 1n) {
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
    || gap.ptyId !== current.ptyId
  ) return;
  const resumesAt = BigInt(gap.resumesAtOutputSeq);
  if (resumesAt <= lastAppliedOutputSeq) return;
  terminalView.value?.writeGap();
  lastAppliedOutputSeq = resumesAt - 1n;
}

function applyEvent(event: LocalSessionEvent) {
  if (!session.value || binding) {
    bufferPendingEvent(event);
    return;
  }
  if (
    event.sessionId !== session.value.sessionId
    || event.generation !== session.value.generation
  ) return;
  const eventSeq = BigInt(event.eventSeq);
  if (eventSeq <= lastAppliedEventSeq) return;
  lastAppliedEventSeq = eventSeq;
  switch (event.payload.kind) {
    case "stateChanged":
      updateSummary({
        ...session.value,
        state: event.payload.state,
        stateRevision: event.stateRevision,
        eventSeq: event.eventSeq,
        exit: event.payload.exit,
        failureReason: event.payload.failureReason,
        updatedAtUnixMs: event.occurredAtUnixMs,
      });
      if (event.payload.state === "running" && props.active) activateFromTab();
      break;
    case "attachmentChanged":
      if (event.payload.change === "attached" && event.payload.attachment.viewId === viewId) {
        attachment.value = event.payload.attachment;
        updateSummary({
          ...session.value,
          ptyId: event.payload.attachment.ptyId,
          stateRevision: event.stateRevision,
          attachmentRevision: event.payload.attachmentRevision,
          eventSeq: event.eventSeq,
          updatedAtUnixMs: event.occurredAtUnixMs,
        });
      } else if (
        event.payload.change === "detached"
        && event.payload.attachment.attachmentId === attachment.value?.attachmentId
      ) {
        attachment.value = null;
        applyFocusLease(null);
      }
      break;
    case "inputLeaseChanged":
      applyFocusLease(event.payload.lease ? { kind: "local", lease: event.payload.lease } : null);
      break;
    case "outputFrame":
      applyOutputItem({ kind: "frame", payload: event.payload.frame });
      break;
    case "outputGap":
      applyOutputItem({ kind: "gap", payload: event.payload.gap });
      break;
  }
}

function replayPendingEvents(sessionId: string, generation: string) {
  const key = `${sessionId}:${generation}`;
  const events = pendingEvents
    .splice(0)
    .filter((event) => event.sessionId === sessionId && event.generation === generation)
    .sort((left, right) => Number(BigInt(left.eventSeq) - BigInt(right.eventSeq)));
  pendingEventBytes = pendingEvents.reduce(
    (total, event) => total + pendingEventSize(event),
    0,
  );
  if (pendingEventGaps.delete(key)) terminalView.value?.writeGap();
  events.forEach(applyEvent);
}

async function open() {
  if (opening.value) return;
  opening.value = true;
  openFailed.value = false;
  binding = true;
  released = false;
  try {
    await nextTick();
    const size = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
    const response = await openLocalSession({ viewId, rows: size.rows, cols: size.cols }, applyEvent);
    // A restarted session begins at sequence 1. Retain existing scrollback,
    // but never deduplicate or label new-session input or output with the old process counter.
    lastAppliedEventSeq = 0n;
    lastAppliedOutputSeq = 0n;
    inputSequence = 0n;
    promptBoundaryInputSequence = null;
    attachment.value = response.attachment;
    updateSummary(response.session);
    binding = false;
    replayPendingEvents(response.session.sessionId, response.session.generation);
    startAttachmentHeartbeat();
    if (response.session.state === "running" && props.active) activateFromTab();
    if (props.active) terminalView.value?.focus();
  } catch {
    clearPendingEvents();
    openFailed.value = true;
    emit("state", "failed", session.value);
  } finally {
    binding = false;
    opening.value = false;
  }
}

async function attachExistingSession(resumeRenderedOutput = false) {
  const current = session.value;
  if (opening.value || !current) return;
  const renderedGeneration = current.generation;
  const renderedOutputSeq = lastAppliedOutputSeq;
  opening.value = true;
  binding = true;
  released = false;
  if (!resumeRenderedOutput) {
    lastAppliedEventSeq = 0n;
    lastAppliedOutputSeq = 0n;
  }
  try {
    const details = await getLocalSession(current.sessionId);
    const canResumeRenderedOutput = resumeRenderedOutput
      && details.session.generation === renderedGeneration;
    if (!canResumeRenderedOutput) {
      lastAppliedEventSeq = 0n;
      lastAppliedOutputSeq = 0n;
    }
    const response = await attachLocalSession({
      sessionId: details.session.sessionId,
      expectedGeneration: details.session.generation,
      expectedStateRevision: details.session.stateRevision,
      viewId,
      afterOutputSeq: canResumeRenderedOutput && renderedOutputSeq > 0n
        ? renderedOutputSeq.toString()
        : null,
    }, applyEvent);
    attachment.value = response.attachment;
    updateSummary({
      ...details.session,
      stateRevision: response.stateRevision,
      attachmentRevision: response.attachmentRevision,
    });
    const snapshotEventSeq = BigInt(details.session.eventSeq);
    if (snapshotEventSeq > lastAppliedEventSeq) lastAppliedEventSeq = snapshotEventSeq;
    response.replay.forEach(applyOutputItem);
    binding = false;
    replayPendingEvents(details.session.sessionId, details.session.generation);
    startAttachmentHeartbeat();
    if (details.session.state === "running" && props.active) activateFromTab();
  } catch {
    clearPendingEvents();
    emit("state", "failed", session.value);
  } finally {
    binding = false;
    opening.value = false;
  }
}

function currentFocusTarget() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!props.active || current?.state !== "running" || !current.ptyId || !currentAttachment?.ptyId) {
    return null;
  }
  return {
    kind: "local" as const,
    target: {
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      ptyId: current.ptyId,
      attachmentId: currentAttachment.attachmentId,
      viewId,
    },
  };
}

function applyFocusLease(inputLease: TerminalInputLease | null) {
  const nextLease = inputLease?.kind === "local" ? inputLease.lease : null;
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
  if (lease.value) void flushPendingResize();
}

function stopLeaseHeartbeat() {
  if (leaseTimer !== null) window.clearInterval(leaseTimer);
  leaseTimer = null;
}

function startLeaseHeartbeat() {
  stopLeaseHeartbeat();
  leaseTimer = window.setInterval(() => void renewLease(), 5_000);
}

function stopAttachmentHeartbeat() {
  if (attachmentHeartbeatTimer !== null) window.clearInterval(attachmentHeartbeatTimer);
  attachmentHeartbeatTimer = null;
}

function startAttachmentHeartbeat() {
  stopAttachmentHeartbeat();
  attachmentHeartbeatTimer = window.setInterval(() => void heartbeatAttachment(), 10_000);
}

async function heartbeatAttachment() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment || binding || released) return;
  try {
    attachment.value = await heartbeatLocalAttachment({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedAttachmentRevision: currentAttachment.attachmentRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId,
    });
  } catch {
    stopAttachmentHeartbeat();
    applyFocusLease(null);
    attachment.value = null;
    void attachExistingSession(true);
  }
}

async function renewLease() {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.ptyId || !currentAttachment || !currentLease || current.state !== "running") return;
  try {
    lease.value = await renewLocalInputLease({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      ptyId: current.ptyId,
      attachmentId: currentAttachment.attachmentId,
      viewId,
      leaseId: currentLease.leaseId,
      focusEpoch: currentLease.focusEpoch,
      inputEpoch: currentLease.inputEpoch,
    });
  } catch {
    applyFocusLease(null);
  }
}

async function send(value: string) {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.ptyId || !currentAttachment || !currentLease || !writable.value) {
    throw new Error("Local terminal input is unavailable");
  }
  inputSequence += 1n;
  // A prompt can arrive after typed-ahead input; do not mistake this line for empty input.
  promptBoundaryInputSequence = ["\r", "\n", "\u0003"].includes(value) ? inputSequence : null;
  await sendLocalInput({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    expectedStateRevision: current.stateRevision,
    ptyId: current.ptyId,
    attachmentId: currentAttachment.attachmentId,
    viewId,
    leaseId: currentLease.leaseId,
    focusEpoch: currentLease.focusEpoch,
    inputEpoch: currentLease.inputEpoch,
    clientSeq: inputSequence.toString(),
    bytes: Array.from(new TextEncoder().encode(value)),
  });
}

async function handleTerminalInput(value: string) {
  try {
    await send(value);
  } catch {
    // The PTY write is non-idempotent. Its IPC rejection may be
    // delivery-uncertain, so stop accepting input until Core grants a fresh
    // focus lease and tell the user to inspect terminal output before retrying.
    applyFocusLease(null);
    const current = session.value;
    tips.show({
      scope: `local-terminal-input:${current?.sessionId ?? props.paneId}:${current?.generation ?? "none"}`,
      tone: "error",
      title: t("quickCommands.runFailed"),
    });
  }
}


async function terminateForClose() {
  const current = session.value;
  if (
    !current
    || terminating.value
    || ["exited", "closed"].includes(current.state)
    || (current.state === "failed" && !cleanupPending.value)
  ) return;
  terminating.value = true;
  try {
    const summary = await terminateLocalSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
    });
    updateSummary(summary);
  } finally {
    terminating.value = false;
  }
}

async function releaseRendererBinding() {
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment || released) return;
  released = true;
  try {
    await detachLocalSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId,
      intent: "rendererUnavailable",
      confirmation: null,
    });
  } catch {
    // The Core liveness timeout performs the same bounded renderer cleanup.
  }
}

function registerInputTarget() {
  if (unregisterInputTarget) return;
  unregisterInputTarget = registerTerminalInputTarget({
    id: props.paneId,
    label: () => shellLabel.value,
    focusTarget: currentFocusTarget,
    applyFocusLease,
    canAcceptInput: () => writable.value,
    send,
  });
}

function activateFromTab() {
  if (!props.active) return;
  registerInputTarget();
  void focusTerminalInputTarget(props.paneId);
  terminalView.value?.focus();
}

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

defineExpose({ terminateForClose, activateFromTab, deactivateFromTab, runShortcut });

onMounted(() => {
  if (props.active) activateFromTab();
  window.addEventListener("beforeunload", releaseRendererBinding);
  if (session.value) void attachExistingSession();
  else if (!props.deferredStart) void open();
});

watch(() => props.active, (active) => {
  if (!active) deactivateFromTab();
});

onBeforeUnmount(() => {
  window.removeEventListener("beforeunload", releaseRendererBinding);
  stopLeaseHeartbeat();
  stopAttachmentHeartbeat();
  clearPendingEvents();
  deactivateFromTab();
  void releaseRendererBinding();
});
</script>

<template>
  <section
    class="local-terminal-pane"
    @pointerdown="activateTerminalSurface"
    @focusin="activateTerminalSurface"
  >
    <header class="local-terminal-pane__status">
      <div class="local-terminal-pane__identity">
        <NvxStatusLabel :tone="state === 'running' ? 'success' : state === 'failed' ? 'danger' : 'neutral'">
          {{ stateLabel }}
        </NvxStatusLabel>
        <span
          class="local-terminal-pane__shell"
          :title="shellLabel"
        >
          <NvxIcon
            :icon="SquareTerminal"
            :size="16"
          />
          {{ shellLabel }}
        </span>
      </div>
      <div class="local-terminal-pane__actions">
        <NvxNativeTerminalTools
          ref="nativeTools"
          :pane-id="paneId"
          :label="shellLabel"
          :host-id="null"
          :session="nativeScope"
          :active="active"
          :writable="writable"
          :draft="inputDraft"
          :current-draft="() => terminalView?.currentDraft() ?? null"
          :enable="enableNativeShell"
          @invalidate-draft="terminalView?.invalidateDraft()"
          @prompt="acceptNativePrompt"
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
          :show-layout-actions="active"
          @split="emit('split', $event)"
          @close="emit('close')"
        >
          <NvxButton
            v-if="!['exited', 'failed', 'closed'].includes(state) || cleanupPending"
            class="local-terminal-pane__terminate"
            variant="ghost"
            size="sm"
            :loading="terminating"
            @click="terminateForClose"
          >
            <NvxIcon
              :icon="Unplug"
              :size="16"
            />
            {{ t("localSession.terminate") }}
          </NvxButton>
          <NvxButton
            v-else
            variant="ghost"
            size="sm"
            :loading="opening"
            @click="open()"
          >
            <NvxIcon
              :icon="SquareTerminal"
              :size="16"
            />
            {{ t("localSession.restart") }}
          </NvxButton>
        </NvxTerminalPaneControls>
        <NvxTerminalPaneOverflowMenu
          :plugin-context-key="pluginContextKey"
          class="local-terminal-pane__overflow"
          :can-split-horizontal="canSplitHorizontal"
          :can-split-vertical="canSplitVertical"
          :has-selection="hasSelection"
          :show-layout-actions="active"
          show-session-action
          :session-action-label="['exited', 'failed', 'closed'].includes(state) && !cleanupPending ? t('localSession.restart') : t('localSession.terminate')"
          :session-action-icon="['exited', 'failed', 'closed'].includes(state) && !cleanupPending ? SquareTerminal : Unplug"
          :session-action-disabled="terminating || opening"
          :session-action-danger="!['exited', 'failed', 'closed'].includes(state) || cleanupPending"
          @search="openSearch"
          @copy="copySelection"
          @split="emit('split', $event)"
          @session="['exited', 'failed', 'closed'].includes(state) && !cleanupPending ? open() : terminateForClose()"
          @close="emit('close')"
        />
      </div>
    </header>
    <NvxPluginExtensionTarget
      class="local-terminal-pane__annotation"
      target-id="terminal.annotation"
      :instance-key="pluginContextKey"
      :display-label="shellLabel"
    />
    <NvxInlineNotice
      v-if="failureMessage"
      class="local-terminal-pane__failure"
      tone="error"
      :title="failureMessage"
    />
    <NvxTerminalView
      ref="terminalView"
      :pane-id="paneId"
      :shell-prompt-key="shellPromptKey"
      :read-only="!writable"
      :terminal-label="t('localSession.terminalLabel', { label: shellLabel })"
      :gap-label="t('localSession.outputGap')"
      @input="handleTerminalInput"
      @resize="resize"
      @selection-change="hasSelection = $event"
      @search-request="openSearch"
      @draft-change="inputDraft = $event"
      @bell-attention="emit('bellAttention', $event)"
    />
  </section>
</template>

<style scoped>
.local-terminal-pane {
  position: relative;
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  background: var(--nvx-color-terminal-bg);
}

.local-terminal-pane__status {
  container-name: local-terminal-pane-toolbar;
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

.local-terminal-pane__identity,
.local-terminal-pane__shell,
.local-terminal-pane__actions {
  display: inline-flex;
  min-width: 0;
  align-items: center;
}

.local-terminal-pane__overflow {
  display: none;
}

.local-terminal-pane__actions {
  flex: none;
  gap: var(--nvx-space-1);
}

.local-terminal-pane__identity {
  gap: var(--nvx-space-3);
}

.local-terminal-pane__shell {
  gap: var(--nvx-space-1);
  overflow: hidden;
  color: var(--nvx-color-terminal-muted);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.local-terminal-pane__terminate {
  color: var(--nvx-color-terminal-muted);
}

.local-terminal-pane__terminate:hover:not(:disabled) {
  background: var(--nvx-color-danger);
  color: var(--nvx-color-on-danger);
}

@container local-terminal-pane-toolbar (max-width: 560px) {
  .local-terminal-pane__actions :deep(.terminal-tools > .nvx-icon-button),
  .local-terminal-pane__actions :deep(.terminal-pane-controls) {
    display: none;
  }

  .local-terminal-pane__overflow {
    display: inline-flex;
  }
}

.local-terminal-pane__failure {
  position: absolute;
  z-index: var(--nvx-z-sticky);
  top: 44px;
  right: var(--nvx-space-3);
  left: var(--nvx-space-3);
}
</style>
