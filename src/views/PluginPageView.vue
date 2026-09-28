<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";

import { NvxPluginExtensionTarget } from "../components/plugins";
import NvxPluginSettingsDialog from "../components/plugins/NvxPluginSettingsDialog.vue";
import { NvxInlineNotice } from "../components/ui";
import { listInstalledPlugins } from "../core-api/client";
import { usePluginExtensionsStore } from "../stores/pluginExtensions";
import { pluginPageTab, useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useRouteReveal } from "../routeReveal";
import { isWorkspaceTabView } from "../workspace-window-context";
import { snapshotWorkspaceTabs } from "../workspace-tab-windows";

const { t } = useI18n();
const route = useRoute();
const extensions = usePluginExtensionsStore();
const workspaceTabs = useWorkspaceTabsStore();
const revealRoute = useRouteReveal();
const pluginId = computed(() => String(route.params.pluginId ?? ""));
const pageId = computed(() => String(route.params.pageId ?? ""));
const navigationItem = computed(() => extensions.navigation.find((item) => (
  item.pluginId === pluginId.value && item.navigation.pageId === pageId.value
)) ?? null);
const instanceKey = computed(() => `${pluginId.value}|${pageId.value}`);
const pageInstanceKey = instanceKey.value;
const sshSyncPermissionDenied = ref(false);
const settingsTarget = ref<{ pluginId: string; pageId: string; pluginName: string; fieldKey: string } | null>(null);
const pluginTarget = ref<InstanceType<typeof NvxPluginExtensionTarget> | null>(null);
const pageReady = ref(false);
const pageError = ref<string | null>(null);
let disposed = false;

function isCurrentPage(key: string): boolean {
  return !disposed && key === pageInstanceKey && instanceKey.value === key;
}

function pageFailureCode(error: unknown): string {
  const code = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return /^workspace_tab\.[a-z_]+$/.test(code) ? code : "workspace_tab.unavailable";
}

async function openOwnedPage() {
  const key = instanceKey.value;
  const item = navigationItem.value;
  if (!item || !isCurrentPage(key)) return;
  // Only the page's own Tab WebView renders it; the router opens shells' requests as a Page Tab.
  if (isTauri()) {
    const owned = (await snapshotWorkspaceTabs()).owned.find((record) => record.id === pluginPageTab(item).groupId);
    if (!owned) throw new Error("workspace_tab.not_found");
    if (owned.kind !== "page") throw new Error("workspace_tab.kind_conflict");
  }
  if (!isCurrentPage(key)) return;
  workspaceTabs.ensurePluginPageTab(item);
  pageReady.value = true;
}

function openSettings(plugin: string, pluginName: string, fieldKey: string) {
  if (!navigationItem.value || plugin !== pluginId.value) return;
  settingsTarget.value = { pluginId: plugin, pageId: pageId.value, pluginName, fieldKey };
}

function refreshAfterSettingsSaved(plugin: string) {
  if (plugin === pluginId.value) {
    void pluginTarget.value?.refreshAfterSettingsChange(plugin).catch(() => undefined);
  }
}

async function loadPermissionState() {
  const installed = (await listInstalledPlugins()).find((plugin) => plugin.pluginId === pluginId.value);
  sshSyncPermissionDenied.value = installed?.capabilities.includes("sshSync") === true
    && !installed.grants.some((grant) => grant.capability === "sshSync" && grant.granted);
}

async function load() {
  const key = instanceKey.value;
  await Promise.all([
    extensions.loadNavigation(),
    loadPermissionState().catch(() => { sshSyncPermissionDenied.value = false; }),
  ]);
  if (!isCurrentPage(key)) return;
  if (!navigationItem.value && isWorkspaceTabView()) {
    window.dispatchEvent(new CustomEvent("norishell:plugin-page-unavailable", { detail: pluginId.value }));
    return;
  }
  await openOwnedPage();
}

function handlePermissionChanged() {
  void loadPermissionState().catch(() => undefined);
}

function retryAfterTabMove() {
  if (isWorkspaceTabView() && !pageReady.value && !disposed) {
    void load().catch((error: unknown) => { pageError.value = pageFailureCode(error); });
  }
}

watch([pluginId, pageId, navigationItem], () => {
  if (settingsTarget.value && (!navigationItem.value || settingsTarget.value.pluginId !== pluginId.value
    || settingsTarget.value.pageId !== pageId.value)) {
    settingsTarget.value = null;
  }
  if (navigationItem.value && pageReady.value && isCurrentPage(pageInstanceKey)) {
    workspaceTabs.ensurePluginPageTab(navigationItem.value);
  }
});
onMounted(() => {
  window.addEventListener("norishell:plugin-special-permission-changed", handlePermissionChanged);
  window.addEventListener("norishell:workspace-tab-context-changed", retryAfterTabMove);
  void load().catch((error: unknown) => {
    if (disposed) return;
    pageError.value = pageFailureCode(error);
  }).finally(revealRoute);
});
onBeforeUnmount(() => {
  disposed = true;
  window.removeEventListener("norishell:plugin-special-permission-changed", handlePermissionChanged);
  window.removeEventListener("norishell:workspace-tab-context-changed", retryAfterTabMove);
});
</script>

<template>
  <main class="plugin-page">
    <template v-if="navigationItem && pageReady">
      <NvxInlineNotice
        v-if="sshSyncPermissionDenied"
        tone="warning"
        :title="t('plugins.page.sshSyncPermissionRequiredTitle')"
      >
        {{ t("plugins.page.sshSyncPermissionRequiredDescription", { plugin: navigationItem.pluginName }) }}
      </NvxInlineNotice>
      <section class="plugin-page__surface">
        <NvxPluginExtensionTarget
          ref="pluginTarget"
          target-id="app.page"
          :instance-key="instanceKey"
          :display-label="navigationItem.navigation.label"
          :show-identity="false"
          @open-settings="openSettings"
        />
      </section>
      <NvxPluginSettingsDialog
        v-if="settingsTarget"
        :plugin-id="settingsTarget.pluginId"
        :plugin-name="settingsTarget.pluginName"
        :initial-field-key="settingsTarget.fieldKey"
        @saved="refreshAfterSettingsSaved($event.pluginId)"
        @close="settingsTarget = null"
      />
    </template>
    <NvxInlineNotice
      v-else-if="pageError || !navigationItem"
      tone="warning"
      :title="t('plugins.page.unavailable')"
    >
      {{ pageError }}
    </NvxInlineNotice>
  </main>
</template>

<style scoped>
/* A Page Tab WebView clips its root; the page owns vertical scrolling. */
.plugin-page { display: grid; align-content: start; gap: var(--nvx-space-4); min-width: 0; min-height: 0; overflow-y: auto; padding: var(--nvx-space-6); }
.plugin-page__surface { display: grid; min-width: 0; gap: var(--nvx-space-4); }
</style>
