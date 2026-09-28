import type { NativeTransferNavigationTarget } from "./native-transfer-navigation";
import type { Router } from "vue-router";
import type { NativeTrayAction } from "./core-api/generated/core-api";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { useUiStore } from "./stores/ui";
import {
  createManagedTerminalTab,
  createManagedTerminalForHost,
  createManagedQuickConnect,
  deactivateWorkspaceTabView,
  focusManagedTerminalSession,
  focusManagedDesktopSession,
  focusManagedFileSession,
  openManagedTransferTarget,
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
  // Settings and Tunnels are shell pages, not managed Tab content.
  await deactivateWorkspaceTabView();
  switch (action.kind) {
    case "settings": await router.push({ path: "/settings", query: { section: "desktop" } }); return;
    case "vault": await router.push({ path: "/settings", query: { section: "vault" } }); return;
    case "openTunnels": await router.push({ path: "/tunnels", query: action.sessionId ? { focusSessionId: action.sessionId, focusGeneration: action.generation, focusOperation: operation } : {} }); return;
  }
}
