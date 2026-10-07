<script lang="ts">
export interface NvxSelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}
</script>

<script setup lang="ts">
import { Check, ChevronDown } from "lucide-vue-next";
import {
  computed,
  nextTick,
  onBeforeUnmount,
  ref,
  useId,
  watch,
  type CSSProperties,
} from "vue";

import { mountMobileSheetBackdrop, pushMobileOverlay, type MobileOverlayHandle, type MobileSheetBackdropHandle } from "../../mobile-overlays";
import { isMobileLayout } from "../../platform-host";
import NvxIcon from "./NvxIcon.vue";

const props = withDefaults(
  defineProps<{
    id?: string;
    modelValue: string;
    options?: readonly NvxSelectOption[];
    placeholder?: string;
    disabled?: boolean;
    ariaLabel?: string;
    popupMinWidth?: number;
    /** Phone sheet only: accessible name and placeholder of the option filter shown for long lists. */
    searchLabel?: string;
  }>(),
  {
    id: undefined,
    options: () => [],
    placeholder: undefined,
    disabled: false,
    ariaLabel: undefined,
    popupMinWidth: undefined,
    searchLabel: undefined,
  },
);

const emit = defineEmits<{ "update:modelValue": [value: string] }>();

const root = ref<HTMLElement>();
const isProtected = ref(false);
const trigger = ref<HTMLButtonElement>();
const listbox = ref<HTMLElement>();
const isOpen = ref(false);
const activeIndex = ref(-1);
const opensAbove = ref(false);
const isInDialog = ref(false);
const popupStyle = ref<CSSProperties>({});
const generatedId = useId();

// Phone shell: the listbox is a bottom sheet with a filter for long lists (platform fact, not width).
const mobile = isMobileLayout();
const MOBILE_SEARCH_THRESHOLD = 8;
const sheet = ref<HTMLElement>();
const searchInput = ref<HTMLInputElement>();
const filter = ref("");
const showSearch = computed(() => mobile && props.options.length > MOBILE_SEARCH_THRESHOLD);
const visibleIndexes = computed(() => {
  const needle = filter.value.trim().toLowerCase();
  const indexes: number[] = [];
  props.options.forEach((option, index) => {
    if (!needle || option.label.toLowerCase().includes(needle)) indexes.push(index);
  });
  return indexes;
});
let overlay: MobileOverlayHandle | null = null;
let removeBackdrop: MobileSheetBackdropHandle | null = null;

function presentMobileSheet() {
  removeBackdrop ??= mountMobileSheetBackdrop(() => close({ restoreFocus: true }));
  overlay ??= pushMobileOverlay(() => { close({ restoreFocus: true }); return true; }, () => [removeBackdrop?.element, sheet.value]);
  void nextTick(() => {
    overlay?.refresh();
    if (showSearch.value) searchInput.value?.focus();
  });
}

function dismissMobileSheet() {
  removeBackdrop?.();
  removeBackdrop = null;
  overlay?.release();
  overlay = null;
  filter.value = "";
}

const listboxId = computed(() => `${props.id ?? generatedId}-listbox`);
const selectedIndex = computed(() =>
  props.options.findIndex((option) => option.value === props.modelValue),
);
const selectedOption = computed(() => props.options[selectedIndex.value]);
const triggerLabel = computed(
  () => selectedOption.value?.label ?? props.placeholder ?? props.modelValue,
);
const activeDescendant = computed(() =>
  isOpen.value && activeIndex.value >= 0 ? optionId(activeIndex.value) : undefined,
);

function optionId(index: number) {
  return `${listboxId.value}-option-${index}`;
}

function firstEnabledIndex() {
  return props.options.findIndex((option) => !option.disabled);
}

function lastEnabledIndex() {
  for (let index = props.options.length - 1; index >= 0; index -= 1) {
    if (!props.options[index]?.disabled) return index;
  }
  return -1;
}

function initialActiveIndex(direction: "first" | "last" = "first") {
  if (selectedIndex.value >= 0 && !props.options[selectedIndex.value]?.disabled) {
    return selectedIndex.value;
  }
  return direction === "last" ? lastEnabledIndex() : firstEnabledIndex();
}

function updatePopupPosition() {
  if (!trigger.value) return;

  const viewportPadding = 8;
  const popupGap = 4;
  const maximumHeight = 240;
  const optionHeight = 40;
  const rect = trigger.value.getBoundingClientRect();
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const desiredHeight = Math.min(
    maximumHeight,
    props.options.length * optionHeight + viewportPadding,
  );
  const spaceBelow = viewportHeight - rect.bottom - viewportPadding - popupGap;
  const spaceAbove = rect.top - viewportPadding - popupGap;
  const useAbove = spaceBelow < desiredHeight && spaceAbove > spaceBelow;
  const availableHeight = Math.max(optionHeight, useAbove ? spaceAbove : spaceBelow);
  const width = Math.min(
    Math.max(rect.width, props.popupMinWidth ?? 0),
    viewportWidth - viewportPadding * 2,
  );
  const left = Math.min(
    Math.max(viewportPadding, rect.left),
    Math.max(viewportPadding, viewportWidth - viewportPadding - width),
  );

  opensAbove.value = useAbove;
  popupStyle.value = {
    left: `${left}px`,
    width: `${width}px`,
    maxHeight: `${Math.min(maximumHeight, availableHeight)}px`,
    ...(useAbove
      ? { bottom: `${viewportHeight - rect.top + popupGap}px`, top: "auto" }
      : { top: `${rect.bottom + popupGap}px`, bottom: "auto" }),
  };
}

function scrollActiveOptionIntoView() {
  void nextTick(() => {
    const option = listbox.value?.querySelector<HTMLElement>(
      `#${CSS.escape(optionId(activeIndex.value))}`,
    );
    option?.scrollIntoView?.({ block: "nearest" });
  });
}

function open(direction: "first" | "last" = "first") {
  if (props.disabled || isOpen.value || firstEnabledIndex() < 0) return;
  activeIndex.value = initialActiveIndex(direction);
  isInDialog.value = Boolean(root.value?.closest('[role="dialog"]'));
  isProtected.value = Boolean(root.value?.closest("[data-theme-protected]"));
  if (mobile) popupStyle.value = {};
  else updatePopupPosition();
  isOpen.value = true;
  if (mobile) presentMobileSheet();
  scrollActiveOptionIntoView();
}

function close({ restoreFocus = false } = {}) {
  if (!isOpen.value) return;
  isOpen.value = false;
  if (mobile) dismissMobileSheet();
  if (restoreFocus) void nextTick(() => trigger.value?.focus());
}

function toggle() {
  if (isOpen.value) close();
  else open();
}

function moveActive(step: 1 | -1) {
  if (firstEnabledIndex() < 0) return;

  // The filtered sheet only cycles through the options that are currently listed.
  const candidates = showSearch.value ? visibleIndexes.value : props.options.map((_, index) => index);
  if (!candidates.length) return;
  let position = candidates.indexOf(activeIndex.value);
  for (let attempts = 0; attempts < candidates.length; attempts += 1) {
    position = (position + step + candidates.length) % candidates.length;
    const next = candidates[position]!;
    if (!props.options[next]?.disabled) {
      activeIndex.value = next;
      scrollActiveOptionIntoView();
      return;
    }
  }
}

function select(index: number) {
  const option = props.options[index];
  if (!option || option.disabled) return;
  emit("update:modelValue", option.value);
  close({ restoreFocus: true });
}

function onKeydown(event: KeyboardEvent) {
  if (props.disabled) return;

  switch (event.key) {
    case "ArrowDown":
      event.preventDefault();
      if (isOpen.value) moveActive(1);
      else open("first");
      break;
    case "ArrowUp":
      event.preventDefault();
      if (isOpen.value) moveActive(-1);
      else open("last");
      break;
    case "Home":
      event.preventDefault();
      if (!isOpen.value) open("first");
      activeIndex.value = firstEnabledIndex();
      scrollActiveOptionIntoView();
      break;
    case "End":
      event.preventDefault();
      if (!isOpen.value) open("last");
      activeIndex.value = lastEnabledIndex();
      scrollActiveOptionIntoView();
      break;
    case "Enter":
    case " ":
      event.preventDefault();
      if (isOpen.value) select(activeIndex.value);
      else open();
      break;
    case "Escape":
      if (!isOpen.value) return;
      event.preventDefault();
      close({ restoreFocus: true });
      break;
    case "Tab":
      close();
      break;
  }
}

function onDocumentPointerDown(event: PointerEvent) {
  const target = event.target as Node | null;
  if (root.value?.contains(target) || listbox.value?.contains(target) || sheet.value?.contains(target)) return;
  close();
}

function onViewportChange() {
  if (isOpen.value && !mobile) updatePopupPosition();
}

watch(isOpen, (open) => {
  if (open) {
    document.addEventListener("pointerdown", onDocumentPointerDown, true);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onViewportChange, true);
  } else {
    document.removeEventListener("pointerdown", onDocumentPointerDown, true);
    window.removeEventListener("resize", onViewportChange);
    window.removeEventListener("scroll", onViewportChange, true);
  }
});

watch(
  () => [props.modelValue, props.options, props.disabled] as const,
  () => {
    if (props.disabled) {
      close();
      return;
    }
    if (isOpen.value) {
      activeIndex.value = initialActiveIndex();
      if (!mobile) updatePopupPosition();
      scrollActiveOptionIntoView();
    }
  },
);

onBeforeUnmount(() => {
  dismissMobileSheet();
  document.removeEventListener("pointerdown", onDocumentPointerDown, true);
  window.removeEventListener("resize", onViewportChange);
  window.removeEventListener("scroll", onViewportChange, true);
});
</script>

<template>
  <span
    ref="root"
    class="nvx-select"
  >
    <button
      :id="id"
      ref="trigger"
      class="nvx-select__trigger"
      type="button"
      role="combobox"
      aria-haspopup="listbox"
      :aria-label="ariaLabel"
      :aria-expanded="isOpen"
      :aria-controls="listboxId"
      :aria-activedescendant="activeDescendant"
      :disabled="disabled"
      @click="toggle"
      @keydown="onKeydown"
    >
      <span class="nvx-select__value">{{ triggerLabel }}</span>
      <NvxIcon
        class="nvx-select__chevron"
        :class="{ 'nvx-select__chevron--open': isOpen }"
        :icon="ChevronDown"
        :size="16"
      />
    </button>

    <Teleport to="body">
      <template v-if="isOpen">
        <div
          v-if="mobile"
          ref="sheet"
          class="nvx-select__sheet"
          :class="{ 'nvx-select__sheet--in-dialog': isInDialog }"
          :data-theme-protected="isProtected ? '' : undefined"
          data-nvx-mobile-sheet
        >
          <input
            v-if="showSearch"
            ref="searchInput"
            v-model="filter"
            class="nvx-select__search"
            type="search"
            :aria-label="searchLabel ?? ariaLabel"
            :placeholder="searchLabel"
            :aria-controls="listboxId"
            autocomplete="off"
            @keydown="onKeydown"
          >
          <ul
            :id="listboxId"
            ref="listbox"
            class="nvx-select__listbox nvx-select__listbox--sheet"
            role="listbox"
          >
            <li
              v-for="index in visibleIndexes"
              :id="optionId(index)"
              :key="`${options[index]!.value}-${index}`"
              class="nvx-select__option"
              :class="{
                'nvx-select__option--active': activeIndex === index,
                'nvx-select__option--selected': modelValue === options[index]!.value,
                'nvx-select__option--disabled': options[index]!.disabled,
              }"
              role="option"
              :aria-selected="modelValue === options[index]!.value"
              :aria-disabled="options[index]!.disabled || undefined"
              @pointerdown.prevent
              @click="select(index)"
            >
              <span class="nvx-select__option-label">{{ options[index]!.label }}</span>
              <NvxIcon
                v-if="modelValue === options[index]!.value"
                class="nvx-select__check"
                :icon="Check"
                :size="16"
              />
            </li>
          </ul>
        </div>
        <ul
          v-else
          :id="listboxId"
          ref="listbox"
          class="nvx-select__listbox"
          :class="{
            'nvx-select__listbox--above': opensAbove,
            'nvx-select__listbox--in-dialog': isInDialog,
          }"
          :data-theme-protected="isProtected ? '' : undefined"
          role="listbox"
          :style="popupStyle"
        >
          <li
            v-for="(option, index) in options"
            :id="optionId(index)"
            :key="`${option.value}-${index}`"
            class="nvx-select__option"
            :class="{
              'nvx-select__option--active': activeIndex === index,
              'nvx-select__option--selected': modelValue === option.value,
              'nvx-select__option--disabled': option.disabled,
            }"
            role="option"
            :aria-selected="modelValue === option.value"
            :aria-disabled="option.disabled || undefined"
            @pointerdown.prevent
            @mouseenter="!option.disabled && (activeIndex = index)"
            @click="select(index)"
          >
            <span class="nvx-select__option-label">{{ option.label }}</span>
            <NvxIcon
              v-if="modelValue === option.value"
              class="nvx-select__check"
              :icon="Check"
              :size="16"
            />
          </li>
        </ul>
      </template>
    </Teleport>
  </span>
</template>

<style scoped>
.nvx-select {
  position: relative;
  display: inline-block;
  width: 100%;
}

.nvx-select__trigger {
  display: flex;
  align-items: center;
  width: 100%;
  min-height: var(--nvx-control-height-md);
  padding: 0 var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: start;
  cursor: pointer;
  transition: border-color var(--nvx-motion-fast), box-shadow var(--nvx-motion-fast), background-color var(--nvx-motion-fast);
}

.nvx-select__trigger:hover:not(:disabled) {
  background: var(--nvx-color-bg-hover);
}

.nvx-select__trigger:focus-visible {
  border-color: var(--nvx-color-accent);
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.nvx-select__trigger:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.nvx-select__value,
.nvx-select__option-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-select__value {
  flex: 1 1 auto;
}

.nvx-select__chevron {
  flex: 0 0 auto;
  margin-inline-start: var(--nvx-space-2);
  color: var(--nvx-color-text-tertiary);
  transition: transform var(--nvx-motion-fast);
}

.nvx-select__chevron--open {
  transform: rotate(180deg);
}

.nvx-select__listbox {
  position: fixed;
  z-index: var(--nvx-z-popover);
  min-width: 0;
  margin: 0;
  padding: var(--nvx-space-1);
  overflow-y: auto;
  overscroll-behavior: contain;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  box-shadow: var(--nvx-shadow-overlay);
  list-style: none;
}

.nvx-select__listbox--in-dialog {
  z-index: calc(var(--nvx-z-dialog) + 1);
}

.nvx-select__option {
  display: flex;
  align-items: center;
  min-height: var(--nvx-control-height-md);
  padding: 0 var(--nvx-space-2);
  border-radius: var(--nvx-radius-sm);
  cursor: pointer;
}

.nvx-select__option--active {
  background: var(--nvx-color-bg-hover);
}

.nvx-select__option--selected {
  color: var(--nvx-color-accent);
}

.nvx-select__option--disabled {
  color: var(--nvx-color-text-tertiary);
  cursor: not-allowed;
  opacity: 0.55;
}

.nvx-select__option-label {
  flex: 1 1 auto;
}

.nvx-select__check {
  flex: 0 0 auto;
  margin-inline-start: var(--nvx-space-2);
  color: var(--nvx-color-accent);
}

@media (prefers-reduced-motion: reduce) {
  .nvx-select__chevron {
    transition: none;
  }
}

/* Phone sheet: the global `[data-nvx-mobile-sheet]` rules place the wrapper; the list flows inside it. */
.nvx-select__sheet {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  color: var(--nvx-color-text-primary);
}

.nvx-select__search {
  min-height: var(--nvx-control-height-touch);
  margin: var(--nvx-space-1) var(--nvx-space-2) var(--nvx-space-2);
  padding: 0 var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-canvas);
  color: inherit;
  font: inherit;
}

.nvx-select__search:focus-visible {
  border-color: var(--nvx-color-accent);
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.nvx-select__listbox--sheet {
  position: static;
  z-index: auto;
  min-height: 0;
  padding: 0;
  overflow-y: auto;
  border: 0;
  border-radius: 0;
  box-shadow: none;
}
</style>
