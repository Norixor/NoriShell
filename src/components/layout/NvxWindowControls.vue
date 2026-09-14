<script setup lang="ts">
import { Minus, Square, X } from "lucide-vue-next";
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  performWindowAction,
  physicalWindowsCaptionHitRegion,
  setWindowsMaximizeHitRegion,
} from "../../platform-window";
import { NvxIcon, NvxIconButton } from "../ui";

const { t } = useI18n();
const controlsRoot = ref<HTMLElement | null>(null);
let resizeObserver: ResizeObserver | null = null;
let pendingMeasurement: number | null = null;

async function perform(action: "close" | "minimize" | "maximize") {
  try {
    await performWindowAction(action === "maximize" ? "toggleMaximize" : action);
  } catch {
    console.error(`window.${action}_failed`);
  }
}

async function publishMaximizeHitRegion() {
  pendingMeasurement = null;
  const button = controlsRoot.value?.querySelector<HTMLElement>("[data-windows-maximize]");
  const region = button
    ? physicalWindowsCaptionHitRegion(button.getBoundingClientRect(), window.devicePixelRatio)
    : null;
  try {
    await setWindowsMaximizeHitRegion(region);
  } catch {
    console.error("window.maximize_hit_region_failed");
  }
}

function scheduleMaximizeHitRegion() {
  if (pendingMeasurement !== null) cancelAnimationFrame(pendingMeasurement);
  pendingMeasurement = requestAnimationFrame(() => void publishMaximizeHitRegion());
}

onMounted(() => {
  scheduleMaximizeHitRegion();
  window.addEventListener("resize", scheduleMaximizeHitRegion);
  if (typeof ResizeObserver !== "undefined" && controlsRoot.value) {
    resizeObserver = new ResizeObserver(scheduleMaximizeHitRegion);
    resizeObserver.observe(controlsRoot.value);
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("resize", scheduleMaximizeHitRegion);
  resizeObserver?.disconnect();
  if (pendingMeasurement !== null) cancelAnimationFrame(pendingMeasurement);
  void setWindowsMaximizeHitRegion(null).catch(() => undefined);
});
</script>

<template>
  <div
    ref="controlsRoot"
    class="nvx-window-controls"
    :aria-label="t('window.controls')"
    data-tauri-drag-region="false"
  >
    <NvxIconButton
      class="nvx-window-controls__windows"
      :label="t('window.minimize')"
      @click="perform('minimize')"
    >
      <NvxIcon
        :icon="Minus"
        :size="16"
      />
    </NvxIconButton>
    <NvxIconButton
      class="nvx-window-controls__windows"
      data-windows-maximize
      :label="t('window.maximize')"
      @click="perform('maximize')"
    >
      <NvxIcon
        :icon="Square"
        :size="16"
      />
    </NvxIconButton>
    <NvxIconButton
      class="nvx-window-controls__windows nvx-window-controls__windows--close"
      :label="t('window.close')"
      @click="perform('close')"
    >
      <NvxIcon
        :icon="X"
        :size="16"
      />
    </NvxIconButton>
  </div>
</template>

<style scoped>
.nvx-window-controls {
  display: flex;
  align-items: center;
  align-self: stretch;
}

.nvx-window-controls__windows {
  width: var(--nvx-window-control-windows-button-width);
  height: 100%;
  border-radius: 0;
  color: var(--nvx-color-text-primary);
}

.nvx-window-controls__windows :deep(svg) {
  width: var(--nvx-window-control-windows-icon-size);
  height: var(--nvx-window-control-windows-icon-size);
}

.nvx-window-controls__windows--close:hover {
  background: var(--nvx-window-control-windows-close-hover);
  color: #fff;
}
</style>
