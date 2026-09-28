<script setup lang="ts">
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    values: readonly number[];
    tone?: "accent" | "muted";
    max?: number;
  }>(),
  { tone: "accent", max: 100 },
);

const VIEW_WIDTH = 100;
const VIEW_HEIGHT = 24;

const points = computed(() => {
  const first = props.values[0];
  // A single reading still draws a level line so the cell never collapses while history builds up.
  const values = props.values.length === 1 && first !== undefined ? [first, first] : props.values;
  if (values.length < 2) return "";
  const step = VIEW_WIDTH / (values.length - 1);
  return values
    .map((value, index) => {
      const ratio = Math.min(1, Math.max(0, value / props.max));
      return `${(index * step).toFixed(2)},${(VIEW_HEIGHT - 1 - ratio * (VIEW_HEIGHT - 2)).toFixed(2)}`;
    })
    .join(" ");
});
const area = computed(() => (points.value ? `0,${VIEW_HEIGHT} ${points.value} ${VIEW_WIDTH},${VIEW_HEIGHT}` : ""));
</script>

<template>
  <svg
    class="nvx-sparkline"
    :class="`nvx-sparkline--${tone}`"
    :viewBox="`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`"
    preserveAspectRatio="none"
    aria-hidden="true"
    focusable="false"
  >
    <polygon
      v-if="area && tone === 'accent'"
      class="nvx-sparkline__area"
      :points="area"
    />
    <polyline
      v-if="points"
      class="nvx-sparkline__line"
      :points="points"
    />
  </svg>
</template>

<style scoped>
.nvx-sparkline {
  display: block;
  width: 100%;
  height: 22px;
  overflow: visible;
  color: var(--nvx-color-accent);
}

.nvx-sparkline--muted {
  color: var(--nvx-color-text-tertiary);
}

.nvx-sparkline__area {
  fill: currentColor;
  opacity: 0.1;
}

.nvx-sparkline__line {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
  vector-effect: non-scaling-stroke;
}

.nvx-sparkline--muted .nvx-sparkline__line {
  stroke-dasharray: 3 3;
}
</style>
