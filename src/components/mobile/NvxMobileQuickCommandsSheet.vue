<script setup lang="ts">
import { useI18n } from "vue-i18n";

import NvxQuickCommandsSidebar from "../terminal/NvxQuickCommandsSidebar.vue";
import { NvxDialog } from "../ui";

/** Quick Commands reuse their sidebar inside a shared dialog on phones; sending still goes through the focused Pane only. */
defineProps<{ modelValue: boolean }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();
const { t } = useI18n();
</script>

<template>
  <NvxDialog
    :model-value="modelValue"
    :title="t('mobile.quickCommandsTitle')"
    :close-label="t('mobile.closeSheet')"
    sheet-size="tall"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="nvx-mobile-quick-commands">
      <NvxQuickCommandsSidebar />
    </div>
  </NvxDialog>
</template>

<style scoped>
.nvx-mobile-quick-commands {
  display: flex;
  height: 100%;
  min-height: 320px;
}

/* Inside the sheet the sidebar is ordinary flow content, not the narrow-window overlay it uses on desktop. */
.nvx-mobile-quick-commands :deep(.quick-commands) {
  position: static;
  z-index: auto;
  flex: 1 1 auto;
  inset: auto;
  width: auto;
  min-height: 0;
  max-width: none;
  border-left: 0;
}

/* The sheet already carries the title; keep only the sidebar's add action from its header. */
.nvx-mobile-quick-commands :deep(.quick-commands__header h2) {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}

.nvx-mobile-quick-commands :deep(.quick-commands__header) {
  justify-content: flex-end;
}
</style>
