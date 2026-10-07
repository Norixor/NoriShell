<script setup lang="ts">
import { ChevronDown, ChevronUp, Ellipsis, Folder, LayoutDashboard, PanelRightOpen, Server, SquareTerminal } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import { useWorkspaceTabsStore } from "../../stores/workspaceTabs";
import NvxTerminalExtraKeys from "../terminal/NvxTerminalExtraKeys.vue";
import { NvxIcon, NvxIconButton } from "../ui";
import NvxMobileMoreSheet, { isMoreSheetRoute } from "./NvxMobileMoreSheet.vue";
import NvxMobileQuickCommandsSheet from "./NvxMobileQuickCommandsSheet.vue";
import { useMobileImeVisible, useMobileTerminalFocusMode, useMobileTerminalImmersive } from "./useMobileLandscape";

/**
 * Phone bottom navigation: the primary routes of the desktop rail, with the rest
 * behind "More". Above it, on the Terminal page, sits the helper-key row with the
 * Quick Commands entry at its end. Switching pages only navigates; Sessions keep
 * their lifecycle exactly as on desktop. The bar publishes its height as
 * `--nvx-mobile-bottom-inset` so tips and sheets can stay above it. In landscape
 * with a Terminal Tab the navigation row collapses behind a slim handle so the
 * terminal gets the rows. Whenever the soft keyboard is open (any page: a terminal,
 * a Host form, a dialog field) the navigation row and the handle are gone, so the
 * keyboard pushes only the content and the helper keys sit directly on it; the
 * helper keys stay because they are terminal input.
 */
const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const workspaceTabs = useWorkspaceTabsStore();
const moreOpen = ref(false);
const quickCommandsOpen = ref(false);
const root = ref<HTMLElement | null>(null);
const focusMode = useMobileTerminalFocusMode();
// With the soft keyboard open in landscape even the handle goes; only the helper keys remain.
const immersive = useMobileTerminalImmersive(focusMode);
const imeVisible = useMobileImeVisible();
const navExpanded = ref(false);
const navCollapsed = computed(() => imeVisible.value || (focusMode.value && (immersive.value || !navExpanded.value)));
// Leaving focus mode (or arriving at another page) resets the temporary expansion.
watch([focusMode, () => route.path], () => { navExpanded.value = false; });

interface NavItem { to: string; label: string; icon: Component }
const items = computed<NavItem[]>(() => [
  { to: "/overview", label: t("navigation.overview"), icon: LayoutDashboard },
  { to: "/terminal", label: t("navigation.terminal"), icon: SquareTerminal },
  { to: "/hosts", label: t("navigation.hosts"), icon: Server },
  { to: "/sftp", label: t("navigation.sftp"), icon: Folder },
]);
const activePath = computed(() => {
  const path = route.path;
  if (path === "/new") return "/terminal";
  return path;
});
// "More" is highlighted only for a route it actually lists, never as a fallback while a page loads.
const moreActive = computed(() => isMoreSheetRoute(route.path));
const showExtraKeys = computed(() => route.path === "/terminal" && workspaceTabs.terminalTabs.length > 0);

function open(item: NavItem) {
  navExpanded.value = false;
  if (route.path !== item.to) void router.push(item.to);
}

function openMore() {
  navExpanded.value = false;
  moreOpen.value = true;
}

const BOTTOM_INSET_PROPERTY = "--nvx-mobile-bottom-inset";
let observer: ResizeObserver | null = null;
function publishInset() {
  const height = root.value?.getBoundingClientRect().height ?? 0;
  document.documentElement.style.setProperty(BOTTOM_INSET_PROPERTY, `${Math.round(height)}px`);
}

onMounted(() => {
  if (typeof ResizeObserver !== "undefined" && root.value) {
    observer = new ResizeObserver(publishInset);
    observer.observe(root.value);
  }
  publishInset();
});
onBeforeUnmount(() => {
  observer?.disconnect();
  document.documentElement.style.removeProperty(BOTTOM_INSET_PROPERTY);
});

</script>

<template>
  <div
    ref="root"
    class="nvx-mobile-bottom"
  >
    <NvxTerminalExtraKeys v-if="showExtraKeys">
      <template #trailing>
        <NvxIconButton
          class="nvx-mobile-bottom__quick-commands"
          :label="t('mobile.openQuickCommands')"
          aria-haspopup="dialog"
          :aria-expanded="quickCommandsOpen"
          @pointerdown.prevent
          @click="quickCommandsOpen = true"
        >
          <NvxIcon
            :icon="PanelRightOpen"
            :size="20"
          />
        </NvxIconButton>
      </template>
    </NvxTerminalExtraKeys>
    <button
      v-if="focusMode && !immersive && !imeVisible"
      class="nvx-mobile-nav__handle"
      type="button"
      :aria-label="t(navExpanded ? 'mobile.hideNavigation' : 'mobile.showNavigation')"
      :aria-expanded="navExpanded"
      @click="navExpanded = !navExpanded"
    >
      <NvxIcon
        :icon="navExpanded ? ChevronDown : ChevronUp"
        :size="16"
      />
    </button>
    <nav
      v-if="!navCollapsed"
      class="nvx-mobile-nav"
      :aria-label="t('navigation.primary')"
    >
      <button
        v-for="item in items"
        :key="item.to"
        class="nvx-mobile-nav__item"
        type="button"
        :class="{ 'nvx-mobile-nav__item--active': activePath === item.to }"
        :aria-current="activePath === item.to ? 'page' : undefined"
        :aria-label="item.label"
        @click="open(item)"
      >
        <span class="nvx-mobile-nav__pill">
          <NvxIcon
            :icon="item.icon"
            :size="22"
          />
        </span>
        <span>{{ item.label }}</span>
      </button>
      <button
        class="nvx-mobile-nav__item"
        type="button"
        :class="{ 'nvx-mobile-nav__item--active': moreActive }"
        :aria-current="moreActive ? 'page' : undefined"
        :aria-label="t('mobile.more')"
        aria-haspopup="dialog"
        :aria-expanded="moreOpen"
        @click="openMore"
      >
        <span class="nvx-mobile-nav__pill">
          <NvxIcon
            :icon="Ellipsis"
            :size="22"
          />
        </span>
        <span>{{ t("mobile.more") }}</span>
      </button>
    </nav>
    <NvxMobileMoreSheet v-model="moreOpen" />
    <NvxMobileQuickCommandsSheet
      v-if="showExtraKeys"
      v-model="quickCommandsOpen"
    />
  </div>
</template>

<style scoped>
.nvx-mobile-bottom {
  position: relative;
  z-index: var(--nvx-z-sticky);
  display: flex;
  flex: none;
  flex-direction: column;
  background: var(--nvx-color-bg-surface);
}

.nvx-mobile-nav {
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  padding: 0 env(safe-area-inset-right, 0px) env(safe-area-inset-bottom, 0px) env(safe-area-inset-left, 0px);
}

.nvx-mobile-nav__item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-items: center;
  justify-content: center;
  min-height: 60px;
  padding: var(--nvx-space-1) 2px;
  border: 0;
  background: transparent;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  cursor: pointer;
  transition: color var(--nvx-motion-fast);
  -webkit-tap-highlight-color: transparent;
}

.nvx-mobile-nav__item > span:last-child {
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-mobile-nav__pill {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 56px;
  height: 30px;
  border-radius: 15px;
  transition: background-color var(--nvx-motion-fast), color var(--nvx-motion-fast);
}

.nvx-mobile-nav__item--active {
  color: var(--nvx-color-accent);
  font-weight: var(--nvx-font-weight-medium);
}

.nvx-mobile-nav__item--active .nvx-mobile-nav__pill {
  background: var(--nvx-color-accent-soft);
}

.nvx-mobile-nav__item:active .nvx-mobile-nav__pill {
  background: var(--nvx-color-bg-hover);
}

.nvx-mobile-nav__item:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -2px;
}

/* Landscape handle: full-width so it is easy to hit, slim so the terminal keeps its rows. */
.nvx-mobile-nav__handle {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 24px;
  padding: 0 0 env(safe-area-inset-bottom, 0px);
  border: 0;
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-tertiary);
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.nvx-mobile-nav__handle:active { background: var(--nvx-color-bg-hover); }
.nvx-mobile-nav__handle:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: -2px;
}

.nvx-mobile-nav__handle[aria-expanded="true"] {
  padding-bottom: 0;
}

.nvx-mobile-bottom__quick-commands {
  width: 44px;
  height: 40px;
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-subtle);
}

@media (prefers-reduced-motion: reduce) {
  .nvx-mobile-nav__item,
  .nvx-mobile-nav__pill {
    transition: none;
  }
}
</style>
