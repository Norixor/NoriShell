<script setup lang="ts">
import { useI18n } from "vue-i18n";

defineProps<{ full?: boolean }>();
const { t } = useI18n();
</script>

<template>
  <div
    class="workspace-incoming"
    :class="{ 'workspace-incoming--full': full }"
    role="status"
    aria-live="polite"
  >
    <div class="workspace-incoming__card">
      <span class="workspace-incoming__label">{{ t("workspaceTabs.movingIn") }}</span>
      <div
        class="workspace-incoming__bar workspace-incoming__bar--title"
        aria-hidden="true"
      />
      <div
        class="workspace-incoming__bar workspace-incoming__bar--long"
        aria-hidden="true"
      />
      <div
        class="workspace-incoming__bar workspace-incoming__bar--short"
        aria-hidden="true"
      />
    </div>
  </div>
</template>

<style scoped>
.workspace-incoming {
  position: absolute;
  top: 12px;
  right: 16px;
  z-index: 5;
  width: min(260px, calc(100% - 32px));
  pointer-events: none;
}

.workspace-incoming--full {
  inset: 0;
  display: grid;
  place-items: center;
  width: auto;
  background: var(--nvx-color-bg-canvas);
}

.workspace-incoming__card {
  width: 100%;
  padding: 18px;
  border: 1px solid var(--nvx-color-border);
  border-radius: 12px;
  background: var(--nvx-color-bg-surface);
  box-shadow: 0 10px 28px rgb(0 0 0 / 9%);
}

.workspace-incoming--full .workspace-incoming__card {
  width: min(480px, calc(100% - 48px));
  padding: 24px;
}

.workspace-incoming__label {
  display: block;
  margin-bottom: 18px;
  color: var(--nvx-color-text-secondary);
  font-size: 13px;
}

.workspace-incoming__bar {
  height: 10px;
  margin-top: 12px;
  border-radius: 5px;
  background: var(--nvx-color-border);
  opacity: .65;
  animation: workspace-incoming-pulse 1.2s ease-in-out infinite alternate;
}

.workspace-incoming__bar--title { width: 38%; height: 16px; }
.workspace-incoming__bar--long { width: 86%; }
.workspace-incoming__bar--short { width: 61%; }

@keyframes workspace-incoming-pulse {
  to { opacity: .32; }
}

@media (prefers-reduced-motion: reduce) {
  .workspace-incoming__bar { animation: none; }
}
</style>
