import { registerNativeTransferNavigation } from "./native-transfer-navigation";
import { nextTick } from "vue";
import type { Router } from "vue-router";
import type { NativeTrayAction } from "./core-api/generated/core-api";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";

/** Dispatches only one-time actions already consumed by Core; resource pages still verify the exact generation. */
export async function navigateNativeTrayAction(
  action: NativeTrayAction,
  router: Router,
  workspace: ReturnType<typeof useWorkspaceTabsStore>,
): Promise<void> {
  const blocked = () => workspace.terminalActivationBlocked
    || Boolean(document.querySelector('[role="dialog"][aria-modal="true"]'));
  if (blocked()) throw new Error("tray action unavailable");
  const operation = crypto.randomUUID();
  if (["newTerminal", "newLocalTerminal", "quickConnect", "focusTerminal", "focusTelnet", "focusSshSession", "focusLocalSession"].includes(action.kind)) {
    await router.push("/terminal");
    await nextTick();
    const controller = workspace.terminalController;
    if (!controller || blocked()) throw new Error("tray action unavailable");
    switch (action.kind) {
      case "newTerminal": controller.create(); return;
      case "newLocalTerminal": if (controller.createLocal()) return; break;
      case "quickConnect": if (controller.quickConnect()) return; break;
      case "focusTerminal": if (controller.focusNativeSession(action.scope)) return; break;
      case "focusSshSession": if (controller.focusSshSession(action.sessionId, action.generation)) return; break;
      case "focusLocalSession": if (controller.focusLocalSession(action.sessionId, action.generation)) return; break;
      case "focusTelnet": if (controller.focusTelnetSession(action.sessionId, action.generation, action.socketId)) return; break;
    }
    throw new Error("tray action unavailable");
  }
  workspace.terminalController?.deactivate();
  switch (action.kind) {
    case "settings": await router.push({ path: "/settings", query: { section: "desktop" } }); return;
    case "vault": await router.push({ path: "/settings", query: { section: "vault" } }); return;
    case "openHost": await router.push({ path: "/terminal", query: { hostId: action.hostId, connectOperationId: operation, source: "tray" } }); return;
    case "focusDesktop": await router.push({ path: "/desktop", query: { focusSessionId: action.sessionId, focusGeneration: action.generation, focusOperation: operation } }); return;
    case "openTunnels": await router.push({ path: "/tunnels", query: action.sessionId ? { focusSessionId: action.sessionId, focusGeneration: action.generation, focusOperation: operation } : {} }); return;
    case "openTransfers":
      if (action.transferId) {
        if (!action.sourceFence || !action.targetFence || !action.stateRevision) throw new Error("tray action unavailable");
        registerNativeTransferNavigation(operation, { kind: "intent", transferId: action.transferId, minimumRevision: action.stateRevision, source: action.sourceFence, target: action.targetFence });
      }
      await router.push({ path: "/sftp", query: { focusTransfers: "true", focusTransferId: action.transferId, focusOperation: operation } }); return;
    case "openSftp": await router.push({ path: "/sftp", query: { focusSessionId: action.sessionId, focusGeneration: action.generation, focusOperation: operation } }); return;
  }
}
