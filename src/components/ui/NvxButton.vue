<script setup lang="ts">
import { LoaderCircle } from "lucide-vue-next";

import NvxIcon from "./NvxIcon.vue";

withDefaults(
  defineProps<{
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "sm" | "md";
    loading?: boolean;
    loadingLabel?: string;
    disabled?: boolean;
    type?: "button" | "submit" | "reset";
  }>(),
  {
    variant: "primary",
    size: "md",
    loading: false,
    loadingLabel: undefined,
    disabled: false,
    type: "button",
  },
);

defineEmits<{ click: [event: MouseEvent] }>();
</script>

<template>
  <button
    class="nvx-button"
    :class="[`nvx-button--${variant}`, `nvx-button--${size}`]"
    :type="type"
    :disabled="disabled || loading"
    :aria-busy="loading"
    :aria-label="loading && loadingLabel ? loadingLabel : undefined"
    @click="$emit('click', $event)"
  >
    <span
      class="nvx-button__content"
      :class="{ 'nvx-button__content--hidden': loading }"
    >
      <slot />
    </span>
    <span
      v-if="loading"
      class="nvx-button__loader"
      aria-hidden="true"
    >
      <NvxIcon
        :icon="LoaderCircle"
        :size="16"
      />
      <span v-if="loadingLabel">{{ loadingLabel }}</span>
    </span>
  </button>
</template>

<style scoped>
.nvx-button {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: var(--nvx-border-width) solid transparent;
  border-radius: var(--nvx-radius-md);
  font-weight: var(--nvx-font-weight-medium);
  cursor: pointer;
  transition: background-color var(--nvx-motion-fast), border-color var(--nvx-motion-fast), color var(--nvx-motion-fast);
}

.nvx-button--sm {
  min-height: var(--nvx-control-height-sm);
  padding: 0 var(--nvx-space-3);
  font-size: var(--nvx-font-size-sm);
}

.nvx-button--md {
  min-height: var(--nvx-control-height-md);
  padding: 0 var(--nvx-space-4);
}

.nvx-button--primary {
  background: var(--nvx-color-accent);
  color: var(--nvx-color-on-accent);
}

.nvx-button--primary:hover:not(:disabled) {
  background: var(--nvx-color-accent-hover);
}

.nvx-button--secondary {
  border-color: var(--nvx-color-border-strong);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
}

.nvx-button--secondary:hover:not(:disabled),
.nvx-button--ghost:hover:not(:disabled) {
  background: var(--nvx-color-bg-hover);
}

.nvx-button--ghost {
  background: transparent;
}

.nvx-button--danger {
  background: var(--nvx-color-danger);
  color: var(--nvx-color-on-danger);
}

.nvx-button:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.nvx-button__content--hidden {
  visibility: hidden;
}

.nvx-button__content {
  display: inline-flex;
  gap: var(--nvx-space-2);
  align-items: center;
  justify-content: center;
  white-space: nowrap;
}

.nvx-button__loader {
  position: absolute;
  inset: 0;
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
  justify-content: center;
  padding-inline: var(--nvx-space-3);
  white-space: nowrap;
}

.nvx-button__loader :deep(svg) {
  animation: nvx-spin 0.8s linear infinite;
}

@keyframes nvx-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
