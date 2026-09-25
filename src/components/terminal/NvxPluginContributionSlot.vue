<script setup lang="ts">
import { Blocks, Check, Copy } from "lucide-vue-next";
import { createPinia, getActivePinia } from "pinia";
import { computed, inject, onActivated, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import type { PluginContributionSlot } from "../../core-api/generated/core-api";
import { usePluginsStore, type SafePluginContributionPanel } from "../../stores/plugins";
import { NvxPluginExtensionTarget } from "../plugins";
import { NvxButton, NvxIcon, NvxIconButton, NvxStatusLabel } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import { terminalPluginToolsKey } from "./terminalPluginTools";

const props = withDefaults(defineProps<{
  extensionSlot: PluginContributionSlot;
  menu?: boolean;
  menuHeading?: string;
  toolbarMenu?: boolean;
  instanceKey?: string;
  displayLabel?: string;
}>(), {
  menu: false,
  menuHeading: "",
  toolbarMenu: false,
  instanceKey: "global",
  displayLabel: undefined,
});

const { t } = useI18n();
const plugins = usePluginsStore(getActivePinia() ?? createPinia());
const copyKey = ref<string | null>(null);
const feedback = ref("");
const { rootRef, triggerRef, viewportPanelRef, viewportPanelStyle, open: toolbarMenuOpen, closeMenu: closeToolbarMenu, toggleMenu, handleMenuKeyDown: onMenuKeyDown } = usePopoverMenu({ enabled: props.toolbarMenu });
const genericContributionCount = ref(0);
const terminalPluginTools = inject(terminalPluginToolsKey, null);
const hasTerminalTools = computed(() => props.extensionSlot === "terminalToolbar"
  && Boolean(terminalPluginTools?.available.value));
const panels = computed(() => plugins.contributions.filter((panel) => panel.slot === props.extensionSlot));
const usesMenuItems = computed(() => props.menu || props.toolbarMenu);
const targetId = computed(() => ({
  pluginsPage: "plugins.page",
  terminalSidebar: "terminal.sidebar",
  terminalToolbar: "terminal.toolbar",
  sftpContextMenu: "sftp.contextMenu",
  hostDetailTools: "host.detail.tools",
  overviewCardActions: "overview.card.actions",
  commandPalette: "commandPalette",
} satisfies Record<PluginContributionSlot, string>)[props.extensionSlot]);

function actionKey(panel: SafePluginContributionPanel, actionId: string) {
  return `${panel.pluginId}:${panel.slot}:${panel.instanceGeneration}:${panel.contributionRevision}:${actionId}`;
}

async function refresh() {
  await plugins.loadContributions().catch(() => undefined);
}

async function invoke(panel: SafePluginContributionPanel, actionId: string) {
  const result = await plugins.invokeContribution(panel, actionId);
  feedback.value = result ? "" : t("plugins.toolbar.actionFailed");
}

async function copyValue(panel: SafePluginContributionPanel, copyId: string) {
  const key = `${panel.pluginId}:${panel.slot}:${panel.contributionRevision}:${copyId}`;
  if (copyKey.value !== null) return;
  copyKey.value = key;
  const text = await plugins.copyContribution(panel, copyId);
  if (text === null) {
    feedback.value = t("plugins.toolbar.copyFailed");
  } else {
    try {
      await navigator.clipboard.writeText(text);
      feedback.value = t("plugins.toolbar.copied");
    } catch {
      feedback.value = t("plugins.toolbar.copyFailed");
    }
  }
  window.setTimeout(() => {
    if (copyKey.value === key) copyKey.value = null;
  }, 900);
}

function toggleToolbarMenu() {
  if (hasTerminalTools.value && !panels.value.length && !genericContributionCount.value) {
    terminalPluginTools?.open(props.instanceKey);
    return;
  }
  toggleMenu();
}

onMounted(() => {
  void refresh();
});
onActivated(() => void refresh());
</script>

<template>
  <div
    v-show="panels.length || genericContributionCount || hasTerminalTools"
    :ref="rootRef"
    class="plugin-slot"
    :class="{ 'plugin-slot--menu': menu, 'plugin-slot--toolbar-menu': toolbarMenu }"
    :aria-label="t('plugins.toolbar.label')"
  >
    <p
      v-if="menu && menuHeading"
      class="plugin-slot__menu-heading"
    >
      {{ menuHeading }}
    </p>
    <template v-if="toolbarMenu">
      <NvxIconButton
        :ref="triggerRef"
        size="sm"
        :label="t('plugins.toolbar.open')"
        aria-haspopup="menu"
        :aria-expanded="toolbarMenuOpen"
        @click.stop="toggleToolbarMenu"
      >
        <NvxIcon
          :icon="Blocks"
          :size="16"
        />
      </NvxIconButton>
      <Teleport to="body">
        <div
          v-show="toolbarMenuOpen"
          :ref="viewportPanelRef"
          class="plugin-slot__toolbar-popover"
          :style="viewportPanelStyle"
          role="menu"
          :aria-label="t('plugins.toolbar.label')"
          @keydown="onMenuKeyDown"
        >
          <button
            v-if="hasTerminalTools"
            class="plugin-slot__menu-item"
            type="button"
            role="menuitem"
            @click.stop="terminalPluginTools?.open(instanceKey); closeToolbarMenu()"
          >
            {{ t('plugins.tools.open') }}
          </button>
          <template
            v-for="panel in panels"
            :key="`${panel.pluginId}:${panel.slot}:${panel.instanceGeneration}`"
          >
            <p class="plugin-slot__plugin-name">
              {{ panel.pluginName }}
            </p>
            <template
              v-for="(node, index) in panel.nodes"
              :key="node.kind === 'action' ? node.actionId : node.kind === 'copy' ? node.copyId : index"
            >
              <output
                v-if="node.kind === 'text'"
                class="plugin-slot__text"
                :title="node.text"
              >{{ node.text }}</output>
              <NvxStatusLabel
                v-else-if="node.kind === 'status'"
                :tone="node.tone"
              >
                {{ node.label }}
              </NvxStatusLabel>
              <button
                v-else-if="node.kind === 'action'"
                class="plugin-slot__menu-item"
                type="button"
                role="menuitem"
                :disabled="plugins.invokingActionKey !== null"
                @click.stop="invoke(panel, node.actionId)"
              >
                {{ node.label }}
              </button>
              <button
                v-else
                class="plugin-slot__menu-item"
                type="button"
                role="menuitem"
                :disabled="copyKey !== null || plugins.invokingActionKey !== null"
                @click.stop="copyValue(panel, node.copyId)"
              >
                {{ node.label }}
              </button>
            </template>
          </template>
          <NvxPluginExtensionTarget
            :target-id="targetId"
            :instance-key="instanceKey"
            :display-label="displayLabel"
            @availability="genericContributionCount = $event"
          />
        </div>
      </Teleport>
    </template>
    <button
      v-if="!toolbarMenu && menu && hasTerminalTools"
      class="plugin-slot__menu-item"
      type="button"
      role="menuitem"
      @click.stop="terminalPluginTools?.open(instanceKey)"
    >
      {{ t('plugins.tools.open') }}
    </button>
    <NvxPluginExtensionTarget
      v-if="!toolbarMenu"
      :target-id="targetId"
      :instance-key="instanceKey"
      :display-label="displayLabel"
      @availability="genericContributionCount = $event"
    />
    <template
      v-for="panel in panels"
      v-else
      :key="`${panel.pluginId}:${panel.slot}:${panel.instanceGeneration}`"
    >
      <template
        v-for="(node, index) in panel.nodes"
        :key="node.kind === 'action' ? node.actionId : node.kind === 'copy' ? node.copyId : index"
      >
        <output
          v-if="node.kind === 'text'"
          class="plugin-slot__text"
          :title="node.text"
        >{{ node.text }}</output>
        <NvxStatusLabel
          v-else-if="node.kind === 'status'"
          :tone="node.tone"
        >
          {{ node.label }}
        </NvxStatusLabel>
        <button
          v-else-if="node.kind === 'action' && usesMenuItems"
          class="plugin-slot__menu-item"
          type="button"
          role="menuitem"
          :disabled="plugins.invokingActionKey !== null"
          @click.stop="invoke(panel, node.actionId)"
        >
          {{ node.label }}
        </button>
        <NvxButton
          v-else-if="node.kind === 'action'"
          size="sm"
          variant="ghost"
          :loading="plugins.invokingActionKey === actionKey(panel, node.actionId)"
          :disabled="plugins.invokingActionKey !== null"
          @click.stop="invoke(panel, node.actionId)"
        >
          {{ node.label }}
        </NvxButton>
        <button
          v-else-if="usesMenuItems"
          class="plugin-slot__menu-item"
          type="button"
          role="menuitem"
          :disabled="copyKey !== null || plugins.invokingActionKey !== null"
          @click.stop="copyValue(panel, node.copyId)"
        >
          {{ node.label }}
        </button>
        <NvxIconButton
          v-else
          size="sm"
          :label="node.label"
          :disabled="copyKey !== null || plugins.invokingActionKey !== null"
          @click.stop="copyValue(panel, node.copyId)"
        >
          <NvxIcon
            :icon="copyKey?.endsWith(`:${node.copyId}`) ? Check : Copy"
            :size="16"
          />
        </NvxIconButton>
      </template>
    </template>
    <span
      class="plugin-slot__feedback"
      aria-live="polite"
    >{{ feedback }}</span>
  </div>
</template>

<style scoped>
.plugin-slot { display: inline-flex; min-width: 0; gap: var(--nvx-space-1); align-items: center; }
.plugin-slot--menu { display: grid; width: 100%; gap: 0; }
.plugin-slot__menu-heading { margin: 0; padding: var(--nvx-space-1) var(--nvx-space-3); color: var(--nvx-color-text-tertiary); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-semibold); }
.plugin-slot--toolbar-menu { position: relative; flex: none; }
.plugin-slot__text { max-width: min(28vw, 260px); overflow: hidden; color: var(--nvx-color-terminal-muted); font-family: var(--nvx-font-mono); font-size: var(--nvx-font-size-xs); text-overflow: ellipsis; white-space: nowrap; }
.plugin-slot--menu .plugin-slot__text { max-width: 100%; padding: var(--nvx-space-2) var(--nvx-space-3); }
.plugin-slot__toolbar-popover { max-height: min(480px, calc(100dvh - 16px)); overflow: auto; overscroll-behavior: contain; position: fixed; z-index: var(--nvx-z-popover); display: grid; box-sizing: border-box; min-width: 264px; max-width: min(380px, calc(100vw - var(--nvx-space-6))); gap: var(--nvx-space-1); padding: var(--nvx-space-2); border: var(--nvx-border-width) solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); box-shadow: var(--nvx-shadow-overlay); }
.plugin-slot__plugin-name { margin: 0; padding: var(--nvx-space-1) var(--nvx-space-2); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); font-weight: var(--nvx-font-weight-semibold); }
.plugin-slot__toolbar-popover .plugin-slot__text { max-width: 100%; padding: var(--nvx-space-2); }
.plugin-slot__menu-item { display: flex; width: 100%; min-height: 36px; align-items: center; padding: var(--nvx-space-2) var(--nvx-space-3); border: 0; background: transparent; color: var(--nvx-color-text-primary); font: inherit; text-align: left; }
.plugin-slot__menu-item:hover:not(:disabled), .plugin-slot__menu-item:focus-visible { background: var(--nvx-color-bg-subtle); outline: none; }
.plugin-slot__feedback { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
</style>
