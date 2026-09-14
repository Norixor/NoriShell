import { registerNativeTransferNavigation } from "./native-transfer-navigation";
import type { Router } from "vue-router";
import type { NativeResourceNotificationClick } from "./core-api/native-notifications";
import type { NativeTrayAction } from "./core-api/generated/core-api";
import { fetchSshSessionSnapshot, fetchTelnetSessionSnapshot, fetchForwardSessionSnapshot, fetchSftpSessionSnapshot, fetchSftpTransferIntentSnapshot } from "./core-api/client";
import { desktopClient } from "./core-api/desktop-client";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { navigateNativeTrayAction } from "./native-tray-navigation";

/** A notification can be clicked long after its resource exits. Expired notifications finish silently and never reconnect. */
export async function navigateNativeResourceNotification(
  payload: NativeResourceNotificationClick,
  router: Router,
  workspace: ReturnType<typeof useWorkspaceTabsStore>,
  alive: () => boolean,
): Promise<void> {
  try {
    let action: NativeTrayAction;
    switch (payload.kind) {
      case "ssh": {
        const session = (await fetchSshSessionSnapshot()).sessions.find((item) => item.sessionId === payload.sessionId && item.generation === payload.generation && item.stateRevision === payload.stateRevision);
        if (!session) return;
        action = { kind: "focusSshSession", sessionId: session.sessionId, generation: session.generation }; break;
      }
      case "telnet": {
        const session = (await fetchTelnetSessionSnapshot()).sessions.find((item) => item.sessionId === payload.sessionId && item.generation === payload.generation && item.stateRevision === payload.stateRevision);
        if (!session) return;
        action = { kind: "focusTelnet", sessionId: session.sessionId, generation: session.generation, socketId: session.socketId }; break;
      }
      case "forward": {
        const session = (await fetchForwardSessionSnapshot()).sessions.find((item) => item.sessionId === payload.sessionId && item.generation === payload.generation && item.stateRevision === payload.stateRevision);
        if (!session) return;
        action = { kind: "openTunnels", sessionId: session.sessionId, generation: session.generation }; break;
      }
      case "desktop": {
        const session = (await desktopClient.snapshot()).find((item) => item.id === payload.sessionId && item.generation === payload.generation && item.revision === payload.revision);
        if (!session) return;
        action = { kind: "focusDesktop", sessionId: session.id, generation: session.generation }; break;
      }
      case "sftpTransfer": {
        const [intents, legacy] = await Promise.all([fetchSftpTransferIntentSnapshot(), fetchSftpSessionSnapshot()]);
        const transfer = intents.transfers.find((item) => item.transferId === payload.transferId && item.stateRevision === payload.stateRevision);
        if (transfer) {
          action = { kind: "openTransfers", transferId: transfer.transferId, stateRevision: transfer.stateRevision, sourceFence: transfer.sourceFence, targetFence: transfer.targetFence }; break;
        }
        const old = legacy.transfers.find((item) => item.transferId === payload.transferId && item.stateRevision === payload.stateRevision);
        if (!old || !alive() || workspace.terminalActivationBlocked || document.querySelector('[role="dialog"][aria-modal="true"]')) return;
        const operation = crypto.randomUUID();
        registerNativeTransferNavigation(operation, { kind: "legacy", transferId: old.transferId, minimumRevision: old.stateRevision, sessionId: old.sessionId, generation: old.generation });
        workspace.terminalController?.deactivate();
        await router.push({ path: "/sftp", query: { focusTransfers: "true", focusTransferId: old.transferId, focusOperation: operation } });
        return;
      }
    }
    if (alive()) await navigateNativeTrayAction(action, router, workspace);
  } catch { /* Expired notifications never reconnect or trigger new notifications or protected interaction. */ }
}
