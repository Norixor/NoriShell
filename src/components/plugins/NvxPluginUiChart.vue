<script setup lang="ts">
import { computed } from "vue";

interface PluginUiChartSeries {
  label: string;
  values: number[];
}

const props = defineProps<{
  label: string;
  chartKind: "line" | "bar";
  labels: string[];
  series: PluginUiChartSeries[];
}>();

const width = 360;
const height = 160;
const padding = 22;
const palette = [
  "var(--nvx-color-accent)",
  "var(--nvx-color-success)",
  "var(--nvx-color-warning)",
  "var(--nvx-color-danger)",
];
const plotWidth = width - padding * 2;
const plotHeight = height - padding * 2;
const extent = computed(() => {
  const values = props.series.flatMap((series) => series.values);
  return { min: Math.min(0, ...values), max: Math.max(1, ...values) };
});
const zeroY = computed(() => valueY(0));

function valueY(value: number) {
  const { min, max } = extent.value;
  return height - padding - ((value - min) * plotHeight) / (max - min);
}

function xFor(index: number, count: number) {
  return padding + (index * plotWidth) / Math.max(1, count - 1);
}

function points(values: number[]) {
  return values.map((value, index) => `${xFor(index, values.length)},${valueY(value)}`).join(" ");
}

function barX(index: number, seriesIndex: number) {
  const band = plotWidth / props.labels.length;
  const groupWidth = Math.min(band * 0.8, 18 * props.series.length);
  const itemWidth = groupWidth / props.series.length;
  return padding + index * band + (band - groupWidth) / 2 + seriesIndex * itemWidth;
}

function barWidth() {
  const band = plotWidth / props.labels.length;
  return Math.max(1, Math.min((band * 0.8) / props.series.length - 1, 18));
}

function barY(value: number) {
  return Math.min(valueY(value), zeroY.value);
}

function barHeight(value: number) {
  return Math.abs(valueY(value) - zeroY.value);
}
</script>

<template>
  <figure class="plugin-ui-chart">
    <figcaption>{{ label }}</figcaption>
    <svg
      :viewBox="`0 0 ${width} ${height}`"
      role="img"
      :aria-label="label"
    >
      <line
        :x1="padding"
        :x2="padding"
        :y1="padding"
        :y2="height - padding"
        class="plugin-ui-chart__axis"
      />
      <line
        :x1="padding"
        :x2="width - padding"
        :y1="zeroY"
        :y2="zeroY"
        class="plugin-ui-chart__axis"
      />
      <template
        v-for="(item, seriesIndex) in series"
        :key="item.label"
      >
        <polyline
          v-if="chartKind === 'line'"
          fill="none"
          :stroke="palette[seriesIndex % palette.length]"
          stroke-width="2"
          :points="points(item.values)"
        />
        <rect
          v-for="(value, itemIndex) in item.values"
          v-else
          :key="itemIndex"
          :x="barX(itemIndex, seriesIndex)"
          :y="barY(value)"
          :width="barWidth()"
          :height="barHeight(value)"
          :fill="palette[seriesIndex % palette.length]"
        />
      </template>
    </svg>
    <div class="plugin-ui-chart__legend">
      <span
        v-for="(item, index) in series"
        :key="item.label"
      >
        <i :style="{ background: palette[index % palette.length] }" />
        {{ item.label }}
      </span>
    </div>
  </figure>
</template>

<style scoped>
.plugin-ui-chart { display: grid; margin: 0; gap: var(--nvx-space-2); }
.plugin-ui-chart figcaption { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-sm); }
.plugin-ui-chart svg { width: 100%; max-height: 200px; }
.plugin-ui-chart__axis { stroke: var(--nvx-color-border); }
.plugin-ui-chart__legend { display: flex; flex-wrap: wrap; gap: var(--nvx-space-2); color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }
.plugin-ui-chart__legend i { display: inline-block; width: 8px; height: 8px; margin-right: 3px; border-radius: 50%; }
</style>
