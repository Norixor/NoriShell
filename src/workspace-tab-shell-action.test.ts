import { beforeEach, describe, expect, it, vi } from "vitest";

const bridge = vi.hoisted(() => ({
  emitTo: vi.fn(async (label: string, event: string, payload: unknown) => {
    void label; void event; void payload;
  }),
  listen: vi.fn(),
  context: vi.fn(async () => ({ id: "tab-one", ownerWindow: "main" })),
  tabView: true,
  perform: vi.fn(async () => "created"),
}));

vi.mock("@tauri-apps/api/event", () => ({ emitTo: bridge.emitTo, listen: bridge.listen }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ label: "workspace-tab-tab-one" }) }));
vi.mock("./workspace-window-context", () => ({ isWorkspaceTabView: () => bridge.tabView }));
vi.mock("./workspace-tab-windows", () => ({ getWorkspaceTabContext: bridge.context }));
vi.mock("./workspace-tab-view-shell", () => ({ performWorkspaceTabShellAction: bridge.perform }));

import { openWorkspaceTerminalHost, requestWorkspaceTabShellAction } from "./workspace-tab-shell-action";

describe("workspace Tab shell action result", () => {
  let receive: ((event: { payload: { id: string; operationId: string; code?: string } }) => void) | null;
  beforeEach(() => {
    vi.clearAllMocks();
    bridge.tabView = true;
    window.history.replaceState(null, "", "/workspace-tab.html?tabId=tab-one");
    receive = null;
    bridge.listen.mockImplementation(async (_name, handler) => {
      receive = handler;
      return vi.fn();
    });
  });

  it("waits for the shell's completed operation instead of treating delivery as success", async () => {
    let resolved = false;
    const pending = requestWorkspaceTabShellAction({ type: "new-desktop", profileId: "profile-one" })
      .then(() => { resolved = true; });
    await vi.waitFor(() => expect(bridge.emitTo).toHaveBeenCalledOnce());
    expect(resolved).toBe(false);
    const message = bridge.emitTo.mock.calls[0]![2] as { id: string; operationId: string };
    receive!({ payload: { id: message.id, operationId: message.operationId } });
    await pending;
    expect(resolved).toBe(true);
  });

  it("returns the shell's stable failure code to the source view", async () => {
    const pending = requestWorkspaceTabShellAction({ type: "new-desktop", profileId: "profile-one" });
    await vi.waitFor(() => expect(bridge.emitTo).toHaveBeenCalledOnce());
    const message = bridge.emitTo.mock.calls[0]![2] as { id: string; operationId: string };
    receive!({ payload: { id: message.id, operationId: message.operationId,
      code: "workspace_tab.desktop_profile_missing" } });
    await expect(pending).rejects.toThrow("workspace_tab.desktop_profile_missing");
  });

  it("closes a disposable source only after the target was created", async () => {
    const pending = requestWorkspaceTabShellAction({ type: "new-file", kind: "local" },
      { closeSourceOnSuccess: true });
    await vi.waitFor(() => expect(bridge.emitTo).toHaveBeenCalledOnce());
    const message = bridge.emitTo.mock.calls[0]![2] as { id: string; operationId: string };
    expect(bridge.emitTo.mock.calls[0]![0]).toEqual({ kind: "Webview", label: "main" });
    expect(bridge.emitTo.mock.calls[0]![1]).toBe("workspace-tab-shell-action");
    expect(message).toMatchObject({ ownerWindow: "main" });
    receive!({ payload: { id: message.id, operationId: message.operationId } });
    await expect(pending).resolves.toBeUndefined();
    expect(bridge.emitTo).toHaveBeenCalledOnce();
  });

  it("preserves a disposable source if target creation fails", async () => {
    const pending = requestWorkspaceTabShellAction({ type: "new-file", kind: "local" },
      { closeSourceOnSuccess: true });
    await vi.waitFor(() => expect(bridge.emitTo).toHaveBeenCalledOnce());
    const message = bridge.emitTo.mock.calls[0]![2] as { id: string; operationId: string };
    receive!({ payload: { id: message.id, operationId: message.operationId,
      code: "workspace_tab.view_create_failed" } });
    await expect(pending).rejects.toThrow("workspace_tab.view_create_failed");
    expect(bridge.emitTo).toHaveBeenCalledOnce();
  });

  it("forwards a Tab WebView's Host launch to its owner shell", async () => {
    const pending = openWorkspaceTerminalHost({ hostId: "host-one", source: "hosts" });
    await vi.waitFor(() => expect(bridge.emitTo).toHaveBeenCalledOnce());
    const message = bridge.emitTo.mock.calls[0]![2] as { id: string; operationId: string; action: unknown };
    expect(message.action).toMatchObject({ type: "navigate", path: "/terminal",
      query: { hostId: "host-one", source: "hosts", connectOperationId: expect.any(String) } });
    receive!({ payload: { id: message.id, operationId: message.operationId } });
    await pending;
    expect(bridge.perform).not.toHaveBeenCalled();
  });

  it("lets a shell page create the managed Tab directly", async () => {
    bridge.tabView = false;
    await openWorkspaceTerminalHost({ hostId: "host-one" });
    expect(bridge.emitTo).not.toHaveBeenCalled();
    expect(bridge.perform).toHaveBeenCalledWith({ type: "navigate", path: "/terminal",
      query: { hostId: "host-one", connectOperationId: expect.any(String) } });
  });

  it("rejects a shell request from a renderer that is not a Tab WebView", async () => {
    bridge.tabView = false;
    await expect(requestWorkspaceTabShellAction({ type: "new-terminal" })).rejects.toThrow("workspace_tab.invalid_view");
  });
});
