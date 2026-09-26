<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";

import { NvxPluginExtensionTarget } from "../components/plugins";
import NvxPluginSettingsDialog from "../components/plugins/NvxPluginSettingsDialog.vue";
import { NvxInlineNotice } from "../components/ui";
import { listInstalledPlugins } from "../core-api/client";
import { usePluginExtensionsStore } from "../stores/pluginExtensions";
import { useWorkspaceTabsStore } from "../stores/workspaceTabs";
import { useRouteReveal } from "../routeReveal";

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
const sshSyncPermissionDenied = ref(false);
const settingsTarget = ref<{ pluginId: string; pageId: string; pluginName: string; fieldKey: string } | null>(null);
const pluginTarget = ref<InstanceType<typeof NvxPluginExtensionTarget> | null>(null);

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
  await Promise.all([
    extensions.loadNavigation(),
    loadPermissionState().catch(() => { sshSyncPermissionDenied.value = false; }),
  ]);
  if (navigationItem.value) workspaceTabs.ensurePluginPageTab(navigationItem.value);
}

function handlePermissionChanged() {
  void loadPermissionState().catch(() => undefined);
}

watch([pluginId, pageId, navigationItem], () => {
  if (settingsTarget.value && (!navigationItem.value || settingsTarget.value.pluginId !== pluginId.value
    || settingsTarget.value.pageId !== pageId.value)) {
    settingsTarget.value = null;
  }
  if (navigationItem.value) workspaceTabs.ensurePluginPageTab(navigationItem.value);
});
onMounted(() => {
  window.addEventListener("norishell:plugin-special-permission-changed", handlePermissionChanged);
  void load().catch(() => undefined).finally(revealRoute);
});
onBeforeUnmount(() => {
  window.removeEventListener("norishell:plugin-special-permission-changed", handlePermissionChanged);
});
</script>

<template>
  <main class="plugin-page">
    <template v-if="navigationItem">
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
      v-else
      tone="warning"
      :title="t('plugins.page.unavailable')"
    />
  </main>
</template>

<style scoped>
.plugin-page { display: grid; align-content: start; gap: var(--nvx-space-4); min-width: 0; padding: var(--nvx-space-6); }
.plugin-page__surface { display: grid; min-width: 0; gap: var(--nvx-space-4); }
</style>
