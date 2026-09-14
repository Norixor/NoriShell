<script setup lang="ts">
withDefaults(
  defineProps<{
    id?: string;
    modelValue: boolean;
    disabled?: boolean;
  }>(),
  {
    id: undefined,
    disabled: false,
  },
);

defineEmits<{ "update:modelValue": [value: boolean] }>();
</script>

<template>
  <label
    class="nvx-checkbox"
    :class="{ 'nvx-checkbox--disabled': disabled }"
  >
    <input
      :id="id"
      class="nvx-checkbox__control"
      type="checkbox"
      :checked="modelValue"
      :disabled="disabled"
      @change="$emit('update:modelValue', ($event.target as HTMLInputElement).checked)"
    >
    <span class="nvx-checkbox__content">
      <span class="nvx-checkbox__label"><slot /></span>
      <span
        v-if="$slots.hint"
        class="nvx-checkbox__hint"
      ><slot name="hint" /></span>
    </span>
  </label>
</template>

<style scoped>
.nvx-checkbox {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: flex-start;
  padding: var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border-subtle);
  border-radius: var(--nvx-radius-md);
  color: var(--nvx-color-text-primary);
  cursor: pointer;
}

.nvx-checkbox:hover:not(.nvx-checkbox--disabled) {
  border-color: var(--nvx-color-border-strong);
  background: var(--nvx-color-bg-hover);
}

.nvx-checkbox__control {
  width: 18px;
  height: 18px;
  margin: 2px 0 0;
  accent-color: var(--nvx-color-accent);
}

.nvx-checkbox__control:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: var(--nvx-space-1);
}

.nvx-checkbox__content {
  display: grid;
  gap: 2px;
}

.nvx-checkbox__label {
  font-weight: var(--nvx-font-weight-medium);
}

.nvx-checkbox__hint {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

.nvx-checkbox--disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
</style>
