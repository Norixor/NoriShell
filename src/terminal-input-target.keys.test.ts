import { beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  changeTerminalInputFocus: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
}));
vi.mock("./core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("./core-api/client")>(), ...client,
}));

import type { TerminalInputFocusTarget, TerminalInputLease } from "./core-api/generated/core-api";
import {
  canSendKeysToFocusedTerminal,
  focusedTerminalUsesApplicationCursorKeys,
  focusTerminalInputTarget,
  registerTerminalInputTarget,
  resetTerminalInputFocusForTests,
  sendKeysToFocusedTerminal,
} from "./terminal-input-target";

function focusTarget(id: string): TerminalInputFocusTarget {
  return { kind: "ssh", target: {
    sessionId: `019d0000-0000-7000-8000-000000000${id}1`, expectedGeneration: "1", expectedStateRevision: "2",
    channelId: `019d0000-0000-7000-8000-000000000${id}2`, attachmentId: `019d0000-0000-7000-8000-000000000${id}3`,
    viewId: `019d0000-0000-7000-8000-000000000${id}4`,
  } };
}

function lease(target: TerminalInputFocusTarget, epoch: string): TerminalInputLease {
  if (target.kind !== "ssh") throw new Error("expected SSH target");
  return { kind: "ssh", lease: {
    leaseId: `019d0000-0000-7000-8000-000000000${epoch}5`, sessionId: target.target.sessionId,
    generation: target.target.expectedGeneration, attachmentId: target.target.attachmentId, viewId: target.target.viewId,
    focusEpoch: epoch, inputEpoch: epoch, expiresAtUnixMs: Date.now() + 60_000,
  } };
}

describe("helper key sending", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetTerminalInputFocusForTests();
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({ focusEpoch: "0", target: null, lease: null });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target }) => {
      const focusEpoch = (BigInt(expectedFocusEpoch) + 1n).toString();
      return { focusEpoch, target, lease: target ? lease(target, focusEpoch) : null };
    });
  });

  it("fails closed without an acknowledged writable Pane and sends raw bytes afterwards", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    let mode: "normal" | "application" = "normal";
    const target = focusTarget("a");
    registerTerminalInputTarget({
      id: "pane-a", label: () => "a", focusTarget: () => target, applyFocusLease: () => undefined,
      canAcceptInput: () => true, cursorKeysMode: () => mode, send,
    });
    expect(canSendKeysToFocusedTerminal.value).toBe(false);
    expect(await sendKeysToFocusedTerminal("\x1b")).toBe("unavailable");
    expect(send).not.toHaveBeenCalled();
    await focusTerminalInputTarget("pane-a");
    expect(canSendKeysToFocusedTerminal.value).toBe(true);
    expect(focusedTerminalUsesApplicationCursorKeys()).toBe(false);
    mode = "application";
    expect(focusedTerminalUsesApplicationCursorKeys()).toBe(true);
    expect(await sendKeysToFocusedTerminal("\x1bOA")).toBe("sent");
    expect(send).toHaveBeenCalledWith("\x1bOA");
  });

  it("uses the raw-input permission for Panes that refuse Quick Commands and reports a failed send", async () => {
    const send = vi.fn().mockRejectedValue(new Error("ipc lost"));
    const target = focusTarget("b");
    registerTerminalInputTarget({
      id: "pane-b", label: () => "b", focusTarget: () => target, applyFocusLease: () => undefined,
      canAcceptInput: () => false, canAcceptRawInput: () => true, send,
    });
    await focusTerminalInputTarget("pane-b");
    expect(canSendKeysToFocusedTerminal.value).toBe(true);
    expect(await sendKeysToFocusedTerminal("\t")).toBe("failed");
    expect(send).toHaveBeenCalledTimes(1);
  });
});
