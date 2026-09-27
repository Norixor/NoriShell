import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { watch, type WatchStopHandle } from "vue";
import type { Router } from "vue-router";

import { i18n } from "./locales";
import { parseTerminalTabHandoff } from "./terminal-workspace-handoff";
import { useWorkspaceTabsStore, type TerminalHeaderController } from "./stores/workspaceTabs";
import { useTipsStore } from "./stores/tips";
import { createDesktopHandoff } from "./workspace-desktop-handoff";
import { createFileHandoff } from "./workspace-file-handoff";
import { createPageHandoff } from "./workspace-page-handoff";
import {
  moveWorkspaceTab,
  registerWorkspaceHandoff,
  startWorkspaceTabTransfers,
  syncLocalWorkspaceTabRecords,
  type WorkspaceTabHandoff,
} from "./workspace-tab-transfer";
import { closeWorkspaceWindow, snapshotWorkspaceTabs, updateWorkspaceTab, type WorkspaceTabKind } from "./workspace-tab-windows";
import { isWorkspaceChildWindow } from "./workspace-window-context";

async function waitForTerminalController(store: ReturnType<typeof useWorkspaceTabsStore>): Promise<TerminalHeaderController> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const controller = store.terminalController;
    if (controller?.snapshotTabHandoff && controller.freezeTabHandoff && controller.importTabHandoff
      && controller.commitTabHandoff && controller.commitImportedTabHandoff
      && controller.rollbackTabHandoff && controller.discardImportedTab) return controller;
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  throw new Error("workspace_tab.terminal_unavailable");
}

function terminalHandoff(store: ReturnType<typeof useWorkspaceTabsStore>, router: Router): WorkspaceTabHandoff {
  return {
    kind: "terminal",
    owns: (id) => store.terminalTabs.some((tab) => tab.groupId === id),
    async snapshot(id) {
      return (await waitForTerminalController(store)).snapshotTabHandoff!(id);
    },
    async freeze(id) { await (await waitForTerminalController(store)).freezeTabHandoff!(id); },
    async import(id, payload) {
      const parsed = parseTerminalTabHandoff(payload);
      if (!parsed || parsed.tabId !== id) throw new Error("workspace_tab.invalid_terminal");
      await (await waitForTerminalController(store)).importTabHandoff!(parsed);
    },
    commit(id) { store.terminalController?.commitTabHandoff?.(id); },
    async rollback(id) { await (await waitForTerminalController(store)).rollbackTabHandoff!(id); },
    async discard(id) { await (await waitForTerminalController(store)).discardImportedTab!(id); },
    async activate(id) {
      const controller = await waitForTerminalController(store);
      await controller.commitImportedTabHandoff!(id);
      controller.activate(id);
    },
    async activateExisting(id) {
      await router.push("/terminal");
      (await waitForTerminalController(store)).activate(id);
    },
  };
}

export async function startWorkspaceTabWindowUi(
  store: ReturnType<typeof useWorkspaceTabsStore>,
  router: Router,
): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  const fileHandoff = createFileHandoff(store, router);
  const disposers = [
    registerWorkspaceHandoff(createPageHandoff(store, router)),
    registerWorkspaceHandoff(terminalHandoff(store, router)),
    registerWorkspaceHandoff(fileHandoff),
    registerWorkspaceHandoff(createDesktopHandoff(store, router)),
  ];
  const prepareKind = async (kind: WorkspaceTabKind) => {
    const route = kind === "terminal" ? "/terminal"
      : kind === "desktop" ? "/desktop"
        : kind === "file" ? "/sftp" : null;
    if (route && router.currentRoute.value.path !== route) await router.push(route);
  };
  const stopTransfers = await startWorkspaceTabTransfers(prepareKind);
  const stopWatch: WatchStopHandle = watch(
    () => [
      ...store.terminalTabs.map((tab) => ({ id: tab.groupId, kind: "terminal" as const })),
      ...store.pageTabs.map((tab) => ({ id: tab.groupId, kind: "page" as const })),
      ...store.fileTabs.map((tab) => ({ id: tab.groupId, kind: "file" as const })),
      ...store.desktopTabs.map((tab) => ({ id: tab.groupId, kind: "desktop" as const })),
    ],
    (visible) => { void syncLocalWorkspaceTabRecords(visible); },
    { immediate: true, deep: true },
  );
  let stopTerminalProjection: (() => void) | null = null;
  let projectionWrite = Promise.resolve();
  const writeProjection = (id: string, payload: unknown) => {
    projectionWrite = projectionWrite.then(async () => {
      const state = await snapshotWorkspaceTabs();
      const owned = state.owned.find((record) => record.id === id);
      if (!owned || JSON.stringify(owned.payload) === JSON.stringify(payload)) return;
      await updateWorkspaceTab(id, owned.revision, payload);
    }).catch(() => undefined);
  };
  const stopProjectionWatch = watch(() => store.terminalController, (controller) => {
    stopTerminalProjection?.();
    stopTerminalProjection = controller?.observeTabHandoffSnapshots?.((snapshots) => {
      for (const snapshot of snapshots) writeProjection(snapshot.tabId, snapshot);
    }) ?? null;
  }, { immediate: true });
  const fileProjectionStops = new Map<string, () => void>();
  const stopFileProjectionWatch = watch(
    () => store.fileTabs.map((tab) => tab.groupId),
    (ids) => {
      const current = new Set(ids);
      for (const [id, stop] of fileProjectionStops) {
        if (current.has(id)) continue;
        stop();
        fileProjectionStops.delete(id);
      }
      for (const id of ids) {
        if (fileProjectionStops.has(id)) continue;
        fileProjectionStops.set(id, fileHandoff.observe(id, (snapshot) => writeProjection(id, snapshot)));
      }
    },
    { immediate: true },
  );

  let closing = false;
  let stopClose: UnlistenFn | null = null;
  if (isWorkspaceChildWindow()) {
    const tips = useTipsStore();
    stopClose = await listen("workspace-window-close-requested", () => {
      if (closing) return;
      closing = true;
      void (async () => {
        try {
          const state = await snapshotWorkspaceTabs();
          for (const record of state.owned) await moveWorkspaceTab(record.id, record.kind, "main");
          await closeWorkspaceWindow();
        } finally { closing = false; }
      })().catch(() => {
        closing = false;
        tips.show({ scope: "workspace-window-close", tone: "error", title: i18n.global.t("workspaceTabs.moveFailed") });
      });
    });
  }
  return () => {
    stopClose?.();
    stopWatch();
    stopProjectionWatch();
    stopTerminalProjection?.();
    stopFileProjectionWatch();
    for (const stop of fileProjectionStops.values()) stop();
    stopTransfers();
    for (const dispose of disposers) dispose();
  };
}
