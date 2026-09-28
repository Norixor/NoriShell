import { createApp, watch } from "vue";
import { createPinia } from "pinia";

import WorkspaceWindowApp from "./views/WorkspaceWindowApp.vue";
import { initializeApplicationPreferences, observeApplicationPreferenceProjection } from "./application-preferences-startup";
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
router.addRoute({ path: "/workspace-window", component: () => import("./views/WorkspaceWindowEmpty.vue") });
if (!window.location.hash || window.location.hash === "#/") window.location.hash = "#/workspace-window";

async function revealWorkspaceWindow() {
  const ui = useUiStore(pinia);
  const theme = useAppThemeStore(pinia);
  if (ui.uiZoomBusy || theme.loading) {
    await new Promise<void>((resolve) => {
      const stop = watch([() => ui.uiZoomBusy, () => theme.loading], ([zoomBusy, loading]) => {
        if (!zoomBusy && !loading) {
          stop();
          resolve();
        }
      }, { flush: "sync" });
    });
  }
  await revealWindowAfterMount();
}

async function start() {
  try {
    await initializeApplicationPreferences(pinia);
    const stopProjection = observeApplicationPreferenceProjection(pinia);
    window.addEventListener("pagehide", stopProjection, { once: true });
    createApp(WorkspaceWindowApp).use(pinia).use(i18n).use(router).mount("#workspace-window-app");
    await router.isReady().then(revealWorkspaceWindow, revealWorkspaceWindow);
  } catch {
    const root = document.querySelector("#workspace-window-app");
    if (root) {
      const message = document.createElement("p");
      message.textContent = navigator.language.startsWith("zh")
        ? "工作区窗口无法加载。请重试；标签页和连接没有被关闭。"
        : "Could not load this workspace window. Retry; its tabs and connections were not closed.";
      const retry = document.createElement("button");
      retry.textContent = navigator.language.startsWith("zh") ? "重试" : "Retry";
      retry.addEventListener("click", () => window.location.reload());
      root.replaceChildren(message, retry);
    }
    await revealWindowAfterMount();
  }
}

void start();
