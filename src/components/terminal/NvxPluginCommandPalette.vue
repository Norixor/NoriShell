<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { NvxDialog } from "../ui";
import NvxPluginContributionSlot from "./NvxPluginContributionSlot.vue";
import NvxPluginAppIntegrations from "../plugins/NvxPluginAppIntegrations.vue";

const { t } = useI18n();
const open = ref(false);

function handleShortcut(event: KeyboardEvent) {
  if (event.repeat || event.isComposing || event.altKey || event.shiftKey || event.key.toLowerCase() !== "k") return;
  if (document.querySelector('[role="dialog"], dialog[open]')) return;
  const target = event.target;
  if (target instanceof Element && target.closest('input, textarea, select, [contenteditable="true"]')) return;
  const isMac = navigator.platform.toLowerCase().includes("mac");
  const matches = isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!matches) return;
  event.preventDefault();
  open.value = true;
}

onMounted(() => window.addEventListener("keydown", handleShortcut, { capture: true }));
onBeforeUnmount(() => window.removeEventListener("keydown", handleShortcut, { capture: true }));
</script>

<template>
  <NvxDialog
    :model-value="open"
    :title="t('plugins.commandPalette.title')"
    :description="t('plugins.commandPalette.description')"
    :close-label="t('plugins.commandPalette.close')"
    @update:model-value="open = $event"
  >
    <NvxPluginAppIntegrations
      v-if="open"
      commands-only
    />
    <NvxPluginContributionSlot
      extension-slot="commandPalette"
      instance-key="global"
    />
  </NvxDialog>
</template>
