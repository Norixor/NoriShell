import type { Router } from "vue-router";

import { desktopClient } from "./core-api/desktop-client";
import type { useWorkspaceTabsStore, DesktopHeaderController } from "./stores/workspaceTabs";
import type { WorkspaceTabHandoff } from "./workspace-tab-transfer";
import type { WorkspaceTabSnapshot } from "./workspace-tab-windows";

type WorkspaceTabsStore = ReturnType<typeof useWorkspaceTabsStore>;

export interface DesktopTabHandoffSnapshot {
  schemaVersion: 1;
  tabId: string;
  sessionId: string;
  generation: string;
}

export function parseDesktopHandoff(value: unknown, id: string): DesktopTabHandoffSnapshot | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (Object.keys(record).length !== 4 || record.schemaVersion !== 1 || record.tabId !== id
    || typeof record.sessionId !== "string" || !record.sessionId || record.sessionId.length > 128
    || id !== `desktop:${record.sessionId}` || typeof record.generation !== "string"
    || !/^[0-9]+$/.test(record.generation) || record.generation.length > 20) return null;
  return record as unknown as DesktopTabHandoffSnapshot;
}

/** A main window owns legacy unregistered sessions; a child only sees explicit Core ownership. */
export function filterDesktopSessions<T extends { id: string }>(
  sessions: readonly T[],
  ownership: WorkspaceTabSnapshot,
  label: string,
  staged: ReadonlySet<string> = new Set(),
  localCreated: ReadonlySet<string> = new Set(),
): T[] {
  const owned = new Set(ownership.owned.filter((tab) => tab.kind === "desktop").map((tab) => tab.id));
  const others = new Set(ownership.others.filter((tab) => tab.kind === "desktop").map((tab) => tab.id));
  return sessions.filter((session) => {
    const id = `desktop:${session.id}`;
    if (staged.has(id)) return true;
    if (others.has(id)) return false;
    return owned.has(id) || localCreated.has(id) || label === "main";
  });
}

async function controller(store: WorkspaceTabsStore): Promise<DesktopHeaderController> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const current = store.desktopController;
    if (current?.snapshotHandoff && current.freezeHandoff && current.importHandoff
      && current.commitHandoff && current.rollbackHandoff && current.discardHandoff
      && current.admitHandoff) return current;
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  throw new Error("workspace_tab.desktop_unavailable");
}

/** Transfer a bounded session handle; the Core session and protocol transport remain alive. */
export function createDesktopHandoff(store: WorkspaceTabsStore, router: Router): WorkspaceTabHandoff<DesktopTabHandoffSnapshot> {
  return {
    kind: "desktop",
    owns: (id) => store.desktopTabs.some((tab) => tab.groupId === id),
    async snapshot(id) { return (await controller(store)).snapshotHandoff!(id); },
    async freeze(id) { await (await controller(store)).freezeHandoff!(id); },
    async import(id, payload) {
      const parsed = parseDesktopHandoff(payload, id);
      if (!parsed) throw new Error("workspace_tab.invalid_desktop");
      const live = await desktopClient.snapshot();
      const session = live.find((item) => item.id === parsed.sessionId && item.generation === parsed.generation);
      if (!session) throw new Error("workspace_tab.desktop_session_stale");
      await (await controller(store)).importHandoff!(session);
    },
    commit(id) { store.desktopController?.commitHandoff?.(id); },
    async rollback(id) { await (await controller(store)).rollbackHandoff!(id); },
    async discard(id) { await (await controller(store)).discardHandoff!(id); },
    async activate(id) {
      const current = await controller(store);
      await current.admitHandoff!(id);
      current.activate(id);
      await router.push("/desktop");
    },
    async activateExisting(id) {
      if (!store.desktopTabs.some((tab) => tab.groupId === id)) throw new Error("workspace_tab.not_found");
      (await controller(store)).activate(id);
      await router.push("/desktop");
    },
  };
}
