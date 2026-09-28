import { createApp, watch } from "vue";
import { createPinia } from "pinia";

import App from "./App.vue";
import { initializeApplicationPreferences, observeApplicationPreferenceProjection } from "./application-preferences-startup";
import { parseCoreApiError } from "./core-api/client";
import { i18n } from "./locales";
import { router } from "./router";
import { useAppThemeStore } from "./stores/appTheme";
import { useUiStore } from "./stores/ui";
import { disableDefaultWebviewContextMenu } from "./webview-context-menu";
import { revealWindowAfterMount } from "./window-first-show";
import "./styles/tokens.css";
import "./styles/base.css";

const pinia = createPinia();
disableDefaultWebviewContextMenu();

async function revealMainWindow(): Promise<void> {
  const ui = useUiStore(pinia);
  const appTheme = useAppThemeStore(pinia);
  // Startup zoom and plugin theme resolution can change the initial layout.
  if (ui.uiZoomBusy || appTheme.loading) {
    await new Promise<void>((resolve) => {
      const stop = watch([() => ui.uiZoomBusy, () => appTheme.loading], ([zoomBusy, themeLoading]) => {
        if (!zoomBusy && !themeLoading) {
          stop();
          resolve();
        }
      }, { flush: "sync" });
    });
  }
  await revealWindowAfterMount();
}

function showStartupFailure(stage: "preferences" | "application", error: unknown) {
  const root = document.querySelector("#app");
  if (!root) return;
  const message = document.createElement("p");
  const chinese = navigator.language.startsWith("zh");
  message.textContent = stage === "preferences"
    ? (chinese ? "应用偏好读取失败。请重试；本机设置没有被覆盖。" : "Could not load application preferences. Retry; your local settings were not overwritten.")
    : (chinese ? "应用启动失败。请重试。" : "Could not start the application. Retry.");
  const diagnostic = document.createElement("p");
  diagnostic.textContent = parseCoreApiError(error)?.code
    ?? (stage === "preferences" ? "application_preferences.startup_failed" : "application_startup.failed");
  const retry = document.createElement("button");
  retry.textContent = chinese ? "重试" : "Retry";
  retry.addEventListener("click", () => window.location.reload());
  root.replaceChildren(message, diagnostic, retry);
}

async function start() {
  try {
    await initializeApplicationPreferences(pinia);
    const stopProjection = observeApplicationPreferenceProjection(pinia);
    window.addEventListener("pagehide", stopProjection, { once: true });
  } catch (error) {
    showStartupFailure("preferences", error);
    await revealWindowAfterMount();
    return;
  }
  try {
    createApp(App).use(pinia).use(i18n).use(router).mount("#app");
    await router.isReady().then(revealMainWindow, revealMainWindow);
  } catch (error) {
    showStartupFailure("application", error);
    await revealWindowAfterMount();
  }
}

void start();
