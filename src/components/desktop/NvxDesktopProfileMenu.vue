<script setup lang="ts">
import { Ellipsis, Pencil, Trash2 } from "lucide-vue-next";
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { NvxButton, NvxIcon, NvxIconButton } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";

const props = defineProps<{ disabled: boolean; active: boolean }>();
const emit = defineEmits<{ edit: []; delete: [] }>();
const { t } = useI18n();
const { rootRef, triggerRef, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();
const position = ref({ top: "0px", left: "0px" });
let triggerElement: HTMLElement | null = null;

function toggle(event: MouseEvent) {
  triggerElement = event.currentTarget as HTMLElement;
  const rect = triggerElement.getBoundingClientRect();
  position.value = {
    top: `${Math.max(8, Math.min(rect.bottom + 4, window.innerHeight - 84))}px`,
    left: `${Math.max(8, Math.min(rect.right - 144, window.innerWidth - 152))}px`,
  };
  toggleMenu();
}

function run(action: "edit" | "delete") {
  closeMenu();
  triggerElement?.focus();
  if (action === "edit") emit("edit");
  else emit("delete");
}

function dismiss() { closeMenu(); }
watch(() => [props.active, props.disabled], () => {
  if (!props.active || props.disabled) closeMenu();
});
onMounted(() => {
  window.addEventListener("resize", dismiss);
  window.addEventListener("scroll", dismiss, true);
});
onBeforeUnmount(() => {
  window.removeEventListener("resize", dismiss);
  window.removeEventListener("scroll", dismiss, true);
});
</script>

<template>
  <div
    :ref="rootRef"
    class="desktop-profile-menu"
  >
    <NvxIconButton
      :ref="triggerRef"
      size="sm"
      :label="t('desktop.moreActions')"
      :disabled="disabled"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click="toggle"
    >
      <NvxIcon
        :icon="Ellipsis"
        :size="16"
      />
    </NvxIconButton>
    <div
      v-if="open"
      class="desktop-profile-menu__popover"
      :style="position"
      role="menu"
      :aria-label="t('desktop.moreActions')"
      @keydown="handleMenuKeyDown"
    >
      <NvxButton
        variant="ghost"
        size="sm"
        role="menuitem"
        :disabled="disabled"
        @click="run('edit')"
      >
        <NvxIcon
          :icon="Pencil"
          :size="16"
        />{{ t('desktop.edit') }}
      </NvxButton>
      <NvxButton
        class="desktop-profile-menu__delete"
        variant="ghost"
        size="sm"
        role="menuitem"
        :disabled="disabled"
        @click="run('delete')"
      >
        <NvxIcon
          :icon="Trash2"
          :size="16"
        />{{ t('desktop.delete') }}
      </NvxButton>
    </div>
  </div>
</template>

<style scoped>
.desktop-profile-menu { display: flex; flex: none; }
.desktop-profile-menu > .nvx-icon-button { width: 26px; height: 26px; }
.desktop-profile-menu__popover { position: fixed; z-index: var(--nvx-z-popover); display: grid; width: 144px; padding: 4px; border: 1px solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); box-shadow: var(--nvx-shadow-overlay); }
.desktop-profile-menu__popover > .nvx-button { justify-content: flex-start; min-height: 28px; padding-inline: var(--nvx-space-2); font-size: var(--nvx-font-size-xs); }
.desktop-profile-menu__delete { color: var(--nvx-color-danger); }
</style>
