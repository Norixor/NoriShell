<script setup lang="ts">
import { CircleAlert, Info } from "lucide-vue-next";
import { computed } from "vue";

import NvxIcon from "./NvxIcon.vue";

const props = withDefaults(
  defineProps<{
    tone?: "info" | "warning" | "error";
    title?: string;
  }>(),
  { tone: "info", title: undefined },
);

const icon = computed(() => (props.tone === "info" ? Info : CircleAlert));
</script>

<template>
  <section
    class="nvx-inline-notice"
    :class="`nvx-inline-notice--${tone}`"
    role="status"
  >
    <NvxIcon
      :icon="icon"
      :size="20"
    />
    <div>
      <strong v-if="title">{{ title }}</strong>
      <div class="nvx-inline-notice__body">
        <slot />
      </div>
    </div>
  </section>
</template>

<style scoped>
.nvx-inline-notice {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: flex-start;
  padding: var(--nvx-space-3) var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-subtle);
}

.nvx-inline-notice--info {
  border-color: var(--nvx-color-accent);
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.nvx-inline-notice--warning {
  background: var(--nvx-color-warning-soft);
  color: var(--nvx-color-warning);
}

.nvx-inline-notice--error {
  background: var(--nvx-color-danger-soft);
  color: var(--nvx-color-danger);
}

.nvx-inline-notice__body {
  color: var(--nvx-color-text-primary);
}
</style>
