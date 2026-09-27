import { createRouter, createWebHashHistory } from "vue-router";
import { isTauri } from "@tauri-apps/api/core";
import { focusWorkspaceWindowTarget, snapshotWorkspaceTabs, workspaceWindowLabel } from "./workspace-tab-windows";

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

router.beforeEach(async (to) => {
  if (!isTauri()) return;
  const pluginPage = /^\/plugin\/([^/]+)\/([^/]+)$/.exec(to.path);
  if (!pluginPage) return;
  let tabId: string;
  try {
    tabId = `page:plugin:${decodeURIComponent(pluginPage[1]!)}:${decodeURIComponent(pluginPage[2]!)}`;
  } catch { return; }
  const state = await snapshotWorkspaceTabs().catch(() => null);
  const otherOwner = state?.others.find((tab) => tab.id === tabId)?.owner;
  if (!otherOwner) return;
  await focusWorkspaceWindowTarget(otherOwner).catch(() => undefined);
  return workspaceWindowLabel() === "main" ? "/terminal" : "/workspace-window";
});
