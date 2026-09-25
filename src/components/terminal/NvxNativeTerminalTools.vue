<script setup lang="ts">
import { History, Copy, Trash2 } from "lucide-vue-next";
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";

import { deleteNativeTerminalHistory, listNativeTerminalHistory } from "../../core-api/native-terminal";
import type { NativeTerminalHistoryEntry, NativeTerminalHistoryScope, NativeTerminalSessionScope } from "../../core-api/generated/core-api";
import { captureTerminalInput, type TerminalInputTicket } from "../../terminal-input-target";
import { useNativeTerminalStore } from "../../stores/nativeTerminal";
import { useTipsStore } from "../../stores/tips";
import { historySuggestionSuffix } from "../../terminal/draft";
import { NvxButton, NvxDialog, NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput, NvxSelect } from "../ui";

const props = defineProps<{
  paneId: string;
  label: string;
  hostId: string | null;
  session: NativeTerminalSessionScope | null;
  active: boolean;
  writable: boolean;
  draft: string | null;
  currentDraft: () => string | null;
}>();
const emit = defineEmits<{ focus: []; invalidateDraft: []; suggestionChange: [suggestion: { draft: string; suffix: string } | null] }>();
const { t, locale } = useI18n();
const router = useRouter();
const native = useNativeTerminalStore();
const tips = useTipsStore();
const open = ref(false);
const query = ref("");
const allScopes = ref(false);
const history = shallowRef<NativeTerminalHistoryEntry[]>([]);
const suggestion = shallowRef<NativeTerminalHistoryEntry | null>(null);
const busy = ref(false);
const loading = ref(false);
const historyError = ref(false);
const promptTicket = shallowRef<TerminalInputTicket | null>(null);
let historyRevision = 0;
let suggestionRevision = 0;
let setupRevision = 0;
let historyTimer: ReturnType<typeof setTimeout> | undefined;
let suggestionTimer: ReturnType<typeof setTimeout> | undefined;
let disposed = false;

const config = computed(() => native.settingsSnapshot?.settings);
const historyAvailable = computed(() => native.available && Boolean(config.value?.historyEnabled)
  && Boolean(native.settingsSnapshot?.historyAvailable) && !native.settingsSnapshot?.historyPersistenceFailed);
const historyScope = computed<NativeTerminalHistoryScope | null>(() => props.hostId ? { kind: "host", hostId: props.hostId } : props.session?.kind === "local" ? { kind: "local" } : null);
const canSuggest = computed(() => historyScope.value !== null && historyAvailable.value && !config.value?.historyPaused && props.active && props.writable
  && props.draft !== null);
const scopeOptions = computed(() => [
  ...(historyScope.value ? [{ value: "current", label: t(props.hostId ? "nativeTerminal.thisHost" : "nativeTerminal.local") }] : []),
  { value: "all", label: t("nativeTerminal.global") },
]);

function notify(key: string, tone: "success" | "error" = "success") {
  tips.show({ scope: `native-terminal:${props.paneId}`, tone, title: t(`nativeTerminal.${key}`) });
}
function clearHistoryProjection() { historyRevision++; suggestionRevision++; clearTimeout(suggestionTimer); history.value = []; suggestion.value = null; }
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
  if (!open.value || !historyAvailable.value) { loading.value = false; return; }
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
watch([query, allScopes], () => {
  historyRevision++;
  history.value = [];
  clearTimeout(historyTimer);
  historyTimer = setTimeout(() => { void loadHistory(); }, 150);
});

watch([() => props.draft, canSuggest, () => native.historyEpoch, () => props.session, () => props.hostId], () => {
  const revision = ++suggestionRevision;
  suggestion.value = null;
  clearTimeout(suggestionTimer);
  if (!canSuggest.value || !props.draft || props.draft.length < 2 || open.value) return;
  const draft = props.draft;
  const epoch = native.historyEpoch;
  const requestKey = suggestionRequestKey(draft);
  suggestionTimer = setTimeout(async () => {
    try {
      const entries = await listNativeTerminalHistory({ scope: historyScope.value, query: draft, limit: 2_000 });
      if (disposed || revision !== suggestionRevision || epoch !== native.historyEpoch || requestKey !== suggestionRequestKey(draft) || !canSuggest.value || props.draft !== draft || open.value) return;
      const ranked = new Map<string, { entry: NativeTerminalHistoryEntry; count: number }>();
      for (const entry of entries) {
        if (historySuggestionSuffix(entry.command, draft) === null) continue;
        const existing = ranked.get(entry.command);
        if (existing) {
          existing.count++;
          if (entry.completedAtUnixMs > existing.entry.completedAtUnixMs) existing.entry = entry;
        } else ranked.set(entry.command, { entry, count: 1 });
      }
      const best = [...ranked.values()].sort((left, right) =>
        right.count - left.count
        || right.entry.completedAtUnixMs - left.entry.completedAtUnixMs
        || left.entry.command.localeCompare(right.entry.command))[0];
      suggestion.value = best?.entry ?? null;
    } catch { /* Automatic suggestion failure never interrupts the terminal; the manual panel retains a recoverable error. */ }
  }, 200);
});
watch([suggestion, () => props.draft, canSuggest, open], () => {
  const draft = props.draft;
  const suffix = suggestion.value && draft && canSuggest.value && !open.value
    ? historySuggestionSuffix(suggestion.value.command, draft)
    : null;
  emit("suggestionChange", suffix && draft ? { draft, suffix } : null);
}, { immediate: true });

async function openHistory() {
  if (open.value || busy.value) return;
  const revision = ++setupRevision;
  suggestion.value = null;
  const ticket = captureTerminalInput(props.paneId);
  if (ticket && !await ticket.suspend()) { await ticket.cancel(); return; }
  if (disposed || revision !== setupRevision) { await ticket?.cancel(); return; }
  promptTicket.value = ticket;
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

function insertionSuffix(entry: NativeTerminalHistoryEntry) {
  if (!props.active || !props.writable || !historyAvailable.value) return null;
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
  suggestion.value = null;
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
  emit("suggestionChange", null);
  promptTicket.value = null;
});
function acceptSuggestion() {
  const entry = suggestion.value;
  if (entry && canSuggest.value && !open.value) void insert(entry);
}
defineExpose({ openHistory, acceptSuggestion });
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
          v-else-if="native.settingsSnapshot?.historyPersistenceFailed"
          tone="warning"
        >
          {{ t('nativeTerminal.persistenceFailed') }}
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
@media (max-width: 700px) { .native-terminal-tools__filters { grid-template-columns: minmax(0, 1fr); } .native-terminal-tools__history li { flex-wrap: wrap; } }
</style>
