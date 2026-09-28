<script setup lang="ts">
import { Keyboard } from "lucide-vue-next";
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxIcon, NvxIconButton } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";
import { desktopSpecialKeys, type DesktopSpecialKey } from "./input";

const props = defineProps<{ disabled: boolean; active: boolean }>();
const emit = defineEmits<{ send: [key: DesktopSpecialKey] }>();
const { t } = useI18n();
const { rootRef, triggerRef, viewportPanelRef, viewportPanelStyle, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();
const trigger = ref<HTMLElement | null>(null);

function toggle(event: MouseEvent) {
  trigger.value = event.currentTarget as HTMLElement;
  toggleMenu();
}

function run(key: DesktopSpecialKey) {
  closeMenu();
  trigger.value?.focus();
  emit("send", key);
}

watch(() => [props.active, props.disabled], () => {
  if (!props.active || props.disabled) closeMenu();
});
</script>

<template>
  <div
    :ref="rootRef"
    class="desktop-special-keys"
  >
    <NvxIconButton
      :ref="triggerRef"
      size="sm"
      :label="t('desktop.specialKeys')"
      :disabled="disabled"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click="toggle"
    >
      <NvxIcon
        :icon="Keyboard"
        :size="16"
      />
    </NvxIconButton>
    <div
      v-if="open"
      :ref="viewportPanelRef"
      class="desktop-special-keys__popover"
      :style="viewportPanelStyle"
      role="menu"
      :aria-label="t('desktop.specialKeys')"
      @keydown="handleMenuKeyDown"
    >
      <NvxButton
        v-for="key in desktopSpecialKeys"
        :key="key"
        variant="ghost"
        size="sm"
        role="menuitem"
        :disabled="disabled"
        @click="run(key)"
      >
        {{ t(`desktop.specialKeyNames.${key}`) }}
      </NvxButton>
    </div>
  </div>
</template>

<style scoped>
.desktop-special-keys { display: flex; flex: none; }
.desktop-special-keys__popover { position: fixed; z-index: var(--nvx-z-popover); display: grid; min-width: 168px; padding: 4px; border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); box-shadow: var(--nvx-shadow-overlay); }
.desktop-special-keys__popover > .nvx-button { justify-content: flex-start; min-height: 28px; padding-inline: var(--nvx-space-2); font-size: var(--nvx-font-size-xs); white-space: nowrap; }
</style>
