<script setup lang="ts">
withDefaults(
  defineProps<{
    forId?: string;
    label: string;
    hint?: string;
    error?: string;
  }>(),
  { forId: undefined, hint: undefined, error: undefined },
);
</script>

<template>
  <div class="nvx-field">
    <label
      class="nvx-field__label"
      :for="forId"
    >{{ label }}</label>
    <slot />
    <p
      v-if="error"
      class="nvx-field__error"
    >
      {{ error }}
    </p>
    <p
      v-else-if="hint"
      class="nvx-field__hint"
    >
      {{ hint }}
    </p>
  </div>
</template>

<style scoped>
.nvx-field {
  display: grid;
  gap: var(--nvx-space-2);
}

.nvx-field__label {
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.nvx-field__hint,
.nvx-field__error {
  margin: 0;
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.nvx-field__hint {
  color: var(--nvx-color-text-tertiary);
}

.nvx-field__error {
  color: var(--nvx-color-danger);
}
</style>
