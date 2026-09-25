<script setup lang="ts">
import { Network, RotateCw, Unplug } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import {
  attachTelnetSession,
  detachTelnetSession,
  disconnectTelnetSession,
  heartbeatTelnetAttachment,
  openTelnetSession,
  reconnectTelnetSession,
  renewTelnetInputLease,
  resizeTelnetTerminal,
  sendTelnetInput,
} from "../../core-api/client";
import type {
  TelnetEndpoint,
  TelnetInputLease,
  TelnetRiskConfirmation,
  TelnetSessionAttachment,
  TelnetSessionEvent,
  TelnetSessionOutputItem,
  TelnetSessionState,
  TelnetSessionSummary,
  TerminalInputLease,
} from "../../core-api/generated/core-api";
import {
  focusTerminalInputTarget,
  isTerminalInputTargetFocused,
  registerTerminalInputTarget,
} from "../../terminal-input-target";
import {
  NvxButton,
  NvxCheckbox,
  NvxDialog,
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
  endpoint: TelnetEndpoint;
  existingSession: TelnetSessionSummary | null;
  deferredStart?: boolean;
  active: boolean;
  canSplitHorizontal: boolean;
  canSplitVertical: boolean;
  canSplitWorkspaceRight: boolean;
}>(), { deferredStart: false });

const emit = defineEmits<{
  activate: [paneId: string];
  state: [state: TelnetSessionState, summary: TelnetSessionSummary | null];
  bellAttention: [active: boolean];
  split: [direction: "horizontal" | "vertical"];
  splitWorkspaceRight: [];
  close: [];
}>();

const { t, te } = useI18n();
const terminalView = ref<TerminalViewExpose | null>(null);
const session = ref<TelnetSessionSummary | null>(props.existingSession);
const attachment = ref<TelnetSessionAttachment | null>(null);
const lease = ref<TelnetInputLease | null>(null);
const opening = ref(false);
const openFailed = ref(false);
const disconnecting = ref(false);
const reconnectDialogOpen = ref(false);
const acceptsCleartext = ref(false);
const acceptsMissingIdentity = ref(false);
const acceptsTampering = ref(false);
const pendingEvents: TelnetSessionEvent[] = [];
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
  if (!current?.socketId || !currentAttachment || !currentLease || !ownsLease.value) return null;
  const socketId = current.socketId;
  return {
    key: [
      current.sessionId,
      current.generation,
      currentAttachment.attachmentId,
      dimensions.rows,
      dimensions.cols,
    ].join(":"),
    send: (resizeSeq: string) => resizeTelnetTerminal({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      socketId,
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

const state = computed<TelnetSessionState>(() => session.value?.state
  ?? (openFailed.value ? "failed" : props.deferredStart ? "closed" : "connecting"));
const stateLabel = computed(() => t(`telnetSession.states.${state.value}`));
const failureMessage = computed(() => {
  const reason = session.value?.failureReason;
  if (!reason) return openFailed.value ? t("telnetSession.failureFallback") : null;
  return te(reason.messageKey) ? t(reason.messageKey) : t("telnetSession.failureFallback");
});
const ownsLease = computed(() => {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  return Boolean(
    current?.socketId
    && currentAttachment?.socketId === current.socketId
    && currentLease
    && currentLease.sessionId === current.sessionId
    && currentLease.generation === current.generation
    && currentLease.socketId === current.socketId
    && currentLease.attachmentId === currentAttachment.attachmentId
    && currentLease.viewId === props.paneId
    && currentLease.expiresAtUnixMs > Date.now(),
  );
});
const writable = computed(() => props.active
  && state.value === "running"
  && ownsLease.value
  && isTerminalInputTargetFocused(props.paneId, lease.value?.focusEpoch ?? null));
const reconnectConfirmed = computed(() => acceptsCleartext.value
  && acceptsMissingIdentity.value
  && acceptsTampering.value);

function updateSummary(next: TelnetSessionSummary) {
  session.value = next;
  emit("state", next.state, next);
}

function applyFocusLease(value: TerminalInputLease | null) {
  lease.value = value?.kind === "telnet" ? value.lease : null;
  if (lease.value) void nextTick(() => flushPendingResize());
}

function focusTarget() {
  if (!props.active) return null;
  const current = session.value;
  const currentAttachment = attachment.value;
  if (
    current?.state !== "running"
    || !current.socketId
    || !currentAttachment
    || currentAttachment.socketId !== current.socketId
  ) return null;
  return {
    kind: "telnet" as const,
    target: {
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      socketId: current.socketId,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
    },
  };
}

function applyOutput(item: TelnetSessionOutputItem) {
  const current = session.value;
  if (!current?.socketId) return;
  if (item.kind === "frame") {
    const frame = item.payload;
    if (
      frame.sessionId !== current.sessionId
      || frame.generation !== current.generation
      || frame.socketId !== current.socketId
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
    || gap.socketId !== current.socketId
  ) return;
  const resumesAt = BigInt(gap.resumesAtOutputSeq);
  if (resumesAt > lastOutputSequence) {
    terminalView.value?.writeGap();
    lastOutputSequence = resumesAt - 1n;
  }
}

function applyEvent(event: TelnetSessionEvent) {
  if (binding || !session.value) {
    if (pendingEvents.length < 256) pendingEvents.push(event);
    else pendingEventsDropped = true;
    return;
  }
  if (event.sessionId !== session.value.sessionId) return;
  const sequence = BigInt(event.eventSeq);
  if (sequence <= lastEventSequence) return;
  lastEventSequence = sequence;
  switch (event.payload.kind) {
    case "stateChanged":
      updateSummary(event.payload.session);
      if (event.payload.session.state !== "running") lease.value = null;
      if (event.payload.session.state === "running" && props.active) activateFromTab();
      break;
    case "attachmentAttached":
      if (event.payload.attachment.viewId === props.paneId) {
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

function currentRiskConfirmation(): TelnetRiskConfirmation {
  return {
    endpoint: props.endpoint,
    acceptsCleartextTransport: true,
    acceptsMissingServerIdentity: true,
    acceptsObservationAndTampering: true,
  };
}

async function open(force = false) {
  if (opening.value || (props.deferredStart && !force)) return;
  opening.value = true;
  openFailed.value = false;
  binding = true;
  try {
    await nextTick();
    const dimensions = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
    const response = await openTelnetSession({
      viewId: props.paneId,
      endpoint: props.endpoint,
      riskConfirmation: currentRiskConfirmation(),
      rows: dimensions.rows,
      cols: dimensions.cols,
    }, applyEvent);
    attachment.value = response.attachment;
    updateSummary(response.session);
    binding = false;
    replayPendingEvents();
    startTimers();
    if (props.active) activateFromTab();
  } catch {
    binding = false;
    openFailed.value = true;
    emit("state", "failed", null);
  } finally {
    opening.value = false;
  }
}

async function attachExisting() {
  const current = session.value;
  if (!current || current.state === "closed" || current.state === "failed") return;
  binding = true;
  try {
    const response = await attachTelnetSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      viewId: props.paneId,
      afterOutputSeq: lastOutputSequence > 0n ? lastOutputSequence.toString() : null,
    }, applyEvent);
    attachment.value = response.attachment;
    response.replay.forEach(applyOutput);
    binding = false;
    replayPendingEvents();
    startTimers();
    if (props.active) activateFromTab();
  } catch {
    binding = false;
    openFailed.value = true;
    emit("state", "failed", current);
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
    attachment.value = await heartbeatTelnetAttachment({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedAttachmentRevision: current.attachmentRevision,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
    });
  } catch {
    attachment.value = null;
    lease.value = null;
  }
}

async function renewLease() {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.socketId || !currentAttachment || !currentLease || !writable.value) return;
  try {
    lease.value = await renewTelnetInputLease({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
      socketId: current.socketId,
      attachmentId: currentAttachment.attachmentId,
      viewId: props.paneId,
      leaseId: currentLease.leaseId,
      focusEpoch: currentLease.focusEpoch,
      inputEpoch: currentLease.inputEpoch,
    });
  } catch {
    lease.value = null;
  }
}

async function send(value: string) {
  const current = session.value;
  const currentAttachment = attachment.value;
  const currentLease = lease.value;
  if (!current?.socketId || !currentAttachment || !currentLease || !writable.value) return;
  clientSequence += 1n;
  await sendTelnetInput({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    expectedStateRevision: current.stateRevision,
    socketId: current.socketId,
    attachmentId: currentAttachment.attachmentId,
    viewId: props.paneId,
    leaseId: currentLease.leaseId,
    focusEpoch: currentLease.focusEpoch,
    inputEpoch: currentLease.inputEpoch,
    clientSeq: clientSequence.toString(),
    bytes: [...new TextEncoder().encode(value)],
  });
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
    // Quick Commands are intentionally disabled for cleartext Telnet.
    canAcceptInput: () => false,
    canAcceptRawInput: () => writable.value,
    send,
  });
}

async function disconnectForClose() {
  const current = session.value;
  if (!current || ["closed", "failed"].includes(current.state)) return;
  disconnecting.value = true;
  try {
    updateSummary(await disconnectTelnetSession({
      sessionId: current.sessionId,
      expectedGeneration: current.generation,
      expectedStateRevision: current.stateRevision,
    }));
  } finally {
    disconnecting.value = false;
  }
}

function requestReconnect() {
  acceptsCleartext.value = false;
  acceptsMissingIdentity.value = false;
  acceptsTampering.value = false;
  reconnectDialogOpen.value = true;
}

async function confirmReconnect() {
  const current = session.value;
  if (!reconnectConfirmed.value) return;
  if (!current) {
    reconnectDialogOpen.value = false;
    await open(true);
    return;
  }
  const dimensions = terminalView.value?.dimensions() ?? { rows: 24, cols: 80 };
  const next = await reconnectTelnetSession({
    sessionId: current.sessionId,
    expectedGeneration: current.generation,
    expectedStateRevision: current.stateRevision,
    riskConfirmation: currentRiskConfirmation(),
    rows: dimensions.rows,
    cols: dimensions.cols,
  });
  lastEventSequence = 0n;
  lastOutputSequence = 0n;
  lease.value = null;
  updateSummary(next);
  reconnectDialogOpen.value = false;
}

async function release() {
  if (released) return;
  released = true;
  stopTimers();
  unregisterInputTarget?.();
  unregisterInputTarget = null;
  const current = session.value;
  const currentAttachment = attachment.value;
  if (!current || !currentAttachment) return;
  await detachTelnetSession({
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
  else if (commandId === "terminal.disconnect" && !["closed", "failed"].includes(state.value)) void disconnectForClose();
  else if (commandId === "terminal.history-suggestions") tips.show({ tone: "info", title: t("nativeTerminal.unsupportedSession") });
}
defineExpose({ disconnectForClose, activateFromTab, deactivateFromTab, runShortcut });

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
    class="telnet-pane"
    @pointerdown="activateTerminalSurface"
  >
    <header class="telnet-pane__toolbar">
      <span class="telnet-pane__identity">
        <NvxIcon
          :icon="Network"
          :size="16"
        />
        <strong>{{ label }}</strong>
        <small>{{ endpoint.address }}:{{ endpoint.port }}</small>
      </span>
      <span class="telnet-pane__actions">
        <NvxTerminalTools
          ref="terminalTools"
          :terminal="terminalView"
          :has-selection="hasSelection"
        />
        <NvxStatusLabel :tone="state === 'running' ? 'success' : state === 'failed' ? 'danger' : 'neutral'">
          {{ stateLabel }}
        </NvxStatusLabel>
        <NvxButton
          v-if="state === 'closed' || state === 'failed'"
          size="sm"
          variant="secondary"
          @click.stop="requestReconnect"
        >
          <NvxIcon
            :icon="RotateCw"
            :size="16"
          />
          {{ t("telnetSession.reconnect") }}
        </NvxButton>
        <NvxButton
          v-else
          size="sm"
          variant="ghost"
          :loading="disconnecting"
          @click.stop="disconnectForClose"
        >
          <NvxIcon
            :icon="Unplug"
            :size="16"
          />
          {{ t("telnetSession.disconnect") }}
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
    <NvxInlineNotice
      v-else-if="state === 'running'"
      class="telnet-pane__risk"
      tone="warning"
      :title="t('telnetSession.riskCompact')"
    />
    <NvxTerminalView
      ref="terminalView"
      :pane-id="paneId"
      class="telnet-pane__terminal"
      :read-only="!writable"
      :terminal-label="label"
      :gap-label="t('telnetSession.outputGap')"
      @input="send"
      @resize="resize"
      @selection-change="hasSelection = $event"
      @search-request="terminalTools?.openSearch()"
      @bell-attention="emit('bellAttention', $event)"
    />

    <NvxDialog
      :model-value="reconnectDialogOpen"
      :title="t('telnetSession.reconnectTitle')"
      :description="t('telnetSession.reconnectDescription')"
      :close-label="t('telnetSession.cancel')"
      @update:model-value="reconnectDialogOpen = $event"
    >
      <NvxCheckbox v-model="acceptsCleartext">
        {{ t("telnetSession.acceptCleartext") }}
      </NvxCheckbox>
      <NvxCheckbox v-model="acceptsMissingIdentity">
        {{ t("telnetSession.acceptMissingIdentity") }}
      </NvxCheckbox>
      <NvxCheckbox v-model="acceptsTampering">
        {{ t("telnetSession.acceptTampering") }}
      </NvxCheckbox>
      <template #actions>
        <NvxButton
          variant="ghost"
          @click="reconnectDialogOpen = false"
        >
          {{ t("telnetSession.cancel") }}
        </NvxButton>
        <NvxButton
          :disabled="!reconnectConfirmed"
          @click="confirmReconnect"
        >
          {{ t("telnetSession.reconnect") }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>

<style scoped>
.telnet-pane {
  display: grid;
  grid-template-rows: auto auto minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  background: var(--nvx-color-terminal-bg);
}

.telnet-pane__toolbar,
.telnet-pane__identity,
.telnet-pane__actions {
  display: flex;
  align-items: center;
}

.telnet-pane__toolbar {
  justify-content: space-between;
  gap: 12px;
  min-height: 42px;
  padding: 0 8px 0 12px;
  border-bottom: 1px solid var(--nvx-color-border-subtle);
  background: var(--nvx-color-surface-raised);
}

.telnet-pane__identity,
.telnet-pane__actions { gap: 8px; }
.telnet-pane__identity { min-width: 0; }
.telnet-pane__identity strong,
.telnet-pane__identity small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.telnet-pane__identity small { color: var(--nvx-color-text-muted); }
.telnet-pane__risk { margin: 8px 8px 0; }
.telnet-pane__terminal { min-height: 0; }
</style>
