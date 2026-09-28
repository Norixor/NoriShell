import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";

const shell = vi.hoisted(() => ({
  terminal: vi.fn(async () => "tab"), host: vi.fn(async () => "tab"), quickConnect: vi.fn(async () => undefined),
  showShellRoute: vi.fn(async (navigate?: () => unknown) => { await navigate?.(); }), focusTerminal: vi.fn(async () => undefined),
  focusDesktop: vi.fn(async () => undefined), focusFile: vi.fn(async () => undefined),
  transfer: vi.fn(async () => undefined),
}));

vi.mock("./workspace-tab-view-shell", () => ({
  createManagedTerminalTab: shell.terminal,
  createManagedTerminalForHost: shell.host,
  createManagedQuickConnect: shell.quickConnect,
  focusManagedTerminalSession: shell.focusTerminal,
  focusManagedDesktopSession: shell.focusDesktop,
  focusManagedFileSession: shell.focusFile,
  openManagedTransferTarget: shell.transfer,
  showWorkspaceShellRoute: shell.showShellRoute,
}));

import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { navigateNativeTrayAction } from "./native-tray-navigation";

function setup() {
  setActivePinia(createPinia());
  const workspace = useWorkspaceTabsStore();
  const router = createRouter({ history: createMemoryHistory(), routes: ["/terminal", "/settings", "/tunnels"].map((path) => ({ path, component: { template: "<div />" } })) });
  return { workspace, router };
}

describe("native tray navigation", () => {
  beforeEach(() => vi.clearAllMocks());

  it("creates an explicit local terminal as a managed Tab", async () => {
    const { workspace, router } = setup();
    await navigateNativeTrayAction({ kind: "newLocalTerminal" }, router, workspace);
    expect(shell.terminal).toHaveBeenCalledWith("local");
  });

  it("focuses only an existing managed Session", async () => {
    const { workspace, router } = setup();
    shell.focusTerminal.mockRejectedValueOnce(new Error("workspace_tab.session_unavailable"));
    const action = { kind: "focusSshSession" as const, sessionId: "s", generation: "9007199254740993" };
    await expect(navigateNativeTrayAction(action, router, workspace)).rejects.toThrow("workspace_tab.session_unavailable");
    expect(shell.focusTerminal).toHaveBeenCalledWith(action);
    expect(shell.terminal).not.toHaveBeenCalled();
  });

  it("does not navigate through a blocking dialog", async () => {
    const { workspace, router } = setup();
    const dialog = document.createElement("div"); dialog.setAttribute("role", "dialog"); dialog.setAttribute("aria-modal", "true"); document.body.append(dialog);
    try { await expect(navigateNativeTrayAction({ kind: "newLocalTerminal" }, router, workspace)).rejects.toThrow(); }
    finally { dialog.remove(); }
    expect(shell.terminal).not.toHaveBeenCalled();
  });

  it("preserves the exact transfer revision for destination validation", async () => {
    const { workspace, router } = setup();
    const sourceFence = { kind: "remoteSession" as const, sessionId: "s", generation: "1" };
    const targetFence = { kind: "localCapability" as const, directoryRef: "cap", revision: "1" };
    await navigateNativeTrayAction({ kind: "openTransfers", transferId: "transfer", stateRevision: "9007199254740993", sourceFence, targetFence }, router, workspace);
    expect(shell.transfer).toHaveBeenCalledWith(expect.objectContaining({ focusTransferId: "transfer", focusTransfers: "true" }),
      { kind: "intent", transferId: "transfer", minimumRevision: "9007199254740993", source: sourceFence, target: targetFence });
  });

  it("shows shell pages through the paint-ordered Tab release", async () => {
    const { workspace, router } = setup();
    await navigateNativeTrayAction({ kind: "settings" }, router, workspace);
    expect(shell.showShellRoute).toHaveBeenCalledOnce();
    expect(router.currentRoute.value.fullPath).toBe("/settings?section=desktop");
  });
});
