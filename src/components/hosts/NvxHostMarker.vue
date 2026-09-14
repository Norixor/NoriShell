<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { HostMarker } from "../../host-markers";
const props = defineProps<{ marker?: HostMarker | null }>();
const { t } = useI18n();
const label = computed(() => props.marker?.kind === "custom" ? props.marker.label : props.marker ? t(`hostMarkers.${props.marker.kind}`) : "");
</script>
<template>
  <span
    v-if="marker"
    class="nvx-host-marker"
    :class="`nvx-host-marker--${marker.color}`"
    :title="label"
  >
    <span
      class="nvx-host-marker__dot"
      aria-hidden="true"
    />
    <span class="nvx-host-marker__label">{{ label }}</span>
  </span>
</template>
<style scoped>
.nvx-host-marker { display: inline-flex; flex: 0 1 auto; align-items: center; gap: var(--nvx-space-1); min-width: 0; max-width: 10em; padding: 1px var(--nvx-space-1); border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); font-size: var(--nvx-font-size-xs); line-height: 1.4; vertical-align: middle; }
.nvx-host-marker__label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.nvx-host-marker__dot { width: 6px; height: 6px; flex: 0 0 6px; border-radius: 50%; background: var(--marker-color, var(--nvx-color-text-secondary)); }
.nvx-host-marker--red { --marker-color: var(--nvx-color-danger); }
.nvx-host-marker--amber { --marker-color: var(--nvx-color-warning); }
.nvx-host-marker--green { --marker-color: var(--nvx-color-success); }
.nvx-host-marker--blue { --marker-color: var(--nvx-color-accent); }
.nvx-host-marker--purple { --marker-color: var(--nvx-color-terminal-ansi-magenta, var(--nvx-color-accent)); }
</style>
