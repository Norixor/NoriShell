import type { Router } from "vue-router";

import { desktopClient } from "./core-api/desktop-client";
import type { useWorkspaceTabsStore, DesktopHeaderController } from "./stores/workspaceTabs";
import { whenAvailable, type WorkspaceTabRecovery } from "./workspace-tab-recovery";
import type { WorkspaceTabSnapshot } from "./workspace-tab-windows";

type WorkspaceTabsStore = ReturnType<typeof useWorkspaceTabsStore>;

export interface ActiveDesktopTabSnapshot {
  schemaVersion: 1;
  tabId: string;
  sessionId: string;
  generation: string;
}
export interface IdleDesktopTabSnapshot {
  schemaVersion: 1;
  tabId: string;
  profileId: string;
}
export type DesktopTabHandoffSnapshot = ActiveDesktopTabSnapshot | IdleDesktopTabSnapshot;

export function parseDesktopHandoff(value: unknown, id: string): DesktopTabHandoffSnapshot | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (record.schemaVersion !== 1 || record.tabId !== id) return null;
  if (Object.keys(record).length === 3 && typeof record.profileId === "string"
    && record.profileId.length > 0 && record.profileId.length <= 128) {
    return record as unknown as IdleDesktopTabSnapshot;
  }
  if (Object.keys(record).length !== 4 || typeof record.sessionId !== "string"
    || !record.sessionId || record.sessionId.length > 128
    || typeof record.generation !== "string" || !/^[0-9]+$/.test(record.generation)
    || record.generation.length > 20) return null;
  return record as unknown as DesktopTabHandoffSnapshot;
}

/**
 * Desktop content belongs to exactly one Tab WebView. Shell pages list profiles
 * only, so a renderer without a native Tab sees no session.
 */
export function filterDesktopSessions<T extends { id: string }>(
  sessions: readonly T[],
  ownership: WorkspaceTabSnapshot,
  staged: ReadonlySet<string>,
  nativeViewTabId: string | null,
  nativeSessionId: string | null,
): T[] {
  if (!nativeViewTabId || !nativeSessionId) return [];
  const owned = staged.has(nativeViewTabId)
    || ownership.owned.some((tab) => tab.kind === "desktop" && tab.id === nativeViewTabId);
  return owned ? sessions.filter((session) => session.id === nativeSessionId) : [];
}

function controller(store: WorkspaceTabsStore): Promise<DesktopHeaderController> {
  return whenAvailable(() => {
    const current = store.desktopController;
    return current?.importHandoff && current.importIdle && current.discardHandoff && current.admitHandoff ? current : null;
  }, "workspace_tab.desktop_unavailable");
}

/** Binds a Tab to a bounded session handle; the Core session and protocol transport stay alive. */
export function createDesktopRecovery(store: WorkspaceTabsStore, router: Router): WorkspaceTabRecovery<DesktopTabHandoffSnapshot> {
  return {
    async import(id, payload) {
      const parsed = parseDesktopHandoff(payload, id);
      if (!parsed) throw new Error("workspace_tab.invalid_desktop");
      if ("profileId" in parsed) {
        await (await controller(store)).importIdle!(id, parsed.profileId);
        return;
      }
      const live = await desktopClient.snapshot();
      const session = live.find((item) => item.id === parsed.sessionId && item.generation === parsed.generation);
      if (!session) throw new Error("workspace_tab.desktop_session_stale");
      await (await controller(store)).importHandoff!(session);
    },
    async discard(id) { await (await controller(store)).discardHandoff!(id); },
    async activate(id) {
      const current = await controller(store);
      await current.admitHandoff!(id);
      current.activate(id);
      await router.push("/desktop");
    },
    async activateExisting(id) {
      (await controller(store)).activate(id);
      await router.push("/desktop");
    },
  };
}
