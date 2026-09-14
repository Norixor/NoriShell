import { describe, expect, it, vi } from "vitest";

import { createFencedTerminalResize } from "./fencedTerminalResize";

describe("createFencedTerminalResize", () => {
  it("replays only the latest pending size when a fence becomes available", async () => {
    let ready = false;
    const send = vi.fn().mockResolvedValue(undefined);
    const coordinator = createFencedTerminalResize((dimensions) => ready
      ? {
          key: `session:${dimensions.rows}:${dimensions.cols}`,
          send: (sequence) => send(sequence, dimensions),
        }
      : null);

    coordinator.resize(24, 80);
    coordinator.resize(30, 120);
    await coordinator.flush();
    expect(send).not.toHaveBeenCalled();

    ready = true;
    await coordinator.flush();
    expect(send).toHaveBeenCalledOnce();
    expect(send).toHaveBeenCalledWith("1", { rows: 30, cols: 120 });
  });

  it("keeps a failed size pending and advances the sequence for the retry", async () => {
    let ready = false;
    const send = vi.fn()
      .mockRejectedValueOnce(new Error("uncertain"))
      .mockResolvedValueOnce(undefined);
    const coordinator = createFencedTerminalResize((dimensions) => ready
      ? {
          key: `session:${dimensions.rows}:${dimensions.cols}`,
          send: (sequence) => send(sequence),
        }
      : null);

    coordinator.resize(24, 80);
    ready = true;
    await coordinator.flush();
    await coordinator.flush();
    expect(send).toHaveBeenNthCalledWith(1, "1");
    expect(send).toHaveBeenNthCalledWith(2, "2");
  });
});
