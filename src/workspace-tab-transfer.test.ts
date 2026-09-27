import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { hiddenOutgoingWorkspaceTabs, holdLocalWorkspaceTabClaim, moveWorkspaceTab, moveWorkspaceTabToNewWindow, registerWorkspaceHandoff, startWorkspaceTabTransfers, syncLocalWorkspaceTabRecords, visibleIncomingWorkspaceTabs } from "./workspace-tab-transfer";

const ipc = vi.hoisted(() => {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  return {
    listeners,
    listen: vi.fn(async (name: string, listener: (event: { payload: unknown }) => void) => {
      listeners.set(name, listener);
      return () => { listeners.delete(name); };
    }),
    emitTo: vi.fn(async () => undefined),
    snapshot: vi.fn(),
    register: vi.fn(),
    update: vi.fn(),
    unregister: vi.fn(),
    prepare: vi.fn(),
    freeze: vi.fn(),
    ready: vi.fn(),
    commit: vi.fn(),
    abort: vi.fn(),
    open: vi.fn(),
    close: vi.fn(async () => undefined),
    closeEmpty: vi.fn(async () => undefined),
    hide: vi.fn(async () => undefined),
    show: vi.fn(async () => undefined),
    focus: vi.fn(async () => undefined),
    ownerLabel: "main",
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: ipc.listen, emitTo: ipc.emitTo }));
vi.mock("./workspace-tab-windows", () => ({
  workspaceWindowLabel: () => ipc.ownerLabel,
  snapshotWorkspaceTabs: ipc.snapshot,
  registerWorkspaceTab: ipc.register,
  updateWorkspaceTab: ipc.update,
  unregisterWorkspaceTab: ipc.unregister,
  prepareWorkspaceTabTransfer: ipc.prepare,
  freezeWorkspaceTabTransfer: ipc.freeze,
  readyWorkspaceTabTransfer: ipc.ready,
  commitWorkspaceTabTransfer: ipc.commit,
  abortWorkspaceTabTransfer: ipc.abort,
  openWorkspaceWindow: ipc.open,
  closeWorkspaceWindow: ipc.close,
  closeEmptyWorkspaceWindow: ipc.closeEmpty,
  hideWorkspaceWindow: ipc.hide,
  showWorkspaceWindow: ipc.show,
  focusWorkspaceWindow: ipc.focus,
}));

describe("Workspace Tab owner handoff", () => {
  let stop: (() => void) | null = null;
  let dispose: (() => void) | null = null;
  const payload = { groupId: "page:knownHosts", pageType: "knownHosts", route: "/known-hosts" };
  const record = { id: "page:knownHosts", kind: "page", owner: "main", revision: 3, payload };
  const handler = {
    kind: "page" as const,
    owns: vi.fn(() => true),
    snapshot: vi.fn(async () => payload),
    freeze: vi.fn(async () => undefined),
    import: vi.fn(async () => undefined),
    commit: vi.fn(),
    rollback: vi.fn(async () => undefined),
    discard: vi.fn(async () => undefined),
    activate: vi.fn(),
    activateExisting: vi.fn(),
  };
  const prepareKind = vi.fn(async () => undefined);

  beforeEach(async () => {
    vi.clearAllMocks();
    handler.owns.mockReturnValue(true);
    ipc.ownerLabel = "main";
    ipc.snapshot.mockResolvedValue({ owned: [record], incoming: [], outgoing: [], others: [] });
    ipc.prepare.mockResolvedValue("ticket-1");
    ipc.commit.mockResolvedValue({ ...record, owner: "workspace-a", revision: 4 });
    ipc.abort.mockResolvedValue(undefined);
    dispose = registerWorkspaceHandoff(handler);
    stop = await startWorkspaceTabTransfers(prepareKind);
  });

  afterEach(() => {
    stop?.();
    dispose?.();
    stop = null;
    dispose = null;
    vi.useRealTimers();
  });

  it("does not recover a locally closed tab while its Core record awaits unregister", async () => {
    vi.useFakeTimers();
    await syncLocalWorkspaceTabRecords([{ id: record.id, kind: "page" }]);
    handler.owns.mockReturnValue(false);
    await syncLocalWorkspaceTabRecords([]);

    ipc.listeners.get("workspace-tab-state-changed")?.({ payload: record.id });
    await vi.advanceTimersByTimeAsync(0);

    expect(prepareKind).not.toHaveBeenCalled();
    expect(handler.import).not.toHaveBeenCalled();
    expect(handler.activate).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(16_000);
    expect(ipc.unregister).toHaveBeenCalledWith(record.id, record.revision);
  });

  it("still recovers a Core-owned tab not observed in this window", async () => {
    handler.owns.mockReturnValue(false);
    ipc.listeners.get("workspace-tab-state-changed")?.({ payload: record.id });

    await vi.waitFor(() => expect(handler.activate).toHaveBeenCalledWith(record.id));
    expect(prepareKind).toHaveBeenCalledWith("page");
    expect(handler.import).toHaveBeenCalledWith(record.id, record.payload);
  });

  it("does not recover a Page Tab before its local Core claim reaches Pinia", async () => {
    handler.owns.mockReturnValue(false);
    const release = holdLocalWorkspaceTabClaim(record.id);
    ipc.listeners.get("workspace-tab-state-changed")?.({ payload: record.id });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(prepareKind).not.toHaveBeenCalled();
    expect(handler.import).not.toHaveBeenCalled();
    release();
  });

  it("freezes before target readiness and releases source only after Core commits", async () => {
    ipc.freeze.mockImplementation(async () => {
      expect(handler.freeze).toHaveBeenCalledOnce();
      expect(handler.commit).not.toHaveBeenCalled();
      ipc.listeners.get("workspace-tab-target-ready")?.({ payload: "ticket-1" });
    });
    await moveWorkspaceTab(record.id, "page", "workspace-a");
    expect(ipc.prepare).toHaveBeenCalledWith(record.id, "workspace-a", 3, payload);
    expect(ipc.commit).toHaveBeenCalledWith("ticket-1");
    expect(handler.commit).toHaveBeenCalledOnce();
    expect(handler.rollback).not.toHaveBeenCalled();
  });

  it("aborts ownership and restores the source if the target rejects import", async () => {
    ipc.freeze.mockImplementation(async () => {
      ipc.listeners.get("workspace-tab-import-failed")?.({ payload: { ticket: "ticket-1" } });
    });
    await expect(moveWorkspaceTab(record.id, "page", "workspace-a"))
      .rejects.toThrow("workspace_tab.target_unavailable");
    expect(ipc.commit).not.toHaveBeenCalled();
    expect(ipc.abort).toHaveBeenCalledWith("ticket-1");
    expect(handler.rollback).toHaveBeenCalledOnce();
  });

  it("closes a child window after its final Tab has committed to main", async () => {
    ipc.ownerLabel = "workspace-a";
    ipc.snapshot.mockReset();
    ipc.snapshot.mockResolvedValueOnce({ owned: [{ ...record, owner: "workspace-a" }], incoming: [], outgoing: [], others: [] });
    ipc.snapshot.mockResolvedValueOnce({
      owned: [{ ...record, owner: "workspace-a" }],
      incoming: [],
      outgoing: [{ ticket: "ticket-1", tab: record, source: "workspace-a", target: "main", phase: "offered" }],
      others: [],
    });
    ipc.snapshot.mockResolvedValueOnce({ owned: [], incoming: [], outgoing: [], others: [] });
    ipc.freeze.mockImplementation(async () => {
      ipc.listeners.get("workspace-tab-target-ready")?.({ payload: "ticket-1" });
    });
    await moveWorkspaceTab(record.id, "page", "main");
    expect(ipc.hide).toHaveBeenCalledOnce();
    expect(ipc.close).toHaveBeenCalledOnce();
    expect(ipc.show).not.toHaveBeenCalled();
    expect(hiddenOutgoingWorkspaceTabs.value.size).toBe(0);
  });

  it("shows a hidden child again when the target rejects import", async () => {
    ipc.ownerLabel = "workspace-a";
    ipc.snapshot.mockReset();
    ipc.snapshot.mockResolvedValueOnce({ owned: [{ ...record, owner: "workspace-a" }], incoming: [], outgoing: [], others: [] });
    ipc.snapshot.mockResolvedValueOnce({
      owned: [{ ...record, owner: "workspace-a" }],
      incoming: [],
      outgoing: [{ ticket: "ticket-1", tab: record, source: "workspace-a", target: "main", phase: "offered" }],
      others: [],
    });
    ipc.freeze.mockImplementation(async () => {
      ipc.listeners.get("workspace-tab-import-failed")?.({ payload: { ticket: "ticket-1" } });
    });
    await expect(moveWorkspaceTab(record.id, "page", "main")).rejects.toThrow("workspace_tab.target_unavailable");
    expect(ipc.hide).toHaveBeenCalledOnce();
    expect(ipc.show).toHaveBeenCalledOnce();
    expect(handler.rollback).toHaveBeenCalledOnce();
    expect(visibleIncomingWorkspaceTabs.value).toBe(0);
  });

  it("shows incoming feedback while import is pending and clears it on failure", async () => {
    ipc.ownerLabel = "workspace-a";
    let rejectImport!: (error: Error) => void;
    handler.import.mockImplementationOnce(() => new Promise<undefined>((_resolve, reject) => {
      rejectImport = reject;
    }));
    ipc.snapshot.mockResolvedValue({
      owned: [],
      incoming: [{ ticket: "ticket-2", tab: record, source: "main", target: "workspace-a", phase: "offered" }],
      outgoing: [],
      others: [],
    });
    ipc.listeners.get("workspace-tab-offer")?.({ payload: null });
    await vi.waitFor(() => expect(visibleIncomingWorkspaceTabs.value).toBe(1));
    rejectImport(new Error("import failed"));
    await vi.waitFor(() => expect(visibleIncomingWorkspaceTabs.value).toBe(0));
  });

  it("closes a new empty window when another transfer wins the same Tab", async () => {
    let finishOpen!: (label: string) => void;
    let finishFreeze!: () => void;
    ipc.open.mockImplementationOnce(() => new Promise<string>((resolve) => { finishOpen = resolve; }));
    ipc.freeze.mockImplementationOnce(async () => {
      await new Promise<void>((resolve) => { finishFreeze = resolve; });
      ipc.listeners.get("workspace-tab-target-ready")?.({ payload: "ticket-1" });
    });

    const newWindow = moveWorkspaceTabToNewWindow(record.id, "page");
    await vi.waitFor(() => expect(ipc.open).toHaveBeenCalledOnce());
    const existingWindow = moveWorkspaceTab(record.id, "page", "workspace-b");
    await vi.waitFor(() => expect(ipc.freeze).toHaveBeenCalledOnce());
    finishOpen("workspace-a");
    await newWindow;
    expect(ipc.closeEmpty).toHaveBeenCalledWith("workspace-a");
    finishFreeze();
    await existingWindow;
  });
});
