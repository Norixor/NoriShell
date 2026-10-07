<script setup lang="ts">
import { ChevronRight, Plus, Puzzle } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import { useTipsStore } from "../../stores/tips";
import {
  activateSingleDocumentTab,
  activeSingleDocumentTabId,
  createSingleDocumentTab,
  useSingleDocumentTabs,
  type SingleDocumentTab,
} from "../../single-document-workspace";
import { NvxIcon, NvxIconButton } from "../ui";
import NvxMobilePluginActionsSheet from "./NvxMobilePluginActionsSheet.vue";
import NvxMobileTabSwitcher from "./NvxMobileTabSwitcher.vue";
import { useMobileTerminalFocusMode, useMobileTerminalImmersive } from "./useMobileLandscape";

/**
 * The phone top bar: a leading, left-aligned single-line "current Tab" pill (status dot, bold
 * title, lighter connection state, trailing chevron), the Tab-count badge and a filled "+".
 * Tapping the pill or the count opens the Tab
 * switcher; a horizontal swipe on the pill moves to the previous/next Tab. Without a
 * Tab the pill names the current page. When a plugin contributes to the
 * `app.header.actions` slot, a puzzle button opens the plugin-actions sheet; the
 * slot target itself always lives in that sheet. In landscape with a Terminal Tab
 * the bar compacts so the terminal keeps its rows, and hides while the soft
 * keyboard is open (the Tab switcher and "+" return with the keyboard closed). The bar also mirrors the visual
 * viewport height into a root custom property so the soft keyboard shrinks the app
 * (and the xterm fit follows through the existing ResizeObserver).
 */
const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const tips = useTipsStore();
const { store, tabs } = useSingleDocumentTabs();
const switcherOpen = ref(false);
const scrolled = ref(false);
const pluginActionsOpen = ref(false);
const pluginActionCount = ref(0);
const compact = useMobileTerminalFocusMode();
// Landscape with the soft keyboard open: the bar hides so the terminal keeps more than one row.
const immersive = useMobileTerminalImmersive(compact);

const activeTab = computed<SingleDocumentTab | null>(() => {
  const id = activeSingleDocumentTabId(store, route.path);
  return tabs.value.find((tab) => tab.id === id) ?? null;
});

// Routes without a Tab name themselves in the pill (new ordinary routes opt in here).
const routeLabelKeys: Record<string, string> = {
  "/overview": "navigation.overview",
  "/terminal": "navigation.terminal",
  "/hosts": "navigation.hosts",
  "/sftp": "navigation.sftp",
  "/desktop": "desktop.navigation",
  "/tunnels": "navigation.tunnels",
  "/plugins": "navigation.plugins",
  "/settings": "navigation.settings",
  "/new": "newWorkspace.title",
};

const pageTitle = computed(() => {
  const key = routeLabelKeys[route.path] ?? (route.path.startsWith("/plugin/") ? "navigation.plugins" : null);
  return key ? t(key) : t("mobile.brand");
});

const title = computed(() => {
  const tab = activeTab.value;
  if (!tab) return pageTitle.value;
  return tab.labelKey ? t(tab.labelKey) : tab.label;
});

const subtitle = computed(() => {
  const tab = activeTab.value;
  if (!tab) return tabs.value.length ? t("mobile.tabsCount", tabs.value.length) : t("mobile.noTabs");
  if (tab.kind === "file") return t("sshTerminal.paneCount", tab.paneCount);
  if (tab.kind === "page") return t("workspaceTabs.pageState");
  return tab.stateLabel;
});

const tone = computed(() => activeTab.value?.stateTone ?? "neutral");
const busy = computed(() => store.terminalBusy || store.desktopBusy);

function report(scope: string, titleKey: string) {
  tips.show({ scope, tone: "error", title: t(titleKey) });
}

function create() {
  void createSingleDocumentTab(store, router).catch(() => report("mobile-tab-open", "workspaceTabs.openFailed"));
}

// Horizontal swipe on the pill switches Tabs; the terminal surface never receives gestures from here.
const SWIPE_DISTANCE = 48;
const SWIPE_MAX_DRIFT = 32;
let swipe: { pointerId: number; startX: number; startY: number } | null = null;
let suppressClick = false;

function beginSwipe(event: PointerEvent) {
  if (event.button !== 0) return;
  swipe = { pointerId: event.pointerId, startX: event.clientX, startY: event.clientY };
}

function endSwipe(event: PointerEvent) {
  if (!swipe || swipe.pointerId !== event.pointerId) return;
  const dx = event.clientX - swipe.startX;
  const dy = Math.abs(event.clientY - swipe.startY);
  swipe = null;
  if (Math.abs(dx) < SWIPE_DISTANCE || dy > SWIPE_MAX_DRIFT) return;
  suppressClick = true;
  switchTab(dx < 0 ? 1 : -1);
}

function cancelSwipe() {
  swipe = null;
}

function switchTab(step: 1 | -1) {
  const list = tabs.value;
  if (!list.length || busy.value) return;
  const index = activeTab.value ? list.findIndex((tab) => tab.id === activeTab.value!.id) : -1;
  const target = index < 0 ? list[step > 0 ? 0 : list.length - 1] : list[index + step];
  if (!target || target.id === activeTab.value?.id) return;
  void activateSingleDocumentTab(store, router, target).catch(() => report("mobile-tab-open", "workspaceTabs.openFailed"));
}

function handlePillClick() {
  if (suppressClick) {
    suppressClick = false;
    return;
  }
  switcherOpen.value = true;
}

function handlePillKeydown(event: KeyboardEvent) {
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    switchTab(event.key === "ArrowRight" ? 1 : -1);
  }
}

// The bar casts a soft shadow only while page content underneath is scrolled.
function handleDocumentScroll(event: Event) {
  const target = event.target;
  if (!(target instanceof HTMLElement) || !target.closest(".app-main") || target.closest(".xterm")) return;
  scrolled.value = target.scrollTop > 0;
}

const VIEWPORT_HEIGHT_PROPERTY = "--nvx-mobile-viewport-height";
function syncViewportHeight() {
  const viewport = window.visualViewport;
  if (!viewport) return;
  document.documentElement.style.setProperty(VIEWPORT_HEIGHT_PROPERTY, `${Math.round(viewport.height)}px`);
}

// Sheets size themselves against the bar height (mobile.css), so the compact bar publishes it.
const HEADER_HEIGHT_PROPERTY = "--nvx-mobile-header-height";
const COMPACT_HEADER_HEIGHT = "40px";
watch([compact, immersive], ([compactValue, immersiveValue]) => {
  if (immersiveValue) document.documentElement.style.setProperty(HEADER_HEIGHT_PROPERTY, "0px");
  else if (compactValue) document.documentElement.style.setProperty(HEADER_HEIGHT_PROPERTY, COMPACT_HEADER_HEIGHT);
  else document.documentElement.style.removeProperty(HEADER_HEIGHT_PROPERTY);
}, { immediate: true });

onMounted(() => {
  window.visualViewport?.addEventListener("resize", syncViewportHeight);
  syncViewportHeight();
  document.addEventListener("scroll", handleDocumentScroll, { capture: true, passive: true });
});
onBeforeUnmount(() => {
  window.visualViewport?.removeEventListener("resize", syncViewportHeight);
  document.documentElement.style.removeProperty(VIEWPORT_HEIGHT_PROPERTY);
  document.documentElement.style.removeProperty(HEADER_HEIGHT_PROPERTY);
  document.removeEventListener("scroll", handleDocumentScroll, { capture: true });
});
</script>

<template>
  <header
    v-show="!immersive"
    class="nvx-mobile-shell"
    :class="{ 'nvx-mobile-shell--scrolled': scrolled, 'nvx-mobile-shell--compact': compact }"
  >
    <div class="nvx-mobile-shell__center">
      <button
        class="nvx-mobile-shell__pill"
        type="button"
        :aria-label="t('mobile.currentTab', { title, state: subtitle })"
        :aria-description="t('mobile.swipeHint')"
        aria-haspopup="dialog"
        :aria-expanded="switcherOpen"
        @click="handlePillClick"
        @keydown="handlePillKeydown"
        @pointerdown="beginSwipe"
        @pointerup="endSwipe"
        @pointercancel="cancelSwipe"
      >
        <span
          v-if="activeTab && activeTab.kind !== 'page'"
          class="nvx-mobile-shell__dot"
          :class="`nvx-mobile-shell__dot--${tone}`"
          aria-hidden="true"
        />
        <span class="nvx-mobile-shell__text">
          <strong class="nvx-mobile-shell__title">{{ title }}</strong>
          <span
            class="nvx-mobile-shell__separator"
            aria-hidden="true"
          >·</span>
          <span class="nvx-mobile-shell__subtitle">{{ subtitle }}</span>
        </span>
        <NvxIcon
          class="nvx-mobile-shell__chevron"
          :icon="ChevronRight"
          :size="16"
        />
      </button>
    </div>
    <div class="nvx-mobile-shell__actions">
      <NvxIconButton
        v-if="pluginActionCount > 0"
        class="nvx-mobile-shell__plugin-actions"
        :label="t('mobile.pluginActionsTitle')"
        aria-haspopup="dialog"
        :aria-expanded="pluginActionsOpen"
        @click="pluginActionsOpen = true"
      >
        <NvxIcon
          :icon="Puzzle"
          :size="20"
        />
      </NvxIconButton>
      <button
        class="nvx-mobile-shell__count"
        type="button"
        :aria-label="t('mobile.tabsCount', tabs.length)"
        aria-haspopup="dialog"
        :aria-expanded="switcherOpen"
        @click="switcherOpen = true"
      >
        <span class="nvx-mobile-shell__count-badge">{{ tabs.length }}</span>
      </button>
      <NvxIconButton
        class="nvx-mobile-shell__new"
        :label="t('mobile.newTab')"
        :disabled="busy"
        @click="create"
      >
        <NvxIcon
          :icon="Plus"
          :size="22"
        />
      </NvxIconButton>
    </div>
    <NvxMobileTabSwitcher v-model="switcherOpen" />
    <NvxMobilePluginActionsSheet
      v-model="pluginActionsOpen"
      @availability="pluginActionCount = $event"
    />
  </header>
</template>

<style scoped>
.nvx-mobile-shell {
  position: relative;
  z-index: var(--nvx-z-sticky);
  display: grid;
  flex: none;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: var(--nvx-space-2);
  align-items: center;
  height: calc(var(--nvx-mobile-header-height, 52px) + env(safe-area-inset-top, 0px));
  padding: env(safe-area-inset-top, 0px) calc(var(--nvx-space-2) + env(safe-area-inset-right, 0px)) 0 calc(var(--nvx-space-3) + env(safe-area-inset-left, 0px));
  background: var(--nvx-color-bg-surface);
  transition: box-shadow var(--nvx-motion-fast);
}

.nvx-mobile-shell--scrolled {
  box-shadow: var(--nvx-shadow-bar);
}

/* Landscape with a Terminal Tab: a 40px bar; the pill drops its state text to keep width. */
.nvx-mobile-shell--compact {
  height: calc(40px + env(safe-area-inset-top, 0px));
}

.nvx-mobile-shell--compact .nvx-mobile-shell__pill {
  min-height: 32px;
  border-radius: 16px;
}

.nvx-mobile-shell--compact .nvx-mobile-shell__subtitle,
.nvx-mobile-shell--compact .nvx-mobile-shell__separator {
  display: none;
}

.nvx-mobile-shell--compact .nvx-mobile-shell__count,
.nvx-mobile-shell--compact .nvx-mobile-shell__actions :deep(.nvx-icon-button) {
  width: 40px;
  height: 40px;
}

.nvx-mobile-shell--compact .nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new) {
  border-radius: 10px;
}

.nvx-mobile-shell__center {
  display: flex;
  justify-content: flex-start;
  min-width: 0;
}

/* Outlined pill with an accent-tinted border (tokens only; the tint follows the theme). */
.nvx-mobile-shell__pill {
  display: inline-flex;
  gap: var(--nvx-space-2);
  align-items: center;
  max-width: 100%;
  min-height: 36px;
  padding: 0 var(--nvx-space-2) 0 var(--nvx-space-3);
  border: var(--nvx-border-width) solid color-mix(in srgb, var(--nvx-color-accent) 45%, var(--nvx-color-border));
  border-radius: 18px;
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: left;
  cursor: pointer;
  touch-action: pan-y;
  -webkit-tap-highlight-color: transparent;
  user-select: none;
}

.nvx-mobile-shell__pill:active { background: var(--nvx-color-bg-hover); }
.nvx-mobile-shell__pill:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

.nvx-mobile-shell__dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--nvx-color-text-tertiary);
}

.nvx-mobile-shell__dot--info { background: var(--nvx-color-accent); }
.nvx-mobile-shell__dot--success { background: var(--nvx-color-success); }
.nvx-mobile-shell__dot--warning { background: var(--nvx-color-warning); }
.nvx-mobile-shell__dot--danger { background: var(--nvx-color-danger); }

/* One line: the title truncates first, the state keeps at least part of its text. */
.nvx-mobile-shell__text {
  display: flex;
  gap: var(--nvx-space-1);
  align-items: baseline;
  min-width: 0;
  font-size: var(--nvx-font-size-body);
  line-height: 1.3;
}

.nvx-mobile-shell__title,
.nvx-mobile-shell__subtitle {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-mobile-shell__title {
  /* Shrinks well before the state text, so a long name truncates first. */
  flex: 0 10 auto;
  min-width: 3ch;
  font-weight: var(--nvx-font-weight-semibold);
}

.nvx-mobile-shell__separator,
.nvx-mobile-shell__subtitle {
  flex: none;
  color: var(--nvx-color-text-secondary);
}

.nvx-mobile-shell__subtitle {
  /* No percentage cap: the pill is shrink-to-fit, so a % would truncate even with room to spare. */
  flex: 0 1 auto;
  min-width: 4ch;
  font-size: var(--nvx-font-size-sm);
}

.nvx-mobile-shell__chevron {
  flex: none;
  color: var(--nvx-color-text-tertiary);
}

.nvx-mobile-shell__actions {
  display: flex;
  flex: none;
  align-items: center;
}

.nvx-mobile-shell__count,
.nvx-mobile-shell__actions :deep(.nvx-icon-button) {
  width: 48px;
  height: 48px;
  border-radius: 50%;
}

.nvx-mobile-shell__count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--nvx-color-text-secondary);
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.nvx-mobile-shell__count:active { background: var(--nvx-color-bg-hover); }
.nvx-mobile-shell__count:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -4px;
}

/* Rounded-square outlined count badge, matching the pill's corner language. */
.nvx-mobile-shell__count-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 36px;
  height: 36px;
  padding: 0 6px;
  border: 2px solid var(--nvx-color-border-strong);
  border-radius: 10px;
  color: var(--nvx-color-text-primary);
  font-size: var(--nvx-font-size-sm);
  font-weight: var(--nvx-font-weight-semibold);
  font-variant-numeric: tabular-nums;
  line-height: 1;
}

/* Filled accent "+": a rounded square instead of the ghost circle. */
.nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new) {
  /* 36px visual (same as the pill and the count badge); the ::after keeps a 48px touch target. */
  position: relative;
  width: 36px;
  height: 36px;
  margin-inline: var(--nvx-space-2) var(--nvx-space-1);
  border-radius: 10px;
  background: var(--nvx-color-accent);
  color: var(--nvx-color-on-accent);
}

.nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new)::after {
  position: absolute;
  inset: -6px;
  content: "";
}

/* Outranks the shared ghost hover (a tap leaves :hover sticky on Android). */
.nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new:hover:not(:disabled)),
.nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new:active:not(:disabled)) {
  background: var(--nvx-color-accent);
  color: var(--nvx-color-on-accent);
}

.nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new:active:not(:disabled)) {
  filter: brightness(0.9);
}

.nvx-mobile-shell__actions :deep(.nvx-mobile-shell__new:disabled) {
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}
</style>
