<script setup lang="ts">
import { ChevronDown, ChevronUp, Copy, Search, X } from "lucide-vue-next";
import { nextTick, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";

import { NvxIcon, NvxIconButton, NvxInput } from "../ui";

interface TerminalToolTarget {
  findNext(term: string, incremental?: boolean): boolean;
  findPrevious(term: string): boolean;
  clearSearch(): void;
  selection(): string;
  focus(): void;
}

const props = defineProps<{
  terminal: TerminalToolTarget | null;
  hasSelection: boolean;
}>();

const { t } = useI18n();
const root = ref<HTMLElement | null>(null);
const searchOpen = ref(false);
const query = ref("");
const matchFound = ref(true);
const copyFeedback = ref<"copied" | "failed" | null>(null);
let feedbackTimer: number | null = null;

function focusSearchInput() {
  void nextTick(() => root.value?.querySelector<HTMLInputElement>("input")?.focus());
}

function openSearch() {
  searchOpen.value = true;
  focusSearchInput();
}

function closeSearch() {
  searchOpen.value = false;
  query.value = "";
  matchFound.value = true;
  props.terminal?.clearSearch();
  props.terminal?.focus();
}

function updateSearch(value: string) {
  query.value = value;
  matchFound.value = value ? (props.terminal?.findNext(value, true) ?? false) : true;
}

function findNext() {
  if (!query.value) return;
  matchFound.value = props.terminal?.findNext(query.value) ?? false;
}

function findPrevious() {
  if (!query.value) return;
  matchFound.value = props.terminal?.findPrevious(query.value) ?? false;
}

function showCopyFeedback(value: "copied" | "failed") {
  copyFeedback.value = value;
  if (feedbackTimer !== null) window.clearTimeout(feedbackTimer);
  feedbackTimer = window.setTimeout(() => {
    copyFeedback.value = null;
    feedbackTimer = null;
  }, 1_800);
}

async function copySelection() {
  const selection = props.terminal?.selection() ?? "";
  if (!selection) return;
  try {
    await navigator.clipboard.writeText(selection);
    showCopyFeedback("copied");
  } catch {
    showCopyFeedback("failed");
  }
}

onBeforeUnmount(() => {
  if (feedbackTimer !== null) window.clearTimeout(feedbackTimer);
});

defineExpose({ openSearch, copySelection });
</script>

<template>
  <div
    ref="root"
    class="terminal-tools"
    @pointerdown.stop
    @focusin.stop
  >
    <NvxIconButton
      size="sm"
      :label="t('terminalTools.search')"
      @click="openSearch"
    >
      <NvxIcon
        :icon="Search"
        :size="16"
      />
    </NvxIconButton>
    <NvxIconButton
      size="sm"
      :label="t('terminalTools.copySelection')"
      :disabled="!hasSelection"
      @click="copySelection"
    >
      <NvxIcon
        :icon="Copy"
        :size="16"
      />
    </NvxIconButton>

    <div
      v-if="searchOpen"
      class="terminal-tools__search"
      role="search"
      @keydown.escape.stop.prevent="closeSearch"
    >
      <label class="terminal-tools__field">
        <span class="terminal-tools__visually-hidden">{{ t("terminalTools.searchLabel") }}</span>
        <NvxInput
          :model-value="query"
          :placeholder="t('terminalTools.searchPlaceholder')"
          @update:model-value="updateSearch"
          @keydown.enter.exact.prevent="findNext"
          @keydown.shift.enter.prevent="findPrevious"
        />
      </label>
      <span
        v-if="query && !matchFound"
        class="terminal-tools__result"
        role="status"
      >
        {{ t("terminalTools.noMatches") }}
      </span>
      <NvxIconButton
        size="sm"
        :label="t('terminalTools.previousMatch')"
        :disabled="!query"
        @click="findPrevious"
      >
        <NvxIcon
          :icon="ChevronUp"
          :size="16"
        />
      </NvxIconButton>
      <NvxIconButton
        size="sm"
        :label="t('terminalTools.nextMatch')"
        :disabled="!query"
        @click="findNext"
      >
        <NvxIcon
          :icon="ChevronDown"
          :size="16"
        />
      </NvxIconButton>
      <NvxIconButton
        size="sm"
        :label="t('terminalTools.closeSearch')"
        @click="closeSearch"
      >
        <NvxIcon
          :icon="X"
          :size="16"
        />
      </NvxIconButton>
    </div>

    <span
      class="terminal-tools__feedback"
      role="status"
      aria-live="polite"
    >
      {{ copyFeedback ? t(`terminalTools.${copyFeedback}`) : "" }}
    </span>
  </div>
</template>

<style scoped>
.terminal-tools {
  display: inline-flex;
  align-items: center;
  gap: var(--nvx-space-1);
}

.terminal-tools :deep(.nvx-icon-button) {
  color: var(--nvx-color-terminal-muted);
}

.terminal-tools__search {
  position: absolute;
  z-index: var(--nvx-z-popover);
  top: 44px;
  right: var(--nvx-space-3);
  display: flex;
  width: min(440px, calc(100% - 24px));
  min-height: var(--nvx-control-height-md);
  align-items: center;
  gap: var(--nvx-space-1);
  padding: var(--nvx-space-2);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.terminal-tools__field {
  flex: 1;
  min-width: 0;
}

.terminal-tools__result {
  flex: none;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  white-space: nowrap;
}

.terminal-tools__feedback {
  position: absolute;
  z-index: var(--nvx-z-popover);
  top: 48px;
  right: var(--nvx-space-3);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.terminal-tools__feedback:empty {
  display: none;
}

.terminal-tools__visually-hidden {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
</style>
