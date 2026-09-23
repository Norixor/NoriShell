<script setup lang="ts">
import brandMarkUrl from "../../assets/norishell-mark.svg?no-inline";
import type { DesktopPlatform } from "../../platform";
import NvxWindowControls from "./NvxWindowControls.vue";

withDefaults(defineProps<{ platform: DesktopPlatform; standalone?: boolean }>(), { standalone: false });
</script>

<template>
  <header
    class="nvx-app-header"
    :class="`nvx-app-header--${platform}`"
    data-tauri-drag-region="deep"
  >
    <div
      v-if="platform === 'macos'"
      class="nvx-app-header__native-controls-inset"
      aria-hidden="true"
    />
    <div
      class="nvx-app-header__brand"
      data-tauri-drag-region="deep"
    >
      <img
        class="nvx-app-header__brand-mark"
        :src="brandMarkUrl"
        width="28"
        height="28"
        alt=""
        aria-hidden="true"
        draggable="false"
      >
      <strong>NoriShell</strong>
    </div>
    <div
      id="nvx-terminal-tabs-host"
      class="nvx-app-header__tabs-host"
      data-tauri-drag-region="deep"
    >
      <slot name="tabs" />
    </div>
    <NvxWindowControls
      v-if="platform === 'windows'"
      :standalone="standalone"
    />
  </header>
</template>

<style scoped>
.nvx-app-header {
  position: sticky;
  z-index: var(--nvx-z-sticky);
  top: 0;
  display: grid;
  grid-template-columns: auto auto minmax(0, 1fr);
  align-items: center;
  height: var(--nvx-layout-header-height);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.nvx-app-header--windows {
  grid-template-columns: auto minmax(0, 1fr) auto;
}

.nvx-app-header--other {
  grid-template-columns: auto minmax(0, 1fr) auto;
}

.nvx-app-header__native-controls-inset {
  align-self: stretch;
  width: var(
    --nvx-window-native-controls-inset,
    var(--nvx-window-control-macos-native-inset-fallback)
  );
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
  pointer-events: none;
}

.nvx-app-header__brand {
  display: flex;
  align-items: center;
}

.nvx-app-header__brand {
  gap: var(--nvx-space-2);
  width: max-content;
  min-width: 0;
  padding: 0 var(--nvx-space-5);
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
  font-size: var(--nvx-font-size-md);
  user-select: none;
  -webkit-user-select: none;
}

.nvx-app-header__brand-mark {
  display: block;
  flex: 0 0 auto;
  width: 28px;
  height: 28px;
  pointer-events: none;
  -webkit-user-drag: none;
}

.nvx-app-header__brand strong {
  pointer-events: none;
}

.nvx-app-header--macos .nvx-app-header__brand {
  padding-inline: var(--nvx-window-control-macos-brand-gap);
}

.nvx-app-header__tabs-host {
  align-self: stretch;
  min-width: 0;
}

.nvx-app-header__tabs-host :deep(.nvx-terminal-tab-bar) {
  height: 100%;
  min-height: 0;
  border-bottom: 0;
}

@media (max-width: 1180px) {
  .nvx-app-header__brand strong {
    display: none;
  }

  .nvx-app-header__brand {
    min-width: 0;
    padding-right: var(--nvx-space-3);
    padding-left: var(--nvx-space-3);
  }
}
</style>
