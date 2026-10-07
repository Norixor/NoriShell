import { registerNativeTransferNavigation } from "./native-transfer-navigation";
import type { NativeTransferNavigationTarget } from "./native-transfer-navigation";
import type { Router } from "vue-router";
import type { NativeResourceNotificationClick } from "./core-api/native-notifications";
import type { NativeTrayAction } from "./core-api/generated/core-api";
import { fetchSshSessionSnapshot, fetchTelnetSessionSnapshot, fetchForwardSessionSnapshot, fetchSftpSessionSnapshot, fetchSftpTransferIntentSnapshot } from "./core-api/client";
import { canUseDesktopCore } from "./core-api/client";
import { desktopClient } from "./core-api/desktop-client";
import type { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { navigateNativeTrayAction } from "./native-tray-navigation";
import { openManagedTransferTarget } from "./workspace-tab-view-shell";
import { isMobileLayout, workspaceHostMode } from "./platform-host";
import { i18n } from "./locales";
import { useTipsStore } from "./stores/tips";

/** Phones have no tray or window to fall back to, so a tap on a stale notification says why nothing opened. */
function reportResourceGone(alive: () => boolean) {
  if (!isMobileLayout() || !alive()) return;
  useTipsStore().show({ scope: "native-notification", tone: "info", title: i18n.global.t("mobile.notificationResourceGone") });
}

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
        if (!session) { reportResourceGone(alive); return; }
        action = { kind: "focusSshSession", sessionId: session.sessionId, generation: session.generation }; break;
      }
      case "telnet": {
        const session = (await fetchTelnetSessionSnapshot()).sessions.find((item) => item.sessionId === payload.sessionId && item.generation === payload.generation && item.stateRevision === payload.stateRevision);
        if (!session) { reportResourceGone(alive); return; }
        action = { kind: "focusTelnet", sessionId: session.sessionId, generation: session.generation, socketId: session.socketId }; break;
      }
      case "forward": {
        const session = (await fetchForwardSessionSnapshot()).sessions.find((item) => item.sessionId === payload.sessionId && item.generation === payload.generation && item.stateRevision === payload.stateRevision);
        if (!session) { reportResourceGone(alive); return; }
        action = { kind: "openTunnels", sessionId: session.sessionId, generation: session.generation }; break;
      }
      case "desktop": {
        const session = (await desktopClient.snapshot()).find((item) => item.id === payload.sessionId && item.generation === payload.generation && item.revision === payload.revision);
        if (!session) { reportResourceGone(alive); return; }
        if (workspaceHostMode() !== "nativeTabs") {
          // The single document owns the Desktop page; the page verifies the generation itself.
          if (!alive() || workspace.terminalActivationBlocked || document.querySelector('[role="dialog"][aria-modal="true"]')) return;
          workspace.terminalController?.deactivate();
          await router.push({ path: "/desktop", query: { focusSessionId: session.id, focusGeneration: session.generation, focusOperation: crypto.randomUUID() } });
          return;
        }
        action = { kind: "focusDesktop", sessionId: session.id, generation: session.generation }; break;
      }
      case "sftpTransfer": {
        const [intents, legacy] = await Promise.all([fetchSftpTransferIntentSnapshot(), fetchSftpSessionSnapshot()]);
        const transfer = intents.transfers.find((item) => item.transferId === payload.transferId && item.stateRevision === payload.stateRevision);
        if (transfer && workspaceHostMode() !== "nativeTabs") {
          // Single document: the SFTP page consumes the registered target like a legacy transfer.
          if (!alive() || workspace.terminalActivationBlocked || document.querySelector('[role="dialog"][aria-modal="true"]')) return;
          const operation = crypto.randomUUID();
          registerNativeTransferNavigation(operation, { kind: "intent", transferId: transfer.transferId, minimumRevision: transfer.stateRevision, source: transfer.sourceFence, target: transfer.targetFence });
          workspace.terminalController?.deactivate();
          await router.push({ path: "/sftp", query: { focusTransfers: "true", focusTransferId: transfer.transferId, focusOperation: operation } });
          return;
        }
        if (transfer) {
          action = { kind: "openTransfers", transferId: transfer.transferId, stateRevision: transfer.stateRevision, sourceFence: transfer.sourceFence, targetFence: transfer.targetFence }; break;
        }
        const old = legacy.transfers.find((item) => item.transferId === payload.transferId && item.stateRevision === payload.stateRevision);
        if (!old) { reportResourceGone(alive); return; }
        if (!alive() || workspace.terminalActivationBlocked || document.querySelector('[role="dialog"][aria-modal="true"]')) return;
        const operation = crypto.randomUUID();
        const target: NativeTransferNavigationTarget = { kind: "legacy", transferId: old.transferId, minimumRevision: old.stateRevision, sessionId: old.sessionId, generation: old.generation };
        const query = { focusTransfers: "true", focusTransferId: old.transferId, focusOperation: operation };
        // Single document (Android) has no native file Tab to activate; the SFTP page consumes the registered target.
        if (canUseDesktopCore() && workspaceHostMode() === "nativeTabs") {
          await openManagedTransferTarget(query, target);
          return;
        }
        registerNativeTransferNavigation(operation, target);
        workspace.terminalController?.deactivate();
        await router.push({ path: "/sftp", query });
        return;
      }
    }
    if (alive()) await navigateNativeTrayAction(action, router, workspace);
  } catch { /* Expired notifications never reconnect or trigger new notifications or protected interaction. */ }
}
