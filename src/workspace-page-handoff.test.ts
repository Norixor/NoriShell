import { createPinia } from "pinia";
import { describe, expect, it } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";

import { useWorkspaceTabsStore, type WorkspacePageTab } from "./stores/workspaceTabs";
import { createPageHandoff } from "./workspace-page-handoff";

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

function createWindowHandoff() {
  const store = useWorkspaceTabsStore(createPinia());
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/terminal", component: { template: "<div />" } }],
  });
  return { store, handoff: createPageHandoff(store, router) };
}

describe("Page Tab handoff", () => {
  it.each(pageTabs)("moves $pageType out and back without losing the Page Tab", async (tab) => {
    const source = createWindowHandoff();
    const target = createWindowHandoff();
    expect(source.store.importPageTab(tab)).toBe(true);

    const outgoing = await source.handoff.snapshot(tab.groupId);
    await source.handoff.freeze(tab.groupId);
    expect(source.store.pageTabs).toEqual([]);
    await target.handoff.import(tab.groupId, outgoing);
    source.handoff.commit(tab.groupId);
    expect(target.store.pageTabs).toEqual([tab]);

    const incoming = await target.handoff.snapshot(tab.groupId);
    await target.handoff.freeze(tab.groupId);
    expect(target.store.pageTabs).toEqual([]);
    await source.handoff.import(tab.groupId, incoming);
    target.handoff.commit(tab.groupId);
    expect(source.store.pageTabs).toEqual([tab]);
  });
});
