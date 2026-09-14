<script setup lang="ts">
withDefaults(
  defineProps<{
    tone?: "default" | "warning" | "danger";
  }>(),
  { tone: "default" },
);
</script>

<template>
  <section
    class="nvx-card"
    :class="`nvx-card--${tone}`"
  >
    <slot />
  </section>
</template>

<style scoped>
.nvx-card {
  min-width: 0;
  padding: var(--nvx-space-5);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
}

.nvx-card--warning {
  border-color: var(--nvx-color-warning);
}

.nvx-card--danger {
  border-color: var(--nvx-color-danger);
}
</style>
