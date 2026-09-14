<script setup lang="ts">
import { LoaderCircle } from "lucide-vue-next";

import NvxIcon from "./NvxIcon.vue";

withDefaults(
  defineProps<{
    tone?: "neutral" | "danger";
    loading?: boolean;
    disabled?: boolean;
  }>(),
  {
    tone: "neutral",
    loading: false,
    disabled: false,
  },
);

defineEmits<{ click: [event: MouseEvent] }>();
</script>

<template>
  <button
    type="button"
    class="nvx-text-action"
    :class="[
      `nvx-text-action--${tone}`,
      { 'nvx-text-action--loading': loading },
    ]"
    :disabled="disabled || loading"
    :aria-busy="loading"
    @click="$emit('click', $event)"
  >
    <span :class="{ 'nvx-text-action__content--hidden': loading }">
      <slot />
    </span>
    <span
      v-if="loading"
      class="nvx-text-action__loader"
      aria-hidden="true"
    >
      <NvxIcon
        :icon="LoaderCircle"
        :size="16"
      />
    </span>
  </button>
</template>

<style scoped>
.nvx-text-action {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin: 0;
  padding: 0;
  border: 0;
  background: transparent;
  font: inherit;
  font-weight: var(--nvx-font-weight-medium);
  line-height: inherit;
  cursor: pointer;
  transition: color var(--nvx-motion-fast), opacity var(--nvx-motion-fast);
}

.nvx-text-action--neutral {
  color: var(--nvx-color-text-secondary);
}

.nvx-text-action--neutral:hover:not(:disabled) {
  color: var(--nvx-color-text-primary);
}

.nvx-text-action--danger {
  color: var(--nvx-color-danger);
}

.nvx-text-action:hover:not(:disabled) {
  text-decoration: underline;
  text-underline-offset: 0.18em;
}

.nvx-text-action:active:not(:disabled) {
  opacity: 0.72;
}

.nvx-text-action:focus-visible {
  border-radius: var(--nvx-radius-sm);
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 3px;
}

.nvx-text-action:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.nvx-text-action__content--hidden {
  visibility: hidden;
}

.nvx-text-action__loader {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  animation: nvx-spin 0.8s linear infinite;
}

@keyframes nvx-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .nvx-text-action__loader {
    animation: none;
  }
}
</style>
