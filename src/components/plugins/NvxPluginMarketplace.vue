<script setup lang="ts">
import {
  Search,
  RefreshCw,
  ShieldAlert,
  X,
} from "lucide-vue-next";
import { computed, ref, useId, watch } from "vue";
import { useI18n } from "vue-i18n";

import { NvxButton, NvxIcon, NvxIconButton, NvxInlineNotice } from "../ui";
import type { PluginCatalogEntryDto } from "../../core-api/client";
import type { InstalledPluginSummary, PluginCapability } from "../../core-api/generated/core-api";
import { usePluginsStore } from "../../stores/plugins";
import { usePluginIconsStore } from "../../stores/pluginIcons";
import applePlatformIcon from "../../assets/platforms/apple.svg?no-inline";
import linuxPlatformIcon from "../../assets/platforms/linux.svg?no-inline";
import windowsPlatformIcon from "../../assets/platforms/windows.svg?no-inline";
import { comparePluginVersions } from "./pluginCatalogVersion";
import { specialPluginCapabilities } from "./pluginCapabilities";
import automationPreview from "../../assets/plugin-marketplace/automation.png";
import commandPalettePreview from "../../assets/plugin-marketplace/command-palette.png";
import gitIntegrationPreview from "../../assets/plugin-marketplace/git-integration.png";
import hostMonitorPreview from "../../assets/plugin-marketplace/host-monitor.png";
import jsonFormatterPreview from "../../assets/plugin-marketplace/json-formatter.png";
import materialThemePreview from "../../assets/plugin-marketplace/material-light-theme.png";
import sftpSyncPreview from "../../assets/plugin-marketplace/sftp-sync.png";
import sqlRunnerPreview from "../../assets/plugin-marketplace/sql-runner.png";
import sshToolkitPreview from "../../assets/plugin-marketplace/ssh-toolkit.png";
import terminalEnhancerPreview from "../../assets/plugin-marketplace/terminal-enhancer.png";
import { officialPluginIcons } from "../../assets/plugin-marketplace/officialPluginIcons";

type MarketplaceCategory = "terminal" | "connection" | "files" | "appearance" | "automation" | "developer" | "monitoring";
type MarketplacePlatform = "macos" | "windows" | "linux";
type MarketplacePermission = "terminal" | "host" | "storage" | "special";
type MarketplaceIcon = keyof typeof pluginPreviewImages;

interface MarketplacePlugin {
  id: string;
  name: string;
  publisher: string;
  version: string;
  minimumVersion: string;
  category: MarketplaceCategory;
  icon: MarketplaceIcon;
  platforms: MarketplacePlatform[];
  permissions: MarketplacePermission[];
  extensionTargets: string[];
  releaseDate: string;
  entry?: PluginCatalogEntryDto;
}

const pluginPreviewImages = {
  activity: hostMonitorPreview,
  automation: automationPreview,
  command: commandPalettePreview,
  database: sqlRunnerPreview,
  files: sftpSyncPreview,
  git: gitIntegrationPreview,
  keys: sshToolkitPreview,
  terminal: terminalEnhancerPreview,
  theme: materialThemePreview,
  code: jsonFormatterPreview,
} as const;

const platformIcons: Record<MarketplacePlatform, string> = {
  macos: applePlatformIcon,
  windows: windowsPlatformIcon,
  linux: linuxPlatformIcon,
};

const visualFixtureCatalog: MarketplacePlugin[] = [
  {
    id: "dev.norishell.terminal-enhancer",
    name: "NoriShell Terminal Enhancer",
    publisher: "NoriShell",
    version: "1.3.0",
    minimumVersion: "1.2.0",
    category: "terminal",
    icon: "terminal",
    platforms: ["windows", "macos", "linux"],
    permissions: ["terminal", "host", "special"],
    extensionTargets: ["terminal.toolbar", "terminal.sidebar", "commandPalette"],
    releaseDate: "2026-08-28",
  },
  {
    id: "dev.norishell.ssh-toolkit",
    name: "NoriShell SSH Toolkit",
    publisher: "NoriShell",
    version: "1.2.1",
    minimumVersion: "1.2.0",
    category: "connection",
    icon: "keys",
    platforms: ["windows", "macos", "linux"],
    permissions: ["host", "storage"],
    extensionTargets: ["host.detail.tools", "commandPalette"],
    releaseDate: "2026-08-26",
  },
  {
    id: "dev.norishell.sftp-sync",
    name: "NoriShell SFTP Sync",
    publisher: "NoriShell",
    version: "1.1.0",
    minimumVersion: "1.2.0",
    category: "files",
    icon: "files",
    platforms: ["windows", "macos", "linux"],
    permissions: ["host", "storage"],
    extensionTargets: ["sftp.contextMenu", "commandPalette"],
    releaseDate: "2026-08-24",
  },
  {
    id: "dev.norishell.material-theme",
    name: "Material Light Theme",
    publisher: "NoriShell Labs",
    version: "1.0.0",
    minimumVersion: "1.1.0",
    category: "appearance",
    icon: "theme",
    platforms: ["windows", "macos", "linux"],
    permissions: ["storage"],
    extensionTargets: ["terminal.toolbar"],
    releaseDate: "2026-08-21",
  },
  {
    id: "dev.norishell.automation",
    name: "NoriShell Automation",
    publisher: "NoriShell",
    version: "1.4.0",
    minimumVersion: "1.2.0",
    category: "automation",
    icon: "automation",
    platforms: ["windows", "macos", "linux"],
    permissions: ["terminal", "host", "storage", "special"],
    extensionTargets: ["terminal.toolbar", "host.detail.tools", "commandPalette"],
    releaseDate: "2026-08-20",
  },
  {
    id: "dev.norishell.command-palette",
    name: "Command Palette",
    publisher: "NoriShell Labs",
    version: "1.2.0",
    minimumVersion: "1.2.0",
    category: "developer",
    icon: "command",
    platforms: ["windows", "macos", "linux"],
    permissions: ["storage"],
    extensionTargets: ["commandPalette"],
    releaseDate: "2026-08-18",
  },
  {
    id: "dev.norishell.host-monitor",
    name: "Host Monitor",
    publisher: "NoriShell Labs",
    version: "1.1.0",
    minimumVersion: "1.2.0",
    category: "monitoring",
    icon: "activity",
    platforms: ["windows", "macos", "linux"],
    permissions: ["host", "storage"],
    extensionTargets: ["overview.card.actions", "host.detail.tools"],
    releaseDate: "2026-08-15",
  },
  {
    id: "dev.norishell.sql-runner",
    name: "SQL Query Runner",
    publisher: "NoriShell Labs",
    version: "1.0.1",
    minimumVersion: "1.2.0",
    category: "developer",
    icon: "database",
    platforms: ["windows", "macos", "linux"],
    permissions: ["terminal", "storage"],
    extensionTargets: ["terminal.sidebar", "commandPalette"],
    releaseDate: "2026-08-12",
  },
  {
    id: "dev.norishell.json-formatter",
    name: "JSON Formatter",
    publisher: "NoriShell Labs",
    version: "1.0.0",
    minimumVersion: "1.1.0",
    category: "developer",
    icon: "code",
    platforms: ["windows", "macos", "linux"],
    permissions: ["storage"],
    extensionTargets: ["terminal.toolbar", "commandPalette"],
    releaseDate: "2026-08-08",
  },
  {
    id: "dev.norishell.git-integration",
    name: "Git Integration",
    publisher: "NoriShell Labs",
    version: "1.0.0",
    minimumVersion: "1.2.0",
    category: "developer",
    icon: "git",
    platforms: ["windows", "macos", "linux"],
    permissions: ["terminal", "storage"],
    extensionTargets: ["terminal.sidebar", "commandPalette"],
    releaseDate: "2026-08-04",
  },
];

const categories: MarketplaceCategory[] = ["terminal", "connection", "files", "appearance", "automation", "developer", "monitoring"];
const platforms: MarketplacePlatform[] = ["windows", "macos", "linux"];
const permissions: MarketplacePermission[] = ["terminal", "host", "storage", "special"];

const { t, locale } = useI18n();
const props = defineProps<{ busy?: boolean; preparingPluginId?: string | null }>();
const emit = defineEmits<{
  prepare: [entry: PluginCatalogEntryDto];
  manage: [installed: InstalledPluginSummary];
}>();
const plugins = usePluginsStore();
const pluginIcons = usePluginIconsStore();
const query = ref("");
const selectedCategory = ref<MarketplaceCategory | "all">("all");
const selectedPlatforms = ref<MarketplacePlatform[]>([]);
const selectedPermissions = ref<MarketplacePermission[]>([]);
const selectedPluginId = ref("");
const inspectorOpen = ref(true);
const showPermissionHelp = ref(false);
const showVersions = ref(false);
const selectedReleaseKey = ref("");
const detailsId = useId();
const visualFixtureMode = computed(() => {
  if (!import.meta.env.DEV) return false;
  const hashQuery = window.location.hash.split("?", 2)[1] ?? "";
  const params = new URLSearchParams(hashQuery || window.location.search);
  return params.get("visualFixture") === "plugins";
});

const highRiskCapabilities = new Set<PluginCapability>([
  ...specialPluginCapabilities,
  "terminalRequestInput",
]);

function categoryFor(entry: PluginCatalogEntryDto): MarketplaceCategory {
  if (entry.capabilities.includes("sshSync") || entry.capabilities.some((value) => value.startsWith("host"))) return "connection";
  if (entry.capabilities.some((value) => value.startsWith("sftp"))) return "files";
  if (entry.capabilities.includes("metricsRead")) return "monitoring";
  if (entry.capabilities.some((value) => value.startsWith("terminal"))) return "terminal";
  if (entry.capabilities.includes("uiHostCss")) return "appearance";
  return "developer";
}

function iconFor(category: MarketplaceCategory): MarketplaceIcon {
  if (category === "connection") return "keys";
  if (category === "files") return "files";
  if (category === "monitoring") return "activity";
  if (category === "terminal") return "terminal";
  if (category === "appearance") return "theme";
  if (category === "automation") return "automation";
  return "code";
}

function imageFor(plugin: MarketplacePlugin): string {
  return pluginIcons.imageFor(plugin.id, "catalog") ?? officialPluginIcons[plugin.id] ?? pluginPreviewImages[plugin.icon];
}

function permissionsFor(capabilities: PluginCapability[]): MarketplacePermission[] {
  const result = new Set<MarketplacePermission>();
  if (capabilities.some((value) => value.startsWith("terminal") || value === "clipboardWrite")) result.add("terminal");
  if (capabilities.some((value) => value.startsWith("host") || value === "sshSync")) result.add("host");
  if (capabilities.includes("storagePlugin")) result.add("storage");
  if (capabilities.some((value) => highRiskCapabilities.has(value))) result.add("special");
  return [...result];
}

function declaredPlatforms(platform: string): MarketplacePlatform[] {
  if (platform === "desktop" || platform === "all") return [...platforms];
  if (platform === "macos" || platform === "darwin") return ["macos"];
  if (platform === "windows") return ["windows"];
  if (platform === "linux") return ["linux"];
  return [];
}

function releaseKey(entry: PluginCatalogEntryDto) {
  return `${entry.version}:${entry.platform}:${entry.packageSha256}`;
}

function fromCatalogEntry(entry: PluginCatalogEntryDto): MarketplacePlugin {
  const category = categoryFor(entry);
  const timestamp = Number(entry.details?.releasePublishedAtUnixMs ?? 0);
  const date = timestamp > 0 ? new Date(timestamp) : null;
  return {
    id: entry.pluginId, name: entry.name, publisher: entry.publisher, version: entry.version,
    minimumVersion: entry.minimumAppVersion, category, icon: iconFor(category),
    platforms: declaredPlatforms(entry.platform), permissions: permissionsFor(entry.capabilities),
    extensionTargets: entry.details?.extensionTargets ?? [],
    releaseDate: date && !Number.isNaN(date.getTime()) ? date.toISOString().slice(0, 10) : "", entry,
  };
}

// The built-in Norixor product description fills only missing copy; release notes still come entirely from the signed catalog.
function descriptionFor(plugin: MarketplacePlugin) {
  return plugin.entry ? plugin.entry.details?.description
    || t(plugin.id === "org.norixor" ? "plugins.norixorDescription" : "plugins.marketplace.descriptionUnavailable")
    : t(`plugins.marketplace.descriptions.${plugin.icon}`);
}
function releaseNotesFor(plugin: MarketplacePlugin): string[] {
  return plugin.entry ? plugin.entry.details?.releaseNotes ?? [] : [t("plugins.marketplace.releaseNoteItems.feature"), t("plugins.marketplace.releaseNoteItems.improvement")];
}

const catalog = computed<MarketplacePlugin[]>(() => {
  if (visualFixtureMode.value) return visualFixtureCatalog;
  const latestByPlugin = new Map<string, PluginCatalogEntryDto>();
  for (const entry of plugins.catalog?.entries ?? []) {
    const current = latestByPlugin.get(entry.pluginId);
    if (!current || comparePluginVersions(entry.version, current.version) === 1) {
      latestByPlugin.set(entry.pluginId, entry);
    }
  }
  return [...latestByPlugin.values()].map(fromCatalogEntry)
    .sort((left, right) => Number(right.id === "org.norixor") - Number(left.id === "org.norixor"));
});

const extensionTargetLocaleKeys: Record<string, string> = {
  "plugins.page": "pluginsPage",
  "app.header.actions": "appHeaderActions",
  "app.content.before": "appContentBefore",
  "app.content.after": "appContentAfter",
  "app.content.sidebar": "appContentSidebar",
  "app.content.footer": "appContentFooter",
  "app.content.floating": "appContentFloating",
  "app.navigation": "appNavigation",
  "app.page": "appPage",
  "terminal.tools": "terminalTools",
  "terminal.header": "terminalHeader",
  "terminal.footer": "terminalFooter",
  "terminal.floating": "terminalFloating",
  "terminal.contextMenu": "terminalContextMenu",
  "terminal.annotation": "terminalAnnotation",
  "terminal.toolbar": "terminalToolbar",
  "terminal.sidebar": "terminalSidebar",
  "sftp.toolbar": "sftpToolbar",
  "sftp.contextMenu": "sftpContextMenu",
  "sftp.transfer.actions": "sftpTransferActions",
  "hosts.toolbar": "hostsToolbar",
  "host.detail.tools": "hostDetailTools",
  "overview.toolbar": "overviewToolbar",
  "overview.card.actions": "overviewCardActions",
  "tunnels.toolbar": "tunnelsToolbar",
  "settings.tools": "settingsTools",
  commandPalette: "commandPalette",
};
function extensionTargetLabel(target: string) {
  const key = extensionTargetLocaleKeys[target];
  return key ? t(`plugins.marketplace.extensionTargetLabels.${key}`) : target;
}

const categoryCounts = computed(() => Object.fromEntries(categories.map((category) => [
  category,
  catalog.value.filter((plugin) => plugin.category === category).length,
])) as Record<MarketplaceCategory, number>);

const filteredPlugins = computed(() => {
  const normalizedQuery = query.value.trim().toLocaleLowerCase();
  return catalog.value.filter((plugin) => {
    const matchesQuery = normalizedQuery.length === 0
      || `${plugin.name} ${plugin.publisher} ${plugin.id}`.toLocaleLowerCase().includes(normalizedQuery);
    const matchesCategory = selectedCategory.value === "all" || plugin.category === selectedCategory.value;
    const matchesPlatforms = selectedPlatforms.value.length === 0
      || selectedPlatforms.value.every((platform) => plugin.platforms.includes(platform));
    const matchesPermissions = selectedPermissions.value.length === 0
      || selectedPermissions.value.every((permission) => plugin.permissions.includes(permission));
    return matchesQuery && matchesCategory && matchesPlatforms && matchesPermissions;
  });
});

const selectedPlugin = computed(() => {
  if (!inspectorOpen.value) return null;
  return filteredPlugins.value.find((plugin) => plugin.id === selectedPluginId.value)
    ?? filteredPlugins.value[0]
    ?? null;
});

const catalogVersions = computed(() => (plugins.catalog?.entries ?? [])
  .filter((entry) => entry.pluginId === selectedPlugin.value?.id)
  .sort((left, right) => comparePluginVersions(right.version, left.version) ?? 0));
const inspectedPlugin = computed(() => {
  const selected = selectedPlugin.value;
  if (!selected) return null;
  const release = catalogVersions.value.find((entry) => releaseKey(entry) === selectedReleaseKey.value);
  return release ? fromCatalogEntry(release) : selected;
});
watch(() => selectedPlugin.value?.id, () => {
  selectedReleaseKey.value = "";
  showVersions.value = false;
  showPermissionHelp.value = false;
});
function formatDate(value: string) {
  if (!value) return t("plugins.marketplace.dateUnavailable");
  return new Intl.DateTimeFormat(locale.value, { dateStyle: "medium", timeZone: "UTC" }).format(new Date(value));
}
function packageSize(entry: PluginCatalogEntryDto) {
  return t("plugins.bytes", { count: new Intl.NumberFormat(locale.value).format(entry.packageSize) });
}

const ordinaryPermissions = computed(() => {
  if (!inspectedPlugin.value) return [];
  if (inspectedPlugin.value.entry) {
    return inspectedPlugin.value.entry.capabilities.filter((capability) => !highRiskCapabilities.has(capability));
  }
  const result: string[] = [];
  if (inspectedPlugin.value.permissions.includes("terminal")) result.push("terminalSession");
  if (inspectedPlugin.value.icon === "terminal") result.push("clipboard");
  if (inspectedPlugin.value.permissions.includes("host")) result.push("hostInfo");
  if (inspectedPlugin.value.permissions.includes("storage")) result.push("pluginStorage");
  return result;
});

const specialPermissions = computed(() => inspectedPlugin.value?.permissions.includes("special")
  ? inspectedPlugin.value.entry?.capabilities.filter((capability) => highRiskCapabilities.has(capability))
    ?? ["terminalInput", "hostUi"]
  : []);

function permissionLabel(permission: string) {
  return permission in {
    terminalSession: true,
    clipboard: true,
    hostInfo: true,
    pluginStorage: true,
    terminalInput: true,
    hostUi: true,
  }
    ? t(`plugins.marketplace.permissionDetails.${permission}`)
    : t(`plugins.capabilities.${permission}.label`);
}

function installedPlugin(plugin: MarketplacePlugin): InstalledPluginSummary | null {
  return plugins.installed.find((candidate) => candidate.pluginId === plugin.id) ?? null;
}

function isPreparing(plugin: MarketplacePlugin) {
  return props.preparingPluginId === plugin.id;
}

function installLabel(plugin: MarketplacePlugin) {
  const installed = installedPlugin(plugin);
  if (isPreparing(plugin)) return t("plugins.marketplace.preparing");
  if (installed?.activeVersion === plugin.version) return t(installed.packageSha256 === plugin.entry?.packageSha256
    ? "plugins.marketplace.installed" : "plugins.marketplace.packageDiffers");
  if (installed && comparePluginVersions(plugin.version, installed.activeVersion) === -1) return t("plugins.marketplace.newerInstalled");
  if (plugin.entry?.compatibility !== "compatible") return t("plugins.marketplace.incompatible");
  if (installed && comparePluginVersions(plugin.version, installed.activeVersion) === 1) return t("plugins.marketplace.update");
  return t("plugins.install");
}

function installDisabled(plugin: MarketplacePlugin) {
  const installed = installedPlugin(plugin);
  return !plugin.entry || plugin.entry.compatibility !== "compatible"
    || (installed !== null && comparePluginVersions(plugin.version, installed.activeVersion) !== 1)
    || props.busy || Boolean(props.preparingPluginId) || plugins.catalogRefreshing;
}

function installSelected() {
  const selected = inspectedPlugin.value;
  if (!selected?.entry || installDisabled(selected)) return;
  emit("prepare", selected.entry);
}

function toggleFilter<T extends string>(target: T[], value: T) {
  return target.includes(value) ? target.filter((item) => item !== value) : [...target, value];
}

function resetFilters() {
  query.value = "";
  selectedCategory.value = "all";
  selectedPlatforms.value = [];
  selectedPermissions.value = [];
}

function selectPlugin(pluginId: string) {
  selectedPluginId.value = pluginId;
  inspectorOpen.value = true;
  showPermissionHelp.value = false;
}
</script>

<template>
  <section
    class="plugin-marketplace"
    :class="{ 'plugin-marketplace--without-inspector': !inspectedPlugin }"
    :aria-label="t('plugins.marketplace.title')"
  >
    <aside class="marketplace-filters">
      <div class="marketplace-panel-heading">
        <h2>{{ t("plugins.marketplace.filters.title") }}</h2>
        <button
          class="marketplace-reset"
          type="button"
          @click="resetFilters"
        >
          {{ t("plugins.marketplace.filters.clear") }}
        </button>
      </div>

      <label class="marketplace-search">
        <NvxIcon
          :icon="Search"
          :size="16"
          aria-hidden="true"
        />
        <input
          v-model="query"
          type="search"
          :placeholder="t('plugins.marketplace.searchPlaceholder')"
        >
      </label>

      <div class="marketplace-filter-group">
        <h3>{{ t("plugins.marketplace.filters.category") }}</h3>
        <button
          class="marketplace-category"
          :class="{ 'marketplace-category--active': selectedCategory === 'all' }"
          type="button"
          @click="selectedCategory = 'all'"
        >
          <span>{{ t("plugins.marketplace.categories.all") }}</span><small>{{ catalog.length }}</small>
        </button>
        <button
          v-for="category in categories"
          :key="category"
          class="marketplace-category"
          :class="{ 'marketplace-category--active': selectedCategory === category }"
          type="button"
          @click="selectedCategory = category"
        >
          <span>{{ t(`plugins.marketplace.categories.${category}`) }}</span><small>{{ categoryCounts[category] }}</small>
        </button>
      </div>

      <div class="marketplace-filter-group">
        <h3>{{ t("plugins.marketplace.filters.platform") }}</h3>
        <label
          v-for="platform in platforms"
          :key="platform"
          class="marketplace-check"
        >
          <input
            type="checkbox"
            :checked="selectedPlatforms.includes(platform)"
            @change="selectedPlatforms = toggleFilter(selectedPlatforms, platform)"
          >
          <span>{{ t(`plugins.marketplace.platforms.${platform}`) }}</span>
        </label>
      </div>

      <div class="marketplace-filter-group">
        <h3>{{ t("plugins.marketplace.filters.permission") }}</h3>
        <label
          v-for="permission in permissions"
          :key="permission"
          class="marketplace-check"
        >
          <input
            type="checkbox"
            :checked="selectedPermissions.includes(permission)"
            @change="selectedPermissions = toggleFilter(selectedPermissions, permission)"
          >
          <span>{{ t(`plugins.marketplace.permissions.${permission}`) }}</span>
        </label>
      </div>

      <p class="marketplace-result-count">
        {{ t("plugins.marketplace.resultCount", { count: filteredPlugins.length }) }}
      </p>
    </aside>

    <section
      class="marketplace-results"
      aria-labelledby="marketplace-results-title"
    >
      <header class="marketplace-results__header">
        <h2 id="marketplace-results-title">
          {{ t("plugins.marketplace.listTitle") }}
        </h2>
        <NvxButton
          v-if="!visualFixtureMode"
          size="sm"
          variant="ghost"
          :disabled="plugins.catalogLoading || plugins.catalogRefreshing"
          :loading="plugins.catalogRefreshing"
          :loading-label="t('plugins.marketplace.refreshing')"
          @click="plugins.refreshCatalog"
        >
          <NvxIcon
            :icon="RefreshCw"
            :size="16"
            aria-hidden="true"
          />
          {{ plugins.catalogRefreshing ? t("plugins.marketplace.refreshing") : t("plugins.marketplace.refresh") }}
        </NvxButton>
      </header>

      <NvxInlineNotice
        v-if="!visualFixtureMode && plugins.catalogLoading"
        class="marketplace-state"
        tone="info"
        role="status"
      >
        {{ t("plugins.marketplace.loading") }}
      </NvxInlineNotice>
      <NvxInlineNotice
        v-else-if="!visualFixtureMode && plugins.catalogErrorCode"
        class="marketplace-state"
        tone="error"
        role="alert"
      >
        <span>{{ t(`plugins.errors.${plugins.catalogErrorCode}`) }}</span>
        <NvxButton
          size="sm"
          variant="ghost"
          :disabled="plugins.catalogRefreshing"
          :loading="plugins.catalogRefreshing"
          @click="plugins.refreshCatalog"
        >
          {{ t("plugins.marketplace.retry") }}
        </NvxButton>
      </NvxInlineNotice>
      <p
        v-else-if="!filteredPlugins.length"
        class="marketplace-empty"
        role="status"
      >
        {{ t("plugins.marketplace.empty") }}
      </p>
      <div
        v-else
        class="marketplace-list"
        role="listbox"
        :aria-label="t('plugins.marketplace.listTitle')"
      >
        <button
          v-for="plugin in filteredPlugins"
          :key="plugin.id"
          class="marketplace-plugin"
          :class="{ 'marketplace-plugin--selected': selectedPlugin?.id === plugin.id }"
          type="button"
          role="option"
          :aria-selected="selectedPlugin?.id === plugin.id"
          @click="selectPlugin(plugin.id)"
        >
          <span
            class="marketplace-plugin__select"
            aria-hidden="true"
          />
          <span class="marketplace-plugin__icon">
            <img
              :key="imageFor(plugin)"
              :src="imageFor(plugin)"
              alt=""
              @error="pluginIcons.discardImage(plugin.id, 'catalog', ($event.target as HTMLImageElement).src)"
            >
          </span>
          <span class="marketplace-plugin__copy">
            <strong>{{ plugin.name }}</strong>
            <small>{{ plugin.publisher }}</small>
            <span>{{ descriptionFor(plugin) }}</span>
          </span>
          <span class="marketplace-plugin__version">
            {{ plugin.version }}
            <small v-if="installedPlugin(plugin)">{{ t(`plugins.installStates.${installedPlugin(plugin)?.state}`) }}</small>
          </span>
          <span
            class="marketplace-plugin__platforms"
            :aria-label="t('plugins.marketplace.compatiblePlatforms')"
          >
            <span
              v-for="platform in plugin.platforms"
              :key="platform"
              :title="t(`plugins.marketplace.platforms.${platform}`)"
            >
              <img
                :src="platformIcons[platform]"
                :alt="t(`plugins.marketplace.platforms.${platform}`)"
              >
            </span>
          </span>
          <span
            class="marketplace-plugin__permission-dots"
            :aria-label="t('plugins.marketplace.permissionSummary')"
          >
            <i
              v-for="permission in plugin.permissions"
              :key="permission"
              :class="{ 'is-special': permission === 'special' }"
            />
          </span>
        </button>
      </div>
    </section>

    <aside
      v-if="inspectedPlugin"
      class="marketplace-inspector"
      :aria-label="t('plugins.marketplace.detailsTitle')"
    >
      <template v-if="inspectedPlugin">
        <NvxIconButton
          class="marketplace-inspector__close"
          size="sm"
          :label="t('plugins.marketplace.closeDetails')"
          @click="inspectorOpen = false"
        >
          <NvxIcon
            :icon="X"
            :size="20"
            aria-hidden="true"
          />
        </NvxIconButton>
        <header class="marketplace-inspector__identity">
          <span class="marketplace-inspector__icon">
            <img
              :key="imageFor(inspectedPlugin)"
              :src="imageFor(inspectedPlugin)"
              alt=""
              @error="pluginIcons.discardImage(inspectedPlugin.id, 'catalog', ($event.target as HTMLImageElement).src)"
            >
          </span>
          <div>
            <h2>{{ inspectedPlugin.name }}</h2>
            <p>{{ inspectedPlugin.publisher }}</p>
          </div>
        </header>

        <p class="marketplace-inspector__description">
          {{ descriptionFor(inspectedPlugin) }}
        </p>

        <dl class="marketplace-facts">
          <div><dt>{{ t("plugins.version") }}</dt><dd>{{ inspectedPlugin.version }}</dd></div>
          <div>
            <dt>{{ t("plugins.platform") }}</dt><dd class="marketplace-fact-platforms">
              <span
                v-for="platform in inspectedPlugin.platforms"
                :key="platform"
                :title="t(`plugins.marketplace.platforms.${platform}`)"
              >
                <img
                  :src="platformIcons[platform]"
                  :alt="t(`plugins.marketplace.platforms.${platform}`)"
                >
              </span>
            </dd>
          </div>
          <div><dt>{{ t("plugins.marketplace.minimumVersion") }}</dt><dd>NoriShell {{ inspectedPlugin.minimumVersion }}+</dd></div>
          <template v-if="inspectedPlugin.entry">
            <div><dt>{{ t("plugins.marketplace.compatibilityLabel") }}</dt><dd>{{ t(`plugins.compatibility.${inspectedPlugin.entry.compatibility}`) }}</dd></div>
            <div><dt>{{ t("plugins.packageSize") }}</dt><dd>{{ packageSize(inspectedPlugin.entry) }}</dd></div>
            <div>
              <dt>{{ t("plugins.marketplace.packageId") }}</dt><dd class="marketplace-package-id">
                {{ inspectedPlugin.id }}
              </dd>
            </div>
          </template>
          <template v-if="installedPlugin(inspectedPlugin)">
            <div><dt>{{ t("plugins.marketplace.installedVersion") }}</dt><dd>{{ installedPlugin(inspectedPlugin)?.activeVersion }}</dd></div>
            <div><dt>{{ t("plugins.columns.state") }}</dt><dd>{{ t(`plugins.installStates.${installedPlugin(inspectedPlugin)?.state}`) }}</dd></div>
          </template>
        </dl>

        <section class="marketplace-inspector__section">
          <h3>{{ t("plugins.marketplace.requiredPermissions") }}</h3>
          <p v-if="!ordinaryPermissions.length && !specialPermissions.length && !inspectedPlugin.entry?.unsupportedCapabilities?.length">
            {{ t("plugins.noCapabilities") }}
          </p>
          <h4 v-if="ordinaryPermissions.length">
            {{ t("plugins.marketplace.ordinaryCapabilities") }}
          </h4>
          <ul class="marketplace-permission-list">
            <li
              v-for="permission in ordinaryPermissions"
              :key="permission"
            >
              <div>
                <span>{{ permissionLabel(permission) }}</span>
                <small
                  v-if="showPermissionHelp && inspectedPlugin.entry"
                  class="marketplace-permission-description"
                >{{ t(`plugins.capabilities.${permission}.description`) }}</small>
              </div>
            </li>
          </ul>
          <template v-if="specialPermissions.length">
            <h4 class="marketplace-special-heading">
              {{ t("plugins.marketplace.specialCapabilities") }}
            </h4>
            <ul class="marketplace-permission-list marketplace-permission-list--special">
              <li
                v-for="permission in specialPermissions"
                :key="permission"
              >
                <NvxIcon
                  :icon="ShieldAlert"
                  :size="16"
                  aria-hidden="true"
                />
                <div>
                  <span>{{ permissionLabel(permission) }}</span>
                  <small
                    v-if="showPermissionHelp && inspectedPlugin.entry"
                    class="marketplace-permission-description"
                  >{{ t(`plugins.capabilities.${permission}.description`) }}</small>
                </div>
              </li>
            </ul>
          </template>
          <template v-if="inspectedPlugin.entry?.unsupportedCapabilities?.length">
            <h4>{{ t("plugins.marketplace.unsupportedCapabilities") }}</h4>
            <ul class="marketplace-permission-list">
              <li
                v-for="capability in inspectedPlugin.entry.unsupportedCapabilities"
                :key="capability"
              >
                {{ capability }}
              </li>
            </ul>
          </template>
          <button
            class="marketplace-learn-permissions"
            type="button"
            :aria-expanded="showPermissionHelp"
            :aria-controls="`${detailsId}-permission-help`"
            @click="showPermissionHelp = !showPermissionHelp"
          >
            {{ t("plugins.marketplace.learnPermissions") }}
          </button>
          <p
            v-if="showPermissionHelp"
            :id="`${detailsId}-permission-help`"
            class="marketplace-security-note"
          >
            {{ t("plugins.marketplace.vaultBoundary") }}
          </p>
        </section>

        <section class="marketplace-inspector__section">
          <h3>{{ t("plugins.marketplace.extensionTargets") }}</h3>
          <p v-if="!inspectedPlugin.extensionTargets.length">
            {{ t("plugins.marketplace.locationsUnavailable") }}
          </p>
          <ul
            v-else
            class="marketplace-target-list"
          >
            <li
              v-for="target in inspectedPlugin.extensionTargets"
              :key="target"
            >
              {{ extensionTargetLabel(target) }}
            </li>
          </ul>
        </section>

        <section class="marketplace-inspector__section">
          <h3>{{ t("plugins.marketplace.releaseNotes") }}</h3>
          <div class="marketplace-release-meta">
            <strong>{{ inspectedPlugin.version }}</strong>
            <span v-if="inspectedPlugin.releaseDate">{{ t("plugins.marketplace.publishedOn") }} <time :datetime="inspectedPlugin.releaseDate">{{ formatDate(inspectedPlugin.releaseDate) }}</time></span>
          </div>
          <ul
            v-if="releaseNotesFor(inspectedPlugin).length"
            class="marketplace-release-list"
          >
            <li
              v-for="(note, index) in releaseNotesFor(inspectedPlugin)"
              :key="index"
            >
              {{ note }}
            </li>
          </ul>
          <p
            v-else
            class="marketplace-release-empty"
          >
            {{ t("plugins.marketplace.releaseNotesUnavailable") }}
          </p>
          <button
            v-if="catalogVersions.length"
            class="marketplace-version-link"
            type="button"
            :aria-expanded="showVersions"
            :aria-controls="`${detailsId}-versions`"
            @click="showVersions = !showVersions"
          >
            {{ t("plugins.marketplace.viewAllVersions", { count: catalogVersions.length }) }}
          </button>
          <div
            v-if="showVersions"
            :id="`${detailsId}-versions`"
            class="marketplace-versions"
          >
            <p>{{ t("plugins.marketplace.versionHistoryScope") }}</p>
            <button
              v-for="entry in catalogVersions"
              :key="releaseKey(entry)"
              type="button"
              class="marketplace-version"
              :aria-pressed="releaseKey(entry) === releaseKey(inspectedPlugin.entry!)"
              @click="selectedReleaseKey = releaseKey(entry)"
            >
              <span><strong>{{ entry.version }}</strong><small>{{ t(`plugins.compatibility.${entry.compatibility}`) }}</small></span>
              <small>{{ t("plugins.marketplace.minimumVersion") }} {{ entry.minimumAppVersion }} · {{ packageSize(entry) }}</small>
            </button>
          </div>
        </section>

        <footer class="marketplace-inspector__actions">
          <span class="marketplace-install-pending">
            <NvxButton
              size="sm"
              :disabled="installDisabled(inspectedPlugin)"
              :loading="isPreparing(inspectedPlugin)"
              :loading-label="t('plugins.marketplace.preparing')"
              @click="installSelected"
            >{{ installLabel(inspectedPlugin) }}</NvxButton>
          </span>
          <NvxButton
            v-if="installedPlugin(inspectedPlugin)"
            size="sm"
            variant="ghost"
            :disabled="busy"
            @click="emit('manage', installedPlugin(inspectedPlugin)!)"
          >
            {{ t("plugins.marketplace.manageInstalled") }}
          </NvxButton>
          <small
            v-if="inspectedPlugin.entry && inspectedPlugin.entry.compatibility !== 'compatible'"
            class="marketplace-action-note"
          >{{ t(`plugins.compatibility.${inspectedPlugin.entry.compatibility}`) }}</small>
        </footer>
      </template>
    </aside>
  </section>
</template>

<style scoped>
.plugin-marketplace {
  display: grid;
  grid-template-columns: 280px minmax(420px, 1fr) 360px;
  min-height: 0;
  flex: 1;
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.plugin-marketplace--without-inspector {
  grid-template-columns: 280px minmax(0, 1fr);
}

.marketplace-filters,
.marketplace-results,
.marketplace-inspector {
  min-width: 0;
  min-height: 0;
}

.marketplace-filters {
  display: flex;
  flex-direction: column;
  padding: var(--nvx-space-4);
  border-inline-end: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-canvas);
}

.marketplace-panel-heading,
.marketplace-results__header {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  justify-content: space-between;
}

.marketplace-panel-heading h2,
.marketplace-results__header h2,
.marketplace-inspector h2,
.marketplace-filter-group h3,
.marketplace-inspector__section h3,
.marketplace-inspector p {
  margin: 0;
}

.marketplace-panel-heading h2,
.marketplace-results__header h2 {
  font-size: var(--nvx-font-size-body);
}

.marketplace-reset {
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--nvx-color-accent);
  font: inherit;
  font-size: var(--nvx-font-size-xs);
  cursor: pointer;
}

.marketplace-reset:focus-visible,
.marketplace-category:focus-visible,
.marketplace-plugin:focus-visible,
.marketplace-search:focus-within {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 1px;
}

.marketplace-search {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
  height: var(--nvx-control-height-sm);
  margin-top: var(--nvx-space-4);
  padding: 0 var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-tertiary);
}

.marketplace-search input {
  min-width: 0;
  width: 100%;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--nvx-color-text-primary);
  font: inherit;
  font-size: var(--nvx-font-size-xs);
}

.marketplace-filter-group {
  display: grid;
  gap: 2px;
  margin-top: var(--nvx-space-5);
}

.marketplace-filter-group h3,
.marketplace-inspector__section h3 {
  margin-bottom: var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  font-weight: var(--nvx-font-weight-semibold);
}

.marketplace-category {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 30px;
  padding: 0 var(--nvx-space-3);
  border: 0;
  border-radius: var(--nvx-radius-sm);
  background: transparent;
  color: var(--nvx-color-text-secondary);
  font: inherit;
  font-size: var(--nvx-font-size-sm);
  text-align: start;
  cursor: pointer;
}

.marketplace-category:hover,
.marketplace-category--active {
  background: var(--nvx-color-accent-soft);
  color: var(--nvx-color-accent);
}

.marketplace-category small {
  font-size: var(--nvx-font-size-xs);
}

.marketplace-check {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
  min-height: 28px;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  cursor: pointer;
}

.marketplace-check input {
  width: 15px;
  height: 15px;
  margin: 0;
  accent-color: var(--nvx-color-accent);
}

.marketplace-result-count {
  margin: auto 0 0;
  padding-top: var(--nvx-space-5);
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-results {
  overflow: auto;
}

.marketplace-results__header {
  position: sticky;
  z-index: var(--nvx-z-sticky);
  top: 0;
  min-height: 48px;
  padding: 0 var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.marketplace-results__header span {
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-list {
  display: grid;
}

.marketplace-plugin {
  display: grid;
  grid-template-columns: 16px 38px minmax(180px, 1fr) 58px 92px 42px;
  gap: var(--nvx-space-3);
  align-items: center;
  min-height: 72px;
  padding: var(--nvx-space-2) var(--nvx-space-4);
  border: 0;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
  color: var(--nvx-color-text-primary);
  font: inherit;
  text-align: start;
  cursor: pointer;
}

.marketplace-plugin:hover {
  background: var(--nvx-color-bg-hover);
}

.marketplace-plugin--selected {
  background: var(--nvx-color-accent-soft);
}

.marketplace-plugin__select {
  width: 10px;
  height: 10px;
  box-sizing: border-box;
  border: var(--nvx-border-width) solid var(--nvx-color-border-strong);
  border-radius: 50%;
}

.marketplace-plugin--selected .marketplace-plugin__select {
  border: 3px solid var(--nvx-color-accent);
}

.marketplace-plugin__icon,
.marketplace-inspector__icon {
  display: inline-flex;
  overflow: hidden;
  align-items: center;
  justify-content: center;
  border-radius: var(--nvx-radius-md);
}

.marketplace-plugin__icon {
  width: 38px;
  height: 38px;
}

.marketplace-plugin__icon img,
.marketplace-inspector__icon img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.marketplace-plugin__copy {
  display: grid;
  min-width: 0;
}

.marketplace-plugin__copy strong,
.marketplace-plugin__copy small,
.marketplace-plugin__copy > span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.marketplace-plugin__copy strong {
  font-size: var(--nvx-font-size-sm);
}

.marketplace-plugin__copy small,
.marketplace-plugin__copy > span,
.marketplace-plugin__version,
.marketplace-plugin__platforms {
  color: var(--nvx-color-text-tertiary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-plugin__version {
  display: grid;
  gap: 2px;
}

.marketplace-plugin__version small {
  color: var(--nvx-color-text-secondary);
  font-size: 10px;
}

.marketplace-plugin__platforms {
  display: flex;
  gap: 9px;
  align-items: center;
  justify-content: center;
}

.marketplace-plugin__platforms > span,
.marketplace-fact-platforms > span {
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.marketplace-plugin__platforms img,
.marketplace-fact-platforms img {
  display: block;
  width: 16px;
  height: 16px;
  object-fit: contain;
  filter: grayscale(1);
  opacity: 0.38;
}

.marketplace-fact-platforms {
  display: flex;
  gap: 9px;
  align-items: center;
}

:global([data-theme="dark"] .marketplace-plugin__platforms img),
:global([data-theme="dark"] .marketplace-fact-platforms img) {
  filter: grayscale(1) invert(1);
  opacity: 0.44;
}

.marketplace-plugin__permission-dots {
  display: flex;
  gap: 3px;
  justify-content: flex-end;
}

.marketplace-plugin__permission-dots i {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--nvx-color-success);
}

.marketplace-plugin__permission-dots i.is-special {
  background: var(--nvx-color-warning);
}

.marketplace-empty {
  margin: var(--nvx-space-6);
  color: var(--nvx-color-text-secondary);
}

.marketplace-state {
  margin: var(--nvx-space-4);
}

.marketplace-inspector {
  position: relative;
  overflow: auto;
  padding: var(--nvx-space-4);
  border-inline-start: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-canvas);
}

.marketplace-inspector__close {
  position: absolute;
  top: var(--nvx-space-3);
  right: var(--nvx-space-3);
}

.marketplace-inspector__identity {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: center;
  padding-inline-end: var(--nvx-space-8);
}

.marketplace-inspector__icon {
  width: 48px;
  height: 48px;
}

.marketplace-inspector h2 {
  font-size: var(--nvx-font-size-md);
  line-height: var(--nvx-line-height-md);
}

.marketplace-inspector__identity p,
.marketplace-inspector__description,
.marketplace-inspector__section p {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-sm);
  line-height: var(--nvx-line-height-sm);
}

.marketplace-inspector__description {
  margin-top: var(--nvx-space-3) !important;
}

.marketplace-facts {
  display: grid;
  gap: var(--nvx-space-1);
  margin: var(--nvx-space-4) 0 0;
  padding: var(--nvx-space-3) 0;
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
}

.marketplace-facts > div {
  display: grid;
  grid-template-columns: 92px minmax(0, 1fr);
  gap: var(--nvx-space-3);
}

.marketplace-facts dt,
.marketplace-facts dd {
  margin: 0;
  font-size: var(--nvx-font-size-xs);
}

.marketplace-facts dt {
  color: var(--nvx-color-text-tertiary);
}

.marketplace-facts dd {
  color: var(--nvx-color-text-secondary);
}

.marketplace-inspector__section {
  margin-top: var(--nvx-space-4);
}

.marketplace-inspector__section h4 {
  margin: 0 0 var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
  font-weight: var(--nvx-font-weight-medium);
}

.marketplace-inspector__section .marketplace-special-heading {
  margin-top: var(--nvx-space-3);
  color: var(--nvx-color-warning);
}

.marketplace-permission-list,
.marketplace-target-list,
.marketplace-release-list {
  display: grid;
  gap: var(--nvx-space-1);
  margin: 0;
  padding-inline-start: var(--nvx-space-5);
}

.marketplace-permission-list li {
  display: flex;
  gap: var(--nvx-space-2);
  align-items: center;
  min-height: 20px;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-permission-list--special {
  padding: 0;
  list-style: none;
}

.marketplace-permission-list--special li {
  color: var(--nvx-color-warning);
}

.marketplace-learn-permissions,
.marketplace-version-link {
  margin-top: var(--nvx-space-2);
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--nvx-color-accent);
  font: inherit;
  font-size: var(--nvx-font-size-xs);
  cursor: pointer;
}

.marketplace-learn-permissions:focus-visible,
.marketplace-version-link:focus-visible {
  border-radius: var(--nvx-radius-sm);
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: 2px;
}

.marketplace-security-note {
  margin-top: var(--nvx-space-3) !important;
  padding: var(--nvx-space-3);
  border: var(--nvx-border-width) solid var(--nvx-color-border);
  border-radius: var(--nvx-radius-md);
  background: var(--nvx-color-bg-surface);
}

.marketplace-target-list {
  grid-template-columns: 1fr;
}

.marketplace-target-list li {
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-release-meta {
  display: flex;
  gap: var(--nvx-space-3);
  align-items: baseline;
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-release-meta time {
  color: var(--nvx-color-text-tertiary);
}

.marketplace-release-list {
  margin-top: var(--nvx-space-2);
  color: var(--nvx-color-text-secondary);
  font-size: var(--nvx-font-size-xs);
}

.marketplace-install-pending {
  display: grid;
  gap: 2px;
  width: 100%;
  text-align: center;
}

.marketplace-install-pending small {
  color: var(--nvx-color-text-tertiary);
  font-size: 10px;
}

.marketplace-inspector__actions {
  position: sticky;
  bottom: calc(-1 * var(--nvx-space-4));
  z-index: 1;
  display: grid;
  gap: var(--nvx-space-2);
  padding-bottom: var(--nvx-space-4);
  background: var(--nvx-color-bg-canvas);
  margin-top: var(--nvx-space-5);
  padding-top: var(--nvx-space-3);
  border-top: var(--nvx-border-width) solid var(--nvx-color-border);
}

.marketplace-install-pending > :deep(.nvx-button) {
  width: 100%;
}

.marketplace-package-id { overflow-wrap: anywhere; }
.marketplace-permission-description { display: block; margin-top: 3px; color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); line-height: var(--nvx-line-height-sm); }
.marketplace-permission-list--special li { align-items: flex-start; }
.marketplace-permission-list--special li > .nvx-icon { flex: none; margin-top: 2px; }
.marketplace-release-meta { flex-wrap: wrap; gap: 4px 12px; }
.marketplace-release-empty { margin-top: var(--nvx-space-2) !important; }
.marketplace-versions { display: grid; gap: 6px; margin-top: var(--nvx-space-3); max-height: 240px; overflow: auto; }
.marketplace-version { display: grid; gap: 4px; width: 100%; padding: 8px; border: var(--nvx-border-width) solid var(--nvx-color-border); border-radius: var(--nvx-radius-sm); background: var(--nvx-color-bg-surface); color: var(--nvx-color-text-primary); font: inherit; text-align: start; cursor: pointer; }
.marketplace-version[aria-pressed="true"] { border-color: var(--nvx-color-accent); }
.marketplace-version > span { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 4px 8px; }
.marketplace-version small, .marketplace-action-note { color: var(--nvx-color-text-secondary); font-size: var(--nvx-font-size-xs); }

@media (max-width: 1180px) {
  .plugin-marketplace {
    grid-template-columns: 224px minmax(390px, 1fr) 300px;
  }

  .plugin-marketplace--without-inspector {
    grid-template-columns: 224px minmax(0, 1fr);
  }

  .marketplace-plugin {
    grid-template-columns: 16px 38px minmax(160px, 1fr) 54px 40px;
  }

  .marketplace-plugin__platforms {
    display: none;
  }
}

@media (max-width: 980px) {
  .plugin-marketplace {
    grid-template-columns: 200px minmax(0, 1fr);
  }

  .marketplace-inspector {
    grid-column: 1 / -1;
    max-height: 360px;
    border-top: var(--nvx-border-width) solid var(--nvx-color-border);
    border-inline-start: 0;
  }
}

@media (max-width: 700px) {
  .plugin-marketplace {
    display: block;
    overflow: auto;
  }

  .marketplace-filters,
  .marketplace-inspector {
    border: 0;
  }

  .marketplace-result-count,
  .marketplace-filter-group:nth-of-type(n + 3) {
    display: none;
  }

  .marketplace-plugin {
    grid-template-columns: 14px 36px minmax(0, 1fr) 50px;
  }

  .marketplace-plugin__permission-dots,
  .marketplace-plugin__platforms {
    display: none;
  }
}
</style>
