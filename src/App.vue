<script setup lang="ts">
import { takeNativeTrayAction, readyNativeTrayActions } from "./core-api/native-tray";
import { navigateNativeResourceNotification } from "./native-resource-navigation";
import { navigateNativeTrayAction } from "./native-tray-navigation";
import { initializeStartupVaultTip } from "./startup-vault-tip";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createPinia, getActivePinia } from "pinia";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { startPluginAppNavigation } from "./stores/pluginAppIntegrations";

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
onMounted(async () => {
  if (!isTauri()) return;
  disposeStartupVaultTip = initializeStartupVaultTip({ t, tips });
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
  disposeStartupVaultTip?.();
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
                <KeepAlive include="SshTerminalView,DesktopView">
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
