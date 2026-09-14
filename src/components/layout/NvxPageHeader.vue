<script setup lang="ts">
defineProps<{
  breadcrumb: string;
  title: string;
  description?: string;
}>();
</script>

<template>
  <header class="nvx-page-header">
    <p class="nvx-page-header__breadcrumb">
      {{ breadcrumb }}
    </p>
    <div class="nvx-page-header__row">
      <div>
        <h1>{{ title }}</h1>
        <p v-if="description">
          {{ description }}
        </p>
      </div>
      <div
        v-if="$slots.actions"
        class="nvx-page-header__actions"
      >
        <slot name="actions" />
      </div>
    </div>
  </header>
</template>

<style scoped>
.nvx-page-header {
  margin-bottom: var(--nvx-space-6);
}

.nvx-page-header__breadcrumb {
  margin: 0 0 var(--nvx-space-2);
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-sm);
}

.nvx-page-header__row {
  display: flex;
  gap: var(--nvx-space-6);
  align-items: flex-start;
  justify-content: space-between;
}

h1,
p {
  margin-top: 0;
}

h1 {
  margin-bottom: var(--nvx-space-2);
  font-size: var(--nvx-font-size-xl);
  line-height: var(--nvx-line-height-xl);
  font-weight: var(--nvx-font-weight-semibold);
}

.nvx-page-header__row p {
  margin-bottom: 0;
  color: var(--nvx-color-text-secondary);
}

.nvx-page-header__actions {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
}
</style>
