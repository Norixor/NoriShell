import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";

import { useWorkspaceTabsStore } from "./stores/workspaceTabs";
import { navigateNativeTrayAction } from "./native-tray-navigation";

const managed = vi.hoisted(() => ({
  createManagedTerminalTab: vi.fn(),
  createManagedTerminalForHost: vi.fn(),
  createManagedQuickConnect: vi.fn(),
  focusManagedTerminalSession: vi.fn(),
  focusManagedDesktopSession: vi.fn(),
  focusManagedFileSession: vi.fn(),
  openManagedTransferTarget: vi.fn(),
}));

vi.mock("./core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("./core-api/client")>(),
  canUseDesktopCore: () => true,
}));
vi.mock("./workspace-tab-view-shell", () => managed);

function setup() {
  setActivePinia(createPinia());
  const workspace = useWorkspaceTabsStore();
  const router = createRouter({ history: createMemoryHistory(), routes: ["/terminal", "/desktop", "/sftp"].map((path) => ({ path, component: { template: "<div />" } })) });
  return { workspace, router };
}

beforeEach(() => { for (const mock of Object.values(managed)) mock.mockReset(); });

describe("native managed resource navigation", () => {
  it("focuses the original remote desktop session with its exact generation", async () => {
    const { workspace, router } = setup();
    await navigateNativeTrayAction({ kind: "focusDesktop", sessionId: "desktop", generation: "9007199254740993" }, router, workspace);
    expect(managed.focusManagedDesktopSession).toHaveBeenCalledExactlyOnceWith("desktop", "9007199254740993");
    expect(router.currentRoute.value.path).not.toBe("/desktop");
  });

  it("routes host creation and terminal focus through the manager", async () => {
    const { workspace, router } = setup();
    await navigateNativeTrayAction({ kind: "openHost", hostId: "host" }, router, workspace);
    expect(managed.createManagedTerminalForHost).toHaveBeenCalledWith(expect.objectContaining({ hostId: "host", source: "tray" }));
    await navigateNativeTrayAction({ kind: "focusSshSession", sessionId: "session", generation: "9007199254740993" }, router, workspace);
    expect(managed.focusManagedTerminalSession).toHaveBeenCalledExactlyOnceWith({ kind: "focusSshSession", sessionId: "session", generation: "9007199254740993" });
  });

  it("passes the exact transfer fence to the child realm instead of the main realm Map", async () => {
    const { workspace, router } = setup();
    const sourceFence = { kind: "remoteSession" as const, sessionId: "session", generation: "9007199254740993" };
    const targetFence = { kind: "localCapability" as const, directoryRef: "directory", revision: "7" };
    await navigateNativeTrayAction({ kind: "openTransfers", transferId: "transfer", stateRevision: "9007199254740993", sourceFence, targetFence }, router, workspace);
    expect(managed.openManagedTransferTarget).toHaveBeenCalledWith(expect.objectContaining({ focusTransferId: "transfer" }), {
      kind: "intent", transferId: "transfer", minimumRevision: "9007199254740993", source: sourceFence, target: targetFence,
    });
  });
});
