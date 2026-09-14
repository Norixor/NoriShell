<script setup lang="ts">
import { BellRing, ChevronLeft, ChevronRight, Plus, SquareTerminal, X } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, type Component } from "vue";

import { NvxIcon, NvxIconButton } from "../ui";
import NvxHostMarker from "../hosts/NvxHostMarker.vue";
import type { HostMarker } from "../../host-markers";

export interface TerminalTabItem {
  groupId: string;
  label: string;
  stateLabel: string;
  icon?: Component;
  disabled?: boolean;
  hostMarker?: HostMarker | null;
  completionCount?: number;
  bellAttention?: boolean;
  bellAttentionLabel?: string;
}

const props = withDefaults(
  defineProps<{
    items: readonly TerminalTabItem[];
    modelValue: string;
    label: string;
    newLabel: string;
    closeLabel?: string;
    closeAllLabel?: string;
    closeLeftLabel?: string;
    closeRightLabel?: string;
    contextMenuLabel?: string;
    scrollBackwardLabel?: string;
    scrollForwardLabel?: string;
    closable?: boolean;
    busy?: boolean;
    createDisabled?: boolean;
  }>(),
  {
    closeLabel: "Close tab",
    closeAllLabel: "Close all tabs",
    closeLeftLabel: "Close tabs to the left",
    closeRightLabel: "Close tabs to the right",
    contextMenuLabel: "Tab actions",
    scrollBackwardLabel: "Show earlier tabs",
    scrollForwardLabel: "Show later tabs",
    closable: true,
    busy: false,
    createDisabled: false,
  },
);

const emit = defineEmits<{
  "update:modelValue": [groupId: string];
  create: [];
  close: [groupId: string];
  closeMany: [groupIds: string[]];
}>();

const tabList = ref<HTMLElement | null>(null);
const tabButtons = new Map<string, HTMLButtonElement>();
const tabItems = new Map<string, HTMLElement>();
const hasOverflow = ref(false);
const canScrollBackward = ref(false);
const canScrollForward = ref(false);
const contextMenu = ref<{ index: number; left: number; top: number } | null>(null);
let resizeObserver: ResizeObserver | null = null;

const contextTarget = computed(() => (
  contextMenu.value ? props.items[contextMenu.value.index] ?? null : null
));

function setTabButton(groupId: string, element: unknown) {
  if (element instanceof HTMLButtonElement) tabButtons.set(groupId, element);
  else tabButtons.delete(groupId);
}

function setTabItem(groupId: string, element: unknown) {
  if (element instanceof HTMLElement) tabItems.set(groupId, element);
  else tabItems.delete(groupId);
}

function updateOverflowState() {
  const list = tabList.value;
  if (!list) return;
  const maxScrollLeft = Math.max(0, list.scrollWidth - list.clientWidth);
  hasOverflow.value = maxScrollLeft > 1;
  canScrollBackward.value = list.scrollLeft > 1;
  canScrollForward.value = list.scrollLeft < maxScrollLeft - 1;
}

function scrollTabs(direction: -1 | 1) {
  const list = tabList.value;
  if (!list) return;
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  list.scrollBy({
    left: direction * Math.max(180, Math.round(list.clientWidth * 0.72)),
    behavior: reduceMotion ? "auto" : "smooth",
  });
}

function handleWheel(event: WheelEvent) {
  const list = tabList.value;
  if (!list || !hasOverflow.value) return;
  const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
  if (delta === 0) return;
  event.preventDefault();
  list.scrollLeft += delta;
}

function focusTab(index: number) {
  const count = props.items.length;
  if (count === 0) return;
  const nextIndex = (index + count) % count;
  const groupId = props.items[nextIndex]?.groupId;
  if (!groupId) return;
  emit("update:modelValue", groupId);
  void nextTick(() => tabButtons.get(groupId)?.focus());
}

function handleTabKeydown(event: KeyboardEvent, index: number) {
  if (event.key === "ArrowRight") {
    event.preventDefault();
    focusTab(index + 1);
  } else if (event.key === "ArrowLeft") {
    event.preventDefault();
    focusTab(index - 1);
  } else if (event.key === "Home") {
    event.preventDefault();
    focusTab(0);
  } else if (event.key === "End") {
    event.preventDefault();
    focusTab(props.items.length - 1);
  }
}

function menuItems() {
  return Array.from(
    document.querySelectorAll<HTMLButtonElement>(
      ".nvx-terminal-tab-bar__context-menu [role='menuitem']:not(:disabled)",
    ),
  );
}

function closeContextMenu(restoreFocus = false) {
  const target = contextTarget.value;
  contextMenu.value = null;
  if (restoreFocus && target) void nextTick(() => tabButtons.get(target.groupId)?.focus());
}

function openTabContextMenu(event: MouseEvent, index: number) {
  if (props.busy || props.items[index]?.disabled) return;
  event.preventDefault();
  const menuWidth = 196;
  const menuHeight = 104;
  contextMenu.value = {
    index,
    left: Math.max(8, Math.min(event.clientX, window.innerWidth - menuWidth - 8)),
    top: Math.max(8, Math.min(event.clientY, window.innerHeight - menuHeight - 8)),
  };
  void nextTick(() => menuItems()[0]?.focus());
}

function requestContextClose(scope: "all" | "left" | "right") {
  const target = contextMenu.value;
  if (!target) return;
  const groupIds = scope === "all"
    ? props.items.map((item) => item.groupId)
    : scope === "left"
      ? props.items.slice(0, target.index).map((item) => item.groupId)
      : props.items.slice(target.index + 1).map((item) => item.groupId);
  if (groupIds.length) emit("closeMany", groupIds);
  closeContextMenu(true);
}

function handleDocumentPointerDown(event: PointerEvent) {
  if (contextMenu.value && !(event.target as Element).closest(".nvx-terminal-tab-bar__context-menu")) {
    closeContextMenu();
  }
}

function handleDocumentKeyDown(event: KeyboardEvent) {
  if (!contextMenu.value) return;
  if (event.key === "Escape") {
    event.preventDefault();
    closeContextMenu(true);
    return;
  }
  const items = menuItems();
  if (!items.length) return;
  const currentIndex = Math.max(0, items.indexOf(document.activeElement as HTMLButtonElement));
  let nextIndex: number | null = null;
  if (event.key === "ArrowDown") nextIndex = (currentIndex + 1) % items.length;
  else if (event.key === "ArrowUp") nextIndex = (currentIndex - 1 + items.length) % items.length;
  else if (event.key === "Home") nextIndex = 0;
  else if (event.key === "End") nextIndex = items.length - 1;
  else if (event.key === "Tab") closeContextMenu();
  if (nextIndex === null) return;
  event.preventDefault();
  items[nextIndex]?.focus();
}

watch(
  () => [props.items.length, props.modelValue] as const,
  () => void nextTick(() => {
    updateOverflowState();
    tabItems.get(props.modelValue)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }),
);

onMounted(() => {
  resizeObserver = new ResizeObserver(updateOverflowState);
  if (tabList.value) resizeObserver.observe(tabList.value);
  updateOverflowState();
  document.addEventListener("pointerdown", handleDocumentPointerDown);
  document.addEventListener("keydown", handleDocumentKeyDown);
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  document.removeEventListener("pointerdown", handleDocumentPointerDown);
  document.removeEventListener("keydown", handleDocumentKeyDown);
});
</script>

<template>
  <div
    class="nvx-terminal-tab-bar"
    data-tauri-drag-region="deep"
  >
    <div
      class="nvx-terminal-tab-bar__strip"
      data-tauri-drag-region="deep"
    >
      <NvxIconButton
        v-if="hasOverflow"
        class="nvx-terminal-tab-bar__scroll"
        data-tauri-drag-region="false"
        size="sm"
        :label="scrollBackwardLabel"
        :disabled="!canScrollBackward"
        @click="scrollTabs(-1)"
      >
        <NvxIcon
          :icon="ChevronLeft"
          :size="16"
        />
      </NvxIconButton>
      <div
        ref="tabList"
        class="nvx-terminal-tab-bar__tabs"
        data-tauri-drag-region="deep"
        role="tablist"
        :aria-label="label"
        @scroll="updateOverflowState"
        @wheel="handleWheel"
      >
        <div
          v-for="(item, index) in items"
          :key="item.groupId"
          :ref="(element) => setTabItem(item.groupId, element)"
          class="nvx-terminal-tab-bar__item"
          :class="{ 'nvx-terminal-tab-bar__item--active': modelValue === item.groupId }"
          data-tauri-drag-region="false"
          @contextmenu="openTabContextMenu($event, index)"
        >
          <button
            :ref="(element) => setTabButton(item.groupId, element)"
            class="nvx-terminal-tab-bar__tab"
            data-tauri-drag-region="false"
            type="button"
            role="tab"
            :aria-selected="modelValue === item.groupId"
            :tabindex="modelValue === item.groupId || (!modelValue && index === 0) ? 0 : -1"
            :disabled="busy || item.disabled"
            @click="$emit('update:modelValue', item.groupId)"
            @keydown="handleTabKeydown($event, index)"
          >
            <NvxIcon
              :icon="item.icon ?? SquareTerminal"
              :size="16"
            />
            <span class="nvx-terminal-tab-bar__identity">
              <span class="nvx-terminal-tab-bar__name-row">
                <span class="nvx-terminal-tab-bar__name">{{ item.label }}</span>
                <NvxHostMarker
                  v-if="item.hostMarker"
                  :marker="item.hostMarker"
                />
                <span
                  v-if="item.completionCount"
                  class="nvx-terminal-tab-bar__completion"
                >{{ item.completionCount }}</span>
                <span
                  v-if="item.bellAttention"
                  class="nvx-terminal-tab-bar__bell-attention"
                  :aria-label="item.bellAttentionLabel"
                  role="img"
                >
                  <NvxIcon
                    :icon="BellRing"
                    :size="16"
                    aria-hidden="true"
                  />
                </span>
              </span>
              <span class="nvx-terminal-tab-bar__state">{{ item.stateLabel }}</span>
            </span>
          </button>
          <NvxIconButton
            v-if="closable"
            class="nvx-terminal-tab-bar__close"
            data-tauri-drag-region="false"
            size="sm"
            :label="`${closeLabel}：${item.label}`"
            :disabled="busy || item.disabled"
            @click="$emit('close', item.groupId)"
          >
            <NvxIcon
              :icon="X"
              :size="16"
            />
          </NvxIconButton>
        </div>
      </div>
      <NvxIconButton
        v-if="hasOverflow"
        class="nvx-terminal-tab-bar__scroll"
        data-tauri-drag-region="false"
        size="sm"
        :label="scrollForwardLabel"
        :disabled="!canScrollForward"
        @click="scrollTabs(1)"
      >
        <NvxIcon
          :icon="ChevronRight"
          :size="16"
        />
      </NvxIconButton>
    </div>

    <div
      class="nvx-terminal-tab-bar__actions"
      data-tauri-drag-region="deep"
    >
      <span
        class="nvx-terminal-tab-bar__action-island"
        data-tauri-drag-region="false"
      >
        <slot name="actions" />
      </span>
      <NvxIconButton
        class="nvx-terminal-tab-bar__create"
        data-tauri-drag-region="false"
        :label="newLabel"
        :disabled="createDisabled || busy"
        @click="$emit('create')"
      >
        <NvxIcon
          :icon="Plus"
          :size="20"
        />
      </NvxIconButton>
      <span
        class="nvx-terminal-tab-bar__action-island"
        data-tauri-drag-region="false"
      >
        <slot name="trailing-actions" />
      </span>
    </div>

    <div
      v-if="contextMenu && contextTarget"
      class="nvx-terminal-tab-bar__context-menu"
      data-tauri-drag-region="false"
      role="menu"
      :aria-label="contextMenuLabel"
      :style="{ left: `${contextMenu.left}px`, top: `${contextMenu.top}px` }"
    >
      <button
        class="nvx-terminal-tab-bar__context-menu-item"
        type="button"
        role="menuitem"
        :disabled="contextMenu.index === 0"
        @click="requestContextClose('left')"
      >
        {{ closeLeftLabel }}
      </button>
      <button
        class="nvx-terminal-tab-bar__context-menu-item"
        type="button"
        role="menuitem"
        :disabled="contextMenu.index === items.length - 1"
        @click="requestContextClose('right')"
      >
        {{ closeRightLabel }}
      </button>
      <div
        class="nvx-terminal-tab-bar__context-menu-separator"
        role="separator"
      />
      <button
        class="nvx-terminal-tab-bar__context-menu-item nvx-terminal-tab-bar__context-menu-item--danger"
        type="button"
        role="menuitem"
        @click="requestContextClose('all')"
      >
        {{ closeAllLabel }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.nvx-terminal-tab-bar__name-row { display: flex; align-items: center; gap: 4px; min-width: 0; }
.nvx-terminal-tab-bar__name-row > .nvx-terminal-tab-bar__name { min-width: 0; }
.nvx-terminal-tab-bar__name-row :deep(.nvx-host-marker) { max-width: 6em; font-size: 10px; flex-shrink: 0; }
.nvx-terminal-tab-bar__completion { display: inline-flex; align-items: center; justify-content: center; min-width: 15px; height: 15px; padding: 0 3px; border-radius: var(--nvx-radius-sm); font-size: 10px; color: var(--nvx-color-accent); background: var(--nvx-color-accent-soft); flex-shrink: 0; }
.nvx-terminal-tab-bar__bell-attention { display: inline-flex; align-items: center; justify-content: center; flex: 0 0 auto; color: var(--nvx-color-warning, currentColor); }
.nvx-terminal-tab-bar {
  display: flex;
  min-width: 0;
  min-height: 48px;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.nvx-terminal-tab-bar__tabs {
  display: flex;
  flex: 1 1 auto;
  gap: var(--nvx-space-1);
  align-items: center;
  min-width: 0;
  padding: 0 var(--nvx-space-1);
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
  overscroll-behavior: contain;
}

.nvx-terminal-tab-bar__tabs::-webkit-scrollbar {
  display: none;
}

.nvx-terminal-tab-bar__strip {
  display: flex;
  flex: 1 1 auto;
  min-width: 0;
  align-items: center;
  overflow: hidden;
}

.nvx-terminal-tab-bar__scroll {
  flex: 0 0 auto;
  align-self: center;
  margin: 0 var(--nvx-space-1);
}

.nvx-terminal-tab-bar__item {
  display: flex;
  flex: 0 0 clamp(168px, 18vw, 232px);
  align-items: center;
  height: var(--nvx-control-height-md);
  overflow: hidden;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-secondary);
  transition: background-color var(--nvx-motion-fast), color var(--nvx-motion-fast);
}

.nvx-terminal-tab-bar__tab {
  display: flex;
  flex: 1 1 auto;
  gap: var(--nvx-space-2);
  align-items: center;
  min-width: 0;
  align-self: stretch;
  padding: 0 var(--nvx-space-2) 0 var(--nvx-space-3);
  border: 0;
  background: transparent;
  color: inherit;
  text-align: start;
  cursor: pointer;
}

.nvx-terminal-tab-bar__item:hover {
  background: var(--nvx-color-bg-hover);
  color: var(--nvx-color-text-primary);
}

.nvx-terminal-tab-bar__item--active {
  background: var(--nvx-color-bg-subtle);
  color: var(--nvx-color-text-primary);
}

.nvx-terminal-tab-bar__item--active:hover {
  background: var(--nvx-color-bg-hover);
}

.nvx-terminal-tab-bar__item--active .nvx-terminal-tab-bar__state {
  color: var(--nvx-color-text-secondary);
}

.nvx-terminal-tab-bar__tab:focus-visible {
  z-index: 1;
  border-radius: inherit;
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -2px;
}

.nvx-terminal-tab-bar__close {
  z-index: 1;
  flex: 0 0 auto;
  margin-right: var(--nvx-space-1);
}

.nvx-terminal-tab-bar__context-menu {
  position: fixed;
  z-index: var(--nvx-z-popover);
  display: grid;
  width: min(196px, calc(100vw - 16px));
  padding: 2px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.nvx-terminal-tab-bar__context-menu-item {
  min-height: 30px;
  padding: 0 var(--nvx-space-2);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-xs);
  text-align: start;
  cursor: pointer;
}

.nvx-terminal-tab-bar__context-menu-item:hover:not(:disabled),
.nvx-terminal-tab-bar__context-menu-item:focus-visible {
  outline: none;
  background: var(--nvx-color-bg-hover);
}

.nvx-terminal-tab-bar__context-menu-item:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.nvx-terminal-tab-bar__context-menu-item--danger {
  color: var(--nvx-color-danger);
}

.nvx-terminal-tab-bar__context-menu-separator {
  height: var(--nvx-border-width);
  margin: 2px var(--nvx-space-2);
  background: var(--nvx-color-border);
}

.nvx-terminal-tab-bar__identity {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.nvx-terminal-tab-bar__name,
.nvx-terminal-tab-bar__state {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-terminal-tab-bar__name {
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.nvx-terminal-tab-bar__state {
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.nvx-terminal-tab-bar__actions {
  display: flex;
  flex: 0 0 auto;
  gap: var(--nvx-space-2);
  align-items: center;
  padding: 0 var(--nvx-space-3);
}

.nvx-terminal-tab-bar__action-island {
  display: contents;
}

.nvx-terminal-tab-bar__create {
  background: transparent;
  color: var(--nvx-color-text-primary);
}

.nvx-terminal-tab-bar__create:hover:not(:disabled) {
  background: var(--nvx-color-bg-hover);
}
</style>
