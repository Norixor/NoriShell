<script setup lang="ts">
import { takeNativeTrayAction, readyNativeTrayActions } from "./core-api/native-tray";
import { navigateNativeResourceNotification } from "./native-resource-navigation";
import { navigateNativeTrayAction } from "./native-tray-navigation";
import { initializeStartupVaultTip } from "./startup-vault-tip";
import {
  getPendingSshSyncPreferences,
  resolvePendingSshSyncPreferences,
  startSshSyncPreferencesBridge,
} from "./ssh-sync-preferences-bridge";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createPinia, getActivePinia } from "pinia";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { startPluginAppNavigation } from "./stores/pluginAppIntegrations";
import { PREFERENCE_GROUP_IDS } from "./preferences-transfer";

import {
  NvxAppHeader,
  NvxNavigationRail,
  NvxWindowFrame,
  NvxWorkspaceTabBar,
} from "./components/layout";
import { NvxPluginCommandPalette } from "./components/terminal";
import { NvxPluginExtensionTarget } from "./components/plugins";
import NvxPluginFloatingControls from "./components/plugins/NvxPluginFloatingControls.vue";
import { NvxButton, NvxDialog, NvxTips } from "./components/ui";
import {
  completePluginSafeModeStartup,
  requestApplicationExit,
  setPluginLocale,
} from "./core-api/client";
import type { ExitReadiness, InstalledPluginSummary, PluginSpecialPermissionOutcome, PluginApprovedHostSessionLaunch, PluginSettingsChanged } from "./core-api/generated/core-api";
import { invalidatePluginHostDom } from "./plugins/hostDomBroker";
import { usePluginsStore } from "./stores/plugins";
import { usePluginExtensionsStore } from "./stores/pluginExtensions";
import { useTipsStore } from "./stores/tips";
import { useAppThemeStore } from "./stores/appTheme";
import { useAppUpdateStore } from "./stores/appUpdate";
import { useUiStore } from "./stores/ui";
import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { useNativeTerminalStore } from "./stores/nativeTerminal";
import { setNativeNotificationContext, type NativeTerminalNotificationClick, type NativeResourceNotificationClick } from "./core-api/native-notifications";
import { requestExitAfterTerminalWorkspaceFlush } from "./terminal-workspace-persistence";
import { acceptSftpPluginNavigation, discardSftpPluginNavigations } from "./views/sftpPluginNavigation";

function isToolWindowExitCancelled(error: unknown) { return typeof error === "object" && error !== null && "code" in error && error.code === "app.tool_window_exit_cancelled"; }

const ui = useUiStore();
const appTheme = useAppThemeStore();
const { t } = useI18n();
const router = useRouter();
const pinia = getActivePinia() ?? createPinia();
const plugins = usePluginsStore(pinia);
const pluginExtensions = usePluginExtensionsStore(pinia);
watch(() => plugins.installed.map((item) => `${item.pluginId}:${item.stateVersion}:${item.state}`).join("|"), () => {
  if (isTauri()) void appTheme.refreshThemes();
}, { immediate: true });
const tips = useTipsStore(pinia);
const workspaceTabs = useWorkspaceTabsStore(pinia);
const nativeTerminal = useNativeTerminalStore(pinia);
const appUpdate = useAppUpdateStore(pinia);
let updateCheckTimer: ReturnType<typeof setInterval> | null = null;
function updateNativeWindowFocus() {
  nativeTerminal.setAppFocused(document.hasFocus() && document.visibilityState === "visible");
}

watch([() => ui.locale, () => nativeTerminal.visiblePaneId, () => nativeTerminal.sessions], () => {
  if (!isTauri()) return;
  const session = nativeTerminal.sessions.find((item) => item.session.paneId === nativeTerminal.visiblePaneId)?.session ?? null;
  void setNativeNotificationContext(ui.locale, session).catch(() => { /* Settings exposes permission and availability. */ });
}, { immediate: true });

type ContentRegion = "before" | "after" | "sidebar" | "footer";
const contentRegions: (ContentRegion | "route")[] = ["before", "route", "sidebar", "after", "footer"];
const routePath = computed(() => router.currentRoute.value.path);
// New ordinary routes opt in here. Settings also contains Vault management.
const contentRouteLabels: Record<string, string> = {
  "/terminal": "navigation.terminal",
  "/overview": "navigation.overview",
  "/hosts": "navigation.hosts",
  "/sftp": "navigation.sftp",
  "/tunnels": "navigation.tunnels",
  "/plugins": "navigation.plugins",
};
const contentRouteLabel = computed(() => {
  const key = contentRouteLabels[routePath.value]
    ?? (/^\/plugin\/[^/]+\/[^/]+$/.test(routePath.value) ? "navigation.plugins" : null);
  return key ? t(key) : null;
});
const contentInstanceKey = ref(`route:${crypto.randomUUID()}`);
const contentAvailability = ref<Partial<Record<ContentRegion, number>>>({});
const contentTargets = computed(() => contentRegions.map((region) => ({
  region,
  instanceKey: contentInstanceKey.value,
})));

watch(routePath, () => {
  contentAvailability.value = {};
  contentInstanceKey.value = `route:${crypto.randomUUID()}`;
}, { flush: "sync" });

function updateContentAvailability(region: ContentRegion, instanceKey: string, count: number) {
  if (instanceKey !== contentInstanceKey.value) return;
  contentAvailability.value = { ...contentAvailability.value, [region]: count };
}

ui.applyPreferences();
void ui.setUiZoom(ui.uiZoom, false).then((success) => {
  if (!success) tips.show({ scope: "settings-zoom", tone: "error", title: t("sshSettings.applicationPreferences.zoomFailed") });
});

let unlistenPluginProtocolLaunch: UnlistenFn | null = null;
let unlistenApplicationExit: UnlistenFn | null = null;
let unlistenNativeResourceNotification: UnlistenFn | null = null;
let unlistenNativeNotification: UnlistenFn | null = null;
let unlistenPluginHostSession: UnlistenFn | null = null;
let unlistenPluginHostNavigation: UnlistenFn | null = null;
let unlistenPluginSpecialPermission: UnlistenFn | null = null;
let unlistenPluginTerminalInput: UnlistenFn | null = null;
let unlistenPluginRuntimeInvalidated: UnlistenFn | null = null;
let unlistenPluginRuntimeReady: UnlistenFn | null = null;
let unlistenPluginSettingsChanged: UnlistenFn | null = null;
let trayDisposed = false;
let trayReady = false;
const trayUnlisteners: UnlistenFn[] = [];
const consumedTrayTokens = new Set<string>();
function trayUnavailable() {
  if (!trayDisposed) tips.show({ scope: "native-tray", tone: "error", title: t("errors.tray.actionUnavailable") });
}
async function consumeTrayToken(token: string) {
  if (trayDisposed || typeof token !== "string" || consumedTrayTokens.has(token)) return;
  consumedTrayTokens.add(token);
  if (consumedTrayTokens.size > 128) consumedTrayTokens.delete(consumedTrayTokens.values().next().value!);
  try {
    const action = await takeNativeTrayAction(token);
    if (!trayDisposed) await navigateNativeTrayAction(action, router, workspaceTabs);
  } catch { trayUnavailable(); }
}
async function refreshTrayReady() {
  try {
    const tokens = await readyNativeTrayActions(ui.locale);
    for (const token of tokens) await consumeTrayToken(token);
  } catch { trayUnavailable(); }
}
watch(() => ui.locale, () => { if (trayReady) void refreshTrayReady(); });
watch(() => ui.locale, (locale) => {
  if (!isTauri()) return;
  void setPluginLocale(locale).catch(() => {
    tips.show({ tone: "error", title: t("sshSettings.applicationPreferences.languageChangeFailed") });
  });
});
async function startTrayNavigation() {
  for (const [event, callback] of [
    ["native-tray-action", ({ payload }: { payload: { token: string } }) => { void consumeTrayToken(payload.token); }],
    ["native-tray-error", () => trayUnavailable()],
    ["native-tray-vault-changed", () => window.dispatchEvent(new Event("norishell:vault-changed"))],
  ] as const) {
    const unlisten = await listen<{ token: string }>(event, callback);
    if (trayDisposed) { unlisten(); return; }
    trayUnlisteners.push(unlisten);
  }
  trayReady = true;
  await refreshTrayReady();
}
let applicationExitInFlight = false;
let stopPluginAppNavigation: (() => void) | null = null;
const exitReadiness = ref<ExitReadiness | null>(null);
const confirmedExitInFlight = ref(false);
const confirmedExitFailed = ref(false);

const exitBlockerLabels = computed(() => {
  const counts = new Map<string, number>();
  for (const blocker of exitReadiness.value?.blockers ?? []) {
    counts.set(blocker.kind, (counts.get(blocker.kind) ?? 0) + 1);
  }
  return Array.from(counts, ([kind, count]) => t(`lifecycle.blockers.${kind}`, { count }));
});

async function confirmResourceCleanupAndExit() {
  if (confirmedExitInFlight.value) return;
  confirmedExitInFlight.value = true;
  confirmedExitFailed.value = false;
  try {
    await requestExitAfterTerminalWorkspaceFlush(() => requestApplicationExit(true));
  } catch (error) {
    if (isToolWindowExitCancelled(error)) { exitReadiness.value = null; confirmedExitFailed.value = false; return; }
    confirmedExitFailed.value = true;
  } finally {
    confirmedExitInFlight.value = false;
  }
}

let disposeStartupVaultTip: (() => void) | undefined;
let stopSshSyncPreferencesBridge: (() => void) | undefined;
const pendingSshSyncPreferences = ref<Awaited<ReturnType<typeof getPendingSshSyncPreferences>>>(null);
const sshSyncPreferencesReviewOpen = ref(false);
const sshSyncPreferencesReviewBusy = ref(false);
const sshSyncPreferencesReviewError = ref(false);
let pendingSshSyncPreferencesRead = 0;
async function refreshPendingSshSyncPreferences() {
  const read = ++pendingSshSyncPreferencesRead;
  try {
    const pending = await getPendingSshSyncPreferences();
    if (read !== pendingSshSyncPreferencesRead) return;
    pendingSshSyncPreferences.value = pending;
    if (!pendingSshSyncPreferences.value) sshSyncPreferencesReviewOpen.value = false;
  } catch {
    // A failed Core read must not clear a previously visible review request.
    sshSyncPreferencesReviewError.value = true;
  }
}
async function resolveSshSyncPreferences(resolution: "retry" | "useRemote" | "keepLocal") {
  if (sshSyncPreferencesReviewBusy.value) return;
  sshSyncPreferencesReviewBusy.value = true;
  sshSyncPreferencesReviewError.value = false;
  try {
    await resolvePendingSshSyncPreferences(resolution);
    await refreshPendingSshSyncPreferences();
  } catch {
    sshSyncPreferencesReviewError.value = true;
    await refreshPendingSshSyncPreferences();
  } finally {
    sshSyncPreferencesReviewBusy.value = false;
  }
}
function onSshSyncPreferencesChanged() { void refreshPendingSshSyncPreferences(); }
onMounted(async () => {
  if (!isTauri()) return;
  void appUpdate.checkForUpdates();
  updateCheckTimer = setInterval(() => { void appUpdate.checkForUpdates(); }, 6 * 60 * 60 * 1000);
  disposeStartupVaultTip = initializeStartupVaultTip({ t, tips });
  window.addEventListener("norishell:ssh-sync-preferences:changed", onSshSyncPreferencesChanged);
  void startSshSyncPreferencesBridge().then((stop) => {
    if (trayDisposed) stop();
    else {
      stopSshSyncPreferencesBridge = stop;
      void refreshPendingSshSyncPreferences();
    }
  }).catch(() => { /* Core rejects collection when the trusted bridge is unavailable. */ });
  stopPluginAppNavigation = await startPluginAppNavigation(router);
  unlistenPluginProtocolLaunch = await listen("plugin-protocol-launch", () => { void router.push("/terminal"); });
  nativeTerminal.start();
  void startTrayNavigation().catch(trayUnavailable);
  unlistenNativeNotification = await listen<NativeTerminalNotificationClick>("native-terminal-notification-click", ({ payload }) => {
    workspaceTabs.terminalController?.focusNativeSession?.(payload.scope);
  });
  unlistenNativeResourceNotification = await listen<NativeResourceNotificationClick>("native-resource-notification-click", ({ payload }) => {
    void navigateNativeResourceNotification(payload, router, workspaceTabs, () => !trayDisposed);
  });
  updateNativeWindowFocus();
  window.addEventListener("focus", updateNativeWindowFocus);
  window.addEventListener("blur", updateNativeWindowFocus);
  document.addEventListener("visibilitychange", updateNativeWindowFocus);
  await setPluginLocale(ui.locale).catch(() => {
    tips.show({
      tone: "error",
      title: t("sshSettings.applicationPreferences.languageChangeFailed"),
    });
  });
  unlistenApplicationExit = await listen("application-exit-requested", async () => {
    if (applicationExitInFlight) return;
    applicationExitInFlight = true;
    try {
      const readiness = await requestExitAfterTerminalWorkspaceFlush(requestApplicationExit);
      confirmedExitFailed.value = false;
      exitReadiness.value = readiness.canExit ? null : readiness;
    } catch (error) {
      if (isToolWindowExitCancelled(error)) { exitReadiness.value = null; confirmedExitFailed.value = false; return; }
      confirmedExitFailed.value = true;
      exitReadiness.value = exitReadiness.value ?? { canExit: false, blockers: [] };
    } finally {
      applicationExitInFlight = false;
    }
  });
  unlistenPluginHostSession = await listen<PluginApprovedHostSessionLaunch>(
    "plugin-host-session-approved",
    ({ payload }) => {
      if (payload.kind === "terminal") {
        void router.push({
          path: "/terminal",
          query: {
            hostId: payload.hostId,
            connectOperationId: payload.operationId,
            pluginAuthorizationToken: payload.authorizationToken,
            source: "plugin",
          },
        });
      } else if (payload.kind === "sftp") {
        void router.push({ path: "/sftp", query: { hostId: payload.hostId } });
      } else {
        void router.push({ path: "/tunnels", query: { hostId: payload.hostId } });
      }
    },
  );
  unlistenPluginHostNavigation = await listen<unknown>(
    "plugin-host-navigation-requested",
    ({ payload }) => {
      if (acceptSftpPluginNavigation(payload)) void router.push("/sftp");
    },
  );
  unlistenPluginSpecialPermission = await listen<PluginSpecialPermissionOutcome>(
    "plugin-special-permission-changed",
    ({ payload }) => {
      if (payload.kind === "installed") {
        invalidatePluginHostDom(payload.plugin.pluginId);
        discardSftpPluginNavigations(payload.plugin.pluginId);
      } else {
        usePluginsStore(pinia).applySpecialPermissionOutcome(payload);
      }
      window.dispatchEvent(new CustomEvent("norishell:plugin-special-permission-changed", { detail: payload }));
    },
  );
  unlistenPluginTerminalInput = await listen("plugin-terminal-input-decided", () => {
    window.dispatchEvent(new CustomEvent("norishell:plugin-terminal-input-decided"));
  });
  unlistenPluginRuntimeInvalidated = await listen<{ pluginId: string }>(
    "plugin-runtime-invalidated",
    ({ payload }) => {
      invalidatePluginHostDom(payload.pluginId);
      discardSftpPluginNavigations(payload.pluginId);
      workspaceTabs.closePluginPageTabs(payload.pluginId);
      void plugins.refreshInstalled();
      void pluginExtensions.loadNavigation();
      if (String(router.currentRoute.value.params.pluginId ?? "") === payload.pluginId) {
        void router.push("/plugins");
      }
      window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-invalidated"));
    },
  );
  unlistenPluginRuntimeReady = await listen<InstalledPluginSummary>(
    "plugin-runtime-ready",
    () => {
      void plugins.refreshInstalled();
      void pluginExtensions.loadNavigation();
      window.dispatchEvent(new CustomEvent("norishell:plugin-runtime-ready"));
    },
  );
  unlistenPluginSettingsChanged = await listen<PluginSettingsChanged>(
    "plugin-settings-changed",
    ({ payload }) => {
      void pluginExtensions.refreshPluginContributions(payload.pluginId);
    },
  );
  await completePluginSafeModeStartup().catch(() => undefined);
});

onBeforeUnmount(() => {
  if (updateCheckTimer) clearInterval(updateCheckTimer);
  disposeStartupVaultTip?.();
  stopSshSyncPreferencesBridge?.();
  stopSshSyncPreferencesBridge = undefined;
  pendingSshSyncPreferencesRead += 1;
  window.removeEventListener("norishell:ssh-sync-preferences:changed", onSshSyncPreferencesChanged);
  trayDisposed = true;
  trayReady = false;
  trayUnlisteners.splice(0).forEach((unlisten) => unlisten());
  unlistenPluginProtocolLaunch?.();
  unlistenPluginProtocolLaunch = null;
  nativeTerminal.stop();
  stopPluginAppNavigation?.();
  stopPluginAppNavigation = null;
  unlistenNativeResourceNotification?.();
  unlistenNativeResourceNotification = null;
  unlistenNativeNotification?.();
  unlistenNativeNotification = null;
  window.removeEventListener("focus", updateNativeWindowFocus);
  window.removeEventListener("blur", updateNativeWindowFocus);
  document.removeEventListener("visibilitychange", updateNativeWindowFocus);
  unlistenApplicationExit?.();
  unlistenApplicationExit = null;
  unlistenPluginHostSession?.();
  unlistenPluginHostSession = null;
  unlistenPluginHostNavigation?.();
  unlistenPluginHostNavigation = null;
  unlistenPluginSpecialPermission?.();
  unlistenPluginSpecialPermission = null;
  unlistenPluginTerminalInput?.();
  unlistenPluginTerminalInput = null;
  unlistenPluginRuntimeInvalidated?.();
  unlistenPluginRuntimeInvalidated = null;
  unlistenPluginRuntimeReady?.();
  unlistenPluginRuntimeReady = null;
  unlistenPluginSettingsChanged?.();
  unlistenPluginSettingsChanged = null;
});
</script>

<template>
  <NvxWindowFrame
    v-slot="{ platform }"
    :zoom="ui.appliedUiZoom / 100"
  >
    <NvxTips />
    <NvxPluginCommandPalette />
    <div class="app-shell">
      <NvxAppHeader :platform="platform">
        <template #tabs>
          <NvxWorkspaceTabBar />
        </template>
      </NvxAppHeader>
      <div
        v-if="pendingSshSyncPreferences"
        class="app-sync-preferences-banner"
        role="status"
      >
        <span>{{ t("plugins.sshSyncPreferencesReview.title") }}</span>
        <NvxButton
          variant="secondary"
          @click="sshSyncPreferencesReviewOpen = true"
        >
          {{ t("plugins.sshSyncPreferencesReview.title") }}
        </NvxButton>
      </div>
      <div class="app-workspace">
        <NvxNavigationRail />
        <main class="app-main">
          <template
            v-for="target in contentTargets"
            :key="target.region"
          >
            <div
              v-if="target.region === 'route'"
              class="app-route-content"
            >
              <RouterView v-slot="{ Component, route }">
                <KeepAlive include="SshTerminalView,DesktopView,SftpView">
                  <component
                    :is="Component"
                    :key="route.path"
                  />
                </KeepAlive>
              </RouterView>
            </div>
            <div
              v-else-if="contentRouteLabel"
              v-show="(contentAvailability[target.region] ?? 0) > 0"
              class="app-plugin-region"
              :class="`app-plugin-region--${target.region}`"
              :data-plugin-region="target.region"
              role="region"
              :aria-label="t('navigation.plugins')"
              tabindex="0"
            >
              <NvxPluginExtensionTarget
                :key="`${target.region}:${target.instanceKey}`"
                :target-id="`app.content.${target.region}`"
                :instance-key="target.instanceKey"
                :display-label="contentRouteLabel"
                :route-path="routePath"
                @availability="updateContentAvailability(target.region, target.instanceKey, $event)"
              />
            </div>
          </template>
          <NvxPluginFloatingControls
            v-if="contentRouteLabel"
            :key="contentInstanceKey"
            target-id="app.content.floating"
            :instance-key="contentInstanceKey"
            :context-label="contentRouteLabel"
            :preference-scope="`app:${routePath}`"
            :route-path="routePath"
          />
        </main>
      </div>
    </div>

    <NvxDialog
      plugin-protected
      :model-value="sshSyncPreferencesReviewOpen && pendingSshSyncPreferences !== null"
      :title="t('plugins.sshSyncPreferencesReview.title')"
      :description="t(pendingSshSyncPreferences?.ready ? 'plugins.sshSyncPreferencesReview.description' : 'plugins.sshSyncPreferencesReview.preparedDescription')"
      :close-label="t('plugins.sshSyncPreferencesReview.close')"
      :dismissible="!sshSyncPreferencesReviewBusy"
      size="lg"
      @update:model-value="(open) => { if (!open) sshSyncPreferencesReviewOpen = false; }"
    >
      <ul class="app-sync-preferences-groups">
        <li
          v-for="groupId in PREFERENCE_GROUP_IDS"
          :key="groupId"
        >
          <span>{{ t(`preferenceTransfer.groups.${groupId}`) }}</span>
          <span>{{ pendingSshSyncPreferences?.results[groupId]
            ? t(`plugins.sshSyncPreferencesReview.states.${pendingSshSyncPreferences.results[groupId]}`)
            : t('plugins.sshSyncPreferencesReview.pending') }}</span>
        </li>
      </ul>
      <p
        v-if="sshSyncPreferencesReviewError"
        role="alert"
      >
        {{ t("plugins.sshSyncPreferencesReview.error") }}
      </p>
      <template #actions>
        <div class="app-sync-preferences-actions">
          <NvxButton
            data-nvx-dialog-initial-focus
            variant="secondary"
            :disabled="sshSyncPreferencesReviewBusy"
            @click="sshSyncPreferencesReviewOpen = false"
          >
            {{ t("plugins.sshSyncPreferencesReview.close") }}
          </NvxButton>
          <NvxButton
            variant="secondary"
            :disabled="sshSyncPreferencesReviewBusy || !pendingSshSyncPreferences?.ready"
            :loading="sshSyncPreferencesReviewBusy && pendingSshSyncPreferences?.ready"
            @click="resolveSshSyncPreferences('retry')"
          >
            {{ t("plugins.sshSyncPreferencesReview.retry") }}
          </NvxButton>
          <NvxButton
            variant="secondary"
            :disabled="sshSyncPreferencesReviewBusy"
            @click="resolveSshSyncPreferences('keepLocal')"
          >
            {{ t("plugins.sshSyncPreferencesReview.keepLocal") }}
          </NvxButton>
          <NvxButton
            variant="danger"
            :disabled="sshSyncPreferencesReviewBusy || !pendingSshSyncPreferences?.ready"
            @click="resolveSshSyncPreferences('useRemote')"
          >
            {{ t("plugins.sshSyncPreferencesReview.useRemote") }}
          </NvxButton>
        </div>
      </template>
    </NvxDialog>

    <NvxDialog
      plugin-protected
      :model-value="exitReadiness !== null"
      :title="t(confirmedExitFailed ? 'lifecycle.cleanupFailedTitle' : 'lifecycle.blockedTitle')"
      :description="t(confirmedExitFailed ? 'lifecycle.cleanupFailedDescription' : 'lifecycle.blockedDescription')"
      :close-label="t('lifecycle.continueWorking')"
      :dismissible="!confirmedExitInFlight"
      @update:model-value="(open) => { if (!open) { exitReadiness = null; confirmedExitFailed = false; } }"
    >
      <ul class="app-exit-blockers">
        <li
          v-for="label in exitBlockerLabels"
          :key="label"
        >
          {{ label }}
        </li>
      </ul>
      <p
        v-if="confirmedExitFailed"
        role="alert"
      >
        {{ t('lifecycle.cleanupFailed') }}
      </p>
      <template #actions>
        <NvxButton
          data-nvx-dialog-initial-focus
          variant="secondary"
          :disabled="confirmedExitInFlight"
          @click="exitReadiness = null; confirmedExitFailed = false"
        >
          {{ t('lifecycle.continueWorking') }}
        </NvxButton>
        <NvxButton
          variant="danger"
          :loading="confirmedExitInFlight"
          @click="confirmResourceCleanupAndExit"
        >
          {{ t(confirmedExitFailed ? 'lifecycle.retryCleanupAndQuit' : 'lifecycle.disconnectAndQuit') }}
        </NvxButton>
      </template>
    </NvxDialog>
  </NvxWindowFrame>
</template>

<style scoped>
.app-workspace { flex-basis: 0; }

.app-sync-preferences-banner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--nvx-space-3);
  padding: var(--nvx-space-2) var(--nvx-space-4);
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.app-sync-preferences-groups {
  display: grid;
  gap: var(--nvx-space-2);
  margin: 0;
  padding-inline-start: var(--nvx-space-5);
}

.app-sync-preferences-groups li {
  display: flex;
  justify-content: space-between;
  gap: var(--nvx-space-3);
}

.app-sync-preferences-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: var(--nvx-space-2);
  width: 100%;
}

.app-main {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  grid-template-rows: auto minmax(240px, 1fr) auto auto;
  overflow: hidden;
}

.app-route-content {
  grid-column: 1;
  grid-row: 2;
  min-width: 0;
  min-height: 0;
  overflow: auto;
}

.app-plugin-region {
  display: grid;
  grid-column: 1 / -1;
  grid-auto-rows: max-content;
  align-content: start;
  min-width: 0;
  min-height: 0;
  max-height: min(80px, 10vh);
  gap: var(--nvx-space-3);
  padding: var(--nvx-space-2) var(--nvx-space-3);
  overflow: auto;
  overscroll-behavior: contain;
  border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  background: var(--nvx-color-bg-surface);
}

.app-plugin-region:focus-visible {
  outline: var(--nvx-focus-ring-width) solid var(--nvx-color-focus-ring);
  outline-offset: calc(-1 * var(--nvx-focus-ring-width));
}

.app-plugin-region--before { grid-row: 1; }
.app-plugin-region--after { grid-row: 3; }
.app-plugin-region--footer { grid-row: 4; border-bottom: 0; }

.app-plugin-region--sidebar {
  grid-column: 2;
  grid-row: 2;
  width: 304px;
  max-height: none;
  border-bottom: 0;
  border-inline-start: var(--nvx-border-width) solid var(--nvx-color-border);
}

@media (max-width: 1200px) {
  .app-main {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(240px, 1fr) auto auto auto;
  }

  .app-plugin-region--sidebar {
    grid-column: 1;
    grid-row: 3;
    width: auto;
    max-height: min(112px, 14vh);
    border-inline-start: 0;
    border-bottom: var(--nvx-border-width) solid var(--nvx-color-border);
  }

  .app-plugin-region--after { grid-row: 4; }
  .app-plugin-region--footer { grid-row: 5; }
}

.app-exit-blockers {
  display: grid;
  gap: var(--nvx-space-2);
  margin: 0;
  padding-inline-start: var(--nvx-space-5);
}
</style>
