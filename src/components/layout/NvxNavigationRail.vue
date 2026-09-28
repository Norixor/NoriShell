<script setup lang="ts">
import { isTauri } from "@tauri-apps/api/core";
import {
  Check,
  CircleAlert,
  Code,
  Command,
  Copy,
  Download,
  Folder,
  Info,
  LayoutDashboard,
  Link,
  MoreHorizontal,
  Monitor,
  Package,
  Play,
  Plug,
  RefreshCw,
  Search,
  Server,
  Settings,
  Shield,
  Sparkles,
  SquareTerminal,
  StopCircle,
  Terminal,
  Upload,
  Waypoints,
  X,
} from "lucide-vue-next";
import { computed, onMounted, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";

import type { PluginNavigationItem } from "../../core-api/generated/core-api";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { useAppUpdateStore } from "../../stores/appUpdate";
import { showWorkspaceTabFailure } from "../../workspace-tab-errors";
import { openManagedPluginPage, showWorkspaceShellRoute } from "../../workspace-tab-view-shell";
import { activeWorkspaceTabViewId, workspaceTabViewSummary } from "../../workspace-tab-view-state";
import { NvxIcon } from "../ui";

const { t } = useI18n();
const route = useRoute();
const extensions = usePluginExtensionsStore();
const appUpdate = useAppUpdateStore();
const versionAccessibleLabel = computed(() => {
  const version = appUpdate.currentVersion
    ? t("releases.currentVersion", { version: appUpdate.currentVersion })
    : t("releases.versionUnavailable");
  const action = appUpdate.hasUpdate
    ? ` · ${t(appUpdate.supportsAutoInstall ? "releases.versionUpdateAction" : "releases.versionUpdateManualAction")}`
    : "";
  return `${version} · ${appUpdate.statusText}${action}`;
});
const versionUpdateRoute = { path: "/settings", query: { section: "about" } };
function onVersionActivate() {
  if (appUpdate.hasUpdate && appUpdate.supportsAutoInstall) appUpdate.requestInstallConfirmation();
}

const fixedItems = [
  { to: "/overview", labelKey: "navigation.overview", icon: LayoutDashboard },
  { to: "/terminal", labelKey: "navigation.terminal", icon: SquareTerminal },
  { to: "/desktop", labelKey: "desktop.navigation", icon: Monitor },
  { to: "/hosts", labelKey: "navigation.hosts", icon: Server },
  { to: "/sftp", labelKey: "navigation.sftp", icon: Folder },
  { to: "/tunnels", labelKey: "navigation.tunnels", icon: Waypoints },
  { to: "/plugins", labelKey: "navigation.plugins", icon: Package },
] as const;
const iconMap: Record<string, Component> = {
  terminal: Terminal,
  server: Server,
  folder: Folder,
  settings: Settings,
  plugin: Plug,
  command: Command,
  copy: Copy,
  search: Search,
  info: Info,
  check: Check,
  warning: CircleAlert,
  close: X,
  more: MoreHorizontal,
  play: Play,
  stop: StopCircle,
  refresh: RefreshCw,
  download: Download,
  upload: Upload,
  link: Link,
  shield: Shield,
  sparkles: Sparkles,
  code: Code,
};
interface RailItem { to: string; label: string; icon: Component; plugin?: PluginNavigationItem }
const items = computed<RailItem[]>(() => [
  ...fixedItems.map((item) => ({ to: item.to, icon: item.icon, label: t(item.labelKey) })),
  ...extensions.navigation.map((item) => ({
    to: `/plugin/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(item.navigation.pageId)}`,
    label: item.navigation.label,
    icon: iconMap[item.navigation.icon] ?? Plug,
    plugin: item,
  })),
]);

// An active Tab WebView covers the shell page, so it decides the selected item.
const activePath = computed(() => {
  const id = activeWorkspaceTabViewId.value;
  const path = (id ? workspaceTabViewSummary(id)?.route : undefined) ?? route.path;
  if (path === "/new") return "/terminal";
  if (path === "/known-hosts" || path.startsWith("/settings")) return "/settings";
  return path;
});
const isActive = (to: string) => activePath.value === to;

function openItem(item: Pick<RailItem, "plugin">, event: MouseEvent, navigate: (event?: MouseEvent) => unknown) {
  if (!isTauri()) {
    void navigate(event);
    return;
  }
  event.preventDefault();
  if (item.plugin) {
    // A plugin page is a Page Tab; opening it never renders it in the shell first.
    void openManagedPluginPage(item.plugin)
      .catch((error: unknown) => showWorkspaceTabFailure(error, "navigation-plugin-page", "workspace_tab.open_failed", "workspaceTabs.openFailed"));
    return;
  }
  // Leaving the active Tab shows the shell page even when the shell route is unchanged.
  // The Tab stays on screen until that page painted; a failed input release still navigates.
  let navigated = false;
  void showWorkspaceShellRoute(() => { navigated = true; return navigate(); })
    .catch(() => { if (!navigated) void navigate(); });
}
onMounted(() => {
  if (!isTauri()) return;
  void extensions.loadNavigation().catch(() => undefined);
  void appUpdate.loadPackagedVersion();
});
</script>

<template>
  <nav
    class="nvx-navigation-rail"
    :aria-label="t('navigation.primary')"
  >
    <div class="nvx-navigation-rail__main">
      <RouterLink
        v-for="item in items"
        :key="item.to"
        v-slot="{ href, navigate }"
        :to="item.to"
        custom
      >
        <a
          :href="href"
          class="nvx-navigation-rail__item"
          :class="{ 'router-link-active': isActive(item.to) }"
          :aria-current="isActive(item.to) ? 'page' : undefined"
          :aria-label="item.label"
          @click="openItem(item, $event, navigate)"
        >
          <NvxIcon
            :icon="item.icon"
            :size="22"
          />
          <span>{{ item.label }}</span>
        </a>
      </RouterLink>
    </div>
    <div class="nvx-navigation-rail__footer">
      <RouterLink
        v-slot="{ href, navigate }"
        to="/settings"
        custom
      >
        <a
          :href="href"
          class="nvx-navigation-rail__item"
          :class="{ 'router-link-active': isActive('/settings') }"
          :aria-current="isActive('/settings') ? 'page' : undefined"
          :aria-label="t('navigation.settings')"
          @click="openItem({}, $event, navigate)"
        >
          <NvxIcon
            :icon="Settings"
            :size="22"
          />
          <span>{{ t("navigation.settings") }}</span>
        </a>
      </RouterLink>
      <component
        :is="appUpdate.hasUpdate ? 'RouterLink' : 'span'"
        v-bind="appUpdate.hasUpdate ? { to: versionUpdateRoute, onClick: onVersionActivate } : { role: 'status' }"
        class="nvx-navigation-rail__version"
        :class="{ 'nvx-navigation-rail__version--interactive': appUpdate.hasUpdate }"
        :aria-label="versionAccessibleLabel"
        :title="versionAccessibleLabel"
      >
        <span
          class="nvx-navigation-rail__version-text"
          aria-hidden="true"
        >{{ appUpdate.currentVersion ? `v${appUpdate.currentVersion}` : '—' }}</span>
        <span
          class="nvx-navigation-rail__status-dot"
          :class="`nvx-navigation-rail__status-dot--${appUpdate.tone}`"
          aria-hidden="true"
        />
      </component>
    </div>
  </nav>
</template>

<style scoped>
.nvx-navigation-rail {
  display: flex;
  flex: 0 0 var(--nvx-layout-navigation-rail-width);
  flex-direction: column;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: hidden;
  gap: var(--nvx-space-2);
  width: var(--nvx-layout-navigation-rail-width);
  padding: var(--nvx-space-4) var(--nvx-space-2);
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.nvx-navigation-rail__main {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  gap: var(--nvx-space-2);
}

.nvx-navigation-rail__footer {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  align-items: stretch;
  margin: 0 calc(-1 * var(--nvx-space-2)) calc(-1 * var(--nvx-space-4));
  padding: var(--nvx-space-2) var(--nvx-space-2) var(--nvx-space-3);
  border-top-right-radius: 20px;
  background: var(--nvx-color-bg-surface);
}

.nvx-navigation-rail__item {
  position: relative;
  display: flex;
  flex-shrink: 0;
  flex-direction: column;
  gap: var(--nvx-space-1);
  align-items: center;
  justify-content: center;
  min-height: 56px;
  padding: var(--nvx-space-2) 2px;
  border-radius: var(--nvx-radius-md);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  text-decoration: none;
  transition:
    color var(--nvx-motion-fast),
    background-color var(--nvx-motion-fast);
}

.nvx-navigation-rail__version {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  min-height: 28px;
  margin: 0 2px;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  line-height: var(--nvx-line-height-xs);
  text-decoration: none;
  white-space: nowrap;
}

.nvx-navigation-rail__version--interactive {
  border-radius: var(--nvx-radius-md);
  cursor: pointer;
}

.nvx-navigation-rail__version--interactive:hover {
  background: var(--nvx-color-bg-subtle);
}

.nvx-navigation-rail__version--interactive:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

.nvx-navigation-rail__version-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nvx-navigation-rail__status-dot {
  flex-shrink: 0;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--nvx-color-text-tertiary);
}

.nvx-navigation-rail__status-dot--success { background: var(--nvx-color-success); }
.nvx-navigation-rail__status-dot--warning { background: var(--nvx-color-warning); }
.nvx-navigation-rail__status-dot--danger { background: var(--nvx-color-danger); }

.nvx-navigation-rail__item:hover {
  color: var(--nvx-color-text-primary);
  background: var(--nvx-color-bg-subtle);
}

.nvx-navigation-rail__item.router-link-active {
  color: var(--nvx-color-accent);
  background: var(--nvx-color-accent-soft);
}

.nvx-navigation-rail__item:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

.nvx-navigation-rail__item > span {
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
