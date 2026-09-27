import type { Router } from "vue-router";

import type { WorkspacePageTab, WorkspacePageType } from "./stores/workspaceTabs";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import type { WorkspaceTabHandoff } from "./workspace-tab-transfer";
import { isWorkspaceChildWindow } from "./workspace-window-context";

type WorkspaceTabsStore = ReturnType<typeof useWorkspaceTabsStore>;
const PAGE_TYPES: Record<WorkspacePageType, true> = {
  newPage: true,
  knownHosts: true,
  sshIdentities: true,
  plugin: true,
};

function isPageTab(value: unknown): value is WorkspacePageTab {
  if (!value || typeof value !== "object") return false;
  const tab = value as Partial<WorkspacePageTab>;
  return typeof tab.groupId === "string"
    && typeof tab.route === "string"
    && typeof tab.label === "string"
    && typeof tab.pageType === "string"
    && PAGE_TYPES[tab.pageType as WorkspacePageType] === true
    && (tab.labelKey === null || typeof tab.labelKey === "string")
    && (tab.iconName === null || typeof tab.iconName === "string");
}

function isActivePage(router: Router, tab: WorkspacePageTab): boolean {
  const route = router.currentRoute.value;
  if (route.path === tab.route) return true;
  return route.path === "/settings" && (
    (tab.pageType === "knownHosts" && route.query.section === "knownHosts")
    || (tab.pageType === "sshIdentities" && route.query.section === "identities")
  );
}

export function createPageHandoff(store: WorkspaceTabsStore, router: Router): WorkspaceTabHandoff<WorkspacePageTab> {
  const frozen = new Map<string, { tab: WorkspacePageTab; route: string | null }>();
  const imported = new Set<string>();
  const fallback = isWorkspaceChildWindow() ? "/workspace-window" : "/terminal";

  return {
    kind: "page",
    owns: (id) => store.pageTabs.some((tab) => tab.groupId === id),
    async snapshot(id) {
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      if (!tab) throw new Error("workspace_tab.not_found");
      return { ...tab };
    },
    async freeze(id) {
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      if (!tab) throw new Error("workspace_tab.not_found");
      const activeRoute = isActivePage(router, tab) ? router.currentRoute.value.fullPath : null;
      frozen.set(id, { tab: { ...tab }, route: activeRoute });
      if (activeRoute) await router.push(fallback);
      store.closePageTab(id);
    },
    async import(id, value) {
      if (!isPageTab(value) || value.groupId !== id
        || store.pageTabs.some((tab) => tab.groupId === id) || !store.importPageTab(value)) {
        throw new Error("workspace_tab.invalid_page");
      }
      imported.add(id);
    },
    commit(id) { frozen.delete(id); },
    async rollback(id) {
      const previous = frozen.get(id);
      if (!previous) return;
      frozen.delete(id);
      if (!store.importPageTab(previous.tab)) throw new Error("workspace_tab.invalid_page");
      if (previous.route) await router.push(previous.route);
    },
    async discard(id) {
      if (!imported.delete(id)) return;
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      const wasActive = tab ? isActivePage(router, tab) : false;
      store.closePageTab(id);
      if (wasActive) await router.push(fallback);
    },
    activate(id) {
      if (!imported.delete(id)) return;
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      if (tab) void router.push(tab.route);
    },
    async activateExisting(id) {
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      if (!tab) throw new Error("workspace_tab.not_found");
      await router.push(tab.route);
    },
  };
}
