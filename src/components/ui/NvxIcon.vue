<script setup lang="ts">
import type { Component } from "vue";

withDefaults(
  defineProps<{
    icon: Component;
    size?: 16 | 20 | 22;
    label?: string;
  }>(),
  { size: 20, label: undefined },
);
</script>

<template>
  <component
    :is="icon"
    :size="size"
    :aria-hidden="label ? undefined : 'true'"
    :aria-label="label"
    :role="label ? 'img' : undefined"
    :stroke-width="1.75"
  />
</template>

