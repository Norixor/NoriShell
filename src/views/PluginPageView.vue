<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import { NvxPluginExtensionTarget } from "../components/plugins";
import NvxPluginSettingsDialog from "../components/plugins/NvxPluginSettingsDialog.vue";
import { NvxInlineNotice } from "../components/ui";
import { listInstalledPlugins } from "../core-api/client";
import { usePluginExtensionsStore } from "../stores/pluginExtensions";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useRouteReveal } from "../routeReveal";
import { holdLocalWorkspaceTabClaim } from "../workspace-tab-transfer";
import { focusWorkspaceWindowTarget, registerWorkspaceTab, snapshotWorkspaceTabs, unregisterWorkspaceTab, workspaceWindowLabel } from "../workspace-tab-windows";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
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
  if (isTauri()) {
    const groupId = `page:plugin:${item.pluginId}:${item.navigation.pageId}`;
    const tab = {
      groupId,
      pageType: "plugin" as const,
      route: `/plugin/${encodeURIComponent(item.pluginId)}/${encodeURIComponent(item.navigation.pageId)}`,
      labelKey: null,
      label: item.navigation.label,
      iconName: item.navigation.icon,
    };
    const state = await snapshotWorkspaceTabs();
    if (!isCurrentPage(key)) return;
    const otherOwner = state.others.find((record) => record.id === groupId)?.owner;
    if (otherOwner || state.outgoing.some((entry) => entry.tab.id === groupId)
      || state.incoming.some((entry) => entry.tab.id === groupId)) {
      if (otherOwner) await focusWorkspaceWindowTarget(otherOwner).catch(() => undefined);
      if (isCurrentPage(key)) await router.replace(workspaceWindowLabel() === "main" ? "/terminal" : "/workspace-window");
      return;
    }
    const owned = state.owned.find((record) => record.id === groupId);
    if (owned && owned.kind !== "page") throw new Error("workspace_tab.kind_conflict");
    if (!owned) {
      // Claim ownership before adding a visible Tab; Core rejects a concurrent window.
      const releaseClaim = holdLocalWorkspaceTabClaim(groupId);
      try {
        let registered: Awaited<ReturnType<typeof registerWorkspaceTab>>;
        try {
          registered = await registerWorkspaceTab({ id: groupId, kind: "page", payload: tab });
        } catch (error) {
          const latest = await snapshotWorkspaceTabs().catch(() => null);
          const currentOwner = latest?.others.find((record) => record.id === groupId)?.owner;
          if (currentOwner) {
            await focusWorkspaceWindowTarget(currentOwner).catch(() => undefined);
            if (isCurrentPage(key)) await router.replace(workspaceWindowLabel() === "main" ? "/terminal" : "/workspace-window");
            return;
          }
          throw error;
        }
        if (!isCurrentPage(key)) {
          await unregisterWorkspaceTab(groupId, registered.revision).catch(() => undefined);
          return;
        }
        workspaceTabs.ensurePluginPageTab(item);
        pageReady.value = true;
        return;
      } finally {
        releaseClaim();
      }
    }
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
  await openOwnedPage();
}

function handlePermissionChanged() {
  void loadPermissionState().catch(() => undefined);
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
  void load().catch((error: unknown) => {
    if (disposed) return;
    pageError.value = pageFailureCode(error);
  }).finally(revealRoute);
});
onBeforeUnmount(() => {
  disposed = true;
  window.removeEventListener("norishell:plugin-special-permission-changed", handlePermissionChanged);
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
.plugin-page { display: grid; align-content: start; gap: var(--nvx-space-4); min-width: 0; padding: var(--nvx-space-6); }
.plugin-page__surface { display: grid; min-width: 0; gap: var(--nvx-space-4); }
</style>
