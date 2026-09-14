<script setup lang="ts">
import { History, Copy, Trash2, X } from "lucide-vue-next";
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { deleteNativeTerminalHistory, listNativeTerminalHistory } from "../../core-api/native-terminal";
import type { NativeTerminalHistoryEntry, NativeTerminalHistoryScope, NativeTerminalSessionScope, NativeTerminalSessionStatus, NativeTerminalShellKind } from "../../core-api/generated/core-api";
import { captureTerminalInput, type TerminalInputTicket } from "../../terminal-input-target";
import { useNativeTerminalStore } from "../../stores/nativeTerminal";
import { useTipsStore } from "../../stores/tips";
import { historySuggestionSuffix } from "../../terminal/draft";
import { NvxButton, NvxCheckbox, NvxDialog, NvxField, NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";

const props = defineProps<{
  paneId: string;
  label: string;
  hostId: string | null;
  session: NativeTerminalSessionScope | null;
  active: boolean;
  writable: boolean;
  draft: string | null;
  currentDraft: () => string | null;
  enable: (kind: NativeTerminalShellKind) => Promise<NativeTerminalSessionStatus>;
}>();
const emit = defineEmits<{ prompt: [status: NativeTerminalSessionStatus | null]; focus: []; invalidateDraft: [] }>();
const { t, locale } = useI18n();
const router = useRouter();
const native = useNativeTerminalStore();
const tips = useTipsStore();
const open = ref(false);
const section = ref("history");
const query = ref("");
const allScopes = ref(false);
const history = shallowRef<NativeTerminalHistoryEntry[]>([]);
const suggestions = shallowRef<NativeTerminalHistoryEntry[]>([]);
const busy = ref(false);
const loading = ref(false);
const historyError = ref(false);
const selectedShell = ref<NativeTerminalShellKind>("bash");
const confirmedPrompt = ref(false);
const promptTicket = shallowRef<TerminalInputTicket | null>(null);
const dismissedDraft = ref<string | null>(null);
let historyRevision = 0;
let suggestionRevision = 0;
let setupRevision = 0;
let historyTimer: ReturnType<typeof setTimeout> | undefined;
let suggestionTimer: ReturnType<typeof setTimeout> | undefined;
let disposed = false;

const status = computed(() => native.sessionStatus(props.session));
const config = computed(() => native.settingsSnapshot?.settings);
const historyAvailable = computed(() => native.available && Boolean(config.value?.historyEnabled) && Boolean(native.settingsSnapshot?.historyAvailable));
const historyScope = computed<NativeTerminalHistoryScope | null>(() => props.hostId ? { kind: "host", hostId: props.hostId } : props.session?.kind === "local" ? { kind: "local" } : null);
const canSuggest = computed(() => historyScope.value !== null && historyAvailable.value && !config.value?.historyPaused && props.active && props.writable
  && status.value?.captureState === "ready" && status.value?.activity === "prompt" && status.value?.capturesCommand && props.draft !== null);
const sections = computed(() => [
  { value: "history", label: t("nativeTerminal.history") }, { value: "integration", label: t("nativeTerminal.integration") },
]);
const scopeOptions = computed(() => [
  ...(historyScope.value ? [{ value: "current", label: t(props.hostId ? "nativeTerminal.thisHost" : "nativeTerminal.local") }] : []),
  { value: "all", label: t("nativeTerminal.global") },
]);
const shellOptions = [ { value: "bash", label: "Bash" }, { value: "zsh", label: "Zsh" }, { value: "fish", label: "Fish" }, { value: "powerShell", label: "PowerShell" } ];
const promptKey = computed(() => status.value?.captureState === "ready" && status.value.activity === "prompt" && status.value.promptObserved
  ? `${status.value.session.sessionId}:${status.value.session.generation}:${status.value.promptSequence}` : null);
watch(promptKey, () => emit("prompt", promptKey.value ? status.value : null), { immediate: true });

function notify(key: string, tone: "success" | "error" = "success") {
  tips.show({ scope: `native-terminal:${props.paneId}`, tone, title: t(`nativeTerminal.${key}`) });
}
function clearHistoryProjection() { historyRevision++; suggestionRevision++; history.value = []; suggestions.value = []; }
watch(() => native.historyEpoch, () => { clearHistoryProjection(); if (open.value) void loadHistory(); });
watch([() => props.session, () => props.hostId], () => {
  clearHistoryProjection();
  if (open.value) void loadHistory();
}, { deep: true });

function historyRequestKey() {
  return JSON.stringify({
    scope: allScopes.value ? null : historyScope.value,
    query: query.value,
    session: props.session,
  });
}

function suggestionRequestKey(draft: string) {
  return JSON.stringify({ scope: historyScope.value, draft, session: props.session });
}

async function loadHistory() {
  const revision = ++historyRevision;
  history.value = [];
  historyError.value = false;
  if (!open.value || section.value !== "history" || !historyAvailable.value) { loading.value = false; return; }
  loading.value = true;
  const epoch = native.historyEpoch;
  const requestKey = historyRequestKey();
  const scope = allScopes.value ? null : historyScope.value;
  const search = query.value;
  try {
    const entries = await listNativeTerminalHistory({ scope, query: search, limit: 50 });
    if (!disposed && revision === historyRevision && epoch === native.historyEpoch && requestKey === historyRequestKey() && open.value && historyAvailable.value) history.value = entries;
  } catch { if (!disposed && revision === historyRevision && requestKey === historyRequestKey()) historyError.value = true; }
  finally { if (revision === historyRevision && requestKey === historyRequestKey()) loading.value = false; }
}
watch([query, allScopes, section], () => {
  historyRevision++;
  history.value = [];
  clearTimeout(historyTimer);
  historyTimer = setTimeout(() => { void loadHistory(); }, 150);
});

watch([() => props.draft, canSuggest, () => native.historyEpoch, () => props.session, () => props.hostId], () => {
  const revision = ++suggestionRevision;
  suggestions.value = [];
  clearTimeout(suggestionTimer);
  if (!canSuggest.value || !props.draft || props.draft.length < 2 || props.draft === dismissedDraft.value || open.value) return;
  const draft = props.draft;
  const epoch = native.historyEpoch;
  const requestKey = suggestionRequestKey(draft);
  suggestionTimer = setTimeout(async () => {
    try {
      const entries = await listNativeTerminalHistory({ scope: historyScope.value, query: draft, limit: 50 });
      if (disposed || revision !== suggestionRevision || epoch !== native.historyEpoch || requestKey !== suggestionRequestKey(draft) || !canSuggest.value || props.draft !== draft || open.value) return;
      suggestions.value = entries.filter((entry) => historySuggestionSuffix(entry.command, draft) !== null).slice(0, 4);
    } catch { /* Automatic suggestion failure never interrupts the terminal; the manual panel retains a recoverable error. */ }
  }, 200);
});

async function openHistory() {
  if (open.value || busy.value) return;
  const revision = ++setupRevision;
  suggestions.value = [];
  const ticket = captureTerminalInput(props.paneId);
  if (ticket && !await ticket.suspend()) { await ticket.cancel(); return; }
  if (disposed || revision !== setupRevision) { await ticket?.cancel(); return; }
  promptTicket.value = ticket;
  confirmedPrompt.value = false;
  if (status.value) selectedShell.value = status.value.shellKind;
  else if (props.session?.kind === "local" && /zsh/i.test(props.label)) selectedShell.value = "zsh";
  section.value = "history";
  query.value = "";
  allScopes.value = !historyScope.value;
  open.value = true;
  await native.refresh();
  await loadHistory();
}

async function close() {
  setupRevision++;
  open.value = false;
  clearHistoryProjection();
  const ticket = promptTicket.value;
  promptTicket.value = null;
  await ticket?.cancel();
  if (!disposed) emit("focus");
}

async function enableIntegration() {
  const ticket = promptTicket.value;
  if (!ticket || !confirmedPrompt.value || busy.value) return;
  busy.value = true;
  emit("prompt", null);
  try {
    await ticket.perform(() => props.enable(selectedShell.value));
    notify("enabled");
    await native.refresh();
  } catch { notify("enableFailed", "error"); }
  finally { busy.value = false; await close(); }
}

function insertionSuffix(entry: NativeTerminalHistoryEntry) {
  if (status.value?.activity !== "prompt" || status.value.captureState !== "ready" || !status.value.capturesCommand || !props.active || !props.writable || !historyAvailable.value) return null;
  return historySuggestionSuffix(entry.command, props.currentDraft());
}
async function insert(entry: NativeTerminalHistoryEntry) {
  const suffix = insertionSuffix(entry);
  const ticket = open.value ? promptTicket.value : captureTerminalInput(props.paneId);
  if (suffix === null || !ticket || busy.value) { notify("insertUnavailable", "error"); return; }
  const draft = props.currentDraft();
  busy.value = true;
  // Between the check and dispatch, which contain no await, the ticket and Core still make the final focus and generation validation.
  const result = props.currentDraft() === draft ? await ticket.send(suffix) : "unavailable";
  // User input can still arrive during writer acknowledgement, so do not append the suffix to a changed draft again.
  emit("invalidateDraft");
  if (result !== "sent") notify(result === "unavailable" ? "insertUnavailable" : "insertFailed", "error");
  busy.value = false;
  suggestions.value = [];
  if (open.value) await close();
  else emit("focus");
}
async function copy(entry: NativeTerminalHistoryEntry) {
  try { await navigator.clipboard.writeText(entry.command); notify("copied"); }
  catch { notify("copyFailed", "error"); }
}
async function remove(entry: NativeTerminalHistoryEntry) {
  if (busy.value) return;
  busy.value = true;
  try { await deleteNativeTerminalHistory(entry.entryId); notify("removed"); clearHistoryProjection(); await loadHistory(); }
  catch { notify("removeFailed", "error"); }
  finally { busy.value = false; }
}
function timestamp(value: number) { return new Date(value).toLocaleString(locale.value); }
async function showSettings() { await close(); await router.push({ path: "/settings", query: { section: "enhancements" } }); }

onBeforeUnmount(() => {
  disposed = true;
  setupRevision++;
  clearTimeout(historyTimer);
  clearTimeout(suggestionTimer);
  clearHistoryProjection();
  promptTicket.value = null;
});
defineExpose({ openHistory });
</script>

<template>
  <div
    class="native-terminal-tools"
    data-plugin-protected
  >
    <NvxIconButton
      size="sm"
      :label="t('nativeTerminal.tools')"
      @click.stop="openHistory"
    >
      <NvxIcon
        :icon="History"
        :size="16"
      />
    </NvxIconButton>
    <div
      v-if="suggestions.length && !open && active"
      class="native-terminal-tools__suggestions"
      role="group"
      :aria-label="t('nativeTerminal.suggestionLabel')"
      @pointerdown.stop
      @focusin.stop
    >
      <header>
        <small>{{ t('nativeTerminal.suggestionHint') }}</small><NvxIconButton
          size="sm"
          :label="t('nativeTerminal.dismissSuggestions')"
          @mousedown.prevent
          @click="dismissedDraft = draft; suggestions = []"
        >
          <NvxIcon
            :icon="X"
            :size="16"
          />
        </NvxIconButton>
      </header>
      <button
        v-for="entry in suggestions"
        :key="entry.entryId"
        type="button"
        :disabled="busy"
        @mousedown.prevent
        @click="insert(entry)"
      >
        <code>{{ entry.command }}</code>
      </button>
    </div>
    <NvxDialog
      :model-value="open"
      :title="t('nativeTerminal.tools')"
      :description="label"
      :close-label="t('nativeTerminal.close')"
      :dismissible="!busy"
      size="lg"
      plugin-protected
      @update:model-value="(value) => { if (!value) void close(); }"
    >
      <div class="native-terminal-tools__dialog">
        <NvxSelect
          v-model="section"
          :options="sections"
          :aria-label="t('nativeTerminal.tools')"
        />
        <template v-if="section === 'history'">
          <div class="native-terminal-tools__filters">
            <NvxInput
              v-model="query"
              :aria-label="t('nativeTerminal.query')"
              :placeholder="t('nativeTerminal.query')"
              :maxlength="256"
            />
            <NvxSelect
              :model-value="allScopes ? 'all' : 'current'"
              :options="scopeOptions"
              :aria-label="t('nativeTerminal.source')"
              @update:model-value="allScopes = $event === 'all'"
            />
          </div>
          <NvxInlineNotice v-if="!native.available">
            {{ t('nativeTerminal.unavailable') }}
          </NvxInlineNotice>
          <NvxInlineNotice v-else-if="!config?.historyEnabled">
            {{ t('nativeTerminal.disabled') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-else-if="!historyAvailable"
            tone="warning"
          >
            {{ t('nativeTerminal.locked') }}
          </NvxInlineNotice>
          <NvxInlineNotice v-else-if="config.historyPaused">
            {{ t('nativeTerminal.paused') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-if="historyError"
            tone="error"
          >
            {{ t('nativeTerminal.unavailable') }} <NvxButton
              variant="ghost"
              @click="loadHistory"
            >
              {{ t('nativeTerminal.retry') }}
            </NvxButton>
          </NvxInlineNotice>
          <span
            v-else-if="loading"
            role="status"
          >{{ t('nativeTerminal.loading') }}</span>
          <p v-else-if="historyAvailable && !history.length">
            {{ t('nativeTerminal.noHistory') }}
          </p>
          <ul
            v-if="history.length"
            class="native-terminal-tools__history"
          >
            <li
              v-for="entry in history"
              :key="entry.entryId"
            >
              <div class="native-terminal-tools__entry">
                <code>{{ entry.command }}</code><small>{{ timestamp(entry.completedAtUnixMs) }} · {{ entry.scope.kind === 'local' ? t('nativeTerminal.local') : entry.scope.hostId === hostId ? label : t('nativeTerminal.otherHost') }}</small>
              </div>
              <div class="native-terminal-tools__entry-actions">
                <NvxIconButton
                  size="sm"
                  :label="t('nativeTerminal.copy')"
                  @click="copy(entry)"
                >
                  <NvxIcon
                    :icon="Copy"
                    :size="16"
                  />
                </NvxIconButton>
                <NvxIconButton
                  size="sm"
                  :label="t('nativeTerminal.remove')"
                  :disabled="busy"
                  @click="remove(entry)"
                >
                  <NvxIcon
                    :icon="Trash2"
                    :size="16"
                  />
                </NvxIconButton>
              </div>
            </li>
          </ul>
          <NvxInlineNotice v-if="session?.kind === 'ssh' && !hostId">
            {{ t('nativeTerminal.quickConnectHistory') }}
          </NvxInlineNotice>
          <small>{{ t('nativeTerminal.insertHint') }}</small>
        </template>
        <template v-else>
          <div class="native-terminal-tools__integration-state">
            <strong>{{ t(`nativeTerminal.shellStates.${status?.captureState ?? 'disabled'}`) }}</strong><span v-if="status?.captureState === 'ready'">{{ t(`nativeTerminal.activity.${status.activity}`) }}</span>
          </div>
          <p>{{ t('nativeTerminal.integrationHint') }}</p>
          <NvxInlineNotice v-if="session?.kind === 'ssh' && !hostId">
            {{ t('nativeTerminal.quickConnectHistory') }}
          </NvxInlineNotice>
          <NvxInlineNotice
            v-if="status?.captureState === 'unsupported'"
            tone="warning"
          >
            {{ t('nativeTerminal.unsupported') }}
          </NvxInlineNotice>
          <NvxInlineNotice v-if="historyScope && config?.historyEnabled && status?.captureState === 'ready' && !status.capturesCommand">
            {{ t('nativeTerminal.reinstallForHistory') }}
          </NvxInlineNotice>
          <NvxField :label="t('nativeTerminal.shell')">
            <NvxSelect
              v-model="selectedShell"
              :options="shellOptions"
              :aria-label="t('nativeTerminal.shell')"
            /><small>{{ t('nativeTerminal.shellDescription') }}</small>
          </NvxField>
          <NvxCheckbox
            v-model="confirmedPrompt"
            :disabled="busy || !promptTicket"
          >
            {{ t('nativeTerminal.confirmPrompt') }}
          </NvxCheckbox>
          <small>{{ t('nativeTerminal.integrationPrivacy') }}</small>
        </template>
      </div>
      <template #actions>
        <NvxButton
          variant="ghost"
          :disabled="busy"
          @click="showSettings"
        >
          {{ t('nativeTerminal.settings') }}
        </NvxButton>
        <NvxButton
          variant="ghost"
          :disabled="busy"
          @click="close"
        >
          {{ t('nativeTerminal.close') }}
        </NvxButton>
        <NvxButton
          v-if="section === 'integration'"
          :disabled="!confirmedPrompt || !promptTicket"
          :loading="busy"
          :loading-label="t('nativeTerminal.enabling')"
          @click="enableIntegration"
        >
          {{ t('nativeTerminal.enable') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </div>
</template>

<style scoped>
.native-terminal-tools { display: inline-flex; align-items: center; flex-shrink: 0; }
.native-terminal-tools__dialog { display: grid; gap: var(--nvx-space-3); min-width: 0; }
.native-terminal-tools__dialog p { margin: 0; font-size: var(--nvx-font-size-sm); }
.native-terminal-tools__dialog small { color: var(--nvx-color-text-secondary); line-height: 1.5; }
.native-terminal-tools__filters { display: grid; grid-template-columns: minmax(0, 1fr) minmax(160px, 220px); gap: var(--nvx-space-2); }
.native-terminal-tools__history { list-style: none; padding: 0; margin: 0; max-height: min(380px, 44vh); overflow: auto; }
.native-terminal-tools__history li { display: flex; align-items: center; gap: var(--nvx-space-2); padding: 8px 0; border-bottom: 1px solid var(--nvx-color-border-subtle); }
.native-terminal-tools__entry { min-width: 0; flex: 1; display: grid; gap: 3px; }
.native-terminal-tools__entry code { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 12px; }
.native-terminal-tools__entry-actions { display: flex; align-items: center; flex-shrink: 0; }
.native-terminal-tools__integration-state { display: flex; align-items: center; gap: var(--nvx-space-3); font-size: var(--nvx-font-size-sm); }
.native-terminal-tools__suggestions { position: absolute; z-index: var(--nvx-z-popover); bottom: 8px; left: 10px; width: min(480px, calc(100% - 20px)); border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); box-shadow: var(--nvx-shadow-overlay); }
.native-terminal-tools__suggestions header { display: flex; justify-content: space-between; align-items: center; gap: 6px; padding: 1px 6px; color: var(--nvx-color-text-secondary); font-size: 11px; }
.native-terminal-tools__suggestions > button { display: block; border: 0; background: transparent; color: inherit; width: 100%; padding: 6px 9px; text-align: left; cursor: pointer; }
.native-terminal-tools__suggestions > button:hover { background: var(--nvx-color-bg-hover); }
.native-terminal-tools__suggestions > button:focus-visible { outline: 2px solid var(--nvx-color-focus-ring); outline-offset: -2px; }
.native-terminal-tools__suggestions code { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
@media (max-width: 700px) { .native-terminal-tools__filters { grid-template-columns: minmax(0, 1fr); } .native-terminal-tools__history li { flex-wrap: wrap; } }
</style>
