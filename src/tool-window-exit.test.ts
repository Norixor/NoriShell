import { beforeEach, describe, expect, it, vi } from "vitest";
import { registerToolWindowExit } from "./tool-window-exit";

const api = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: api.listen }));

describe("tool window application-exit barrier", () => {
  let receive: (event: { payload: { attemptId: string } }) => Promise<void>;
  beforeEach(() => {
    vi.clearAllMocks();
    api.invoke.mockResolvedValue(undefined);
    api.listen.mockImplementation(async (_name, callback) => { receive = callback; return api.unlisten; });
  });

  it("waits for an event-driven dirty dialog rather than reporting false as refusal", async () => {
    const close = vi.fn();
    const controller = await registerToolWindowExit({ requestClose: async () => false, isBusy: () => false, close });
    await receive({ payload: { attemptId: "attempt-a" } });
    expect(api.invoke).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    await controller.keepOpen();
    expect(api.invoke).toHaveBeenCalledWith("tool_window_exit_reply", { attemptId: "attempt-a", approved: false });
    expect(close).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("refuses exit immediately while saving without invoking editor cleanup", async () => {
    const requestClose = vi.fn(), close = vi.fn();
    const controller = await registerToolWindowExit({ requestClose, isBusy: () => true, close });
    await receive({ payload: { attemptId: "attempt-b" } });
    expect(requestClose).not.toHaveBeenCalled();
    expect(api.invoke).toHaveBeenCalledWith("tool_window_exit_reply", { attemptId: "attempt-b", approved: false });
    expect(close).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("reports approval before destroying the cleaned window", async () => {
    const close = vi.fn(async () => { expect(api.invoke).toHaveBeenCalledWith("tool_window_exit_reply", { attemptId: "attempt-c", approved: true }); });
    const controller = await registerToolWindowExit({ requestClose: async () => true, isBusy: () => false, close });
    await receive({ payload: { attemptId: "attempt-c" } });
    expect(close).toHaveBeenCalledOnce();
    controller.dispose();
  });

  it("reuses a pending confirmation for a newer exit attempt and fences its old response", async () => {
    let resolve!: (value: boolean) => void;
    const requestClose = vi.fn(() => new Promise<boolean>(done => { resolve = done; }));
    const close = vi.fn();
    const controller = await registerToolWindowExit({ requestClose, isBusy: () => false, close });
    const old = receive({ payload: { attemptId: "old" } });
    const latest = receive({ payload: { attemptId: "latest" } });
    expect(requestClose).toHaveBeenCalledOnce();
    resolve(true);
    await Promise.all([old, latest]);
    expect(api.invoke).toHaveBeenCalledExactlyOnceWith("tool_window_exit_reply", { attemptId: "latest", approved: true });
    expect(close).toHaveBeenCalledOnce();
    controller.dispose();
  });

  it("ignores late cleanup approval after explicit refusal", async () => {
    let resolve!: (value: boolean) => void;
    const close = vi.fn();
    const controller = await registerToolWindowExit({ requestClose: () => new Promise(done => { resolve = done; }), isBusy: () => false, close });
    const pending = receive({ payload: { attemptId: "attempt-d" } });
    await controller.keepOpen();
    resolve(true);
    await pending;
    expect(close).not.toHaveBeenCalled();
    expect(api.invoke).toHaveBeenCalledTimes(1);
    controller.dispose();
    expect(api.unlisten).toHaveBeenCalledOnce();
  });
});
