<script setup lang="ts">
import { X } from "lucide-vue-next";
import { nextTick, onBeforeUnmount, ref, watch } from "vue";

import { pushMobileOverlay, type MobileOverlayHandle } from "../../mobile-overlays";
import { isMobileLayout } from "../../platform-host";
import NvxIcon from "./NvxIcon.vue";
import NvxIconButton from "./NvxIconButton.vue";

let dialogSequence = 0;

const props = withDefaults(
  defineProps<{
    modelValue: boolean;
    title: string;
    closeLabel: string;
    description?: string;
    dismissible?: boolean;
    size?: "md" | "lg" | "xl";
    layout?: "default" | "split";
    pluginProtected?: boolean;
    themeProtected?: boolean;
    /**
     * Phone presentation only (ignored on desktop): `auto` sizes the sheet to its
     * content, `tall` reserves about three quarters of the viewport for lists,
     * `full` fills the viewport for editors. `xl` and `split` dialogs default to `full`.
     */
    sheetSize?: "auto" | "tall" | "full";
  }>(),
  {
    description: undefined,
    dismissible: true,
    size: "md",
    layout: "default",
    pluginProtected: false,
    themeProtected: undefined,
    sheetSize: undefined,
  },
);

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  close: [];
}>();

defineSlots<{
  default(): unknown;
  sidebar(): unknown;
  actions(): unknown;
}>();

dialogSequence += 1;
const titleId = `nvx-dialog-title-${dialogSequence}`;
const descriptionId = `nvx-dialog-description-${dialogSequence}`;
const panel = ref<HTMLElement | null>(null);
let restoreTarget: HTMLElement | null = null;

// The phone shell presents every dialog as a bottom sheet: a platform fact, never a viewport measurement.
const mobile = isMobileLayout();
const resolvedSheetSize = () => props.sheetSize ?? (props.size === "xl" || props.layout === "split" ? "full" : "auto");
let overlay: MobileOverlayHandle | null = null;
const sheetOffset = ref(0);
const sheetDragging = ref(false);
let sheetDrag: { pointerId: number; startY: number; startTime: number } | null = null;
const SHEET_DISMISS_DISTANCE = 96;
const SHEET_DISMISS_VELOCITY = 0.6;

function beginSheetDrag(event: PointerEvent) {
  if (!mobile || !props.dismissible || event.button !== 0) return;
  sheetDrag = { pointerId: event.pointerId, startY: event.clientY, startTime: event.timeStamp };
  sheetDragging.value = true;
  (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
}

function moveSheetDrag(event: PointerEvent) {
  if (!sheetDrag || sheetDrag.pointerId !== event.pointerId) return;
  sheetOffset.value = Math.max(0, event.clientY - sheetDrag.startY);
}

function endSheetDrag(event: PointerEvent) {
  if (!sheetDrag || sheetDrag.pointerId !== event.pointerId) return;
  const distance = event.clientY - sheetDrag.startY;
  const velocity = distance / Math.max(1, event.timeStamp - sheetDrag.startTime);
  sheetDrag = null;
  sheetDragging.value = false;
  sheetOffset.value = 0;
  if (distance > SHEET_DISMISS_DISTANCE || (distance > 24 && velocity > SHEET_DISMISS_VELOCITY)) requestClose();
}

function cancelSheetDrag() {
  sheetDrag = null;
  sheetDragging.value = false;
  sheetOffset.value = 0;
}

// The soft keyboard shrinks the sheet; keep the focused field inside the scrollable body.
function keepFocusedFieldVisible() {
  const active = document.activeElement;
  if (active instanceof HTMLElement && panel.value?.contains(active)) active.scrollIntoView({ block: "nearest" });
}

function registerOverlay(open: boolean) {
  if (open && !overlay) {
    overlay = pushMobileOverlay(() => {
      // A blocking dialog consumes the key without closing; cancel is the only way out.
      if (props.dismissible) requestClose();
      return true;
    }, () => [panel.value?.parentElement]);
    // The backdrop renders with this tick; the stack writes its z order once it exists.
    void nextTick(() => overlay?.refresh());
    if (mobile) window.visualViewport?.addEventListener("resize", keepFocusedFieldVisible);
  } else if (!open && overlay) {
    overlay.release();
    overlay = null;
    if (mobile) window.visualViewport?.removeEventListener("resize", keepFocusedFieldVisible);
  }
}

const focusableSelector = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

function focusableElements() {
  return Array.from(panel.value?.querySelectorAll<HTMLElement>(focusableSelector) ?? []).filter(
    (element) => !element.hidden && element.getAttribute("aria-hidden") !== "true",
  );
}

async function focusDialog() {
  await nextTick();
  const initial = panel.value?.querySelector<HTMLElement>("[data-nvx-dialog-initial-focus]");
  const target = initial ?? focusableElements()[0] ?? panel.value;
  // A phone sheet never autofocuses a text field: Android would open the soft keyboard
  // over the sheet the user just opened. The field takes focus when the user taps it.
  if (mobile && target && target !== panel.value && target.matches("input, textarea, select, [contenteditable]")) {
    panel.value?.focus();
    return;
  }
  target?.focus();
}

function requestClose() {
  if (!props.dismissible) return;
  emit("update:modelValue", false);
  emit("close");
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    requestClose();
    return;
  }
  if (event.key !== "Tab") return;

  const focusable = focusableElements();
  if (focusable.length === 0) {
    event.preventDefault();
    panel.value?.focus();
    return;
  }
  const first = focusable[0];
  const last = focusable.at(-1);
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last?.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first?.focus();
  }
}

function restoreFocus() {
  const target = restoreTarget;
  restoreTarget = null;
  void nextTick(() => {
    if (target?.isConnected) target.focus();
  });
}

watch(
  () => props.modelValue,
  (open, wasOpen) => {
    registerOverlay(open);
    if (open) {
      if (!wasOpen) restoreTarget = document.activeElement as HTMLElement | null;
      void focusDialog();
    } else if (wasOpen) {
      cancelSheetDrag();
      restoreFocus();
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  registerOverlay(false);
  restoreFocus();
});
</script>

<template>
  <Teleport to="body">
    <div
      v-if="modelValue"
      class="nvx-dialog__backdrop"
      :class="{ 'nvx-dialog__backdrop--sheet': mobile }"
      :data-plugin-protected="pluginProtected ? '' : undefined"
      :data-theme-protected="(themeProtected ?? pluginProtected) ? '' : undefined"
      @mousedown.self="requestClose"
    >
      <section
        ref="panel"
        class="nvx-dialog"
        :class="[
          layout === 'split' ? 'nvx-dialog--split' : `nvx-dialog--${size}`,
          mobile ? ['nvx-dialog--sheet', `nvx-dialog--sheet-${resolvedSheetSize()}`, { 'nvx-dialog--sheet-dragging': sheetDragging }] : null,
        ]"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        :aria-describedby="description ? descriptionId : undefined"
        tabindex="-1"
        :style="mobile && sheetOffset ? { transform: `translateY(${sheetOffset}px)` } : undefined"
        @keydown="handleKeydown"
      >
        <div
          v-if="mobile"
          class="nvx-dialog__handle"
          :class="{ 'nvx-dialog__handle--static': !dismissible }"
          aria-hidden="true"
          @pointerdown="beginSheetDrag"
          @pointermove="moveSheetDrag"
          @pointerup="endSheetDrag"
          @pointercancel="cancelSheetDrag"
        >
          <span class="nvx-dialog__handle-pill" />
        </div>
        <div class="nvx-dialog__aside">
          <header class="nvx-dialog__header">
            <h2
              :id="titleId"
              class="nvx-dialog__title"
            >
              {{ title }}
            </h2>
            <NvxIconButton
              v-if="dismissible"
              class="nvx-dialog__close"
              :label="closeLabel"
              size="sm"
              @click="requestClose"
            >
              <NvxIcon
                :icon="X"
                :size="16"
              />
            </NvxIconButton>
          </header>
          <p
            v-if="description"
            :id="descriptionId"
            class="nvx-dialog__description"
          >
            {{ description }}
          </p>
          <div
            v-if="layout === 'split'"
            class="nvx-dialog__sidebar"
          >
            <slot name="sidebar" />
          </div>
        </div>
        <div class="nvx-dialog__body">
          <slot />
        </div>
        <footer class="nvx-dialog__actions">
          <slot name="actions" />
        </footer>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.nvx-dialog__backdrop {
  position: fixed;
  /* The phone overlay stack assigns the order; desktop keeps the static token. */
  z-index: var(--nvx-mobile-overlay-z, var(--nvx-z-dialog));
  inset: 0;
  display: grid;
  place-items: center;
  padding: var(--nvx-space-6);
  background: rgb(16 18 23 / 56%);
}

.nvx-dialog {
  max-height: min(720px, calc(100vh - 48px));
  overflow: auto;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  box-shadow: var(--nvx-shadow-overlay);
}

.nvx-dialog__aside { display: contents; }

.nvx-dialog--split {
  position: relative;
  display: grid;
  grid-template-columns: 240px minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr) auto;
  width: min(100%, 1000px);
  height: min(620px, calc(100vh - 48px));
  overflow: hidden;
}

.nvx-dialog--split .nvx-dialog__aside {
  display: flex;
  flex-direction: column;
  grid-column: 1;
  grid-row: 1 / -1;
  min-height: 0;
  background: var(--nvx-color-bg-canvas);
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-dialog--split .nvx-dialog__header { padding: var(--nvx-space-6) var(--nvx-space-5) 0; }
.nvx-dialog--split .nvx-dialog__close { position: absolute; top: var(--nvx-space-5); right: var(--nvx-space-5); }
.nvx-dialog--split .nvx-dialog__description { padding: var(--nvx-space-2) var(--nvx-space-5) 0; }
.nvx-dialog--split .nvx-dialog__sidebar { min-height: 0; padding: var(--nvx-space-4) var(--nvx-space-5); }
.nvx-dialog--split .nvx-dialog__body {
  grid-column: 2;
  grid-row: 1;
  min-height: 0;
  overflow: hidden;
  padding: 64px var(--nvx-space-6) var(--nvx-space-4);
}
.nvx-dialog--split .nvx-dialog__actions {
  grid-column: 2;
  grid-row: 2;
  align-items: center;
  padding: var(--nvx-space-4) var(--nvx-space-6);
}

@media (max-width: 700px) {
  .nvx-dialog--split { display: flex; flex-direction: column; height: min(620px, calc(100vh - 48px)); }
  .nvx-dialog--split .nvx-dialog__aside { flex: none; border-right: 0; border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
  .nvx-dialog--split .nvx-dialog__header { padding: var(--nvx-space-4) var(--nvx-space-4) 0; }
  .nvx-dialog--split .nvx-dialog__description { padding: var(--nvx-space-1) var(--nvx-space-4) 0; }
  .nvx-dialog--split .nvx-dialog__sidebar { padding: var(--nvx-space-2) var(--nvx-space-4) var(--nvx-space-3); }
  .nvx-dialog--split .nvx-dialog__body { flex: 1; padding: var(--nvx-space-4); }
  .nvx-dialog--split .nvx-dialog__actions { flex: none; padding: var(--nvx-space-3) var(--nvx-space-4); }
}

.nvx-dialog--md {
  width: min(100%, 480px);
}

.nvx-dialog--lg {
  width: min(100%, 720px);
}

.nvx-dialog--xl {
  display: grid;
  grid-template-rows: auto auto minmax(0, 1fr) auto;
  width: min(100%, 700px);
  height: min(590px, calc(100vh - 64px));
  max-height: min(590px, calc(100vh - 64px));
  overflow: hidden;
}

.nvx-dialog--xl .nvx-dialog__header {
  padding: var(--nvx-space-4) var(--nvx-space-4) 0;
}

.nvx-dialog--xl .nvx-dialog__description {
  padding: var(--nvx-space-1) var(--nvx-space-4) var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-dialog--xl .nvx-dialog__body {
  min-height: 0;
  overflow: hidden;
  padding: 0;
}

.nvx-dialog--xl :deep(.hosts-editor) {
  margin: 0;
  grid-template-columns: 190px minmax(0, 1fr);
}

.nvx-dialog--xl :deep(.hosts-editor__nav) {
  padding: var(--nvx-space-4) var(--nvx-space-2);
}

.nvx-dialog--xl :deep(.hosts-editor__nav-item) {
  min-height: 38px;
  gap: var(--nvx-space-2);
  padding-inline: var(--nvx-space-2);
}

.nvx-dialog--xl :deep(.hosts-editor__content) {
  padding: var(--nvx-space-5) var(--nvx-space-6);
}

.nvx-dialog--xl .nvx-dialog__actions {
  padding: var(--nvx-space-3) var(--nvx-space-4);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-dialog:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: var(--nvx-space-1);
}

.nvx-dialog__header {
  display: flex;
  gap: var(--nvx-space-4);
  align-items: flex-start;
  justify-content: space-between;
  padding: var(--nvx-space-5) var(--nvx-space-5) 0;
}

.nvx-dialog__title {
  margin: 0;
  font-size: var(--nvx-font-size-md);
  font-weight: var(--nvx-font-weight-semibold);
  line-height: var(--nvx-line-height-md);
}

.nvx-dialog__description,
.nvx-dialog__body {
  margin: 0;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-body);
  line-height: var(--nvx-line-height-body);
}

.nvx-dialog__description {
  padding: var(--nvx-space-2) var(--nvx-space-5) 0;
}

.nvx-dialog__body {
  display: grid;
  gap: var(--nvx-space-3);
  padding: var(--nvx-space-4) var(--nvx-space-5);
}

.nvx-dialog__actions {
  display: flex;
  gap: var(--nvx-space-2);
  justify-content: flex-end;
  padding: 0 var(--nvx-space-5) var(--nvx-space-5);
}

@media (prefers-reduced-motion: no-preference) {
  .nvx-dialog {
    animation: nvx-dialog-enter var(--nvx-motion-overlay) ease-out;
  }
}

@media (max-height: 800px) {
  .nvx-dialog--xl {
    height: auto;
    max-height: calc(100vh - 48px);
  }

  .nvx-dialog--xl :deep(.hosts-editor__nav) {
    gap: 0;
    padding: var(--nvx-space-3);
  }

  .nvx-dialog--xl :deep(.hosts-editor__nav-item) {
    min-height: 34px;
  }

  .nvx-dialog--xl :deep(.hosts-editor__content) {
    padding-block: var(--nvx-space-5);
  }
}

@keyframes nvx-dialog-enter {
  from {
    opacity: 0;
    transform: translateY(8px);
  }
}

/* ---- Phone bottom-sheet presentation (gated by the platform, not by width) ---- */

/* Flex (not grid) so the sheet's percentage max-height resolves against the definite backdrop height. */
.nvx-dialog__backdrop--sheet {
  top: 0;
  right: 0;
  bottom: auto;
  left: 0;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  height: var(--nvx-mobile-viewport-height, 100vh);
  padding: 0;
}

.nvx-dialog--sheet {
  position: relative;
  display: flex;
  flex: none;
  flex-direction: column;
  width: 100%;
  min-width: 0;
  max-width: none;
  height: auto;
  max-height: calc(100% - env(safe-area-inset-top, 0px) - 24px);
  padding-bottom: env(safe-area-inset-bottom, 0px);
  overflow: hidden;
  border: 0;
  border-radius: var(--nvx-radius-sheet) var(--nvx-radius-sheet) 0 0;
  box-shadow: var(--nvx-shadow-overlay);
}

.nvx-dialog--sheet.nvx-dialog--sheet-dragging { transition: none; }
.nvx-dialog--sheet-tall { height: 75%; }
.nvx-dialog--sheet-full { height: calc(100% - env(safe-area-inset-top, 0px) - 24px); }

.nvx-dialog__handle {
  display: flex;
  flex: none;
  justify-content: center;
  padding: var(--nvx-space-2) 0 var(--nvx-space-1);
  touch-action: none;
  cursor: grab;
}

.nvx-dialog__handle--static { cursor: default; }

.nvx-dialog__handle-pill {
  width: 36px;
  height: 4px;
  border-radius: 2px;
  background: var(--nvx-color-border-strong);
}

.nvx-dialog--sheet .nvx-dialog__aside {
  display: flex;
  flex: none;
  flex-direction: column;
  min-height: 0;
  background: transparent;
  border: 0;
}

.nvx-dialog--sheet .nvx-dialog__header {
  position: static;
  gap: var(--nvx-space-3);
  align-items: center;
  padding: 0 var(--nvx-space-4) 0 var(--nvx-space-5);
}

.nvx-dialog--sheet .nvx-dialog__close { position: static; }
.nvx-dialog--sheet .nvx-dialog__title { font-size: var(--nvx-font-size-lg); line-height: var(--nvx-line-height-lg); }
.nvx-dialog--sheet .nvx-dialog__description { padding: var(--nvx-space-1) var(--nvx-space-5) 0; }
.nvx-dialog--sheet .nvx-dialog__sidebar { flex: none; padding: var(--nvx-space-2) var(--nvx-space-5) var(--nvx-space-3); }

.nvx-dialog--sheet .nvx-dialog__body {
  flex: 1 1 auto;
  align-content: start;
  min-height: 0;
  padding: var(--nvx-space-4) var(--nvx-space-5);
  overflow: auto;
  overscroll-behavior: contain;
  -webkit-overflow-scrolling: touch;
}

.nvx-dialog--sheet.nvx-dialog--xl .nvx-dialog__body { padding: 0; }

.nvx-dialog--sheet .nvx-dialog__actions {
  flex: none;
  flex-direction: column;
  gap: var(--nvx-space-2);
  padding: var(--nvx-space-3) var(--nvx-space-5) var(--nvx-space-4);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.nvx-dialog--sheet .nvx-dialog__actions:empty { display: none; }

.nvx-dialog--sheet .nvx-dialog__actions > :deep(.nvx-button),
.nvx-dialog--sheet .nvx-dialog__actions > :deep(button) {
  width: 100%;
  min-height: var(--nvx-control-height-touch);
}

@media (prefers-reduced-motion: no-preference) {
  .nvx-dialog--sheet {
    animation: nvx-dialog-sheet-enter var(--nvx-motion-overlay) ease-out;
  }
}

@keyframes nvx-dialog-sheet-enter {
  from { transform: translateY(32px); opacity: 0.6; }
}
</style>
