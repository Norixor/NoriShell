import type { NativeTransferNavigationTarget } from "./native-transfer-navigation";
import type { Router } from "vue-router";
import type { NativeTrayAction } from "./core-api/generated/core-api";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { useUiStore } from "./stores/ui";
import {
  createManagedTerminalTab,
  createManagedTerminalForHost,
  createManagedQuickConnect,
  focusManagedTerminalSession,
  focusManagedDesktopSession,
  focusManagedFileSession,
  openManagedTransferTarget,
  showWorkspaceShellRoute,
} from "./workspace-tab-view-shell";

/** Dispatches only one-time actions already consumed by Core; resource pages still verify the exact generation. */
export async function navigateNativeTrayAction(
  action: NativeTrayAction,
  router: Router,
  workspace: ReturnType<typeof useWorkspaceTabsStore>,
): Promise<void> {
  if (workspace.terminalActivationBlocked || document.querySelector('[role="dialog"][aria-modal="true"]')) {
    throw new Error("tray action unavailable");
  }
  const operation = crypto.randomUUID();
  switch (action.kind) {
    case "newTerminal":
      await createManagedTerminalTab(useUiStore().newTerminalBehavior === "localTerminal" ? "local" : "welcome"); return;
    case "newLocalTerminal": await createManagedTerminalTab("local"); return;
    case "quickConnect": await createManagedQuickConnect(); return;
    case "openHost": await createManagedTerminalForHost({ hostId: action.hostId, connectOperationId: operation, source: "tray" }); return;
    case "focusTerminal":
    case "focusSshSession":
    case "focusLocalSession":
    case "focusTelnet": await focusManagedTerminalSession(action); return;
    case "focusDesktop": await focusManagedDesktopSession(action.sessionId, action.generation); return;
    case "openSftp": await focusManagedFileSession(action.sessionId, action.generation); return;
    case "openTransfers": {
      const query = { focusTransfers: "true", focusTransferId: action.transferId ?? "", focusOperation: operation };
      if (!action.transferId) {
        await openManagedTransferTarget(query, null);
        return;
      }
      if (!action.sourceFence || !action.targetFence || !action.stateRevision) throw new Error("tray action unavailable");
      const target: NativeTransferNavigationTarget = {
        kind: "intent", transferId: action.transferId, minimumRevision: action.stateRevision,
        source: action.sourceFence, target: action.targetFence,
      };
      await openManagedTransferTarget(query, target); return;
    }
  }
  // Settings and Tunnels are shell pages; the Tab view stays until the page has painted.
  const location = action.kind === "settings" ? { path: "/settings", query: { section: "desktop" } }
    : action.kind === "vault" ? { path: "/settings", query: { section: "vault" } }
      : action.kind === "openTunnels" ? { path: "/tunnels", query: action.sessionId
        ? { focusSessionId: action.sessionId, focusGeneration: action.generation, focusOperation: operation } : {} }
        : null;
  if (location) await showWorkspaceShellRoute(() => router.push(location));
}
