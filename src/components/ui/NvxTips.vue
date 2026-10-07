<script setup lang="ts">
import { CircleCheck, CircleX, Info, TriangleAlert, X } from "lucide-vue-next";
import { computed, reactive } from "vue";
import { useI18n } from "vue-i18n";

import { mobileOverlayDepthRef } from "../../mobile-overlays";
import { isMobileLayout } from "../../platform-host";
import { useTipsStore, type NvxTipTone } from "../../stores/tips";
import NvxButton from "./NvxButton.vue";
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

// Phone shell: tips sit above the bottom bar and swipe away sideways (platform fact, not width).
// While a sheet or dialog is open they move to the top edge instead, so they never cover
// the sheet's action row (sheets anchor to the bottom).
const mobile = isMobileLayout();
const aboveOverlay = computed(() => mobile && mobileOverlayDepthRef.value > 0);
const SWIPE_DISMISS_DISTANCE = 72;
const swipeOffsets = reactive<Record<string, number>>({});
let swipe: { id: string; pointerId: number; startX: number; startY: number; horizontal: boolean | null } | null = null;

function beginSwipe(event: PointerEvent, id: string) {
  if (!mobile || event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
  swipe = { id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, horizontal: null };
}

function moveSwipe(event: PointerEvent) {
  if (!swipe || swipe.pointerId !== event.pointerId) return;
  const dx = event.clientX - swipe.startX;
  const dy = event.clientY - swipe.startY;
  if (swipe.horizontal === null && (Math.abs(dx) > 6 || Math.abs(dy) > 6)) swipe.horizontal = Math.abs(dx) > Math.abs(dy);
  if (!swipe.horizontal) return;
  (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
  swipeOffsets[swipe.id] = dx;
}

function endSwipe(event: PointerEvent) {
  if (!swipe || swipe.pointerId !== event.pointerId) return;
  const { id } = swipe;
  const offset = swipeOffsets[id] ?? 0;
  swipe = null;
  delete swipeOffsets[id];
  if (Math.abs(offset) > SWIPE_DISMISS_DISTANCE) tips.dismiss(id);
}

function swipeStyle(id: string) {
  const offset = swipeOffsets[id];
  return offset ? { transform: `translateX(${offset}px)`, opacity: Math.max(0.3, 1 - Math.abs(offset) / 240) } : undefined;
}
</script>

<template>
  <Teleport to="body">
    <section
      class="nvx-tips"
      :class="{ 'nvx-tips--mobile': mobile, 'nvx-tips--mobile-overlay': aboveOverlay }"
      :aria-label="t('tips.regionLabel')"
    >
      <TransitionGroup name="nvx-tip">
        <article
          v-for="tip in visibleTips"
          :key="tip.id"
          class="nvx-tips__item"
          :class="[
            `nvx-tips__item--${tip.tone}`,
            { 'nvx-tips__item--action': tip.action },
          ]"
          :role="tip.tone === 'error' || tip.tone === 'warning' ? 'alert' : 'status'"
          :style="swipeStyle(tip.id)"
          @pointerdown="beginSwipe($event, tip.id)"
          @pointermove="moveSwipe"
          @pointerup="endSwipe"
          @pointercancel="endSwipe"
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
          <NvxButton
            v-if="tip.action"
            class="nvx-tips__action"
            variant="ghost"
            size="sm"
            :loading="tip.actionPending"
            :loading-label="tip.action.label"
            @click="void tips.runAction(tip.id)"
          >
            {{ tip.action.label }}
          </NvxButton>
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

.nvx-tips__item--action {
  grid-template-columns: auto minmax(0, 1fr) auto auto;
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

/* Phone shell: anchored above the bottom bar (and the soft keyboard), full width with 16px gutters. */
.nvx-tips--mobile {
  top: auto;
  right: 0;
  bottom: calc(var(--nvx-mobile-sheet-bottom, 0px) + var(--nvx-mobile-bottom-inset, 0px));
  left: 0;
  width: auto;
  max-height: 50vh;
  padding: var(--nvx-space-2) var(--nvx-space-4);
}

/* With a sheet open the bottom edge belongs to its actions; tips drop in from the top instead. */
.nvx-tips--mobile-overlay {
  top: calc(env(safe-area-inset-top, 0px) + var(--nvx-space-2));
  bottom: auto;
}

.nvx-tips--mobile .nvx-tips__item {
  touch-action: pan-y;
  border-radius: var(--nvx-radius-md);
}

.nvx-tips--mobile .nvx-tip-enter-from,
.nvx-tips--mobile .nvx-tip-leave-to {
  transform: translateY(var(--nvx-space-2));
}
</style>
