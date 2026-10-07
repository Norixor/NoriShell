import { describe, expect, it } from "vitest";

import {
  createTerminalInputWriter,
  TerminalInputDroppedError,
  TerminalInputFenceChangedError,
  type TerminalInputDispatch,
} from "./input-coalescer";

interface Deferred { resolve: () => void; reject: (error: unknown) => void }

function harness() {
  const writes: Array<{ key: string; value: string; seq: bigint }> = [];
  const pending: Deferred[] = [];
  let key: string | null = "fence-1";
  let seq = 0n;
  const prepare = (): TerminalInputDispatch | null => {
    if (key === null) return null;
    const captured = key;
    return {
      key: captured,
      write(value) {
        seq += 1n;
        const current = seq;
        writes.push({ key: captured, value, seq: current });
        return new Promise<bigint>((resolve, reject) => {
          pending.push({ resolve: () => resolve(current), reject });
        });
      },
    };
  };
  return {
    writer: createTerminalInputWriter(prepare, { maxBatchBytes: 8 }),
    writes,
    pending,
    setKey(next: string | null) { key = next; },
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("createTerminalInputWriter", () => {
  it("keeps one request in flight and merges input typed meanwhile in order", async () => {
    const { writer, writes, pending } = harness();
    const first = writer.send("e");
    const rest = ["c", "h", "o"].map((value) => writer.send(value));
    expect(writes.map((write) => write.value)).toEqual(["e"]);
    pending.shift()!.resolve();
    await first;
    await flush();
    expect(writes.map((write) => write.value)).toEqual(["e", "cho"]);
    pending.shift()!.resolve();
    const results = await Promise.all(rest);
    expect(results.map((result) => result.clientSeq)).toEqual([2n, 2n, 2n]);
    expect(results.map((result) => result.lastInBatch)).toEqual([false, false, true]);
  });

  it("never merges across a fence change and rejects input whose fence moved before sending", async () => {
    const { writer, writes, pending, setKey } = harness();
    void writer.send("a");
    const queuedOld = writer.send("b");
    setKey("fence-2");
    const queuedNew = writer.send("c");
    pending.shift()!.resolve();
    await expect(queuedOld).rejects.toBeInstanceOf(TerminalInputFenceChangedError);
    // The batch captured under the old fence was rejected, so later input is dropped, not redirected.
    await expect(queuedNew).rejects.toBeInstanceOf(TerminalInputDroppedError);
    expect(writes.map((write) => write.value)).toEqual(["a"]);
  });

  it("drops queued input after a delivery-uncertain failure instead of sending it", async () => {
    const { writer, writes, pending } = harness();
    const first = writer.send("ls");
    const second = writer.send("\r");
    pending.shift()!.reject(new Error("ipc"));
    await expect(first).rejects.toThrow("ipc");
    await expect(second).rejects.toBeInstanceOf(TerminalInputDroppedError);
    expect(writes).toHaveLength(1);
    // Later input starts a fresh request.
    const later = writer.send("x");
    pending.shift()!.resolve();
    await expect(later).resolves.toEqual({ clientSeq: 2n, lastInBatch: true });
  });

  it("seals a batch after an item and respects the byte limit", async () => {
    const { writer, writes, pending } = harness();
    void writer.send("a");
    void writer.send("ls", { seal: true });
    void writer.send("x");
    void writer.send("123456789");
    pending.shift()!.resolve();
    await flush();
    pending.shift()!.resolve();
    await flush();
    pending.shift()!.resolve();
    await flush();
    pending.shift()!.resolve();
    await flush();
    expect(writes.map((write) => write.value)).toEqual(["a", "ls", "x", "123456789"]);
  });

  it("rejects immediately when no writable fence exists", async () => {
    const { writer, setKey, writes } = harness();
    setKey(null);
    await expect(writer.send("a")).rejects.toThrow("unavailable");
    expect(writes).toHaveLength(0);
  });
});
