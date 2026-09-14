<script setup lang="ts">
import { CircleCheck, CircleX, Info, TriangleAlert, X } from "lucide-vue-next";
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import { useTipsStore, type NvxTipTone } from "../../stores/tips";
import NvxIcon from "./NvxIcon.vue";
import NvxIconButton from "./NvxIconButton.vue";

const tips = useTipsStore();
const { t } = useI18n();

const toneIcons = {
  info: Info,
  success: CircleCheck,
  warning: TriangleAlert,
  error: CircleX,
} as const;

const visibleTips = computed(() => tips.items);

function iconFor(tone: NvxTipTone) {
  return toneIcons[tone];
}
</script>

<template>
  <Teleport to="body">
    <section
      class="nvx-tips"
      :aria-label="t('tips.regionLabel')"
    >
      <TransitionGroup name="nvx-tip">
        <article
          v-for="tip in visibleTips"
          :key="tip.id"
          class="nvx-tips__item"
          :class="`nvx-tips__item--${tip.tone}`"
          :role="tip.tone === 'error' || tip.tone === 'warning' ? 'alert' : 'status'"
        >
          <NvxIcon
            class="nvx-tips__icon"
            :icon="iconFor(tip.tone)"
            :size="20"
          />
          <div class="nvx-tips__content">
            <strong>{{ tip.title }}</strong>
            <p v-if="tip.message">
              {{ tip.message }}
            </p>
          </div>
          <NvxIconButton
            class="nvx-tips__dismiss"
            size="sm"
            :label="t('tips.dismiss')"
            @click="tips.dismiss(tip.id)"
          >
            <NvxIcon
              :icon="X"
              :size="16"
            />
          </NvxIconButton>
        </article>
      </TransitionGroup>
    </section>
  </Teleport>
</template>

<style scoped>
.nvx-tips {
  position: fixed;
  z-index: var(--nvx-z-toast);
  top: var(--nvx-layout-header-height);
  right: var(--nvx-space-1);
  width: min(384px, calc(100vw - var(--nvx-space-2)));
  padding: var(--nvx-space-3);
  display: grid;
  gap: var(--nvx-space-2);
  max-height: calc(100vh - var(--nvx-layout-header-height));
  overflow-y: auto;
  pointer-events: none;
  contain: layout paint;
}

.nvx-tips__item {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  gap: var(--nvx-space-3);
  align-items: center;
  min-height: 56px;
  padding: var(--nvx-space-3) var(--nvx-space-3) var(--nvx-space-3) var(--nvx-space-4);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-sm);
  background: var(--nvx-color-bg-surface);
  box-shadow: var(--nvx-shadow-toast);
  color: var(--nvx-color-text-primary);
  pointer-events: auto;
}

.nvx-tips__icon,
.nvx-tips__item--info .nvx-tips__icon {
  color: var(--nvx-color-accent);
}

.nvx-tips__item--success .nvx-tips__icon { color: var(--nvx-color-success); }
.nvx-tips__item--warning .nvx-tips__icon { color: var(--nvx-color-warning); }
.nvx-tips__item--error .nvx-tips__icon { color: var(--nvx-color-danger); }

.nvx-tips__content {
  min-width: 0;
  overflow-wrap: anywhere;
}

.nvx-tips__content strong,
.nvx-tips__content p {
  display: block;
  margin: 0;
}

.nvx-tips__content strong {
  font-size: var(--nvx-font-size-body);
  font-weight: var(--nvx-font-weight-semibold);
  line-height: var(--nvx-line-height-body);
}

.nvx-tips__content p {
  margin-top: var(--nvx-space-1);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

.nvx-tips__dismiss {
  align-self: center;
}

.nvx-tip-enter-active,
.nvx-tip-leave-active {
  transition: opacity var(--nvx-motion-overlay), transform var(--nvx-motion-overlay);
}

.nvx-tip-enter-from,
.nvx-tip-leave-to {
  opacity: 0;
  transform: translateY(calc(-1 * var(--nvx-space-2)));
}

@media (prefers-reduced-motion: reduce) {
  .nvx-tip-enter-active,
  .nvx-tip-leave-active {
    transition: none;
  }
}
</style>
