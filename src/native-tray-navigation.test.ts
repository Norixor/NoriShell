import { describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { navigateNativeTrayAction } from "./native-tray-navigation";

function setup() {
  setActivePinia(createPinia());
  const workspace = useWorkspaceTabsStore();
  const controller = {
    create: vi.fn(), createLocal: vi.fn(() => true), quickConnect: vi.fn(() => true),
    focusNativeSession: vi.fn(() => false), focusTelnetSession: vi.fn(() => false),
    focusSshSession: vi.fn(() => false), focusLocalSession: vi.fn(() => false),
    activate: vi.fn(), deactivate: vi.fn(), close: vi.fn(), closeMany: vi.fn(), toggleQuickCommands: vi.fn(),
  };
  workspace.registerTerminalController(controller);
  const router = createRouter({ history: createMemoryHistory(), routes: ["/terminal", "/settings", "/tunnels", "/sftp", "/desktop"].map((path) => ({ path, component: { template: "<div />" } })) });
  return { workspace, controller, router };
}
describe("native tray navigation", () => {
  it("creates explicit local terminals without invoking the default creation path", async () => {
    const { workspace, controller, router } = setup();
    await navigateNativeTrayAction({ kind: "newLocalTerminal" }, router, workspace);
    expect(controller.createLocal).toHaveBeenCalledOnce();
    expect(controller.create).not.toHaveBeenCalled();
  });
  it("rejects stale sessions without creating or reconnecting", async () => {
    const { workspace, controller, router } = setup();
    await expect(navigateNativeTrayAction({ kind: "focusSshSession", sessionId: "s", generation: "9007199254740993" }, router, workspace)).rejects.toThrow();
    expect(controller.focusSshSession).toHaveBeenCalledWith("s", "9007199254740993");
    expect(controller.create).not.toHaveBeenCalled();
  });
  it("does not navigate through a blocking dialog", async () => {
    const { workspace, controller, router } = setup();
    const dialog = document.createElement("div"); dialog.setAttribute("role", "dialog"); dialog.setAttribute("aria-modal", "true"); document.body.append(dialog);
    try { await expect(navigateNativeTrayAction({ kind: "newLocalTerminal" }, router, workspace)).rejects.toThrow(); }
    finally { dialog.remove(); }
    expect(controller.createLocal).not.toHaveBeenCalled();
  });
  it("preserves the exact transfer revision for destination validation", async () => {
    const { workspace, router } = setup();
    await navigateNativeTrayAction({ kind: "openTransfers", transferId: "transfer", stateRevision: "9007199254740993", sourceFence: {kind:"remoteSession",sessionId:"s",generation:"1"}, targetFence:{kind:"localCapability",directoryRef:"cap",revision:"1"} }, router, workspace);
    expect(router.currentRoute.value.query).toMatchObject({ focusTransferId: "transfer", focusTransfers: "true" });
  });
});
