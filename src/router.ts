import { createRouter, createWebHashHistory, START_LOCATION } from "vue-router";
import { isTauri } from "@tauri-apps/api/core";
import { usePluginExtensionsStore } from "./stores/pluginExtensions";
import { showWorkspaceTabFailure } from "./workspace-tab-errors";
import { openManagedPluginPage } from "./workspace-tab-view-shell";
import { workspaceWindowLabel } from "./workspace-tab-windows";
import { isWorkspaceTabView } from "./workspace-window-context";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/terminal" },
    { path: "/overview", component: () => import("./views/OverviewView.vue") },
    { path: "/hosts", component: () => import("./views/HostsView.vue") },
    { path: "/terminal", component: () => import("./views/SshTerminalView.vue") },
    { path: "/new", component: () => import("./views/NewWorkspacePageView.vue") },
    { path: "/desktop", component: () => import("./views/DesktopView.vue") },
    { path: "/sftp", component: () => import("./views/FileWorkspaceView.vue") },
    { path: "/tunnels", component: () => import("./views/TunnelsView.vue") },
    { path: "/settings", component: () => import("./views/SettingsView.vue") },
    {
      path: "/settings/identities",
      redirect: "/settings?section=identities",
    },
    { path: "/known-hosts", redirect: "/settings?section=knownHosts" },
    { path: "/plugins", component: () => import("./views/PluginsView.vue") },
    { path: "/plugin/:pluginId/:pageId", component: () => import("./views/PluginPageView.vue") },
    { path: "/:pathMatch(.*)*", redirect: "/terminal" },
  ],
});

/**
 * A plugin page is a native Page Tab. In a window shell, navigating to its route
 * opens or activates that Tab (or focuses the window that owns it) and never
 * renders the page in the shell. The page's own Tab WebView renders it normally.
 */
export async function guardPluginPageRoute(path: string, initial = false): Promise<string | false | void> {
  if (!isTauri() || isWorkspaceTabView()) return;
  const pluginPage = /^\/plugin\/([^/]+)\/([^/]+)$/.exec(path);
  if (!pluginPage) return;
  const fallback = initial ? (workspaceWindowLabel() === "main" ? "/terminal" : "/workspace-window") : false;
  let pluginId: string;
  let pageId: string;
  try {
    pluginId = decodeURIComponent(pluginPage[1]!);
    pageId = decodeURIComponent(pluginPage[2]!);
  } catch { return fallback; }
  const extensions = usePluginExtensionsStore();
  const find = () => extensions.navigation.find((item) => item.pluginId === pluginId && item.navigation.pageId === pageId);
  if (!find()) await extensions.loadNavigation().catch(() => undefined);
  const item = find();
  if (!item) return "/plugins";
  // The shell may still be starting; the Tab opens once its manager is ready.
  void openManagedPluginPage(item)
    .catch((error: unknown) => showWorkspaceTabFailure(error, "plugin-page-route", "workspace_tab.open_failed", "workspaceTabs.openFailed"));
  return fallback;
}

router.beforeEach(async (to, from) => {
  return guardPluginPageRoute(to.path, from === START_LOCATION);
});
