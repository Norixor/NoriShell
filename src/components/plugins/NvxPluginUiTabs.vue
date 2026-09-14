<script setup lang="ts">
import { computed, ref, useId, watch } from "vue";

import { NvxButton } from "../ui";

interface PluginUiTab {
  id: string;
  label: string;
  children: string[];
}

const props = defineProps<{
  label: string;
  tabs: PluginUiTab[];
  disabled?: boolean;
}>();

const selected = ref(props.tabs[0]?.id ?? "");
const tablistId = useId();
const active = computed(() => props.tabs.find((tab) => tab.id === selected.value) ?? props.tabs[0]);

watch(
  () => props.tabs,
  (tabs) => {
    if (!tabs.some((tab) => tab.id === selected.value)) selected.value = tabs[0]?.id ?? "";
  },
  { deep: true },
);

function tabId(tab: PluginUiTab) {
  return `${tablistId}-${tab.id}`;
}

function panelId(tab: PluginUiTab) {
  return `${tabId(tab)}-panel`;
}

function selectTab(tab: PluginUiTab) {
  if (!props.disabled) selected.value = tab.id;
}

function handleTabKeydown(event: KeyboardEvent, index: number) {
  const keys = ["ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown", "Home", "End"];
  if (!keys.includes(event.key) || props.tabs.length === 0) return;
  event.preventDefault();
  const target = event.key === "Home" ? 0
    : event.key === "End" ? props.tabs.length - 1
      : event.key === "ArrowLeft" || event.key === "ArrowUp"
        ? (index - 1 + props.tabs.length) % props.tabs.length
        : (index + 1) % props.tabs.length;
  const tab = props.tabs[target];
  if (tab) selectTab(tab);
  window.requestAnimationFrame(() => {
    document.getElementById(tab ? tabId(tab) : "")?.focus();
  });
}
</script>

<template>
  <section
    class="plugin-ui-tabs"
    :aria-label="label"
  >
    <div
      class="plugin-ui-tabs__list"
      role="tablist"
      aria-orientation="horizontal"
    >
      <NvxButton
        v-for="(tab, index) in tabs"
        :id="tabId(tab)"
        :key="tab.id"
        size="sm"
        :variant="tab.id === active?.id ? 'secondary' : 'ghost'"
        role="tab"
        :aria-controls="panelId(tab)"
        :aria-selected="tab.id === active?.id"
        :tabindex="tab.id === active?.id ? 0 : -1"
        :disabled="disabled"
        @click="selectTab(tab)"
        @keydown="handleTabKeydown($event, index)"
      >
        {{ tab.label }}
      </NvxButton>
    </div>
    <div
      v-if="active"
      :id="panelId(active)"
      class="plugin-ui-tabs__panel"
      role="tabpanel"
      :aria-labelledby="tabId(active)"
    >
      <slot :tab="active" />
    </div>
  </section>
</template>

<style scoped>
.plugin-ui-tabs { display: grid; min-width: 0; gap: var(--nvx-space-3); }
.plugin-ui-tabs__list { display: flex; gap: var(--nvx-space-1); overflow-x: auto; padding-bottom: var(--nvx-space-1); border-bottom: var(--nvx-border-width) solid var(--nvx-color-border); }
.plugin-ui-tabs__panel { display: grid; min-width: 0; gap: var(--nvx-space-3); }
</style>
