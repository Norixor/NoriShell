import { createApp } from "vue";
import { createPinia } from "pinia";

import WorkspaceTabApp from "./views/WorkspaceTabApp.vue";
import { initializeApplicationPreferences } from "./application-preferences-startup";
import { parseCoreApiError } from "./core-api/client";
import { i18n } from "./locales";
import { disableDefaultWebviewContextMenu } from "./webview-context-menu";
import { markTabBoot } from "./workspace-tab-boot-trace";
import "./styles/tokens.css";
import "./styles/base.css";

const tabId = new URLSearchParams(window.location.search).get("tabId") ?? "";
markTabBoot(tabId, "tab", "script_start");

/** The Tab kind follows from its Core id; its view module loads while preferences load. */
function preloadTabView(id: string): Promise<unknown> {
  if (id.startsWith("file:")) return import("./views/FileWorkspaceView.vue");
  if (id.startsWith("desktop:")) return import("./views/DesktopView.vue");
  if (id.startsWith("page:newPage:")) return import("./views/NewWorkspacePageView.vue");
  if (id.startsWith("page:plugin:")) return import("./views/PluginPageView.vue");
  if (id.startsWith("page:")) return Promise.resolve();
  return import("./views/SshTerminalView.vue");
}
// Only warms the module cache; the router still admits just the route Core assigns.
const routerModule = import("./router");
void preloadTabView(tabId).catch(() => undefined);

const pinia = createPinia();
disableDefaultWebviewContextMenu();
// The child route guard admits only the idle route until Core provides this view's owner and route.
window.location.hash = "#/workspace-tab-idle";

function showFailure(stage: "preferences" | "render", error: unknown) {
  const root = document.querySelector("#workspace-tab-app");
  if (!root) return;
  const message = document.createElement("p");
  const chinese = navigator.language.startsWith("zh");
  message.textContent = stage === "preferences"
    ? (chinese ? "标签页偏好读取失败。请重试；本机设置没有被覆盖。" : "Could not load tab preferences. Retry; your local settings were not overwritten.")
    : (chinese ? "标签页无法加载。连接仍由 Core 保管。" : "This tab could not load. Core still owns its connections.");
  const code = document.createElement("p");
  code.textContent = parseCoreApiError(error)?.code
    ?? (stage === "preferences" ? "workspace_tab.preference_read_failed" : "workspace_tab.render_failed");
  root.replaceChildren(message, code);
}

async function start() {
  try { await initializeApplicationPreferences(pinia); }
  catch (error) { showFailure("preferences", error); return; }
  markTabBoot(tabId, "tab", "preferences_loaded");
  try {
    const { router } = await routerModule;
    router.addRoute({ path: "/workspace-tab-idle", component: { render: () => null } });
    createApp(WorkspaceTabApp).use(pinia).use(i18n).use(router).mount("#workspace-tab-app");
    markTabBoot(tabId, "tab", "app_mounted");
    await router.isReady();
  } catch (error) { showFailure("render", error); }
}

void start();
