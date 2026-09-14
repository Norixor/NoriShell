import { beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  changeTerminalInputFocus: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
}));

vi.mock("./core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./core-api/client")>();
  return { ...actual, ...client };
});

import type {
  TerminalInputFocusChangeResponse,
  TerminalInputFocusTarget,
  TerminalInputLease,
} from "./core-api/generated/core-api";
import {
  canRunInFocusedTerminal,
  captureTerminalInput,
  clearTerminalInputFocus,
  focusTerminalInputTarget,
  registerTerminalInputTarget,
  resetTerminalInputFocusForTests,
  runInFocusedTerminal,
} from "./terminal-input-target";

function focusTarget(id: string): TerminalInputFocusTarget {
  return {
    kind: "ssh",
    target: {
      sessionId: `019d0000-0000-7000-8000-000000000${id}1`,
      expectedGeneration: "1",
      expectedStateRevision: "2",
      channelId: `019d0000-0000-7000-8000-000000000${id}2`,
      attachmentId: `019d0000-0000-7000-8000-000000000${id}3`,
      viewId: `019d0000-0000-7000-8000-000000000${id}4`,
    },
  };
}

function lease(target: TerminalInputFocusTarget, epoch: string): TerminalInputLease {
  if (target.kind !== "ssh") throw new Error("expected SSH target");
  return {
    kind: "ssh",
    lease: {
      leaseId: `019d0000-0000-7000-8000-000000000${epoch}5`,
      sessionId: target.target.sessionId,
      generation: target.target.expectedGeneration,
      attachmentId: target.target.attachmentId,
      viewId: target.target.viewId,
      focusEpoch: epoch,
      inputEpoch: epoch,
      expiresAtUnixMs: Date.now() + 60_000,
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("focused terminal input target", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetTerminalInputFocusForTests();
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({
      focusEpoch: "0",
      target: null,
      lease: null,
    });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target }) => {
      const focusEpoch = (BigInt(expectedFocusEpoch) + 1n).toString();
      return { focusEpoch, target, lease: target ? lease(target, focusEpoch) : null };
    });
  });

  it("disables input before ack and sends only to the acknowledged target", async () => {
    const gate = deferred<TerminalInputFocusChangeResponse>();
    const target = focusTarget("01");
    const send = vi.fn().mockResolvedValue(undefined);
    const applyFocusLease = vi.fn();
    client.changeTerminalInputFocus.mockReturnValueOnce(gate.promise);
    const unregister = registerTerminalInputTarget({
      id: "first",
      label: () => "First",
      focusTarget: () => target,
      applyFocusLease,
      canAcceptInput: () => true,
      send,
    });

    const pending = focusTerminalInputTarget("first");
    expect(canRunInFocusedTerminal.value).toBe(false);
    expect(await runInFocusedTerminal("pwd")).toBe("unavailable");
    gate.resolve({ focusEpoch: "1", target, lease: lease(target, "1") });
    await pending;
    expect(canRunInFocusedTerminal.value).toBe(true);
    expect(await runInFocusedTerminal("df -h")).toBe("sent");
    expect(send).toHaveBeenCalledWith("df -h\r");
    unregister();
  });

  it("does not let an old ack overwrite a rapid A to B to A intent", async () => {
    const gates = [
      deferred<TerminalInputFocusChangeResponse>(),
      deferred<TerminalInputFocusChangeResponse>(),
      deferred<TerminalInputFocusChangeResponse>(),
    ];
    const first = focusTarget("11");
    const second = focusTarget("21");
    const firstApply = vi.fn();
    const secondApply = vi.fn();
    const unregisterFirst = registerTerminalInputTarget({
      id: "first",
      label: () => "First",
      focusTarget: () => first,
      applyFocusLease: firstApply,
      canAcceptInput: () => true,
      send: vi.fn(),
    });
    const unregisterSecond = registerTerminalInputTarget({
      id: "second",
      label: () => "Second",
      focusTarget: () => second,
      applyFocusLease: secondApply,
      canAcceptInput: () => true,
      send: vi.fn(),
    });
    client.changeTerminalInputFocus
      .mockReturnValueOnce(gates[0]!.promise)
      .mockReturnValueOnce(gates[1]!.promise)
      .mockReturnValueOnce(gates[2]!.promise);

    const a1 = focusTerminalInputTarget("first");
    const b = focusTerminalInputTarget("second");
    const a2 = focusTerminalInputTarget("first");
    gates[0]!.resolve({ focusEpoch: "1", target: first, lease: lease(first, "1") });
    await a1;
    expect(firstApply).not.toHaveBeenCalled();
    gates[1]!.resolve({ focusEpoch: "2", target: second, lease: lease(second, "2") });
    await b;
    expect(secondApply).not.toHaveBeenCalled();
    const latestLease = lease(first, "3");
    gates[2]!.resolve({ focusEpoch: "3", target: first, lease: latestLease });
    await a2;
    expect(firstApply).toHaveBeenLastCalledWith(latestLease);
    expect(canRunInFocusedTerminal.value).toBe(true);
    unregisterSecond();
    unregisterFirst();
  });

  it("acknowledges blank focus clear before remaining writable again", async () => {
    const target = focusTarget("31");
    const unregister = registerTerminalInputTarget({
      id: "target",
      label: () => "Target",
      focusTarget: () => target,
      applyFocusLease: vi.fn(),
      canAcceptInput: () => true,
      send: vi.fn(),
    });
    await focusTerminalInputTarget("target");
    const gate = deferred<TerminalInputFocusChangeResponse>();
    client.changeTerminalInputFocus.mockReturnValueOnce(gate.promise);
    const pending = clearTerminalInputFocus();
    expect(canRunInFocusedTerminal.value).toBe(false);
    gate.resolve({ focusEpoch: "2", target: null, lease: null });
    await pending;
    expect(canRunInFocusedTerminal.value).toBe(false);
    expect(client.changeTerminalInputFocus).toHaveBeenLastCalledWith({
      expectedFocusEpoch: "1",
      target: null,
    });
    unregister();
  });

  it("keeps the acknowledged target writable across repeated pointer-like focus", async () => {
    const target = focusTarget("35");
    const applyFocusLease = vi.fn();
    const unregister = registerTerminalInputTarget({
      id: "pointer",
      label: () => "Pointer",
      focusTarget: () => target,
      applyFocusLease,
      canAcceptInput: () => true,
      canAcceptRawInput: () => true,
      send: vi.fn(),
    });

    await focusTerminalInputTarget("pointer");
    expect(canRunInFocusedTerminal.value).toBe(true);
    const acknowledgedLease = applyFocusLease.mock.calls.at(-1)?.[0];

    await expect(focusTerminalInputTarget("pointer")).resolves.toBe(true);

    expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(1);
    expect(applyFocusLease).toHaveBeenCalledTimes(1);
    expect(applyFocusLease).toHaveBeenLastCalledWith(acknowledgedLease);
    expect(canRunInFocusedTerminal.value).toBe(true);
    unregister();
  });

  it("requires a fresh focus request after a registration replacement or target fingerprint change", async () => {
    const firstTarget = focusTarget("36");
    const secondTarget = focusTarget("37");
    const firstApply = vi.fn();
    const secondApply = vi.fn();
    registerTerminalInputTarget({
      id: "same-pane",
      label: () => "First",
      focusTarget: () => firstTarget,
      applyFocusLease: firstApply,
      canAcceptInput: () => true,
      send: vi.fn(),
    });
    await focusTerminalInputTarget("same-pane");

    registerTerminalInputTarget({
      id: "same-pane",
      label: () => "Replacement",
      focusTarget: () => secondTarget,
      applyFocusLease: secondApply,
      canAcceptInput: () => true,
      send: vi.fn(),
    });
    expect(firstApply).toHaveBeenLastCalledWith(null);
    expect(canRunInFocusedTerminal.value).toBe(false);
    await focusTerminalInputTarget("same-pane");
    expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(2);
    expect(canRunInFocusedTerminal.value).toBe(true);

    secondTarget.target.attachmentId = "019d0000-0000-7000-8000-0000000003719";
    expect(captureTerminalInput("same-pane")).toBeNull();
    await focusTerminalInputTarget("same-pane");
    expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(3);
    expect(canRunInFocusedTerminal.value).toBe(true);
  });

  it("does not reuse an acknowledged target while its raw-input lease is stale", async () => {
    const target = focusTarget("38");
    let rawInputWritable = true;
    registerTerminalInputTarget({
      id: "stale-lease",
      label: () => "Stale lease",
      focusTarget: () => target,
      applyFocusLease: vi.fn(),
      canAcceptInput: () => true,
      canAcceptRawInput: () => rawInputWritable,
      send: vi.fn(),
    });
    await focusTerminalInputTarget("stale-lease");

    rawInputWritable = false;
    const gate = deferred<TerminalInputFocusChangeResponse>();
    client.changeTerminalInputFocus.mockReturnValueOnce(gate.promise);
    const refreshed = focusTerminalInputTarget("stale-lease");
    expect(canRunInFocusedTerminal.value).toBe(false);
    await vi.waitFor(() => expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(2));
    rawInputWritable = true;
    gate.resolve({ focusEpoch: "2", target, lease: lease(target, "2") });
    await refreshed;
    expect(canRunInFocusedTerminal.value).toBe(true);
  });

  it("suspends and resumes a paste only for the same exact target, without adding Enter", async () => {
    const target = focusTarget("41");
    const send = vi.fn().mockResolvedValue(undefined);
    registerTerminalInputTarget({ id: "paste", label: () => "Paste", focusTarget: () => target, applyFocusLease: vi.fn(), canAcceptInput: () => true, send });
    await focusTerminalInputTarget("paste");
    const ticket = captureTerminalInput("paste")!;
    expect(await ticket.suspend()).toBe(true);
    expect(canRunInFocusedTerminal.value).toBe(false);
    expect(await ticket.send("echo test")).toBe("sent");
    expect(send).toHaveBeenCalledExactlyOnceWith("echo test");
    expect(await ticket.send("again")).toBe("unavailable");
  });

  it("rejects a delayed confirmation after A to B to A, or generation changes", async () => {
    const first = focusTarget("51");
    const second = focusTarget("61");
    const send = vi.fn();
    for (const [id, target] of [["a", first], ["b", second]] as const) registerTerminalInputTarget({ id, label: () => id, focusTarget: () => target, applyFocusLease: vi.fn(), canAcceptInput: () => true, send });
    await focusTerminalInputTarget("a");
    const ticket = captureTerminalInput("a")!;
    await ticket.suspend();
    await focusTerminalInputTarget("b");
    await focusTerminalInputTarget("a");
    expect(await ticket.send("danger")).toBe("unavailable");
    const nextTicket = captureTerminalInput("a")!;
    await nextTicket.suspend();
    first.target.expectedGeneration = "2";
    expect(await nextTicket.send("danger")).toBe("unavailable");
    expect(send).not.toHaveBeenCalled();
  });

  it("never retries an uncertain send and revokes the local write path", async () => {
    const target = focusTarget("71");
    const send = vi.fn().mockRejectedValue(new Error("response lost"));
    const applyFocusLease = vi.fn();
    registerTerminalInputTarget({ id: "a", label: () => "a", focusTarget: () => target, applyFocusLease, canAcceptInput: () => true, send });
    await focusTerminalInputTarget("a");
    const ticket = captureTerminalInput("a")!;
    expect(await ticket.send("text")).toBe("failed");
    expect(applyFocusLease).toHaveBeenLastCalledWith(null);
    expect(await ticket.send("text")).toBe("unavailable");
    expect(send).toHaveBeenCalledTimes(1);
  });
});
