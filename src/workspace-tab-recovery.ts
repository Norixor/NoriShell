import { watch, type WatchStopHandle } from "vue";
import type { Router } from "vue-router";

import { parseTerminalTabHandoff } from "./terminal-workspace-handoff";
import type { TerminalHeaderController, useWorkspaceTabsStore } from "./stores/workspaceTabs";

type WorkspaceTabsStore = ReturnType<typeof useWorkspaceTabsStore>;

/**
 * Rebinds Tab content from a Core record after its WebView was destroyed with a
 * crashed parent window, or from a new Desktop seed. Live moves never use it.
 */
export interface WorkspaceTabRecovery<T = unknown> {
  import(id: string, snapshot: T): Promise<void>;
  /** Removes a staged import that failed before the owner admitted it. */
  discard(id: string): Promise<void>;
  /** First activation admits the staged import into this renderer. */
  activate(id: string): Promise<void>;
  activateExisting(id: string): Promise<void>;
}

/**
 * Resolves as soon as `read` yields a value (reactively, e.g. when a View registers its
 * controller), or rejects with `code` after `timeoutMs`.
 */
export function whenAvailable<T>(read: () => T | null | undefined, code: string, timeoutMs = 20_000): Promise<T> {
  const current = read();
  if (current) return Promise.resolve(current);
  return new Promise<T>((resolve, reject) => {
    let stop: WatchStopHandle | null = null;
    const timer = window.setTimeout(() => { stop?.(); reject(new Error(code)); }, timeoutMs);
    stop = watch(read, (value) => {
      if (!value) return;
      window.clearTimeout(timer);
      stop?.();
      resolve(value);
    });
  });
}

function terminalController(store: WorkspaceTabsStore): Promise<TerminalHeaderController> {
  return whenAvailable(() => {
    const controller = store.terminalController;
    return controller?.importTabHandoff && controller.commitImportedTabHandoff && controller.discardImportedTab ? controller : null;
  }, "workspace_tab.terminal_unavailable");
}

export function terminalRecovery(store: WorkspaceTabsStore, router: Router): WorkspaceTabRecovery {
  return {
    async import(id, payload) {
      const parsed = parseTerminalTabHandoff(payload);
      if (!parsed || parsed.tabId !== id) throw new Error("workspace_tab.invalid_terminal");
      await (await terminalController(store)).importTabHandoff!(parsed);
    },
    async discard(id) { await (await terminalController(store)).discardImportedTab!(id); },
    async activate(id) {
      const controller = await terminalController(store);
      await controller.commitImportedTabHandoff!(id);
      controller.activate(id);
    },
    async activateExisting(id) {
      await router.push("/terminal");
      (await terminalController(store)).activate(id);
    },
  };
}
