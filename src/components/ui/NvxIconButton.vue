<script setup lang="ts">
withDefaults(
  defineProps<{
    label: string;
    title?: string;
    variant?: "ghost" | "outlined";
    size?: "sm" | "md";
    disabled?: boolean;
  }>(),
  {
    title: undefined,
    variant: "ghost",
    size: "md",
    disabled: false,
  },
);

defineEmits<{ click: [event: MouseEvent] }>();
</script>

<template>
  <button
    class="nvx-icon-button"
    :class="[`nvx-icon-button--${variant}`, `nvx-icon-button--${size}`]"
    type="button"
    :aria-label="label"
    :title="title ?? label"
    :disabled="disabled"
    @click="$emit('click', $event)"
  >
    <slot />
  </button>
</template>

<style scoped>
.nvx-icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: var(--nvx-border-width) solid transparent;
  border-radius: var(--nvx-radius-md);
  background: transparent;
  color: var(--nvx-color-text-secondary);
  cursor: pointer;
  transition: background-color var(--nvx-motion-fast), border-color var(--nvx-motion-fast), color var(--nvx-motion-fast);
}

.nvx-icon-button--sm {
  width: var(--nvx-control-height-sm);
  height: var(--nvx-control-height-sm);
}

.nvx-icon-button--md {
  width: var(--nvx-control-height-md);
  height: var(--nvx-control-height-md);
}

.nvx-icon-button--outlined {
  border-color: var(--nvx-color-border-strong);
}

.nvx-icon-button:hover:not(:disabled) {
  background: var(--nvx-color-bg-hover);
  color: var(--nvx-color-text-primary);
}

.nvx-icon-button:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
</style>
