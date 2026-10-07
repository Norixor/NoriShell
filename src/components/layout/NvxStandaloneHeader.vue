<script setup lang="ts">
import { invoke, isTauri } from "@tauri-apps/api/core";
import { ArrowLeft } from "lucide-vue-next";
import { useI18n } from "vue-i18n";
import { detectDesktopPlatform } from "../../platform";
import { isMobileLayout } from "../../platform-host";
import { requestSecureWindowClose } from "../../secure-transport";
import { NvxIcon, NvxIconButton } from "../ui";
import NvxAppHeader from "./NvxAppHeader.vue";
import NvxWindowFrame from "./NvxWindowFrame.vue";
import { secondaryWindowZoom } from "../../secure-window";

const { t } = useI18n();
const platform = detectDesktopPlatform();
// Android surfaces are full-screen Activities, not windows: a phone app bar replaces
// the desktop brand/title chrome. Decided by platform, never by viewport width.
const mobile = isMobileLayout();
let doubleClickStart: { x: number; y: number } | undefined;

function isCaption(event: MouseEvent) {
  return event.button === 0 && event.target instanceof Element
    && !event.target.closest('button, a, input, select, textarea, [data-tauri-drag-region="false"]');
}

function act(action: "drag" | "maximize") {
  if (isTauri()) void invoke("window_standalone_action", { action }).catch(() => undefined);
}

function onMouseDown(event: MouseEvent) {
  if (!isCaption(event)) return;
  // Keep native gestures caller-bound instead of granting cross-window actions.
  event.stopPropagation();
  event.preventDefault();
  doubleClickStart = undefined;
  if (event.detail === 2 && platform === "macos") {
    doubleClickStart = { x: event.clientX, y: event.clientY };
  } else if (event.detail === 2) act("maximize");
  else if (event.detail === 1) act("drag");
}

function onMouseUp(event: MouseEvent) {
  if (!isCaption(event)) return;
  event.stopPropagation();
  if (event.detail === 2 && doubleClickStart?.x === event.clientX && doubleClickStart.y === event.clientY) act("maximize");
  doubleClickStart = undefined;
}
</script>

<template>
  <header
    v-if="mobile"
    class="nvx-standalone-header nvx-standalone-header--mobile"
  >
    <NvxIconButton
      class="nvx-standalone-header__back"
      :label="t('mobile.closeSurface')"
      @click="requestSecureWindowClose"
    >
      <NvxIcon
        :icon="ArrowLeft"
        :size="22"
      />
    </NvxIconButton>
    <div class="nvx-standalone-header__title">
      <slot />
    </div>
  </header>
  <NvxWindowFrame
    v-else
    class="nvx-standalone-header"
    :zoom="secondaryWindowZoom"
  >
    <NvxAppHeader
      :platform="platform"
      standalone
      @mousedown="onMouseDown"
      @mouseup="onMouseUp"
    >
      <template #tabs>
        <div
          class="nvx-standalone-header__title"
          data-tauri-drag-region="deep"
        >
          <slot />
        </div>
      </template>
    </NvxAppHeader>
  </NvxWindowFrame>
</template>

<style scoped>
.nvx-standalone-header {
  flex: none;
  height: var(--nvx-layout-header-height);
  min-height: var(--nvx-layout-header-height);
}
.nvx-standalone-header__title {
  display: flex;
  align-items: center;
  height: 100%;
  min-width: 0;
  padding-inline: var(--nvx-space-4);
  overflow: hidden;
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-semibold);
  user-select: none;
}

/* Phone app bar: 52px like the main window's mobile header (insets are native padding). */
.nvx-standalone-header--mobile {
  display: flex;
  align-items: center;
  gap: var(--nvx-space-1);
  height: 52px;
  min-height: 52px;
  padding-inline: var(--nvx-space-1) var(--nvx-space-3);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
}

.nvx-standalone-header__back {
  flex: none;
  width: 44px;
  height: 44px;
  border-radius: 50%;
}

.nvx-standalone-header--mobile .nvx-standalone-header__title {
  flex: 1;
  padding-inline: var(--nvx-space-1);
  font-size: var(--nvx-font-size-md);
}
</style>
