<script setup lang="ts">
import { CircleAlert, CircleCheck, CircleHelp, Info } from "lucide-vue-next";
import { computed } from "vue";

import NvxIcon from "./NvxIcon.vue";

const props = withDefaults(
  defineProps<{
    tone?: "neutral" | "info" | "success" | "warning" | "danger";
  }>(),
  { tone: "neutral" },
);

const icon = computed(() => {
  switch (props.tone) {
    case "success":
      return CircleCheck;
    case "warning":
    case "danger":
      return CircleAlert;
    case "info":
      return Info;
    default:
      return CircleHelp;
  }
});
</script>

<template>
  <span
    class="nvx-status-label"
    :class="`nvx-status-label--${tone}`"
  >
    <NvxIcon
      :icon="icon"
      :size="16"
    />
    <span><slot /></span>
  </span>
</template>

<style scoped>
.nvx-status-label {
  display: inline-flex;
  align-items: center;
  gap: var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-medium);
}

.nvx-status-label--info {
  color: var(--nvx-color-accent);
}

.nvx-status-label--success {
  color: var(--nvx-color-success);
}

.nvx-status-label--warning {
  color: var(--nvx-color-warning);
}

.nvx-status-label--danger {
  color: var(--nvx-color-danger);
}
</style>
