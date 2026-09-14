<script setup lang="ts">
import { computed } from "vue";

import { NvxCodeEditor, NvxField } from "../ui";

const props = defineProps<{
  fieldId: string;
  label: string;
  value: string;
  language: string;
  readOnly: boolean;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  field: [id: string, value: string];
}>();

const filename = computed(() => `plugin.${props.language || "txt"}`);

function update(value: string) {
  if (props.readOnly || props.disabled || value.length > 16_384 || knownSecret(value)) return;
  emit("field", props.fieldId, value);
}

function knownSecret(value: string) {
  return /(password=|api_key=|secret=|authorization:\s*bearer|-----begin private key)/i.test(value);
}
</script>

<template>
  <NvxField
    class="plugin-ui-editor"
    :label="label"
  >
    <div class="plugin-ui-editor__surface">
      <NvxCodeEditor
        :model-value="value"
        :filename="filename"
        :readonly="readOnly || disabled"
        :label="label"
        @update:model-value="update"
      />
    </div>
  </NvxField>
</template>

<style scoped>
.plugin-ui-editor__surface { height: 13rem; overflow: hidden; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-md); }
</style>
