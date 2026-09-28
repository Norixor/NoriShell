import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { closeChildWorkspaceWindowIfEmpty, moveWorkspaceTab, moveWorkspaceTabToNewWindow } from "./workspace-tab-transfer";

const native = vi.hoisted(() => ({
  owner: "main",
  snapshot: vi.fn(),
  moveView: vi.fn(),
  openWindow: vi.fn(),
  closeWindow: vi.fn(),
  closeEmpty: vi.fn(),
}));

vi.mock("./workspace-tab-windows", () => ({
  workspaceWindowLabel: () => native.owner,
  snapshotWorkspaceTabs: native.snapshot,
  moveWorkspaceTabView: native.moveView,
  openWorkspaceWindow: native.openWindow,
  closeWorkspaceWindow: native.closeWindow,
  closeEmptyWorkspaceWindow: native.closeEmpty,
}));

const id = "page:knownHosts";
const payload = { groupId: id, pageType: "knownHosts", route: "/known-hosts" };
const record = { id, kind: "page" as const, owner: "main", payload };
const view = { id, label: `workspace-tab-${id}`, ownerWindow: "main", created: false };

describe("native Workspace Tab movement", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    native.owner = "main";
    native.snapshot.mockResolvedValue({ owned: [record], others: [] });
    native.moveView.mockResolvedValue({ ...view, ownerWindow: "workspace-a" });
    native.openWindow.mockResolvedValue("workspace-a");
    native.closeWindow.mockResolvedValue(undefined);
    native.closeEmpty.mockResolvedValue(undefined);
  });
  afterEach(() => { vi.useRealTimers(); });

  it("moves the existing renderer instead of replaying a projection", async () => {
    expect(await moveWorkspaceTab(id, "workspace-a")).toBe(true);
    expect(native.moveView).toHaveBeenCalledExactlyOnceWith(id, "workspace-a");
    expect(native.snapshot).not.toHaveBeenCalled();
  });

  it("lets Core reject a stale owner at the movement boundary", async () => {
    native.moveView.mockRejectedValueOnce("workspace_tab.wrong_owner");
    await expect(moveWorkspaceTab(id, "workspace-a"))
      .rejects.toBe("workspace_tab.wrong_owner");
    expect(native.snapshot).not.toHaveBeenCalled();
  });

  it("does not ask native to move to the current owner", async () => {
    expect(await moveWorkspaceTab(id, "main")).toBe(false);
    expect(native.moveView).not.toHaveBeenCalled();
  });

  it("closes an emptied auxiliary window after the move succeeds", async () => {
    native.owner = "workspace-a";
    native.snapshot.mockResolvedValueOnce({ owned: [], others: [] });
    await moveWorkspaceTab(id, "main");
    await vi.waitFor(() => expect(native.closeWindow).toHaveBeenCalledOnce());
  });

  it("does not misreport a completed reparent when source-window cleanup fails", async () => {
    native.owner = "workspace-a";
    native.snapshot.mockRejectedValueOnce("workspace_window.unavailable");
    await expect(moveWorkspaceTab(id, "main")).resolves.toBe(true);
    await vi.waitFor(() => expect(native.snapshot).toHaveBeenCalledOnce());
  });

  it("lets explicit window close own cleanup after moving its tabs", async () => {
    native.owner = "workspace-a";
    await moveWorkspaceTab(id, "main", false);
    expect(native.snapshot).not.toHaveBeenCalled();
    expect(native.closeWindow).not.toHaveBeenCalled();
  });

  it("closes an auxiliary window after its last Tab closes, but keeps occupied and main windows", async () => {
    native.owner = "workspace-a";
    await closeChildWorkspaceWindowIfEmpty();
    expect(native.closeWindow).not.toHaveBeenCalled();
    native.snapshot.mockResolvedValue({ owned: [], others: [] });
    await closeChildWorkspaceWindowIfEmpty();
    expect(native.closeWindow).toHaveBeenCalledOnce();
    native.owner = "main";
    await closeChildWorkspaceWindowIfEmpty();
    expect(native.closeWindow).toHaveBeenCalledOnce();
  });

  it("retains the source window if native rejects reparenting", async () => {
    native.owner = "workspace-a";
    native.moveView.mockRejectedValueOnce("workspace_tab.target_unavailable");
    await expect(moveWorkspaceTab(id, "main"))
      .rejects.toBe("workspace_tab.target_unavailable");
    expect(native.closeWindow).not.toHaveBeenCalled();
  });

  it("closes a newly opened empty window when the native move fails", async () => {
    native.moveView.mockRejectedValueOnce("workspace_tab.target_unavailable");
    await expect(moveWorkspaceTabToNewWindow(id, { x: 20, y: 30 }))
      .rejects.toBe("workspace_tab.target_unavailable");
    expect(native.openWindow).toHaveBeenCalledWith({ x: 20, y: 30 });
    expect(native.closeEmpty).toHaveBeenCalledWith("workspace-a");
  });

});
