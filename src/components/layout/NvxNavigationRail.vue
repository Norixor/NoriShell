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
  FolderSync,
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

import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import { NvxIcon } from "../ui";

const { t } = useI18n();
const extensions = usePluginExtensionsStore();

const fixedItems = [
  { to: "/overview", labelKey: "navigation.overview", icon: LayoutDashboard },
  { to: "/terminal", labelKey: "navigation.terminal", icon: SquareTerminal },
  { to: "/desktop", labelKey: "desktop.navigation", icon: Monitor },
  { to: "/hosts", labelKey: "navigation.hosts", icon: Server },
  { to: "/sftp", labelKey: "navigation.sftp", icon: FolderSync },
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
const items = computed(() => [
  ...fixedItems.map((item) => ({ ...item, label: t(item.labelKey) })),
  ...extensions.navigation.map((item) => ({
    to: `/plugin/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(item.navigation.pageId)}`,
    label: item.pluginName,
    icon: iconMap[item.navigation.icon] ?? Plug,
  })),
  { to: "/settings", label: t("navigation.settings"), icon: Settings },
]);

onMounted(() => {
  if (isTauri()) void extensions.loadNavigation().catch(() => undefined);
});
</script>

<template>
  <nav
    class="nvx-navigation-rail"
    :aria-label="t('navigation.primary')"
  >
    <RouterLink
      v-for="item in items"
      :key="item.to"
      :to="item.to"
      class="nvx-navigation-rail__item"
    >
      <NvxIcon
        :icon="item.icon"
        :size="22"
      />
      <span>{{ item.label }}</span>
    </RouterLink>
  </nav>
</template>

<style scoped>
.nvx-navigation-rail {
  display: flex;
  flex: 0 0 var(--nvx-layout-navigation-rail-width);
  flex-direction: column;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  gap: var(--nvx-space-2);
  width: var(--nvx-layout-navigation-rail-width);
  padding: var(--nvx-space-4) var(--nvx-space-2);
  border-right: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.nvx-navigation-rail__item {
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

.nvx-navigation-rail__item span {
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
