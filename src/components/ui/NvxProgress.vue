<script setup lang="ts">
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    label: string;
    value?: number | null;
    status?: "available" | "loading" | "disabled" | "unsupported" | "permissionDenied" | "stale" | "error";
    statusLabel?: string;
    variant?: "bar" | "ring";
    size?: "sm" | "md";
  }>(),
  {
    value: null,
    status: "available",
    statusLabel: undefined,
    variant: "bar",
    size: "md",
  },
);

const normalizedValue = computed(() => {
  if (props.value === null || !Number.isFinite(props.value)) return null;
  return Math.min(100, Math.max(0, props.value));
});
const exposesValue = computed(
  () => normalizedValue.value !== null && ["available", "stale"].includes(props.status),
);
const accessibleLabel = computed(() => (
  props.statusLabel ? `${props.label}: ${props.statusLabel}` : props.label
));
const ringCircumference = 2 * Math.PI * 18;
const ringDasharray = computed(() => {
  if (!exposesValue.value) return `0 ${ringCircumference}`;
  const filled = ringCircumference * ((normalizedValue.value ?? 0) / 100);
  return `${filled} ${ringCircumference - filled}`;
});
</script>

<template>
  <div
    class="nvx-progress"
    :class="[
      `nvx-progress--${status}`,
      `nvx-progress--${variant}`,
      `nvx-progress--${size}`,
    ]"
  >
    <template v-if="variant === 'ring'">
      <div
        class="nvx-progress__ring"
        :role="exposesValue ? 'progressbar' : 'status'"
        :aria-label="accessibleLabel"
        :aria-valuemin="exposesValue ? 0 : undefined"
        :aria-valuemax="exposesValue ? 100 : undefined"
        :aria-valuenow="exposesValue ? normalizedValue ?? undefined : undefined"
      >
        <svg
          aria-hidden="true"
          viewBox="0 0 44 44"
        >
          <circle
            class="nvx-progress__ring-track"
            cx="22"
            cy="22"
            r="18"
          />
          <circle
            v-if="exposesValue"
            class="nvx-progress__ring-value"
            cx="22"
            cy="22"
            r="18"
            :stroke-dasharray="ringDasharray"
          />
          <circle
            v-else
            class="nvx-progress__ring-placeholder"
            cx="22"
            cy="22"
            r="18"
          />
        </svg>
        <span class="nvx-progress__ring-content">
          <strong>{{ exposesValue ? `${Math.round(normalizedValue ?? 0)}%` : '—' }}</strong>
          <span>{{ label }}</span>
        </span>
      </div>
    </template>
    <template v-else>
      <div class="nvx-progress__header">
        <span>{{ label }}</span>
        <span class="nvx-progress__value">
          <slot name="value">
            {{ exposesValue ? `${Math.round(normalizedValue ?? 0)}%` : statusLabel }}
          </slot>
        </span>
      </div>
      <div
        class="nvx-progress__track"
        :role="exposesValue ? 'progressbar' : 'status'"
        :aria-label="accessibleLabel"
        :aria-valuemin="exposesValue ? 0 : undefined"
        :aria-valuemax="exposesValue ? 100 : undefined"
        :aria-valuenow="exposesValue ? normalizedValue ?? undefined : undefined"
      >
        <span
          v-if="exposesValue"
          class="nvx-progress__bar"
          :style="{ width: `${normalizedValue}%` }"
        />
        <span
          v-else
          class="nvx-progress__placeholder"
        />
      </div>
    </template>
  </div>
</template>

<style scoped>
.nvx-progress {
  display: grid;
  gap: var(--nvx-space-2);
  min-width: 0;
}

.nvx-progress--sm {
  gap: var(--nvx-space-1);
}

.nvx-progress__header {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: baseline;
  justify-content: space-between;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
}

.nvx-progress__value {
  overflow: hidden;
  color: var(--nvx-color-text-primary);
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-progress__track {
  position: relative;
  overflow: hidden;
  height: 6px;
  border-radius: 999px;
  background: var(--nvx-color-bg-subtle);
}

.nvx-progress--sm .nvx-progress__header {
  gap: var(--nvx-space-2);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
}

.nvx-progress--sm .nvx-progress__track {
  height: 4px;
}

.nvx-progress__bar,
.nvx-progress__placeholder {
  position: absolute;
  inset-block: 0;
  inset-inline-start: 0;
  border-radius: inherit;
}

.nvx-progress__bar {
  max-width: 100%;
  background: var(--nvx-color-accent);
}

.nvx-progress--stale .nvx-progress__bar {
  background: var(--nvx-color-warning);
}

.nvx-progress__placeholder {
  width: 100%;
  border: var(--nvx-border-width) dashed var(--nvx-color-border-strong);
  background: transparent;
}

.nvx-progress--error .nvx-progress__placeholder,
.nvx-progress--permissionDenied .nvx-progress__placeholder {
  border-color: var(--nvx-color-danger);
}

.nvx-progress--ring {
  display: block;
}

.nvx-progress__ring {
  position: relative;
  width: 52px;
  height: 52px;
  color: var(--nvx-color-accent);
}

.nvx-progress--sm .nvx-progress__ring {
  width: 44px;
  height: 44px;
}

.nvx-progress__ring svg {
  display: block;
  width: 100%;
  height: 100%;
  overflow: visible;
  transform: rotate(-90deg);
}

.nvx-progress__ring circle {
  fill: none;
  stroke-width: 4;
}

.nvx-progress__ring-track {
  stroke: var(--nvx-color-bg-subtle);
}

.nvx-progress__ring-value {
  stroke: currentColor;
  stroke-linecap: round;
}

.nvx-progress__ring-placeholder {
  stroke: var(--nvx-color-border-strong);
  stroke-dasharray: 3 4;
}

.nvx-progress__ring-content {
  position: absolute;
  inset: 0;
  display: grid;
  align-content: center;
  justify-items: center;
  color: var(--nvx-color-text-secondary);
  font-size: 9px;
  line-height: 11px;
}

.nvx-progress__ring-content strong {
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-xs);
  font-variant-numeric: tabular-nums;
  font-weight: var(--nvx-font-weight-semibold);
}

.nvx-progress--stale .nvx-progress__ring {
  color: var(--nvx-color-warning);
}

.nvx-progress--error .nvx-progress__ring-placeholder,
.nvx-progress--permissionDenied .nvx-progress__ring-placeholder {
  stroke: var(--nvx-color-danger);
}
</style>
