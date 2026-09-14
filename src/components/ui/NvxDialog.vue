<script setup lang="ts">
import { X } from "lucide-vue-next";
import { nextTick, onBeforeUnmount, ref, watch } from "vue";

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
    pluginProtected?: boolean;
    themeProtected?: boolean;
  }>(),
  {
    description: undefined,
    dismissible: true,
    size: "md",
    pluginProtected: false,
    themeProtected: undefined,
  },
);

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  close: [];
}>();

defineSlots<{
  default(): unknown;
  actions(): unknown;
}>();

dialogSequence += 1;
const titleId = `nvx-dialog-title-${dialogSequence}`;
const descriptionId = `nvx-dialog-description-${dialogSequence}`;
const panel = ref<HTMLElement | null>(null);
let restoreTarget: HTMLElement | null = null;

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
  (initial ?? focusableElements()[0] ?? panel.value)?.focus();
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
    if (open) {
      if (!wasOpen) restoreTarget = document.activeElement as HTMLElement | null;
      void focusDialog();
    } else if (wasOpen) {
      restoreFocus();
    }
  },
  { immediate: true },
);

onBeforeUnmount(restoreFocus);
</script>

<template>
  <Teleport to="body">
    <div
      v-if="modelValue"
      class="nvx-dialog__backdrop"
      :data-plugin-protected="pluginProtected ? '' : undefined"
      :data-theme-protected="(themeProtected ?? pluginProtected) ? '' : undefined"
      @mousedown.self="requestClose"
    >
      <section
        ref="panel"
        class="nvx-dialog"
        :class="`nvx-dialog--${size}`"
        role="dialog"
        aria-modal="true"
        :aria-labelledby="titleId"
        :aria-describedby="description ? descriptionId : undefined"
        tabindex="-1"
        @keydown="handleKeydown"
      >
        <header class="nvx-dialog__header">
          <h2
            :id="titleId"
            class="nvx-dialog__title"
          >
            {{ title }}
          </h2>
          <NvxIconButton
            v-if="dismissible"
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
  z-index: var(--nvx-z-dialog);
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
</style>
