<script setup lang="ts">
import { computed, onMounted, ref } from "vue";

import { detectDesktopPlatform } from "../../platform";
import { getNativeControlsInset } from "../../platform-window";

const props = withDefaults(defineProps<{ zoom?: number }>(), { zoom: 1 });
const platform = detectDesktopPlatform();
const nativeControlsInset = ref<number | null>(null);
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
  try {
    nativeControlsInset.value = await getNativeControlsInset();
  } catch {
    nativeControlsInset.value = null;
  }
});
</script>

<template>
  <div
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
