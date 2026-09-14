<script setup lang="ts">
withDefaults(
  defineProps<{
    id?: string;
    modelValue: string;
    placeholder?: string;
    readonly?: boolean;
    disabled?: boolean;
    invalid?: boolean;
    autocomplete?: string;
    maxlength?: number;
    type?: "text" | "password" | "number";
    min?: number;
    max?: number;
    step?: number;
    ariaLabel?: string;
  }>(),
  {
    id: undefined,
    placeholder: undefined,
    readonly: false,
    disabled: false,
    invalid: false,
    autocomplete: "off",
    maxlength: undefined,
    type: "text",
    min: undefined,
    max: undefined,
    step: undefined,
    ariaLabel: undefined,
  },
);

defineEmits<{ "update:modelValue": [value: string] }>();
</script>

<template>
  <input
    :id="id"
    class="nvx-input"
    :class="{ 'nvx-input--invalid': invalid }"
    :type="type"
    :value="modelValue"
    :placeholder="placeholder"
    :readonly="readonly"
    :disabled="disabled"
    :aria-invalid="invalid"
    :autocomplete="autocomplete"
    :maxlength="maxlength"
    :min="min"
    :max="max"
    :step="step"
    :aria-label="ariaLabel"
    @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)"
  >
</template>

<style scoped>
.nvx-input {
  width: 100%;
  height: var(--nvx-control-height-md);
  min-height: var(--nvx-control-height-md);
  box-sizing: border-box;
  padding: 0 var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font: inherit;
  line-height: normal;
  transition: border-color var(--nvx-motion-fast), box-shadow var(--nvx-motion-fast);
}

.nvx-input::placeholder {
  color: var(--nvx-color-text-tertiary);
}

.nvx-input:focus-visible {
  border-color: var(--nvx-color-accent);
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.nvx-input--invalid {
  border-color: var(--nvx-color-danger);
}

.nvx-input:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
</style>
