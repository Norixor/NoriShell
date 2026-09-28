import { afterEach, describe, expect, it, vi } from "vitest";

import {
  flushTerminalWorkspaceBeforeExit,
  registerTerminalWorkspaceFlush,
  registerRemoteTerminalWorkspaceFlush,
  requestExitAfterTerminalWorkspaceFlush,
  resetTerminalWorkspaceFlushForTests,
} from "./terminal-workspace-persistence";

describe("terminal workspace exit barrier", () => {
  afterEach(resetTerminalWorkspaceFlushForTests);

  it("awaits the active workspace flush and unregisters only its own handler", async () => {
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    const flush = vi.fn(() => pending);
    const unregister = registerTerminalWorkspaceFlush(flush);
    let completed = false;
    const barrier = flushTerminalWorkspaceBeforeExit().then(() => {
      completed = true;
    });

    await Promise.resolve();
    expect(flush).toHaveBeenCalledTimes(1);
    expect(completed).toBe(false);
    release();
    await barrier;
    expect(completed).toBe(true);

    unregister();
    await flushTerminalWorkspaceBeforeExit();
    expect(flush).toHaveBeenCalledTimes(1);
  });

  it("does not authorize native exit when the durable flush fails", async () => {
    const failure = new Error("layout write failed");
    registerTerminalWorkspaceFlush(() => Promise.reject(failure));
    const requestExit = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);

    await expect(requestExitAfterTerminalWorkspaceFlush(requestExit)).rejects.toBe(failure);
    expect(requestExit).not.toHaveBeenCalled();
  });

  it("waits for every Tab WebView layout before native exit", async () => {
    let releaseRemote!: () => void;
    const remotePending = new Promise<void>((resolve) => { releaseRemote = resolve; });
    const local = vi.fn(async () => undefined);
    const remote = vi.fn(() => remotePending);
    registerTerminalWorkspaceFlush(local);
    const unregisterRemote = registerRemoteTerminalWorkspaceFlush(remote);
    const requestExit = vi.fn(async () => undefined);
    const exit = requestExitAfterTerminalWorkspaceFlush(requestExit);
    await vi.waitFor(() => expect(remote).toHaveBeenCalledOnce());
    expect(local).toHaveBeenCalledOnce();
    expect(requestExit).not.toHaveBeenCalled();
    releaseRemote();
    await exit;
    expect(requestExit).toHaveBeenCalledOnce();
    unregisterRemote();
  });
});
