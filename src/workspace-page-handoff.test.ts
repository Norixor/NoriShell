import { createPinia } from "pinia";
import { describe, expect, it } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";

import { useWorkspaceTabsStore, type WorkspacePageTab } from "./stores/workspaceTabs";
import { createPageRecovery } from "./workspace-page-handoff";

const pageTabs: WorkspacePageTab[] = [
  {
    groupId: "page:newPage",
    pageType: "newPage",
    route: "/new",
    labelKey: "newWorkspace.title",
    label: "",
    iconName: null,
  },
  {
    groupId: "page:knownHosts",
    pageType: "knownHosts",
    route: "/known-hosts",
    labelKey: "sshSettings.hostKeys.title",
    label: "",
    iconName: null,
  },
  {
    groupId: "page:sshIdentities",
    pageType: "sshIdentities",
    route: "/settings/identities",
    labelKey: "identitySettings.title",
    label: "",
    iconName: null,
  },
  {
    groupId: "page:plugin:sync:main",
    pageType: "plugin",
    route: "/plugin/sync/main",
    labelKey: null,
    label: "同步",
    iconName: "sync",
  },
];

function createWindowRecovery() {
  const store = useWorkspaceTabsStore(createPinia());
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/terminal", component: { template: "<div />" } },
      ...pageTabs.map((tab) => ({ path: tab.route, component: { template: "<div />" } })),
    ],
  });
  return { store, router, recovery: createPageRecovery(store, router) };
}

describe("Page Tab recovery", () => {
  it("accepts a distinct managed New Page identity while rejecting malformed identities", () => {
    const { store } = createWindowRecovery();
    const managed = {
      ...pageTabs[0]!,
      groupId: "page:newPage:019a0000-0000-7000-8000-000000000001",
    };
    expect(store.importPageTab(managed)).toBe(true);
    expect(store.importPageTab({ ...managed, groupId: "page:newPage:other" })).toBe(false);
    expect(store.importPageTab({ ...managed, route: "/known-hosts" })).toBe(false);
    expect(store.pageTabs).toEqual([managed]);
  });

  it.each(pageTabs)("rebinds $pageType from its Core record and routes on first activation", async (tab) => {
    const { store, router, recovery } = createWindowRecovery();
    await router.push("/terminal");
    await recovery.import(tab.groupId, { ...tab });
    expect(store.pageTabs).toEqual([tab]);
    expect(router.currentRoute.value.path).toBe("/terminal");
    await recovery.activate(tab.groupId);
    expect(router.currentRoute.value.path).toBe(tab.route);
  });

  it("rejects a record for another identity and discards a failed import", async () => {
    const { store, recovery } = createWindowRecovery();
    await expect(recovery.import("page:knownHosts", pageTabs[0]!)).rejects.toThrow("workspace_tab.invalid_page");
    await recovery.import(pageTabs[1]!.groupId, pageTabs[1]!);
    await recovery.discard(pageTabs[1]!.groupId);
    expect(store.pageTabs).toEqual([]);
  });
});
