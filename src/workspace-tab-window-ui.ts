import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Router } from "vue-router";

import { useUiStore } from "./stores/ui";
import { showWorkspaceTabFailure } from "./workspace-tab-errors";
import { moveWorkspaceTab } from "./workspace-tab-transfer";
import { restoreTerminalWorkspace } from "./workspace-tab-terminal-restore";
import { closeWorkspaceWindow, snapshotWorkspaceTabs } from "./workspace-tab-windows";
import { isWorkspaceChildWindow } from "./workspace-window-context";
import { startWorkspaceTabViewShell } from "./workspace-tab-view-shell";
import { workspaceTabViewSummaries } from "./workspace-tab-view-state";

/** Starts the window shell's native Tab manager; main also restores the Terminal workspace once. */
export async function startWorkspaceTabWindowUi(router: Router): Promise<() => void> {
  if (!isTauri()) return () => undefined;
  const stopTabViews = await startWorkspaceTabViewShell(router);
  if (!isWorkspaceChildWindow()) {
    void restoreTerminalWorkspace(useUiStore().terminalStartupBehavior === "restoreHistory");
  }

  let closing = false;
  let stopClose: UnlistenFn | null = null;
  if (isWorkspaceChildWindow()) {
    stopClose = await listen("workspace-window-close-requested", () => {
      if (closing) return;
      closing = true;
      void (async () => {
        try {
          const state = await snapshotWorkspaceTabs();
          const order = new Map(workspaceTabViewSummaries.value.map((tab, index) => [tab.id, index]));
          state.owned.sort((left, right) => (order.get(left.id) ?? Infinity) - (order.get(right.id) ?? Infinity));
          for (const record of state.owned) await moveWorkspaceTab(record.id, "main", false);
          await closeWorkspaceWindow();
        } finally { closing = false; }
      })().catch((error: unknown) => {
        closing = false;
        showWorkspaceTabFailure(error, "workspace-window-close", "workspace_window.close_failed", "workspaceTabs.closeWindowFailed");
      });
    });
  }
  return () => {
    stopClose?.();
    stopTabViews();
  };
}
