import type { Router } from "vue-router";

import type { WorkspacePageTab, WorkspacePageType } from "./stores/workspaceTabs";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import type { WorkspaceTabRecovery } from "./workspace-tab-recovery";

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

export function createPageRecovery(store: WorkspaceTabsStore, router: Router): WorkspaceTabRecovery<WorkspacePageTab> {
  const imported = new Set<string>();

  return {
    async import(id, value) {
      if (!isPageTab(value) || value.groupId !== id
        || store.pageTabs.some((tab) => tab.groupId === id) || !store.importPageTab(value)) {
        throw new Error("workspace_tab.invalid_page");
      }
      imported.add(id);
    },
    async discard(id) {
      if (!imported.delete(id)) return;
      store.closePageTab(id);
    },
    async activate(id) {
      if (!imported.delete(id)) return;
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      if (tab) await router.push(tab.route);
    },
    async activateExisting(id) {
      const tab = store.pageTabs.find((candidate) => candidate.groupId === id);
      if (!tab) throw new Error("workspace_tab.not_found");
      await router.push(tab.route);
    },
  };
}
