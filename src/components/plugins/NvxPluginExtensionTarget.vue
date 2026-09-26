<script setup lang="ts">
import { isTauri } from "@tauri-apps/api/core";
import { createPinia, getActivePinia } from "pinia";
import { computed, onBeforeUnmount, onMounted, ref, watch, type ComponentPublicInstance } from "vue";
import { useI18n } from "vue-i18n";

import type { PluginUiContribution, PluginUiFieldValue } from "../../core-api/generated/core-api";
import { openPluginHostApproval, parseCoreApiError } from "../../core-api/client";
import {
  applyPluginHostDomOperations,
  capturePluginHostDomSnapshot,
  pluginHostDomEpoch,
} from "../../plugins/hostDomBroker";
import {
  usePluginExtensionsStore,
  type PluginTargetLease,
} from "../../stores/pluginExtensions";
import { useTipsStore, type NvxTipTone } from "../../stores/tips";
import { NvxButton, NvxInlineNotice, NvxStatusLabel } from "../ui";
import NvxPluginUiDocument from "./NvxPluginUiDocument.vue";

const props = withDefaults(defineProps<{
  targetId: string;
  instanceKey?: string;
  displayLabel?: string;
  showIdentity?: boolean;
  selectedPluginId?: string | null;
  routePath?: string;
  disabled?: boolean;
}>(), {
  instanceKey: "global",
  displayLabel: undefined,
  showIdentity: true,
  selectedPluginId: undefined,
  routePath: undefined,
  disabled: false,
});

const emit = defineEmits<{
  availability: [count: number];
  catalog: [items: Array<Pick<PluginUiContribution, "pluginId" | "pluginName" | "icon">>];
  openSettings: [pluginId: string, pluginName: string, fieldKey: string];
}>();

const extensions = usePluginExtensionsStore(getActivePinia() ?? createPinia());
const tips = useTipsStore(getActivePinia() ?? createPinia());
const { t, te } = useI18n();
const lease = ref<PluginTargetLease | null>(null);
let openGeneration = 0;
let disposed = false;
const loadFailed = ref(false);
const opening = ref(false);
let loadGeneration = 0;
const terminalSuggestion = ref<{ text: string; description: string | null } | null>(null);
const runtimeProjectionEvents = [
  "norishell:plugin-runtime-ready",
  "norishell:plugin-runtime-invalidated",
] as const;
const contributions = computed(() => (loadFailed.value ? [] : extensions.forContext(lease.value?.context ?? null)).filter(
  (item) => !item.routePaths || (props.routePath !== undefined && item.routePaths.includes(props.routePath)),
));
const visibleContributions = computed(() => props.selectedPluginId === undefined
  ? contributions.value
  : contributions.value.filter((item) => item.pluginId === props.selectedPluginId));
const loading = computed(() => (
  opening.value || (lease.value ? extensions.loadingHandles.has(lease.value.context.contextHandle) : false)
));
const feedbackScope = computed(() => `plugin-action:${props.targetId}:${props.instanceKey}`);

function showFeedback(title: string, tone: NvxTipTone = "info", message?: string) {
  tips.show({
    scope: feedbackScope.value,
    tone,
    title,
    message,
    durationMs: tone === "error" || tone === "warning" ? 0 : undefined,
  });
}

function syncFailureMessage(profile: string, code: string | null, diagnostic?: string | null, httpStatus?: number | null) {
  const key = code ? `plugins.sshSync.errors.${code}` : "";
  const message = key && te(key)
    ? t(key)
    : t("plugins.sshSync.operationFailed", { profile, code: code ?? "unknown" });
  const withStatus = httpStatus === null || httpStatus === undefined ? message : `${message} (HTTP ${httpStatus})`;
  return diagnostic ? `${withStatus} [${diagnostic}]` : withStatus;
}

async function loadContributions(current: PluginTargetLease) {
  const generation = ++loadGeneration;
  try {
    await extensions.loadTargetContributions(current.context);
    if (!disposed && generation === loadGeneration && lease.value === current) loadFailed.value = false;
  } catch {
    if (!disposed && generation === loadGeneration && lease.value === current) {
      loadFailed.value = true;
      terminalSuggestion.value = null;
    }
  }
}

async function open() {
  if (!isTauri()) return;
  const generation = ++openGeneration;
  ++loadGeneration;
  opening.value = true;
  loadFailed.value = false;
  terminalSuggestion.value = null;
  const { targetId, instanceKey, displayLabel } = props;
  const previous = lease.value;
  lease.value = null;
  try {
    if (previous) await extensions.releaseTarget(previous).catch(() => undefined);
    if (disposed || generation !== openGeneration) return;
    const acquired = await extensions.acquireTarget(targetId, instanceKey, displayLabel);
    if (disposed || generation !== openGeneration) {
      await extensions.releaseTarget(acquired).catch(() => undefined);
      return;
    }
    lease.value = acquired;
    await loadContributions(lease.value);
  } catch {
    if (!disposed && generation === openGeneration) loadFailed.value = true;
  } finally {
    if (!disposed && generation === openGeneration) opening.value = false;
  }
}

async function refresh() {
  const current = lease.value;
  if (!current) {
    await open();
    return;
  }
  await loadContributions(current);
}

// Activation changes initialize the newly visible tools. Document revisions are
// deliberately excluded so an on-open response cannot recursively reopen itself.
// Hook changes take effect at the next activation, not during their own response.
const automaticOpenKey = computed(() => JSON.stringify(
  props.disabled || !lease.value ? [] : visibleContributions.value
    .map((item) => [item.pluginId, item.instanceGeneration, item.packageSha256,
      item.target.contextHandle, item.target.targetRevision]),
));
let automaticActivation = "";
let automaticAttempts = new Set<string>();
let automaticOpening = false;

async function initializeVisibleContributions() {
  if (automaticOpening) return;
  automaticOpening = true;
  try {
    while (!disposed) {
      const activation = automaticOpenKey.value;
      if (activation !== automaticActivation) {
        automaticActivation = activation;
        automaticAttempts = new Set();
      }
      if (props.disabled || !lease.value || extensions.busyActionKey !== null) return;
      const contribution = visibleContributions.value.find((item) => (
        !automaticAttempts.has(item.pluginId)
      ));
      if (!contribution) return;
      automaticAttempts.add(contribution.pluginId);
      if (contribution.onOpenActionId) {
        await invoke(contribution, contribution.onOpenActionId, [], true);
      }
    }
  } finally {
    automaticOpening = false;
  }
}

watch([automaticOpenKey, () => extensions.busyActionKey], () => {
  void initializeVisibleContributions().catch(() => undefined);
}, { flush: "post" });

function handleRuntimeProjectionChanged() {
  void refresh().catch(() => undefined);
}

async function invoke(
  contribution: (typeof contributions.value)[number],
  actionId: string,
  fields: PluginUiFieldValue[],
  automatic = false,
) {
  if (props.disabled) return null;
  const settingsActionPrefix = "norishell.openSettings:";
  if (actionId.startsWith(settingsActionPrefix)) {
    const fieldKey = actionId.slice(settingsActionPrefix.length);
    if (props.targetId === "app.page" && !automatic && fields.length === 0 && fieldKey
      && lease.value?.context.contextHandle === contribution.target.contextHandle
      && visibleContributions.value.includes(contribution)
      && contribution.document.nodes.some((node) => node.kind === "button"
        && node.actionId === actionId && !node.disabled)) {
      emit("openSettings", contribution.pluginId, contribution.pluginName, fieldKey);
    }
    return null;
  }
  terminalSuggestion.value = null;
  if (automatic) tips.dismissScope(feedbackScope.value);
  const hostDomEpoch = pluginHostDomEpoch(contribution.pluginId);
  const hostDomSnapshot = capturePluginHostDomSnapshot(contribution.target.contextHandle);
  const response = await extensions.invokeAction(
    contribution,
    actionId,
    fields,
    hostDomSnapshot,
    automatic,
  );
  if (props.disabled || disposed || lease.value?.context.contextHandle !== contribution.target.contextHandle
    || lease.value.context.targetRevision !== contribution.target.targetRevision) return null;
  if (!response) {
    if (automatic) return null;
    const error = parseCoreApiError(extensions.error);
    const permissionDenied = error?.code === "plugin.capability_denied";
    const title = permissionDenied
      ? t("plugins.toolbar.permissionRequired")
      : error?.messageKey && te(error.messageKey)
        ? t(error.messageKey, error.params)
        : t("plugins.toolbar.actionFailed");
    showFeedback(title, permissionDenied ? "warning" : "error");
    return null;
  }
  tips.dismissScope(feedbackScope.value);
  if (response.hostDomOperations !== null) {
    if (!applyPluginHostDomOperations(
      contribution.pluginId,
      response.hostDomOperations,
      hostDomEpoch,
    )) {
      showFeedback(t("plugins.toolbar.actionFailed"), "error");
      return null;
    }
  }
  if (response.clipboardText !== null) {
    try {
      await navigator.clipboard.writeText(response.clipboardText);
      showFeedback(t("plugins.toolbar.copied"), "success");
    } catch {
      showFeedback(t("plugins.toolbar.copyFailed"), "error");
    }
  }
  if (response.hostApprovalId !== null) {
    try {
      await openPluginHostApproval(response.hostApprovalId);
      showFeedback(t("plugins.hostApproval.opened"), "success");
    } catch {
      showFeedback(t("plugins.hostApproval.openFailed"), "error");
    }
  }
  if (response.sshSyncStatus !== null) {
    const status = response.sshSyncStatus;
    const tone = status.operationState === "failed"
      ? "error"
      : status.operationState === "needsReview" ? "warning" : "info";
    const title = status.operationState === "failed" || status.stableErrorCode !== null
      ? syncFailureMessage(status.profileId, status.stableErrorCode, status.diagnosticCode, status.httpStatus)
      : status.operationState === "succeeded"
      ? t("plugins.sshSync.operationSucceeded", { profile: status.profileId })
      : status.operationState === "needsReview"
        ? t("plugins.sshSync.operationNeedsReview", { profile: status.profileId })
        : t("plugins.sshSync.operationIdle", { profile: status.profileId });
    if (!automatic) {
      showFeedback(title, tone);
    }
  }
  terminalSuggestion.value = response.terminalInputSuggestion;
  return {
    contribution: response.contribution,
    closeDialogOnSuccess: contribution.pluginId === "com.norishell.self-host-sync"
      && actionId === "sync.login"
      && response.sshSyncStatus?.accountState === "connected"
      && response.sshSyncStatus.operationState === "idle"
      && response.sshSyncStatus.stableErrorCode === null,
  };
}

async function refreshAfterSettingsChange(pluginId: string) {
  if (disposed || props.disabled || props.targetId !== "app.page") return;
  const contribution = visibleContributions.value.find((item) => item.pluginId === pluginId);
  if (contribution?.onOpenActionId) {
    await invoke(contribution, contribution.onOpenActionId, [], true);
  }
}

defineExpose({ refreshAfterSettingsChange });

// A timer belongs to one visible target activation, not to a document revision.
// Failed background refreshes pause until the next activation; interactive
// actions retain their normal feedback and can recover the displayed document.
const refreshTimers = new Map<string, ReturnType<typeof setTimeout>>();
const refreshPaused = ref(new Set<string>());
const visiblePluginIds = ref(new Set<string>());
const pageVisible = ref(document.visibilityState !== "hidden");
const observedElements = new Map<Element, string>();
let visibilityObserver: IntersectionObserver | null = null;
let refreshActivation = "";
const refreshActivationKey = computed(() => JSON.stringify([
  props.disabled, pageVisible.value, lease.value?.context.contextHandle,
  visibleContributions.value.map((item) => [item.pluginId, item.packageSha256,
    item.instanceGeneration, item.target.targetRevision, item.autoRefresh]),
]));

function clearRefreshTimers() {
  for (const timer of refreshTimers.values()) clearTimeout(timer);
  refreshTimers.clear();
}

function observeContribution(element: Element | ComponentPublicInstance | null, pluginId: string) {
  if (!(element instanceof Element)) return;
  if (observedElements.has(element)) return;
  observedElements.set(element, pluginId);
  if (visibilityObserver) visibilityObserver.observe(element);
  else visiblePluginIds.value = new Set([...visiblePluginIds.value, pluginId]);
}

function synchronizeRefreshTimers() {
  const activation = refreshActivationKey.value;
  if (activation !== refreshActivation) {
    clearRefreshTimers();
    refreshPaused.value.clear();
    refreshActivation = activation;
  }
  for (const [element, pluginId] of observedElements) {
    if (!element.isConnected) {
      visibilityObserver?.unobserve(element);
      observedElements.delete(element);
      if (![...observedElements].some(([candidate, id]) => candidate.isConnected && id === pluginId)) {
        visiblePluginIds.value = new Set([...visiblePluginIds.value].filter((id) => id !== pluginId));
      }
    }
  }
  const eligible = visibleContributions.value.filter((item) => !disposed && !props.disabled
    && pageVisible.value && visiblePluginIds.value.has(item.pluginId)
    && item.autoRefresh && !refreshPaused.value.has(item.pluginId));
  const eligibleIds = new Set(eligible.map((item) => item.pluginId));
  for (const [pluginId, timer] of refreshTimers) {
    if (!eligibleIds.has(pluginId)) { clearTimeout(timer); refreshTimers.delete(pluginId); }
  }
  for (const item of eligible) {
    if (refreshTimers.has(item.pluginId)) continue;
    const hook = item.autoRefresh!;
    const intervalMs = Math.max(5000, Math.min(300000, hook.intervalMs));
    const isCurrent = () => refreshTimers.get(item.pluginId) === timer && activation === refreshActivation;
    const timer = setTimeout(async () => {
      if (!isCurrent() || disposed || props.disabled
        || !pageVisible.value || !visiblePluginIds.value.has(item.pluginId)) return;
      // Keep the timer entry while awaiting so a reactive update cannot overlap.
      if (extensions.busyActionKey === null) {
        const current = visibleContributions.value.find((candidate) => candidate.pluginId === item.pluginId);
        if (current) {
          try {
            const ok = await invoke(current, hook.actionId, [], true);
            if (!ok && isCurrent()) refreshPaused.value.add(item.pluginId);
          } catch {
            if (isCurrent()) refreshPaused.value.add(item.pluginId);
          }
        }
      }
      if (!isCurrent()) return;
      refreshTimers.delete(item.pluginId);
      synchronizeRefreshTimers();
    }, intervalMs);
    refreshTimers.set(item.pluginId, timer);
  }
}

function handlePageVisibility() {
  pageVisible.value = document.visibilityState !== "hidden";
}

watch([refreshActivationKey, visiblePluginIds], synchronizeRefreshTimers, { flush: "post" });

async function copyTerminalSuggestion() {
  if (!terminalSuggestion.value) return;
  try {
    await navigator.clipboard.writeText(terminalSuggestion.value.text);
    showFeedback(t("plugins.toolbar.copied"), "success");
  } catch {
    showFeedback(t("plugins.toolbar.copyFailed"), "error");
  }
}

watch(() => [props.targetId, props.instanceKey, props.displayLabel], () => void open().catch(() => undefined));
watch(contributions, (next) => {
  emit("availability", next.length);
  emit("catalog", next.map(({ pluginId, pluginName, icon }) => ({ pluginId, pluginName, icon })));
}, { immediate: true });
onMounted(() => {
  if (typeof IntersectionObserver !== "undefined") {
    visiblePluginIds.value = new Set();
    visibilityObserver = new IntersectionObserver((entries) => {
      const next = new Set(visiblePluginIds.value);
      for (const entry of entries) {
        const pluginId = observedElements.get(entry.target);
        if (pluginId) { if (entry.isIntersecting) next.add(pluginId); else next.delete(pluginId); }
      }
      visiblePluginIds.value = next;
    });
    for (const element of observedElements.keys()) visibilityObserver.observe(element);
  }
  document.addEventListener("visibilitychange", handlePageVisibility);
  for (const event of runtimeProjectionEvents) {
    window.addEventListener(event, handleRuntimeProjectionChanged);
  }
  void open().catch(() => undefined);
});
onBeforeUnmount(() => {
  disposed = true;
  clearRefreshTimers();
  visibilityObserver?.disconnect();
  document.removeEventListener("visibilitychange", handlePageVisibility);
  openGeneration += 1;
  for (const event of runtimeProjectionEvents) {
    window.removeEventListener(event, handleRuntimeProjectionChanged);
  }
  const current = lease.value;
  lease.value = null;
  if (current) void extensions.releaseTarget(current).catch(() => undefined);
});
</script>

<template>
  <div
    v-if="contributions.length || loading || loadFailed"
    class="plugin-extension-target"
    :data-plugin-target="targetId"
    :aria-busy="loading"
  >
    <NvxInlineNotice
      v-if="loadFailed && targetId === 'app.page'"
      tone="error"
      :title="t('plugins.tools.loadFailed')"
      data-plugin-protected
    >
      <NvxButton
        size="sm"
        variant="secondary"
        :disabled="disabled || loading"
        @click="open"
      >
        {{ t("plugins.tools.retryLoad") }}
      </NvxButton>
    </NvxInlineNotice>
    <NvxButton
      v-else-if="loadFailed"
      size="sm"
      variant="ghost"
      :title="t('plugins.tools.loadFailed')"
      :aria-label="`${t('plugins.tools.loadFailed')} · ${t('plugins.tools.retryLoad')}`"
      :disabled="disabled || loading"
      data-plugin-protected
      @click="open"
    >
      {{ t("plugins.tools.retryLoad") }}
    </NvxButton>
    <section
      v-for="contribution in visibleContributions"
      :ref="(element) => observeContribution(element, contribution.pluginId)"
      :key="`${contribution.pluginId}:${contribution.instanceGeneration}:${contribution.target.contextHandle}`"
      class="plugin-extension-target__contribution"
      :class="{ 'plugin-extension-target__contribution--refresh-paused': refreshPaused.has(contribution.pluginId) }"
      :aria-label="contribution.pluginName"
    >
      <header
        v-if="showIdentity"
        class="plugin-extension-target__identity"
        data-plugin-protected
      >
        <span>{{ t("plugins.pendingInput.plugin") }}</span>
        <strong>{{ contribution.pluginName }}</strong>
      </header>
      <NvxStatusLabel
        v-if="refreshPaused.has(contribution.pluginId)"
        class="plugin-extension-target__refresh-paused"
        tone="warning"
        role="status"
        data-plugin-protected
      >
        {{ t("plugins.tools.autoRefreshPaused") }}
      </NvxStatusLabel>
      <div data-plugin-dom-surface>
        <NvxPluginUiDocument
          :contribution="contribution"
          :busy="disabled || extensions.busyPluginId === contribution.pluginId"
          :actions-blocked="extensions.busyActionKey !== null"
          :request-action="(actionId, fields) => invoke(contribution, actionId, fields)"
        />
      </div>
    </section>
    <aside
      v-if="terminalSuggestion"
      class="plugin-extension-target__suggestion"
    >
      <div>
        <strong>{{ t("plugins.terminalSuggestion.title") }}</strong>
        <small v-if="terminalSuggestion.description">{{ terminalSuggestion.description }}</small>
        <code>{{ terminalSuggestion.text }}</code>
      </div>
      <NvxButton
        size="sm"
        variant="secondary"
        @click="copyTerminalSuggestion"
      >
        {{ t("plugins.terminalSuggestion.copy") }}
      </NvxButton>
    </aside>
  </div>
</template>

<style scoped>
.plugin-extension-target { display: contents; }
.plugin-extension-target__contribution { display: grid; min-width: 0; gap: var(--nvx-space-1); }
.plugin-extension-target__contribution--refresh-paused { display: flex; min-width: max-content; align-items: center; gap: var(--nvx-space-2); }
.plugin-extension-target__contribution--refresh-paused > [data-plugin-dom-surface] { flex: 0 0 auto; }
.plugin-extension-target__refresh-paused { flex: none; font-size: var(--nvx-font-size-xs); line-height: 20px; white-space: nowrap; }
.plugin-extension-target__identity { display: flex; min-width: 0; align-items: center; gap: var(--nvx-space-2); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.plugin-extension-target__identity strong { overflow: hidden; color: var(--nvx-color-text-primary); text-overflow: ellipsis; white-space: nowrap; }
.plugin-extension-target__suggestion { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: var(--nvx-space-3); padding: var(--nvx-space-2) var(--nvx-space-3); border: var(--nvx-border-width) solid var(--nvx-color-border); background: var(--nvx-color-bg-subtle); color: var(--nvx-color-text-primary); }
.plugin-extension-target__suggestion > div { display: grid; min-width: 0; gap: var(--nvx-space-1); }
.plugin-extension-target__suggestion small { color: var(--nvx-color-text-secondary); }
.plugin-extension-target__suggestion code { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
