import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  isTauri: () => true,
  Channel: class<T> {
    onmessage: ((message: T) => void) | null = null;
  },
}));

import {
  attachSshSession,
  decideSshHostKey,
  detachSshSession,
  disconnectSshSession,
  changeSshInputFocus,
  fetchSshInputFocusSnapshot,
  heartbeatSshAttachment,
  openSshSession,
  reconnectSshSession,
  renewSshInputLease,
  resizeSshTerminal,
  sendSshInput,
} from "./client";

describe("SSH renderer binding client", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("mints a fresh operation, attempt, and Channel for every attach", async () => {
    tauri.invoke.mockResolvedValue({});
    const input = {
      sessionId: "019d0000-0000-7000-8000-000000000301",
      expectedGeneration: "2",
      expectedStateRevision: "4",
      viewId: "019d0000-0000-7000-8000-000000000302",
      afterOutputSeq: null,
    };

    await attachSshSession(input, () => undefined);
    await attachSshSession(input, () => undefined);

    const [first, second] = tauri.invoke.mock.calls;
    if (!first || !second) throw new Error("expected two attach invocations");
    expect(first[0]).toBe("ssh_terminal_attach");
    expect(second[0]).toBe("ssh_terminal_attach");
    expect(first[1].request.operationId).not.toBe(second[1].request.operationId);
    expect(first[1].request.attachAttemptId).not.toBe(second[1].request.attachAttemptId);
    expect(first[1].onEvent).not.toBe(second[1].onEvent);
  });

  it("opens with a fresh operation/open attempt/attach attempt and an event Channel", async () => {
    tauri.invoke.mockResolvedValue({});
    const onEvent = vi.fn();
    const target = {
      kind: "host" as const,
      hostId: "019d0000-0000-7000-8000-000000000321",
      expectedHostStateVersion: "9",
    };

    await openSshSession({
      target,
      credentialRefId: "019d0000-0000-7000-8000-000000000322",
      viewId: "019d0000-0000-7000-8000-000000000323",
      rows: 31,
      cols: 117,
    }, onEvent);

    const [command, args] = tauri.invoke.mock.calls[0] ?? [];
    expect(command).toBe("ssh_terminal_open");
    expect(args.request).toMatchObject({
      target,
      credentialRefId: "019d0000-0000-7000-8000-000000000322",
      viewId: "019d0000-0000-7000-8000-000000000323",
      rows: 31,
      cols: 117,
    });
    expect(args.request.operationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7/);
    expect(args.request.idempotencyKey).toBe(`ssh-open-${args.request.operationId}`);
    expect(args.request.openAttemptId).not.toBe(args.request.attachAttemptId);
    expect(args.onEvent.onmessage).toBe(onEvent);
  });

  it("forwards all attachment heartbeat fences", async () => {
    tauri.invoke.mockResolvedValue({});
    await heartbeatSshAttachment({
      sessionId: "019d0000-0000-7000-8000-000000000311",
      expectedGeneration: "7",
      expectedAttachmentRevision: "12",
      attachmentId: "019d0000-0000-7000-8000-000000000312",
      viewId: "019d0000-0000-7000-8000-000000000313",
    });

    expect(tauri.invoke).toHaveBeenCalledWith("ssh_terminal_attachment_heartbeat", {
      request: expect.objectContaining({
        expectedGeneration: "7",
        expectedAttachmentRevision: "12",
        attachmentId: "019d0000-0000-7000-8000-000000000312",
        viewId: "019d0000-0000-7000-8000-000000000313",
      }),
    });
  });

  it("forwards rendererUnavailable detach without weakening its revision fences", async () => {
    tauri.invoke.mockResolvedValue({});
    await detachSshSession({
      sessionId: "019d0000-0000-7000-8000-000000000331",
      expectedGeneration: "5",
      expectedStateRevision: "11",
      attachmentId: "019d0000-0000-7000-8000-000000000332",
      viewId: "019d0000-0000-7000-8000-000000000333",
      intent: "rendererUnavailable",
      confirmation: null,
    });

    const [command, args] = tauri.invoke.mock.calls[0] ?? [];
    expect(command).toBe("ssh_terminal_detach");
    expect(args.request).toMatchObject({
      expectedGeneration: "5",
      expectedStateRevision: "11",
      attachmentId: "019d0000-0000-7000-8000-000000000332",
      viewId: "019d0000-0000-7000-8000-000000000333",
      intent: "rendererUnavailable",
      confirmation: null,
    });
    expect(args.request.idempotencyKey).toBe(`ssh-detach-${args.request.operationId}`);
  });

  it("preserves host-key, input lease, raw input, resize, reconnect, and disconnect shapes", async () => {
    tauri.invoke.mockResolvedValue({});
    const common = {
      sessionId: "019d0000-0000-7000-8000-000000000341",
      expectedGeneration: "7",
    };

    await decideSshHostKey({
      ...common,
      challengeId: "019d0000-0000-7000-8000-000000000342",
      expectedStateRevision: "14",
      attachmentId: "019d0000-0000-7000-8000-000000000343",
      viewId: "019d0000-0000-7000-8000-000000000344",
      decision: "reject",
    });
    await fetchSshInputFocusSnapshot();
    await changeSshInputFocus({
      expectedFocusEpoch: "21",
      target: {
        sessionId: common.sessionId,
        expectedGeneration: common.expectedGeneration,
        expectedStateRevision: "15",
        channelId: "019d0000-0000-7000-8000-000000000346",
        attachmentId: "019d0000-0000-7000-8000-000000000343",
        viewId: "019d0000-0000-7000-8000-000000000344",
      },
    });
    await renewSshInputLease({
      ...common,
      attachmentId: "019d0000-0000-7000-8000-000000000343",
      viewId: "019d0000-0000-7000-8000-000000000344",
      focusEpoch: "22",
      leaseId: "019d0000-0000-7000-8000-000000000345",
      inputEpoch: "3",
    });
    await sendSshInput({
      ...common,
      channelId: "019d0000-0000-7000-8000-000000000346",
      attachmentId: "019d0000-0000-7000-8000-000000000343",
      viewId: "019d0000-0000-7000-8000-000000000344",
      focusEpoch: "22",
      leaseId: "019d0000-0000-7000-8000-000000000345",
      inputEpoch: "3",
      clientSeq: "8",
      bytes: [0, 27, 91, 109, 255],
    });
    await resizeSshTerminal({
      ...common,
      channelId: "019d0000-0000-7000-8000-000000000346",
      attachmentId: "019d0000-0000-7000-8000-000000000343",
      viewId: "019d0000-0000-7000-8000-000000000344",
      focusEpoch: "22",
      leaseId: "019d0000-0000-7000-8000-000000000345",
      inputEpoch: "3",
      resizeSeq: "9",
      rows: 42,
      cols: 132,
    });
    await reconnectSshSession({
      ...common,
      expectedStateRevision: "16",
      attachmentId: "019d0000-0000-7000-8000-000000000343",
      viewId: "019d0000-0000-7000-8000-000000000344",
      credentialRefId: null,
      rows: 24,
      cols: 80,
    });
    await disconnectSshSession({ ...common, expectedStateRevision: "17" });

    expect(tauri.invoke.mock.calls.map(([command]) => command)).toEqual([
      "ssh_terminal_host_key_decide",
      "ssh_terminal_input_focus_snapshot",
      "ssh_terminal_input_focus_change",
      "ssh_terminal_input_lease_renew",
      "ssh_terminal_input",
      "ssh_terminal_resize",
      "ssh_terminal_reconnect",
      "ssh_terminal_disconnect",
    ]);
    expect(tauri.invoke.mock.calls[0]?.[1].request).toMatchObject({
      decision: "reject",
      expectedStateRevision: "14",
    });
    expect(tauri.invoke.mock.calls[4]?.[1].request).toMatchObject({
      clientSeq: "8",
      focusEpoch: "22",
      bytes: [0, 27, 91, 109, 255],
    });
    expect(tauri.invoke.mock.calls[5]?.[1].request).toMatchObject({
      resizeSeq: "9",
      focusEpoch: "22",
      rows: 42,
      cols: 132,
    });
    expect(tauri.invoke.mock.calls[6]?.[1].request.idempotencyKey)
      .toBe(`ssh-reconnect-${tauri.invoke.mock.calls[6]?.[1].request.operationId}`);
    expect(tauri.invoke.mock.calls[7]?.[1].request.idempotencyKey)
      .toBe(`ssh-disconnect-${tauri.invoke.mock.calls[7]?.[1].request.operationId}`);
  });
});
