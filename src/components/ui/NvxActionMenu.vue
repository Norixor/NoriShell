<script setup lang="ts">
import { Ellipsis } from "lucide-vue-next";
import type { Component } from "vue";

import NvxIcon from "./NvxIcon.vue";
import NvxIconButton from "./NvxIconButton.vue";
import { usePopoverMenu } from "./usePopoverMenu";

export interface NvxActionMenuItem {
  id: string;
  label: string;
  icon?: Component;
  disabled?: boolean;
  /** Destructive entries are coloured as danger and separated from the rest. */
  danger?: boolean;
}

/**
 * Overflow menu for secondary actions: a single icon trigger that opens a
 * dropdown on desktop and a bottom action sheet on the phone (`usePopoverMenu`
 * handles both). Pages use it where a row of buttons would otherwise wrap, so
 * one primary action stays visible and the rest sit behind "more".
 */
withDefaults(defineProps<{
  label: string;
  items: NvxActionMenuItem[];
  icon?: Component;
  size?: "sm" | "md";
  disabled?: boolean;
}>(), { icon: () => Ellipsis, size: "sm", disabled: false });

const emit = defineEmits<{ select: [id: string] }>();
const { rootRef, triggerRef, viewportPanelRef, viewportPanelStyle, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();

function select(item: NvxActionMenuItem) {
  closeMenu(true);
  emit("select", item.id);
}
</script>

<template>
  <div
    :ref="rootRef"
    class="nvx-action-menu"
  >
    <NvxIconButton
      :ref="triggerRef"
      :size="size"
      :label="label"
      :disabled="disabled"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click="toggleMenu"
    >
      <NvxIcon
        :icon="icon"
        :size="size === 'sm' ? 16 : 20"
      />
    </NvxIconButton>
    <Teleport to="body">
      <div
        v-if="open"
        :ref="viewportPanelRef"
        class="nvx-action-menu__popover"
        :style="viewportPanelStyle"
        role="menu"
        :aria-label="label"
        @keydown="handleMenuKeyDown"
      >
        <template
          v-for="(item, index) in items"
          :key="item.id"
        >
          <div
            v-if="item.danger && index > 0 && !items[index - 1]?.danger"
            class="nvx-action-menu__separator"
            role="separator"
          />
          <button
            class="nvx-action-menu__item"
            :class="{ 'nvx-action-menu__item--danger': item.danger }"
            type="button"
            role="menuitem"
            :disabled="item.disabled"
            @click="select(item)"
          >
            <NvxIcon
              v-if="item.icon"
              :icon="item.icon"
              :size="16"
            />
            <span
              v-else
              aria-hidden="true"
            />
            <span class="nvx-action-menu__label">{{ item.label }}</span>
          </button>
        </template>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.nvx-action-menu {
  position: relative;
  display: inline-flex;
  flex: none;
}

.nvx-action-menu__popover {
  position: fixed;
  z-index: var(--nvx-z-popover);
  display: grid;
  box-sizing: border-box;
  width: min(232px, calc(100vw - 24px));
  max-height: calc(100dvh - 16px);
  overflow-y: auto;
  padding: 2px;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-overlay);
}

.nvx-action-menu__item {
  display: grid;
  grid-template-columns: 20px minmax(0, 1fr);
  gap: var(--nvx-space-1);
  align-items: center;
  min-height: 30px;
  padding: 0 var(--nvx-space-2);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-sm);
  text-align: left;
  cursor: pointer;
}

.nvx-action-menu__item:hover:not(:disabled),
.nvx-action-menu__item:focus-visible {
  outline: none;
  background: var(--nvx-color-bg-hover);
}

.nvx-action-menu__item:disabled {
  color: var(--nvx-color-text-tertiary);
  cursor: not-allowed;
}

.nvx-action-menu__item--danger:not(:disabled) {
  color: var(--nvx-color-danger);
}

.nvx-action-menu__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-action-menu__separator {
  height: 1px;
  margin: 2px var(--nvx-space-1);
  background: var(--nvx-color-border);
}
</style>
