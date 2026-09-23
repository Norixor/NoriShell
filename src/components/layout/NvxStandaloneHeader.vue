<script setup lang="ts">
import { invoke, isTauri } from "@tauri-apps/api/core";
import { detectDesktopPlatform } from "../../platform";
import NvxAppHeader from "./NvxAppHeader.vue";
import NvxWindowFrame from "./NvxWindowFrame.vue";

const platform = detectDesktopPlatform();
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
  <NvxWindowFrame class="nvx-standalone-header">
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
</style>
