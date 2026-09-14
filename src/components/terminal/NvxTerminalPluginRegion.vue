<script setup lang="ts">
import { ChevronDown, ChevronUp } from "lucide-vue-next";
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { NvxPluginExtensionTarget } from "../plugins";
import { NvxIcon, NvxIconButton } from "../ui";

const props = defineProps<{ region: "header" | "sidebar" | "footer"; instanceKey: string; contextLabel: string; available: boolean }>();
const emit = defineEmits<{ availability: [count: number] }>();
const { t } = useI18n();
const count = ref(0);
const hidden = ref(false);
const preferenceKey = computed(() => `norishell.terminalPluginRegion.${props.region}.hidden.v1`);
try { hidden.value = localStorage.getItem(preferenceKey.value) === "true"; } catch { /* Optional UI preference. */ }
function toggle() {
  hidden.value = !hidden.value;
  try { localStorage.setItem(preferenceKey.value, String(hidden.value)); } catch { /* Optional UI preference. */ }
}
function updateAvailability(value: number) { count.value = value; emit("availability", value); }
</script>
<template>
  <section
    v-show="count > 0 && available"
    class="terminal-plugin-region"
    :class="`terminal-plugin-region--${region}`"
    :data-terminal-plugin-region="region"
  >
    <header
      v-if="region !== 'footer'"
      data-plugin-protected
    >
      <span>{{ t(`plugins.tools.regions.${region}`) }}</span><NvxIconButton
        size="sm"
        :label="t(hidden ? 'plugins.tools.showRegion' : 'plugins.tools.hideRegion')"
        :aria-expanded="!hidden"
        @click="toggle"
      >
        <NvxIcon
          :icon="hidden ? ChevronDown : ChevronUp"
          :size="16"
        />
      </NvxIconButton>
    </header>
    <div
      v-show="region === 'footer' || !hidden"
      class="terminal-plugin-region__content"
      tabindex="0"
      :aria-label="t(`plugins.tools.regions.${region}`)"
    >
      <NvxPluginExtensionTarget
        :key="instanceKey"
        :target-id="`terminal.${region}`"
        :instance-key="instanceKey"
        :display-label="contextLabel"
        route-path="/terminal"
        :show-identity="region !== 'footer'"
        :disabled="!available || (region !== 'footer' && hidden)"
        @availability="updateAvailability"
      />
    </div>
  </section>
</template>
<style scoped>
.terminal-plugin-region { min-width: 0; min-height: 0; background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); border-bottom: 1px solid var(--nvx-color-border); }
.terminal-plugin-region > header { display: flex; height: 24px; align-items: center; justify-content: space-between; padding: 0 10px; color: var(--nvx-color-text-tertiary); font-size: var(--nvx-font-size-xs); }
.terminal-plugin-region > header :deep(button) { width: 22px; height: 22px; }
.terminal-plugin-region__content { display: grid; grid-auto-rows: max-content; align-content: start; max-height: min(80px, 10vh); min-width: 0; padding: 4px 10px 8px; gap: 8px; overflow: auto; overscroll-behavior: contain; container: plugin-content / inline-size; }
.terminal-plugin-region--sidebar { width: 100%; min-width: 300px; border-left: 1px solid var(--nvx-color-border); }
.terminal-plugin-region--sidebar .terminal-plugin-region__content { max-height: min(300px, 35vh); }
.terminal-plugin-region__content:focus-visible { outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring); outline-offset: -2px; }
.terminal-plugin-region--footer { flex: none; height: 28px; overflow: hidden; border-top: 1px solid var(--nvx-color-border); border-bottom: 0; }
.terminal-plugin-region--footer .terminal-plugin-region__content { display: flex; align-items: center; height: 27px; max-height: 27px; padding: 0 10px; gap: 16px; overflow-x: auto; overflow-y: hidden; scrollbar-width: none; font-size: var(--nvx-font-size-xs); }
.terminal-plugin-region--footer .terminal-plugin-region__content::-webkit-scrollbar { display: none; }
.terminal-plugin-region--footer :deep(.plugin-extension-target__contribution) { flex: 0 0 auto; max-width: 100%; }
.terminal-plugin-region--footer :deep(.plugin-ui-document),
.terminal-plugin-region--footer :deep(.plugin-ui-node--horizontal) { gap: 8px; }
.terminal-plugin-region--footer :deep(.plugin-ui-node--horizontal) { flex-wrap: nowrap; }
.terminal-plugin-region--footer :deep(.plugin-ui-node__text) { white-space: nowrap; font-size: var(--nvx-font-size-xs); line-height: 20px; }
.terminal-plugin-region--footer :deep(.nvx-button),
.terminal-plugin-region--footer :deep(.nvx-icon-button) { height: 24px; min-height: 24px; padding-block: 0; }
</style>
