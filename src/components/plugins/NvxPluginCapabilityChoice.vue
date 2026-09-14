<script setup lang="ts">
import { NvxCheckbox } from "../ui";

const props = withDefaults(defineProps<{
  modelValue: boolean;
  disabled?: boolean;
  requiresApproval?: boolean;
}>(), { disabled: false, requiresApproval: false });

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  requestApproval: [];
}>();

function handleClick(event: MouseEvent) {
  if (!props.requiresApproval) return;
  // Cancel the native toggle until the protected window returns a decision.
  event.preventDefault();
  event.stopPropagation();
  if (!props.disabled) emit("requestApproval");
}
</script>

<template>
  <NvxCheckbox
    :model-value="modelValue"
    :disabled="disabled"
    :class="{ 'plugin-capability-choice--protected': requiresApproval && !modelValue }"
    @click.capture="handleClick"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <slot />
    <template #hint>
      <slot name="hint" />
    </template>
  </NvxCheckbox>
</template>

<style scoped>
.plugin-capability-choice--protected {
  color: var(--nvx-color-text-secondary);
  background: var(--nvx-color-bg-hover);
}
</style>
