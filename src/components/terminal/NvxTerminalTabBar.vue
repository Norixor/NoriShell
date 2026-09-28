<script setup lang="ts">
import { BellRing, ChevronLeft, ChevronRight, Plus, SquareTerminal, X } from "lucide-vue-next";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { Menu } from "@tauri-apps/api/menu";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { nextTick, onBeforeUnmount, onMounted, ref, watch, type Component } from "vue";

import { NvxIcon, NvxIconButton } from "../ui";
import NvxHostMarker from "../hosts/NvxHostMarker.vue";
import type { HostMarker } from "../../host-markers";

export interface TerminalTabItem {
  groupId: string;
  label: string;
  stateLabel: string;
  icon?: Component;
  compact?: boolean;
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
    moveToNewWindowLabel?: string;
    moveToMainWindowLabel?: string;
    scrollBackwardLabel?: string;
    scrollForwardLabel?: string;
    closable?: boolean;
    busy?: boolean;
    createDisabled?: boolean;
    dragEnabled?: boolean;
    incomingDrag?: boolean;
  }>(),
  {
    closeLabel: "Close tab",
    closeAllLabel: "Close all tabs",
    closeLeftLabel: "Close tabs to the left",
    closeRightLabel: "Close tabs to the right",
    moveToNewWindowLabel: "Move to a new window",
    moveToMainWindowLabel: "",
    scrollBackwardLabel: "Show earlier tabs",
    scrollForwardLabel: "Show later tabs",
    closable: true,
    busy: false,
    createDisabled: false,
    dragEnabled: false,
    incomingDrag: false,
  },
);

const emit = defineEmits<{
  "update:modelValue": [groupId: string];
  create: [];
  close: [groupId: string];
  closeMany: [groupIds: string[]];
  "tab-pointer-down": [groupId: string, event: PointerEvent];
  "move-to-new-window": [groupId: string];
  "move-to-main-window": [groupId: string];
}>();

const tabList = ref<HTMLElement | null>(null);
const incomingPlaceholder = ref<HTMLElement | null>(null);
const tabButtons = new Map<string, HTMLButtonElement>();
const tabItems = new Map<string, HTMLElement>();
const hasOverflow = ref(false);
const canScrollBackward = ref(false);
const canScrollForward = ref(false);
let resizeObserver: ResizeObserver | null = null;
let nativeMenuBusy = false;

// Tauri keeps every inline menu action channel in an app-wide map keyed by
// MenuId and never removes it when the menu resource closes. Fixed ids make each
// new popup replace (and drop) the previous channels, bounding the retained
// callbacks to this constant set instead of growing with every right click.
// Native popups are modal, so a shared id set cannot route a pending action
// to another open menu.
const NATIVE_TAB_MENU_ID = "norishell.terminal-tab-menu";
const nativeTabMenuItemId = (action: string) => `${NATIVE_TAB_MENU_ID}.${action}`;

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

function handleTabPointerDown(groupId: string, event: PointerEvent) {
  if (!props.dragEnabled || props.busy || event.button !== 0
    || props.items.find((item) => item.groupId === groupId)?.disabled) return;
  emit("tab-pointer-down", groupId, event);
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
  } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
    event.preventDefault();
    if (props.busy || props.items[index]?.disabled) return;
    const groupId = props.items[index]?.groupId;
    if (!groupId) return;
    const bounds = tabButtons.get(groupId)?.getBoundingClientRect();
    openContextMenu(groupId, bounds?.left ?? 0, bounds?.bottom ?? 0);
  }
}

// Tab content lives in native child WebViews layered above this Header
// WebView, so an HTML menu would be clipped; always use the native popup.
function openContextMenu(groupId: string, x: number, y: number) {
  void openNativeContextMenu(groupId, x, y).catch((error: unknown) => {
    console.error("Native Tab menu failed", error);
  });
}

function openTabContextMenu(event: MouseEvent, index: number) {
  if (props.busy || props.items[index]?.disabled) return;
  event.preventDefault();
  const groupId = props.items[index]?.groupId;
  if (groupId) openContextMenu(groupId, event.clientX, event.clientY);
}

async function openNativeContextMenu(groupId: string, x: number, y: number) {
  if (nativeMenuBusy) return;
  const index = props.items.findIndex((item) => item.groupId === groupId);
  if (index < 0 || props.items[index]?.disabled) return;
  nativeMenuBusy = true;
  const close = (scope: "all" | "left" | "right") => {
    const currentIndex = props.items.findIndex((item) => item.groupId === groupId);
    if (currentIndex < 0) return;
    const ids = (scope === "all" ? props.items : scope === "left"
      ? props.items.slice(0, currentIndex) : props.items.slice(currentIndex + 1)).map((item) => item.groupId);
    if (ids.length) emit("closeMany", ids);
  };
  let menu: Menu | null = null;
  try {
    menu = await Menu.new({ id: NATIVE_TAB_MENU_ID, items: [
      ...(props.dragEnabled ? [{ id: nativeTabMenuItemId("move-to-new-window"), text: props.moveToNewWindowLabel,
        action: () => { if (props.items.some((item) => item.groupId === groupId)) emit("move-to-new-window", groupId); } }] : []),
      ...(props.dragEnabled && props.moveToMainWindowLabel ? [{ id: nativeTabMenuItemId("move-to-main-window"),
        text: props.moveToMainWindowLabel,
        action: () => { if (props.items.some((item) => item.groupId === groupId)) emit("move-to-main-window", groupId); } }] : []),
      { id: nativeTabMenuItemId("close-left"), text: props.closeLeftLabel, enabled: index > 0, action: () => close("left") },
      { id: nativeTabMenuItemId("close-right"), text: props.closeRightLabel, enabled: index < props.items.length - 1,
        action: () => close("right") },
      { item: "Separator" as const },
      { id: nativeTabMenuItemId("close-all"), text: props.closeAllLabel, action: () => close("all") },
    ] });
    await menu.popup(new LogicalPosition(x, y), getCurrentWindow());
  } finally {
    if (menu) await menu.close().catch(() => undefined);
    nativeMenuBusy = false;
  }
}

watch(
  () => [props.items.length, props.modelValue] as const,
  () => void nextTick(() => {
    updateOverflowState();
    tabItems.get(props.modelValue)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }),
);

watch(() => props.incomingDrag, () => void nextTick(() => {
  updateOverflowState();
  if (props.incomingDrag) incomingPlaceholder.value?.scrollIntoView({ block: "nearest", inline: "nearest" });
}));

onMounted(() => {
  resizeObserver = new ResizeObserver(updateOverflowState);
  if (tabList.value) resizeObserver.observe(tabList.value);
  updateOverflowState();
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
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
          :class="{
            'nvx-terminal-tab-bar__item--active': modelValue === item.groupId,
            'nvx-terminal-tab-bar__item--compact': item.compact,
          }"
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
            :title="`${item.label} · ${item.stateLabel}`"
            @click="$emit('update:modelValue', item.groupId)"
            @keydown="handleTabKeydown($event, index)"
            @pointerdown="handleTabPointerDown(item.groupId, $event)"
            @dragstart.prevent
          >
            <NvxIcon
              :icon="item.icon ?? SquareTerminal"
              :size="16"
            />
            <span class="nvx-terminal-tab-bar__identity">
              <span class="nvx-terminal-tab-bar__name-row">
                <span class="nvx-terminal-tab-bar__name">{{ item.label }}</span>
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
            <NvxHostMarker
              v-if="item.hostMarker"
              :marker="item.hostMarker"
            />
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
        <div
          v-if="incomingDrag"
          ref="incomingPlaceholder"
          class="nvx-terminal-tab-bar__incoming"
          data-tauri-drag-region="false"
          aria-hidden="true"
        >
          <span class="nvx-terminal-tab-bar__incoming-icon" />
          <span class="nvx-terminal-tab-bar__incoming-lines">
            <span class="nvx-terminal-tab-bar__incoming-line nvx-terminal-tab-bar__incoming-line--title" />
            <span class="nvx-terminal-tab-bar__incoming-line nvx-terminal-tab-bar__incoming-line--state" />
          </span>
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
  </div>
</template>

<style scoped>
.nvx-terminal-tab-bar__incoming {
  display: flex;
  flex: 0 0 clamp(220px, 22vw, 272px);
  align-items: center;
  gap: var(--nvx-space-2);
  height: 44px;
  box-sizing: border-box;
  padding: 0 var(--nvx-space-3);
  border: 1px dashed var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
  pointer-events: none;
}
.nvx-terminal-tab-bar__incoming-icon,
.nvx-terminal-tab-bar__incoming-line {
  display: block;
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-border);
  animation: nvx-tab-incoming-pulse 1.2s ease-in-out infinite alternate;
}
.nvx-terminal-tab-bar__incoming-icon { width: 16px; height: 16px; flex: 0 0 auto; }
.nvx-terminal-tab-bar__incoming-lines { display: grid; flex: 1; gap: 6px; }
.nvx-terminal-tab-bar__incoming-line--title { width: 72%; height: 10px; }
.nvx-terminal-tab-bar__incoming-line--state { width: 48%; height: 8px; }
@keyframes nvx-tab-incoming-pulse { to { opacity: .38; } }
@media (prefers-reduced-motion: reduce) {
  .nvx-terminal-tab-bar__incoming-icon,
  .nvx-terminal-tab-bar__incoming-line { animation: none; }
}

.nvx-terminal-tab-bar__name-row { display: flex; align-items: center; gap: 4px; min-width: 0; overflow: hidden; }
.nvx-terminal-tab-bar__name-row > .nvx-terminal-tab-bar__name { min-width: 0; }
.nvx-terminal-tab-bar__tab :deep(.nvx-host-marker) { max-width: 6em; font-size: 10px; flex: 0 0 auto; }
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
  flex: 0 0 clamp(220px, 22vw, 272px);
  align-items: center;
  height: 44px;
  overflow: hidden;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-secondary);
  transition: background-color var(--nvx-motion-fast), color var(--nvx-motion-fast);
}

.nvx-terminal-tab-bar__item--compact {
  flex: 0 1 auto;
  min-width: 144px;
  max-width: 220px;
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







.nvx-terminal-tab-bar__identity {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 0;
  min-width: 0;
}

.nvx-terminal-tab-bar__name,
.nvx-terminal-tab-bar__state {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-terminal-tab-bar__name {
  flex: 1 1 auto;
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
