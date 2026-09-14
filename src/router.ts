import { createRouter, createWebHashHistory } from "vue-router";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/terminal" },
    { path: "/overview", component: () => import("./views/OverviewView.vue") },
    { path: "/hosts", component: () => import("./views/HostsView.vue") },
    { path: "/terminal", component: () => import("./views/SshTerminalView.vue") },
    { path: "/desktop", component: () => import("./views/DesktopView.vue") },
    { path: "/sftp", component: () => import("./views/SftpView.vue") },
    { path: "/tunnels", component: () => import("./views/TunnelsView.vue") },
    { path: "/settings", component: () => import("./views/SettingsView.vue") },
    {
      path: "/settings/identities",
      component: () => import("./views/IdentitiesSettingsView.vue"),
    },
    { path: "/known-hosts", component: () => import("./views/KnownHostsSettingsView.vue") },
    { path: "/plugins", component: () => import("./views/PluginsView.vue") },
    { path: "/plugin/:pluginId/:pageId", component: () => import("./views/PluginPageView.vue") },
    { path: "/:pathMatch(.*)*", redirect: "/terminal" },
  ],
});
