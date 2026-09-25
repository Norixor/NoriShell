<script setup lang="ts">
import { Columns2, PanelRight, Rows2, X } from "lucide-vue-next";
import { useI18n } from "vue-i18n";

import { NvxIcon, NvxIconButton } from "../ui";
import NvxPluginContributionSlot from "./NvxPluginContributionSlot.vue";

withDefaults(defineProps<{
  canSplitHorizontal: boolean;
  canSplitVertical: boolean;
  canSplitWorkspaceRight: boolean;
  showLayoutActions: boolean;
  pluginContextKey?: string;
}>(), { pluginContextKey: "global-terminal" });

const emit = defineEmits<{
  split: [direction: "horizontal" | "vertical"];
  splitWorkspaceRight: [];
  close: [];
}>();

const { t } = useI18n();
</script>

<template>
  <div class="terminal-pane-controls">
    <NvxPluginContributionSlot
      extension-slot="terminalToolbar"
      toolbar-menu
      :instance-key="pluginContextKey"
    />
    <template v-if="showLayoutActions">
      <div class="terminal-pane-controls__group">
        <NvxIconButton
          size="sm"
          :label="canSplitHorizontal ? t('sshTerminal.splitHorizontal') : t('sshTerminal.splitLimitReached')"
          :disabled="!canSplitHorizontal"
          @click="emit('split', 'horizontal')"
        >
          <NvxIcon
            :icon="Columns2"
            :size="16"
          />
        </NvxIconButton>
        <NvxIconButton
          size="sm"
          :label="canSplitVertical ? t('sshTerminal.splitVertical') : t('sshTerminal.splitLimitReached')"
          :disabled="!canSplitVertical"
          @click="emit('split', 'vertical')"
        >
          <NvxIcon
            :icon="Rows2"
            :size="16"
          />
        </NvxIconButton>
        <NvxIconButton
          size="sm"
          :label="canSplitWorkspaceRight ? t('sshTerminal.splitWorkspaceRight') : t('sshTerminal.splitLimitReached')"
          :disabled="!canSplitWorkspaceRight"
          @click="emit('splitWorkspaceRight')"
        >
          <NvxIcon
            :icon="PanelRight"
            :size="16"
          />
        </NvxIconButton>
      </div>
      <span
        class="terminal-pane-controls__separator"
        aria-hidden="true"
      />
    </template>

    <div
      v-if="$slots.default"
      class="terminal-pane-controls__session"
    >
      <slot />
    </div>

    <template v-if="showLayoutActions">
      <span
        v-if="$slots.default"
        class="terminal-pane-controls__separator"
        aria-hidden="true"
      />
      <NvxIconButton
        class="terminal-pane-controls__close"
        size="sm"
        :label="t('sshTerminal.closePane')"
        @click="emit('close')"
      >
        <NvxIcon
          :icon="X"
          :size="16"
        />
      </NvxIconButton>
    </template>
  </div>
</template>

<style scoped>
.terminal-pane-controls,
.terminal-pane-controls__group,
.terminal-pane-controls__session {
  display: inline-flex;
  flex: none;
  align-items: center;
}

.terminal-pane-controls,
.terminal-pane-controls__group {
  gap: var(--nvx-space-1);
}

.terminal-pane-controls :deep(.nvx-icon-button) {
  color: var(--nvx-color-terminal-muted);
}

.terminal-pane-controls__separator {
  width: var(--nvx-border-width);
  height: 20px;
  flex: none;
  margin: 0 var(--nvx-space-1);
  background: var(--nvx-color-border-strong);
}

.terminal-pane-controls__close:hover:not(:disabled) {
  background: var(--nvx-color-danger);
  color: var(--nvx-color-on-danger);
}
</style>
