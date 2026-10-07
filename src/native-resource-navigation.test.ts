import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "./locales";
import { takeNativeTransferNavigation } from "./native-transfer-navigation";
import { useTipsStore } from "./stores/tips";

const host = vi.hoisted(() => ({ mobile: true, mode: "singleDocument" as "singleDocument" | "nativeTabs" }));
vi.mock("./platform-host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./platform-host")>()),
  isMobileLayout: () => host.mobile,
  workspaceHostMode: () => host.mode,
}));
const client = vi.hoisted(() => ({
  ssh: vi.fn(), telnet: vi.fn(), forward: vi.fn(), sftp: vi.fn(), intents: vi.fn(), desktop: vi.fn(), canUseDesktopCore: () => true,
}));
vi.mock("./core-api/client", () => ({
  fetchSshSessionSnapshot: client.ssh, fetchTelnetSessionSnapshot: client.telnet, fetchForwardSessionSnapshot: client.forward,
  fetchSftpSessionSnapshot: client.sftp, fetchSftpTransferIntentSnapshot: client.intents, canUseDesktopCore: client.canUseDesktopCore,
}));
vi.mock("./core-api/desktop-client", () => ({ desktopClient: { snapshot: client.desktop } }));
const shell = vi.hoisted(() => ({ tray: vi.fn(), transfer: vi.fn() }));
vi.mock("./native-tray-navigation", () => ({ navigateNativeTrayAction: shell.tray }));
vi.mock("./workspace-tab-view-shell", () => ({ openManagedTransferTarget: shell.transfer }));

import { navigateNativeResourceNotification } from "./native-resource-navigation";

function harness() {
  const router = { push: vi.fn<(location: { path: string; query: Record<string, string> }) => Promise<void>>(async () => undefined) };
  const workspace = { terminalActivationBlocked: false, terminalController: { deactivate: vi.fn() } };
  return { router, workspace, run: (payload: Parameters<typeof navigateNativeResourceNotification>[0]) =>
    navigateNativeResourceNotification(payload, router as never, workspace as never, () => true) };
}

describe("native resource notification taps on the phone shell", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    i18n.global.locale.value = "en";
    host.mobile = true;
    host.mode = "singleDocument";
    vi.clearAllMocks();
    document.body.innerHTML = "";
  });

  it("tells the user when the notified session no longer exists instead of failing silently", async () => {
    client.ssh.mockResolvedValue({ sessions: [{ sessionId: "s1", generation: "2", stateRevision: "9" }] });
    const { run, router } = harness();
    await run({ kind: "ssh", eventId: "e1", sessionId: "s1", generation: "1", stateRevision: "9" });
    expect(useTipsStore().items.map((tip) => tip.title)).toEqual(["That resource is no longer available."]);
    expect(router.push).not.toHaveBeenCalled();
    expect(shell.tray).not.toHaveBeenCalled();
  });

  it("stays silent on desktop so the existing behavior is unchanged", async () => {
    host.mobile = false;
    host.mode = "nativeTabs";
    client.telnet.mockResolvedValue({ sessions: [] });
    const { run } = harness();
    await run({ kind: "telnet", eventId: "e2", sessionId: "t1", generation: "1", stateRevision: "1" });
    expect(useTipsStore().items).toHaveLength(0);
  });

  it("focuses a live terminal session through the shared tray action path", async () => {
    client.ssh.mockResolvedValue({ sessions: [{ sessionId: "s1", generation: "1", stateRevision: "9" }] });
    const { run } = harness();
    await run({ kind: "ssh", eventId: "e3", sessionId: "s1", generation: "1", stateRevision: "9" });
    expect(shell.tray).toHaveBeenCalledWith(expect.objectContaining({ kind: "focusSshSession", sessionId: "s1", generation: "1" }), expect.anything(), expect.anything());
    expect(useTipsStore().items).toHaveLength(0);
  });

  it("opens the Desktop page of the single document with the exact session focus", async () => {
    client.desktop.mockResolvedValue([{ id: "d1", generation: "3", revision: "5" }]);
    const { run, router, workspace } = harness();
    await run({ kind: "desktop", eventId: "e4", sessionId: "d1", generation: "3", revision: "5" });
    expect(workspace.terminalController.deactivate).toHaveBeenCalledTimes(1);
    expect(router.push).toHaveBeenCalledWith({ path: "/desktop", query: expect.objectContaining({ focusSessionId: "d1", focusGeneration: "3", focusOperation: expect.any(String) }) });
    expect(shell.tray).not.toHaveBeenCalled();
  });

  it("registers an intent transfer target and opens the SFTP page of the single document", async () => {
    const fence = { kind: "remoteSession", sessionId: "f1", generation: "1" };
    client.intents.mockResolvedValue({ transfers: [{ transferId: "tr1", stateRevision: "4", sourceFence: fence, targetFence: { kind: "local" } }] });
    client.sftp.mockResolvedValue({ transfers: [] });
    const { run, router } = harness();
    await run({ kind: "sftpTransfer", eventId: "e5", transferId: "tr1", stateRevision: "4" });
    const call = router.push.mock.calls[0]![0];
    expect(call.path).toBe("/sftp");
    expect(call.query.focusTransferId).toBe("tr1");
    expect(takeNativeTransferNavigation(call.query.focusOperation!)).toEqual(expect.objectContaining({ kind: "intent", transferId: "tr1", minimumRevision: "4" }));
    expect(shell.transfer).not.toHaveBeenCalled();
  });

  it("opens a direct (legacy) transfer on the SFTP page of the single document instead of a native file Tab", async () => {
    client.intents.mockResolvedValue({ transfers: [] });
    client.sftp.mockResolvedValue({ transfers: [{ transferId: "tr2", stateRevision: "7", sessionId: "f2", generation: "3" }] });
    const { run, router, workspace } = harness();
    await run({ kind: "sftpTransfer", eventId: "e6", transferId: "tr2", stateRevision: "7" });
    expect(shell.transfer).not.toHaveBeenCalled();
    expect(workspace.terminalController.deactivate).toHaveBeenCalledTimes(1);
    const call = router.push.mock.calls[0]![0];
    expect(call.path).toBe("/sftp");
    expect(call.query).toEqual(expect.objectContaining({ focusTransfers: "true", focusTransferId: "tr2" }));
    expect(takeNativeTransferNavigation(call.query.focusOperation!)).toEqual({ kind: "legacy", transferId: "tr2", minimumRevision: "7", sessionId: "f2", generation: "3" });
  });

  it("keeps routing direct transfers through the native file Tab on desktop", async () => {
    host.mobile = false;
    host.mode = "nativeTabs";
    client.intents.mockResolvedValue({ transfers: [] });
    client.sftp.mockResolvedValue({ transfers: [{ transferId: "tr3", stateRevision: "2", sessionId: "f3", generation: "1" }] });
    const { run, router } = harness();
    await run({ kind: "sftpTransfer", eventId: "e7", transferId: "tr3", stateRevision: "2" });
    expect(router.push).not.toHaveBeenCalled();
    expect(shell.transfer).toHaveBeenCalledWith(expect.objectContaining({ focusTransferId: "tr3" }), { kind: "legacy", transferId: "tr3", minimumRevision: "2", sessionId: "f3", generation: "1" });
  });
});
