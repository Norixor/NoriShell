<script setup lang="ts">
import { ChevronDown } from "lucide-vue-next";
import { nextTick, onBeforeUnmount, onMounted, ref, useId, watch, type ComponentPublicInstance, type CSSProperties } from "vue";

import { NvxButton, NvxIcon } from "../ui";
import { usePopoverMenu } from "../ui/usePopoverMenu";

const props = defineProps<{ label: string; disabled: boolean }>();
defineSlots<{ default(props: { selectItem: () => void }): unknown }>();

const { rootRef, triggerRef, open, closeMenu, toggleMenu, handleMenuKeyDown } = usePopoverMenu();
const menuId = useId();
const panel = ref<HTMLElement | null>(null);
let trigger: HTMLElement | null = null;
const panelStyle = ref<CSSProperties>({});

function setTrigger(element: Element | ComponentPublicInstance | null) {
  triggerRef(element);
  trigger = element instanceof HTMLElement
    ? element
    : element && "$el" in element ? element.$el as HTMLElement : null;
}

function selectItem() {
  closeMenu();
  // The dialog-opening watch records current focus; return it to the menu trigger first so dialog closure cannot land on a hidden item.
  trigger?.focus();
}

async function positionMenu() {
  await nextTick();
  if (!open.value || !trigger || !panel.value) return;
  const anchor = trigger.getBoundingClientRect();
  const bounds = panel.value.getBoundingClientRect();
  const margin = 8;
  const below = window.innerHeight - anchor.bottom - margin;
  const above = anchor.top - margin;
  const top = below < bounds.height && above > below
    ? Math.max(margin, anchor.top - bounds.height - margin)
    : Math.min(anchor.bottom + margin, Math.max(margin, window.innerHeight - bounds.height - margin));
  panelStyle.value = {
    left: `${Math.max(margin, Math.min(anchor.right - bounds.width, window.innerWidth - bounds.width - margin))}px`,
    top: `${top}px`,
  };
}

function dismissForViewportChange(event: Event) {
  if (event.target instanceof Node && panel.value?.contains(event.target)) return;
  closeMenu();
}

function openFromKeyboard(event: KeyboardEvent) {
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  if (!open.value) toggleMenu();
  if (event.key === "ArrowUp") {
    void nextTick(() => panel.value?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)').item(
      (panel.value?.querySelectorAll('[role="menuitem"]:not(:disabled)').length ?? 1) - 1,
    )?.focus());
  }
}

watch(open, (value) => { if (value) void positionMenu(); });
watch(() => props.disabled, (value) => { if (value) closeMenu(); });
onMounted(() => {
  window.addEventListener("resize", dismissForViewportChange);
  window.addEventListener("scroll", dismissForViewportChange, true);
});
onBeforeUnmount(() => {
  window.removeEventListener("resize", dismissForViewportChange);
  window.removeEventListener("scroll", dismissForViewportChange, true);
});
</script>

<template>
  <div
    :ref="rootRef"
    class="plugin-ui-menu"
  >
    <NvxButton
      :ref="setTrigger"
      variant="ghost"
      size="sm"
      :disabled="disabled"
      aria-haspopup="menu"
      :aria-expanded="open"
      :aria-controls="menuId"
      @click="toggleMenu"
      @keydown="openFromKeyboard"
    >
      {{ label }}
      <NvxIcon
        :icon="ChevronDown"
        :size="16"
      />
    </NvxButton>
    <!-- Keep this subtree mounted: collapsing the menu must not unmount a shared dialog opened from it. -->
    <div
      v-show="open"
      :id="menuId"
      ref="panel"
      class="plugin-ui-menu__panel"
      role="menu"
      :aria-label="label"
      :style="panelStyle"
      @keydown="handleMenuKeyDown"
    >
      <slot :select-item="selectItem" />
    </div>
  </div>
</template>

<style scoped>
.plugin-ui-menu { display: inline-flex; flex: none; min-width: 0; }
.plugin-ui-menu__panel { position: fixed; z-index: var(--nvx-z-popover); display: grid; gap: var(--nvx-space-1); width: max-content; min-width: min(200px, calc(100vw - 16px)); max-width: min(320px, calc(100vw - 16px)); max-height: calc(100dvh - 16px); overflow: auto; padding: var(--nvx-space-1); border: var(--nvx-border-width) solid var(--nvx-color-border-strong); border-radius: var(--nvx-radius-md); background: var(--nvx-color-bg-surface); box-shadow: var(--nvx-shadow-overlay); }
.plugin-ui-menu__panel :deep([role="menuitem"]) { width: 100%; justify-content: flex-start; text-align: start; }
.plugin-ui-menu__panel :deep([role="menuitem"] .nvx-button__content) { justify-content: flex-start; white-space: normal; }
</style>
