<script setup lang="ts">
import {
  Columns2,
  Copy,
  Ellipsis,
  Rows2,
  Search,
  X,
} from "lucide-vue-next";
import type { Component } from "vue";
import { useI18n } from "vue-i18n";

import { NvxPluginExtensionTarget } from "../plugins";
import { NvxIcon, NvxIconButton } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import NvxPluginContributionSlot from "./NvxPluginContributionSlot.vue";

withDefaults(
  defineProps<{
    canSplitHorizontal: boolean;
    canSplitVertical: boolean;
    hasSelection: boolean;
    showLayoutActions: boolean;
    showSessionAction?: boolean;
    sessionActionLabel?: string;
    sessionActionIcon?: Component | null;
    sessionActionDisabled?: boolean;
    sessionActionDanger?: boolean;
    pluginContextKey?: string;
  }>(),
  {
    showSessionAction: false,
    sessionActionLabel: "",
    sessionActionIcon: null,
    sessionActionDisabled: false,
    sessionActionDanger: false,
    pluginContextKey: "global-terminal",
  },
);

const emit = defineEmits<{
  search: [];
  copy: [];
  split: [direction: "horizontal" | "vertical"];
  session: [];
  close: [];
}>();

const { t } = useI18n();
const { rootRef, triggerRef, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();

function run(action: "search" | "copy" | "session" | "close") {
  if (action === "search") emit("search");
  else if (action === "copy") emit("copy");
  else if (action === "session") emit("session");
  else emit("close");
  closeMenu(true);
}

function runSplit(direction: "horizontal" | "vertical") {
  emit("split", direction);
  closeMenu(true);
}

</script>

<template>
  <div
    :ref="rootRef"
    class="terminal-pane-overflow-menu"
  >
    <NvxIconButton
      :ref="triggerRef"
      size="sm"
      :label="t('terminalTools.moreActions')"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click="toggleMenu"
    >
      <NvxIcon
        :icon="Ellipsis"
        :size="20"
      />
    </NvxIconButton>

    <div
      v-if="open"
      class="terminal-pane-overflow-menu__popover"
      role="menu"
      :aria-label="t('terminalTools.moreActions')"
      @keydown="handleMenuKeyDown"
    >
      <button
        class="terminal-pane-overflow-menu__item"
        type="button"
        role="menuitem"
        @click="run('search')"
      >
        <NvxIcon
          :icon="Search"
          :size="16"
        />
        {{ t("terminalTools.search") }}
      </button>
      <button
        class="terminal-pane-overflow-menu__item"
        type="button"
        role="menuitem"
        :disabled="!hasSelection"
        @click="run('copy')"
      >
        <NvxIcon
          :icon="Copy"
          :size="16"
        />
        {{ t("terminalTools.copySelection") }}
      </button>

      <template v-if="showLayoutActions">
        <div
          class="terminal-pane-overflow-menu__separator"
          role="separator"
        />
        <button
          class="terminal-pane-overflow-menu__item"
          type="button"
          role="menuitem"
          :disabled="!canSplitHorizontal"
          @click="runSplit('horizontal')"
        >
          <NvxIcon
            :icon="Columns2"
            :size="16"
          />
          {{ canSplitHorizontal ? t("sshTerminal.splitHorizontal") : t("sshTerminal.splitLimitReached") }}
        </button>
        <button
          class="terminal-pane-overflow-menu__item"
          type="button"
          role="menuitem"
          :disabled="!canSplitVertical"
          @click="runSplit('vertical')"
        >
          <NvxIcon
            :icon="Rows2"
            :size="16"
          />
          {{ canSplitVertical ? t("sshTerminal.splitVertical") : t("sshTerminal.splitLimitReached") }}
        </button>
      </template>

      <NvxPluginContributionSlot
        class="terminal-pane-overflow-menu__plugins"
        extension-slot="terminalToolbar"
        menu
        :menu-heading="t('plugins.toolbar.label')"
        :instance-key="pluginContextKey"
      />

      <NvxPluginExtensionTarget
        target-id="terminal.contextMenu"
        :instance-key="pluginContextKey"
        :display-label="t('terminalTools.moreActions')"
      />

      <template v-if="showSessionAction">
        <div
          class="terminal-pane-overflow-menu__separator"
          role="separator"
        />
        <button
          class="terminal-pane-overflow-menu__item"
          :class="{ 'terminal-pane-overflow-menu__item--danger': sessionActionDanger }"
          type="button"
          role="menuitem"
          :disabled="sessionActionDisabled"
          @click="run('session')"
        >
          <NvxIcon
            v-if="sessionActionIcon"
            :icon="sessionActionIcon"
            :size="16"
          />
          {{ sessionActionLabel }}
        </button>
      </template>

      <template v-if="showLayoutActions">
        <div
          class="terminal-pane-overflow-menu__separator"
          role="separator"
        />
        <button
          class="terminal-pane-overflow-menu__item terminal-pane-overflow-menu__item--danger"
          type="button"
          role="menuitem"
          @click="run('close')"
        >
          <NvxIcon
            :icon="X"
            :size="16"
          />
          {{ t("sshTerminal.closePane") }}
        </button>
      </template>
    </div>
  </div>
</template>

<style scoped>
.terminal-pane-overflow-menu {
  position: relative;
  display: inline-flex;
  flex: none;
}

.terminal-pane-overflow-menu :deep(.nvx-icon-button) {
  color: var(--nvx-color-terminal-muted);
}

.terminal-pane-overflow-menu__popover {
  position: absolute;
  z-index: var(--nvx-z-popover);
  top: calc(100% + var(--nvx-space-1));
  right: 0;
  display: grid;
  width: min(208px, calc(100vw - 24px));
  padding: 2px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.terminal-pane-overflow-menu__item {
  display: grid;
  grid-template-columns: 20px minmax(0, 1fr);
  min-height: 28px;
  align-items: center;
  gap: var(--nvx-space-1);
  padding: 0 var(--nvx-space-2);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-xs);
  text-align: left;
  cursor: pointer;
}

.terminal-pane-overflow-menu__item:hover:not(:disabled),
.terminal-pane-overflow-menu__item:focus-visible {
  outline: none;
  background: var(--nvx-color-bg-hover);
}

.terminal-pane-overflow-menu__item--danger {
  color: var(--nvx-color-danger);
}

.terminal-pane-overflow-menu__item:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.terminal-pane-overflow-menu__separator {
  height: var(--nvx-border-width);
  margin: 2px var(--nvx-space-2);
  background: var(--nvx-color-border);
}

.terminal-pane-overflow-menu__plugins:deep(.plugin-slot--menu) {
  margin: 2px 0;
  padding: 2px 0;
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.terminal-pane-overflow-menu__plugins:deep(.plugin-slot__menu-item) {
  min-height: 28px;
  border-radius: var(--nvx-radius-sm);
  font-size: var(--nvx-font-size-xs);
}
</style>
