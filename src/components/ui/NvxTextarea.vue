<script setup lang="ts">
withDefaults(
  defineProps<{
    id?: string;
    modelValue: string;
    placeholder?: string;
    rows?: number;
    disabled?: boolean;
    invalid?: boolean;
    maxlength?: number;
  }>(),
  {
    id: undefined,
    placeholder: undefined,
    rows: 5,
    disabled: false,
    invalid: false,
    maxlength: undefined,
  },
);

defineEmits<{ "update:modelValue": [value: string] }>();
</script>

<template>
  <textarea
    :id="id"
    class="nvx-textarea"
    :class="{ 'nvx-textarea--invalid': invalid }"
    :value="modelValue"
    :placeholder="placeholder"
    :rows="rows"
    :disabled="disabled"
    :aria-invalid="invalid"
    :maxlength="maxlength"
    @input="$emit('update:modelValue', ($event.target as HTMLTextAreaElement).value)"
  />
</template>

<style scoped>
.nvx-textarea {
  width: 100%;
  min-height: 112px;
  padding: var(--nvx-space-3);
  resize: vertical;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font-family: var(--nvx-font-mono);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

.nvx-textarea::placeholder {
  color: var(--nvx-color-text-tertiary);
}

.nvx-textarea:focus-visible {
  border-color: var(--nvx-color-accent);
  outline: none;
  box-shadow: 0 0 0 var(--nvx-focus-ring-width) var(--nvx-color-focus-ring);
}

.nvx-textarea--invalid {
  border-color: var(--nvx-color-danger);
}

.nvx-textarea:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
</style>
