<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { detectDesktopPlatform } from "../../platform";
import { getNativeControlsInset, setNativeHeaderHeight } from "../../platform-window";

const props = withDefaults(defineProps<{ zoom?: number }>(), { zoom: 1 });
const platform = detectDesktopPlatform();
const nativeControlsInset = ref<number | null>(null);
const frame = ref<HTMLElement | null>(null);
let header: HTMLElement | null = null;
let headerObserver: ResizeObserver | null = null;

function syncNativeHeaderHeight() {
  if (!header) return;
  // WebView CSS pixels need the applied zoom, not the display backing scale.
  const height = header.getBoundingClientRect().height * props.zoom;
  if (height > 0) void setNativeHeaderHeight(height).catch(() => {
    // Keep AppKit controls available if the window is closing or not yet ready.
  });
}

watch(() => props.zoom, syncNativeHeaderHeight, { flush: "post" });
const frameStyle = computed(() => {
  if (platform !== "macos" || (nativeControlsInset.value === null && props.zoom === 1)) return undefined;
  const inset = `${(nativeControlsInset.value ?? 80) / props.zoom}px`;
  return {
    "--nvx-window-native-controls-inset": inset,
    "--nvx-layout-navigation-rail-width": inset,
  };
});

onMounted(async () => {
  if (platform !== "macos") return;
  header = frame.value?.querySelector<HTMLElement>(".nvx-app-header") ?? null;
  if (header) {
    headerObserver = new ResizeObserver(syncNativeHeaderHeight);
    headerObserver.observe(header);
    syncNativeHeaderHeight();
  }
  try {
    nativeControlsInset.value = await getNativeControlsInset();
  } catch {
    nativeControlsInset.value = null;
  }
});

onBeforeUnmount(() => {
  headerObserver?.disconnect();
  header = null;
});
</script>

<template>
  <div
    ref="frame"
    class="nvx-window-frame"
    :data-platform="platform"
    :style="frameStyle"
  >
    <slot :platform="platform" />
  </div>
</template>

<style scoped>
.nvx-window-frame {
  height: 100%;
  min-height: 100%;
  overflow: hidden;
  background: var(--nvx-color-bg-canvas);
}

.nvx-window-frame[data-platform="macos"] {
  /* The native-control divider and the rail edge form one vertical frame line. */
  --nvx-layout-navigation-rail-width: var(
    --nvx-window-native-controls-inset,
    var(--nvx-window-control-macos-native-inset-fallback)
  );
}
</style>
