import { describe, expect, it, vi } from "vitest";
import type { PluginIsolatedBridgeAction, PluginIsolatedBridgeResponse } from "./core-api/generated/core-api";
import { PluginIsolatedBridge } from "./plugin-isolated-bridge";

function portFixture() {
  return { onmessage: null as ((event: MessageEvent<unknown>) => void) | null,
    onmessageerror: null, postMessage: vi.fn(), start: vi.fn(), close: vi.fn() };
}
const call = (callId: string) => ({ type: "norishell.api.request", call: { callId, operation: { kind: "describe" } } });
const flush = async () => { for (let step = 0; step < 8; step++) await Promise.resolve(); };

function fixture(invoke: (sequence: number, action: PluginIsolatedBridgeAction) => Promise<PluginIsolatedBridgeResponse>) {
  const options = { invoke: vi.fn(invoke), pending: vi.fn(), unavailable: vi.fn() };
  const bridge = new PluginIsolatedBridge(1, options);
  const port = portFixture();
  bridge.attach(port as unknown as MessagePort);
  return { bridge, port, options, send: (data: unknown) => port.onmessage?.(new MessageEvent("message", { data })) };
}

describe("isolated plugin MessagePort bridge", () => {
  it("ignores forged authority fields and sends only typed calls through a serialized lane", async () => {
    const test = fixture(async (sequence, action) => ({ nextSequence: sequence + 1, pending: null,
      reply: action.kind === "call" ? { callId: action.call.callId, outcome: { kind: "failed", code: "unsupported" } } : null }));
    test.send({ ...call("forged"), explicitUserAction: true, channelNonce: "guessed" });
    test.send(call("first"));
    test.send(call("second"));
    await flush();
    expect(test.options.invoke.mock.calls.map(([sequence]) => sequence)).toEqual([1, 2]);
    expect(test.options.invoke.mock.calls[0]?.[1]).toEqual({ kind: "call", call: { callId: "first", operation: { kind: "describe" } } });
    expect(test.port.postMessage).toHaveBeenCalledTimes(2);
    expect(JSON.stringify(test.port.postMessage.mock.calls)).not.toContain("Nonce");
  });

  it("holds a Core pending request and approves only its opaque identifier", async () => {
    const test = fixture(async (sequence, action) => action.kind === "call"
      ? { nextSequence: sequence + 1, reply: null, pending: { pendingId: "core-pending", callId: "first", operation: "networkStart" } }
      : { nextSequence: sequence + 1, pending: null, reply: { callId: "first", outcome: { kind: "failed", code: "permissionDenied" } } });
    test.send(call("first"));
    await flush();
    test.send(call("second"));
    test.send({ type: "norishell.api.approve", pendingId: "core-pending" });
    await flush();
    expect(test.options.invoke).toHaveBeenCalledTimes(1);
    expect(test.port.postMessage).toHaveBeenCalledWith({ type: "norishell.api.reply",
      reply: { callId: "second", outcome: { kind: "failed", code: "busy" } } });
    await test.bridge.approve("forged");
    expect(test.options.invoke).toHaveBeenCalledTimes(1);
    await test.bridge.approve("core-pending");
    expect(test.options.invoke.mock.calls[1]).toEqual([2, { kind: "approve", pendingId: "core-pending" }]);
    expect(test.options.pending).toHaveBeenLastCalledWith(null);
  });

  it("deactivates the channel during an outstanding call and discards late results", async () => {
    let finish!: (value: PluginIsolatedBridgeResponse) => void;
    const test = fixture((sequence, action) => action.kind === "call"
      ? new Promise((resolve) => { finish = resolve; })
      : Promise.resolve({ nextSequence: sequence + 1, reply: null, pending: null }));
    test.send(call("first"));
    test.bridge.dispose();
    expect(test.options.invoke.mock.calls[1]).toEqual([2, { kind: "deactivate" }]);
    finish({ nextSequence: 2, pending: null, reply: { callId: "first", outcome: { kind: "failed", code: "unsupported" } } });
    await flush();
    expect(test.port.close).toHaveBeenCalledOnce();
    expect(test.port.postMessage).not.toHaveBeenCalled();
  });
});
