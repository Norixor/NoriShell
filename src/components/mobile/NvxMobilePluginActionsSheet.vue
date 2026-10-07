<script setup lang="ts">
import { X } from "lucide-vue-next";
import { nextTick, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { mountMobileSheetBackdrop, pushMobileOverlay, type MobileOverlayHandle, type MobileSheetBackdropHandle } from "../../mobile-overlays";
import { NvxPluginExtensionTarget } from "../plugins";
import { NvxIcon, NvxIconButton } from "../ui";

/**
 * Phone placement of the `app.header.actions` slot. The desktop Workspace Tab
 * strip renders this target inline next to "+"; the phone top bar has no room,
 * so the same extension target lives in this bottom sheet. The target stays
 * mounted while the sheet is closed, exactly as the desktop strip keeps it
 * mounted, so Core's contribution, visibility and onOpen rules are identical and
 * only the presentation differs. The contribution count is reported upward so
 * the shell shows its entry button only when a plugin actually contributes.
 */
const props = defineProps<{ modelValue: boolean }>();
const emit = defineEmits<{ "update:modelValue": [open: boolean]; availability: [count: number] }>();
const { t } = useI18n();
const panel = ref<HTMLElement | null>(null);
let overlay: MobileOverlayHandle | null = null;
let removeBackdrop: MobileSheetBackdropHandle | null = null;
let restoreFocus: HTMLElement | null = null;

function close() {
  emit("update:modelValue", false);
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    event.preventDefault();
    close();
  }
}

watch(() => props.modelValue, (open) => {
  if (open) {
    removeBackdrop ??= mountMobileSheetBackdrop(close);
    // Registered with its layer so a dialog a plugin action opens lands above this sheet.
    overlay ??= pushMobileOverlay(() => { close(); return true; }, () => [removeBackdrop?.element, panel.value]);
    restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void nextTick(() => panel.value?.querySelector<HTMLElement>("[data-sheet-close]")?.focus());
  } else {
    overlay?.release();
    overlay = null;
    removeBackdrop?.();
    removeBackdrop = null;
    if (restoreFocus?.isConnected) restoreFocus.focus();
    restoreFocus = null;
  }
}, { immediate: true });

onBeforeUnmount(() => {
  overlay?.release();
  overlay = null;
  removeBackdrop?.();
  removeBackdrop = null;
});
</script>

<template>
  <!-- Hosted under body so no ancestor containment or transform can trap the fixed sheet beneath the scrim. -->
  <Teleport to="body">
    <section
      v-show="modelValue"
      ref="panel"
      class="nvx-mobile-plugin-actions"
      data-nvx-mobile-sheet
      :role="modelValue ? 'dialog' : undefined"
      :aria-modal="modelValue ? 'true' : undefined"
      :aria-label="t('mobile.pluginActionsTitle')"
      :aria-hidden="!modelValue"
      @keydown="handleKeydown"
    >
      <header
        class="nvx-mobile-plugin-actions__header"
        data-plugin-protected
      >
        <h2>{{ t('mobile.pluginActionsTitle') }}</h2>
        <NvxIconButton
          data-sheet-close
          :label="t('mobile.closeSheet')"
          size="sm"
          @click="close"
        >
          <NvxIcon
            :icon="X"
            :size="16"
          />
        </NvxIconButton>
      </header>
      <div class="nvx-mobile-plugin-actions__body">
        <NvxPluginExtensionTarget
          target-id="app.header.actions"
          :show-identity="true"
          @availability="emit('availability', $event)"
        />
      </div>
    </section>
  </Teleport>
</template>

<style scoped>
.nvx-mobile-plugin-actions__header {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--nvx-space-1) var(--nvx-space-2) var(--nvx-space-3);
}

.nvx-mobile-plugin-actions__header h2 {
  margin: 0;
  font-size: var(--nvx-font-size-lg);
  line-height: var(--nvx-line-height-lg);
}

/* One contribution per row; the target's own grid keeps identity above the document. */
.nvx-mobile-plugin-actions__body {
  display: grid;
  gap: var(--nvx-space-3);
  padding: 0 var(--nvx-space-3) var(--nvx-space-2);
}

.nvx-mobile-plugin-actions__body :deep(.plugin-extension-target__contribution) {
  padding: var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
}
</style>
