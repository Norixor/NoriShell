<script setup lang="ts">
import { Ellipsis, History, Info, Power, Settings, ShieldQuestion, Trash2 } from "lucide-vue-next";
import { useI18n } from "vue-i18n";

import type { PluginInstallState } from "../../core-api/generated/core-api";
import { NvxButton, NvxIcon, NvxIconButton } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";

defineProps<{
  state: PluginInstallState;
  disabled?: boolean;
  hasSettings?: boolean;
}>();

const emit = defineEmits<{
  details: [];
  settings: [];
  permissions: [];
  operationPermissions: [];
  enable: [];
  disable: [];
  uninstall: [];
}>();

const { t } = useI18n();
const { rootRef, triggerRef, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();

function run(action: "details" | "settings" | "permissions" | "operationPermissions" | "uninstall") {
  if (action === "details") emit("details");
  else if (action === "settings") emit("settings");
  else if (action === "permissions") emit("permissions");
  else if (action === "operationPermissions") emit("operationPermissions");
  else emit("uninstall");
  closeMenu(true);
}
</script>

<template>
  <div class="plugin-manage-actions">
    <NvxButton
      size="sm"
      variant="ghost"
      :disabled="disabled"
      @click="state === 'enabled' ? $emit('disable') : $emit('enable')"
    >
      <NvxIcon
        :icon="Power"
        :size="16"
        aria-hidden="true"
      />{{ t(state === "enabled" ? "plugins.disable" : "plugins.enable") }}
    </NvxButton>
    <div
      :ref="rootRef"
      class="plugin-manage-actions__menu"
    >
      <NvxIconButton
        :ref="triggerRef"
        size="sm"
        :label="t('plugins.moreActions')"
        aria-haspopup="menu"
        :aria-expanded="open"
        :disabled="disabled"
        @click="toggleMenu"
      >
        <NvxIcon
          :icon="Ellipsis"
          :size="20"
          aria-hidden="true"
        />
      </NvxIconButton>
      <div
        v-if="open"
        class="plugin-manage-actions__popover"
        role="menu"
        :aria-label="t('plugins.moreActions')"
        @keydown="handleMenuKeyDown"
      >
        <button
          v-if="hasSettings"
          class="plugin-manage-actions__item"
          type="button"
          role="menuitem"
          @click="run('settings')"
        >
          <NvxIcon
            :icon="Settings"
            :size="16"
            aria-hidden="true"
          />{{ t("plugins.settings.open") }}
        </button>
        <button
          class="plugin-manage-actions__item"
          type="button"
          role="menuitem"
          @click="run('details')"
        >
          <NvxIcon
            :icon="Info"
            :size="16"
            aria-hidden="true"
          />{{ t("plugins.details") }}
        </button>
        <button
          class="plugin-manage-actions__item"
          type="button"
          role="menuitem"
          @click="run('permissions')"
        >
          <NvxIcon
            :icon="ShieldQuestion"
            :size="16"
            aria-hidden="true"
          />{{ t("plugins.managePermissions") }}
        </button>
        <button
          class="plugin-manage-actions__item"
          type="button"
          role="menuitem"
          @click="run('operationPermissions')"
        >
          <NvxIcon
            :icon="History"
            :size="16"
            aria-hidden="true"
          />{{ t("plugins.approvalPolicy.management.title") }}
        </button>
        <div
          class="plugin-manage-actions__separator"
          role="separator"
        />
        <button
          class="plugin-manage-actions__item plugin-manage-actions__item--danger"
          type="button"
          role="menuitem"
          @click="run('uninstall')"
        >
          <NvxIcon
            :icon="Trash2"
            :size="16"
            aria-hidden="true"
          />{{ t("plugins.uninstall") }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.plugin-manage-actions,
.plugin-manage-actions__menu {
  position: relative;
  display: inline-flex;
  flex: none;
  align-items: center;
}

.plugin-manage-actions {
  gap: 2px;
}

.plugin-manage-actions__popover {
  position: absolute;
  z-index: var(--nvx-z-popover);
  top: calc(100% + var(--nvx-space-1));
  right: 0;
  display: grid;
  width: min(220px, calc(100vw - 24px));
  padding: 2px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.plugin-manage-actions__item {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
  width: 100%;
  min-height: 34px;
  padding: 0 var(--nvx-space-3);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: start;
  cursor: pointer;
}

.plugin-manage-actions__item:hover,
.plugin-manage-actions__item:focus-visible {
  background: var(--nvx-color-bg-hover);
  outline: none;
}

.plugin-manage-actions__item--danger {
  color: var(--nvx-color-danger);
}

.plugin-manage-actions__separator {
  height: 1px;
  margin: 2px var(--nvx-space-2);
  background: var(--nvx-color-border-subtle);
}
</style>
