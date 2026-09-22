<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronUp, Pause, Radio, Search, X } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxDialog, NvxIcon, NvxIconButton, NvxInlineNotice, NvxInput, NvxStatusLabel } from "../ui";
import NvxCodeEditor from "../ui/NvxCodeEditor.vue";
import type { SftpFilePreview, SftpFileTailResult } from "../../core-api/generated/core-api";
import type { ToolTarget } from "../../tool-windows";
import { createUuidV7 } from "../../core-api/ids";
const props = defineProps<{ target: Extract<ToolTarget, { kind: 'sftpFile' }> }>();
const emit = defineEmits<{ saved: []; cancel: []; closeCancelled: [] }>();
const { t } = useI18n();
const content = ref<SftpFilePreview['content'] | null>(null), text = ref(''), original = ref('');
const loading = ref(true), failed = ref(false), saving = ref(false), active = ref(false), image = ref(''), discard = ref(false), reset = ref(false), trimmed = ref(false);
const saveFailed = ref(false);
const root = ref<HTMLElement | null>(null);
const editor = ref<{
  focus(): void;
  updateSearch(query: string): boolean;
  findNext(): boolean;
  findPrevious(): boolean;
} | null>(null);
const searchOpen = ref(false), searchQuery = ref(""), searchMatchFound = ref(true);
const dirty = computed(() => !props.target.tail && text.value !== original.value);
const readonly = computed(() => props.target.tail || content.value?.kind !== 'text' || !content.value.editable);
const serializedText = computed(() => text.value.replace(/\n/g, content.value?.kind === 'text' && content.value.lineEnding === 'crLf' ? '\r\n' : content.value?.kind === 'text' && content.value.lineEnding === 'cr' ? '\r' : '\n'));
const tooLarge = computed(() => new TextEncoder().encode(serializedText.value).length > 1_048_576);
let pollEpoch = 0;
let disposed = false, timer: ReturnType<typeof setTimeout> | undefined, offset = '0', decoder = new TextDecoder(), pendingCarriageReturn = false;
let closeResolve: ((value: boolean) => void) | undefined;
function resolveClose(value: boolean) { if (!value && closeResolve) emit("closeCancelled"); discard.value = false; closeResolve?.(value); closeResolve = undefined; }
function requestClose(): Promise<boolean> {
  if (saving.value || closeResolve) return Promise.resolve(false);
  if (!dirty.value) return Promise.resolve(true);
  discard.value = true;
  return new Promise((resolve) => { closeResolve = resolve; });
}
defineExpose({ requestClose, dirty, busy: saving });
function normalize(value: string, restart = false) {
  if (restart) pendingCarriageReturn = false;
  let combined = (pendingCarriageReturn ? '\r' : '') + value;
  pendingCarriageReturn = combined.endsWith('\r');
  if (pendingCarriageReturn) combined = combined.slice(0, -1);
  return combined.replace(/\r\n?/g, '\n');
}
async function poll() {
  if (disposed || !active.value) return;
  const epoch = pollEpoch;
  try {
    const result = await invoke<SftpFileTailResult>('tool_file_tail', { offset });
    if (disposed || !active.value || epoch !== pollEpoch) return;
    if (result.reset) { decoder = new TextDecoder(); text.value = ''; reset.value = true; }
    text.value += normalize(decoder.decode(new Uint8Array(result.bytes), { stream: true }), result.reset);
    if (text.value.length > 262144) { text.value = text.value.slice(-262144); trimmed.value = true; }
    offset = result.nextOffset;
    timer = setTimeout(() => void poll(), BigInt(offset) < BigInt(result.totalSize) ? 40 : 1000);
  } catch { if (!disposed && epoch === pollEpoch) { failed.value = true; active.value = false; } }
}
function toggleTail() { pollEpoch += 1; active.value = !active.value; clearTimeout(timer); if (active.value) { failed.value = false; void poll(); } }
function focusSearchInput() { void nextTick(() => root.value?.querySelector<HTMLInputElement>(".file-window__search input")?.focus()); }
function openSearch() { searchOpen.value = true; focusSearchInput(); }
function closeSearch() {
  searchOpen.value = false;
  searchQuery.value = "";
  searchMatchFound.value = true;
  editor.value?.updateSearch("");
  editor.value?.focus();
}
function updateSearch(value: string) {
  searchQuery.value = value;
  searchMatchFound.value = value ? (editor.value?.updateSearch(value) ?? false) : true;
}
function moveSearch(direction: "next" | "previous") {
  if (!searchQuery.value) return;
  searchMatchFound.value = direction === "next"
    ? (editor.value?.findNext() ?? false)
    : (editor.value?.findPrevious() ?? false);
}
function handleKeydown(event: KeyboardEvent) {
  if (!(event.metaKey || event.ctrlKey) || event.altKey || event.key.toLocaleLowerCase() !== "f") return;
  event.preventDefault();
  event.stopPropagation();
  openSearch();
}
async function save() {
  if (readonly.value || !dirty.value || saving.value || tooLarge.value) return;
  saving.value = true; failed.value = false; saveFailed.value = false;
  const submitted = serializedText.value;
  try {
    await invoke('tool_file_save', { text: submitted, meta: { requestId: crypto.randomUUID() }, operationId: createUuidV7() });
    original.value = text.value;
    emit('saved');
  } catch { failed.value = true; saveFailed.value = true; }
  finally { saving.value = false; }
}
onMounted(async () => {
  try {
    const result = await invoke<SftpFilePreview>('tool_file_preview');
    if (disposed) return;
    content.value = result.content;
    if (result.content.kind === 'text') {
      text.value = result.content.text.replace(/\r\n?/g, '\n'); original.value = text.value; offset = result.content.endOffset;
      if (props.target.tail) toggleTail();
    } else image.value = URL.createObjectURL(new Blob([new Uint8Array(result.content.bytes)], { type: result.content.mediaType }));
  } catch { failed.value = true; }
  finally { loading.value = false; }
});
onBeforeUnmount(() => { disposed = true; active.value = false; clearTimeout(timer); if (image.value) URL.revokeObjectURL(image.value); resolveClose(false); });
</script>
<template>
  <section
    ref="root"
    class="file-window"
    @keydown="handleKeydown"
  >
    <div
      v-if="loading || failed || tooLarge || reset || trimmed || (content?.kind === 'text' && content.truncated)"
      class="file-window__notices"
    >
      <NvxInlineNotice
        v-if="loading"
        :title="t('sftp.previewLoading')"
      />
      <NvxInlineNotice
        v-if="failed"
        tone="error"
        :title="t(saveFailed ? 'sftp.saveFailed' : 'sftp.previewFailed')"
      />
      <NvxInlineNotice
        v-if="tooLarge"
        tone="warning"
        :title="t('sftp.editorTooLarge')"
      />
      <NvxInlineNotice
        v-if="reset || trimmed"
        :title="t(reset ? 'sftp.tailReset' : 'sftp.tailTrimmed')"
      />
      <NvxInlineNotice
        v-if="content?.kind === 'text' && content.truncated"
        :title="t('sftp.previewTruncated')"
      />
    </div>
    <div
      v-if="content?.kind === 'text'"
      class="file-window__toolbar"
    >
      <NvxStatusLabel
        v-if="target.tail && active"
        tone="success"
      >
        {{ t('sftp.tailLive') }}
      </NvxStatusLabel>
      <span
        v-else
        class="file-window__state"
      >
        {{ t(target.tail ? 'sftp.tailPaused' : readonly ? 'sftp.editorReadOnly' : 'sftp.editorEditable') }}
      </span>
      <div
        v-if="searchOpen"
        class="file-window__search"
        role="search"
        @keydown.escape.stop.prevent="closeSearch"
      >
        <NvxInput
          :model-value="searchQuery"
          :aria-label="t('sftp.searchInFile')"
          :placeholder="t('sftp.searchInFilePlaceholder')"
          @update:model-value="updateSearch"
          @keydown.enter.exact.prevent="moveSearch('next')"
          @keydown.shift.enter.prevent="moveSearch('previous')"
        />
        <span
          v-if="searchQuery && !searchMatchFound"
          class="file-window__search-result"
          role="status"
        >
          {{ t('sftp.noSearchMatches') }}
        </span>
        <NvxIconButton
          size="sm"
          :label="t('sftp.previousSearchMatch')"
          :disabled="!searchQuery"
          @click="moveSearch('previous')"
        >
          <NvxIcon
            :icon="ChevronUp"
            :size="16"
          />
        </NvxIconButton>
        <NvxIconButton
          size="sm"
          :label="t('sftp.nextSearchMatch')"
          :disabled="!searchQuery"
          @click="moveSearch('next')"
        >
          <NvxIcon
            :icon="ChevronDown"
            :size="16"
          />
        </NvxIconButton>
        <NvxIconButton
          size="sm"
          :label="t('sftp.closeFileSearch')"
          @click="closeSearch"
        >
          <NvxIcon
            :icon="X"
            :size="16"
          />
        </NvxIconButton>
      </div>
      <NvxIconButton
        v-else
        size="sm"
        :label="t('sftp.searchInFile')"
        @click="openSearch"
      >
        <NvxIcon
          :icon="Search"
          :size="16"
        />
      </NvxIconButton>
      <NvxButton
        v-if="target.tail"
        variant="secondary"
        size="sm"
        @click="toggleTail"
      >
        <NvxIcon
          :icon="active ? Pause : Radio"
          :size="16"
        />
        {{ t(active ? 'sftp.tailStop' : 'sftp.tailStart') }}
      </NvxButton>
    </div>
    <NvxCodeEditor
      v-if="content?.kind === 'text'"
      ref="editor"
      v-model="text"
      class="file-window__editor"
      :readonly="readonly || saving"
      :filename="target.title"
      :follow-end="active"
      :label="t('sftp.editorLabel', { name: target.title })"
    />
    <img
      v-else-if="image"
      :src="image"
      :alt="target.title"
    >
    <footer class="file-window__footer">
      <NvxButton
        variant="secondary"
        :disabled="saving"
        @click="emit('cancel')"
      >
        {{ t('sftp.closePreview') }}
      </NvxButton>
      <NvxButton
        v-if="!readonly"
        :loading="saving"
        :disabled="!dirty || tooLarge"
        @click="save"
      >
        {{ t('sftp.saveFile') }}
      </NvxButton>
    </footer>
    <NvxDialog
      :model-value="discard"
      :title="t('toolWindows.unsavedTitle')"
      :description="t('toolWindows.unsavedDescription')"
      :close-label="t('toolWindows.keepEditing')"
      @update:model-value="resolveClose(false)"
    >
      <template #actions>
        <NvxButton
          variant="secondary"
          @click="resolveClose(false)"
        >
          {{ t('toolWindows.keepEditing') }}
        </NvxButton>
        <NvxButton
          variant="danger"
          @click="resolveClose(true)"
        >
          {{ t('toolWindows.discard') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </section>
</template>
<style scoped>
.file-window { display: flex; flex-direction: column; height: 100%; min-height: 300px; overflow: hidden; }
.file-window__notices { display: grid; gap: var(--nvx-space-2); padding: var(--nvx-space-2) var(--nvx-space-3); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.file-window__toolbar, .file-window__footer { display: flex; flex: none; align-items: center; gap: var(--nvx-space-2); padding: var(--nvx-space-2) var(--nvx-space-3); background: var(--nvx-color-bg-canvas); }
.file-window__toolbar { min-height: 48px; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.file-window__footer { min-height: 58px; justify-content: flex-end; border-top: var(--nvx-border-width) solid var(--nvx-color-border); }
.file-window__state { flex: 1; min-width: 0; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.file-window__search { display: flex; flex: 1; min-width: 0; align-items: center; justify-content: flex-end; gap: var(--nvx-space-1); }
.file-window__search :deep(.nvx-input) { width: min(320px, 100%); height: var(--nvx-control-height-sm); min-height: var(--nvx-control-height-sm); }
.file-window__search-result { flex: none; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); white-space: nowrap; }
.file-window__editor { flex: 1; min-height: 220px; }
img { width: 100%; max-width: 100%; object-fit: contain; min-height: 0; flex: 1; }
</style>
