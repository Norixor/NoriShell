import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  SshSessionAttachment,
  SshSessionDetails,
  SshSessionEvent,
  SshSessionFailureReason,
  SshSessionInputLease,
  SshSessionState,
  SshSessionSummary,
  SshSessionTarget,
} from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import { useTipsStore } from "../../stores/tips";
import {
  focusedTerminalLabel,
  resetTerminalInputFocusForTests,
  runInFocusedTerminal,
} from "../../terminal-input-target";

const client = vi.hoisted(() => ({
  secureChallenge: vi.fn(),
  attachSshSession: vi.fn(),
  changeTerminalInputFocus: vi.fn(),
  decideSshHostKey: vi.fn(),
  detachSshSession: vi.fn(),
  disconnectSshSession: vi.fn(),
  getSshSession: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
  heartbeatSshAttachment: vi.fn(),
  openSshSession: vi.fn(),
  prepareSshKeyboardInteractiveAnswer: vi.fn(),
  reconnectSshSession: vi.fn(),
  respondSshKeyboardInteractive: vi.fn(),
  renewSshInputLease: vi.fn(),
  resizeSshTerminal: vi.fn(),
  sendSshInput: vi.fn(),
  takeoverSshLoginAutomation: vi.fn(),
}));

vi.mock("../../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../core-api/client")>();
  return { ...actual, ...client };
});

vi.mock("../../core-api/secure-ssh-challenge-client", () => ({ requestSecureSshChallenge: client.secureChallenge }));

import NvxSshTerminalPane from "./NvxSshTerminalPane.vue";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";

const paneId = "019d0000-0000-7000-8000-000000000401";
const target: SshSessionTarget = {
  kind: "host",
  hostId: "019d0000-0000-7000-8000-000000000402",
  expectedHostStateVersion: "4",
};

const writes = {
  bytes: vi.fn(),
  clearSearch: vi.fn(),
  findNext: vi.fn(),
  findPrevious: vi.fn(),
  gap: vi.fn(),
  focus: vi.fn(),
  selection: vi.fn(),
};

const terminalViewStub = defineComponent({
  name: "NvxTerminalView",
  props: {
    readOnly: Boolean,
    reconnectOnInput: Boolean,
    terminalLabel: { type: String, required: true },
    gapLabel: { type: String, required: true },
  },
  emits: ["reconnectRequest", "input", "resize", "searchRequest", "selectionChange"],
  setup(props, { expose }) {
    expose({
      writeBytes: writes.bytes,
      writeGap: writes.gap,
      dimensions: () => ({ rows: 28, cols: 96 }),
      focus: writes.focus,
      findNext: writes.findNext,
      findPrevious: writes.findPrevious,
      clearSearch: writes.clearSearch,
      selection: writes.selection,
    });
    return () => h("div", {
      class: "terminal-view-stub",
      "data-read-only": String(props.readOnly),
    });
  },
});

function summary(
  state: SshSessionState,
  overrides: Partial<SshSessionSummary> = {},
): SshSessionSummary {
  return {
    sessionId: "019d0000-0000-7000-8000-000000000411",
    openAttemptId: "019d0000-0000-7000-8000-000000000412",
    target,
    credentialRefId: "019d0000-0000-7000-8000-000000000413",
    endpoint: { address: "ssh.example.test", port: 22, username: "deploy" },
    generation: "2",
    stateRevision: "7",
    attachmentRevision: "3",
    eventSeq: "0",
    channelId: state === "running" ? "019d0000-0000-7000-8000-000000000414" : null,
    negotiatedAlgorithms: [],
    state,
    closeReason: null,
    failureReason: null,
    attachmentCount: 1,
    createdAtUnixMs: 1,
    updatedAtUnixMs: 2,
    ...overrides,
  };
}

function attachment(overrides: Partial<SshSessionAttachment> = {}): SshSessionAttachment {
  return {
    attachmentId: "019d0000-0000-7000-8000-000000000421",
    attachAttemptId: "019d0000-0000-7000-8000-000000000422",
    sessionId: "019d0000-0000-7000-8000-000000000411",
    generation: "2",
    channelId: "019d0000-0000-7000-8000-000000000414",
    viewId: paneId,
    stateRevision: "7",
    attachmentRevision: "3",
    attachedAtUnixMs: 2,
    ...overrides,
  };
}

function inputLease(overrides: Partial<SshSessionInputLease> = {}): SshSessionInputLease {
  return {
    leaseId: "019d0000-0000-7000-8000-000000000431",
    sessionId: "019d0000-0000-7000-8000-000000000411",
    generation: "2",
    attachmentId: "019d0000-0000-7000-8000-000000000421",
    viewId: paneId,
    focusEpoch: "1",
    inputEpoch: "5",
    expiresAtUnixMs: Date.now() + 60_000,
    ...overrides,
  };
}

function details(session: SshSessionSummary, overrides: Partial<SshSessionDetails> = {}): SshSessionDetails {
  return {
    session,
    attachments: [attachment({ generation: session.generation, channelId: session.channelId })],
    activeHostKeyChallenge: null,
    activeKeyboardInteractiveChallenge: null,
    activeLoginAutomation: null,
    heartbeat: { policyRevision: null, mode: "disabled", transports: [], shell: null },
    inputLease: null,
    ...overrides,
  };
}

function event(
  session: SshSessionSummary,
  payload: SshSessionEvent["payload"],
  eventSeq = "1",
): SshSessionEvent {
  return {
    schemaVersion: 5,
    sessionId: session.sessionId,
    generation: session.generation,
    stateRevision: session.stateRevision,
    eventSeq,
    occurredAtUnixMs: 3,
    payload,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((promiseResolve) => {
    resolve = promiseResolve;
  });
  return { promise, resolve };
}

function dispatchCapturedEvent(
  listener: ((value: SshSessionEvent) => void) | null,
  value: SshSessionEvent,
) {
  if (!listener) throw new Error("expected captured SSH event listener");
  listener(value);
}

function mountPane(
  existingSession: SshSessionSummary | null = null,
  active = true,
  deferredStart = false,
  deferredRecovery: "reconnect" | "credential" | "vaultUnlock" = "reconnect",
  credentialRefId: string | null = "019d0000-0000-7000-8000-000000000413",
) {
  const mountHost = document.createElement("div");
  document.body.append(mountHost);
  return mount(NvxSshTerminalPane, {
    attachTo: mountHost,
    props: {
      paneId,
      label: "Production",
      target,
      credentialRefId,
      existingSession,
      deferredStart,
      deferredRecovery,
      active,
      canSplitHorizontal: true,
      canSplitVertical: true,
    },
    global: {
      plugins: [createPinia(), i18n],
      stubs: { NvxTerminalView: terminalViewStub },
    },
  });
}

function bodyButton(label: string) {
  return Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
    .find((candidate) => candidate.textContent?.trim().includes(label));
}

describe("NvxSshTerminalPane Core IPC contract", () => {
  beforeEach(() => {
    client.secureChallenge.mockReset().mockReturnValue(new Promise(() => undefined));
    vi.clearAllMocks();
    localStorage.clear();
    resetTerminalInputFocusForTests();
    i18n.global.locale.value = "en";
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({
      focusEpoch: "0",
      target: null,
      lease: null,
    });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target }) => {
      const focusEpoch = (BigInt(expectedFocusEpoch) + 1n).toString();
      return {
        focusEpoch,
        target,
        lease: target?.kind === "ssh"
          ? {
              kind: "ssh",
              lease: inputLease({
                sessionId: target.target.sessionId,
                generation: target.target.expectedGeneration,
                attachmentId: target.target.attachmentId,
                viewId: target.target.viewId,
                focusEpoch,
              }),
            }
          : null,
      };
    });
    client.heartbeatSshAttachment.mockResolvedValue(attachment());
    client.renewSshInputLease.mockResolvedValue(inputLease());
    client.sendSshInput.mockResolvedValue(undefined);
    client.resizeSshTerminal.mockResolvedValue(undefined);
    client.detachSshSession.mockResolvedValue({
      kind: "detached",
      session: summary("closed"),
      remainingAttachmentCount: 0,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = "";
  });

  it("keeps a restored Pane disconnected until explicit reconnect", async () => {
    const running = summary("running");
    client.openSshSession.mockResolvedValue({
      operationId: "019d0000-0000-7000-8000-000000000442",
      idempotencyKey: "restored-open",
      openAttemptId: running.openAttemptId,
      stateRevision: running.stateRevision,
      session: running,
      attachment: attachment(),
    });
    const wrapper = mountPane(null, true, true);
    await flushPromises();

    expect(client.openSshSession).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain("Closed");
    bodyButton("Reconnect")?.click();
    await flushPromises();
    expect(client.openSshSession).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("routes credential-gated history to credential entry instead of a blind reconnect", async () => {
    const wrapper = mountPane(null, true, true, "credential", null);
    await flushPromises();

    expect(client.openSshSession).not.toHaveBeenCalled();
    expect(wrapper.get(".ssh-terminal-pane__reconnect").text()).toContain(
      i18n.global.t("sshSession.enterCredential"),
    );
    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();

    expect(client.openSshSession).not.toHaveBeenCalled();
    expect(wrapper.emitted("requestCredential")?.[0]).toEqual([paneId, target]);
    wrapper.unmount();
  });

  it("routes locked history to Vault unlock instead of a blind reconnect", async () => {
    const wrapper = mountPane(null, true, true, "vaultUnlock", null);
    await flushPromises();

    expect(client.openSshSession).not.toHaveBeenCalled();
    expect(wrapper.get(".ssh-terminal-pane__reconnect").text()).toContain(
      i18n.global.t("sshSession.unlockVault"),
    );
    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();

    expect(client.openSshSession).not.toHaveBeenCalled();
    expect(wrapper.emitted("requestVaultUnlock")?.[0]).toEqual([paneId, target]);
    wrapper.unmount();
  });

  it("shows the negotiated policy and algorithms for the exact route stage", async () => {
    const running = summary("running", {
      negotiatedAlgorithms: [{
        routeStage: { kind: "target" },
        policyId: "secure-default",
        policyRevision: "2",
        policyCatalogVersion: "2026.08.29.1",
        keyExchange: "curve25519-sha256",
        hostKey: "ssh-ed25519",
        cipherClientToServer: "chacha20-poly1305@openssh.com",
        cipherServerToClient: "chacha20-poly1305@openssh.com",
        macClientToServer: "none",
        macServerToClient: "none",
      }],
    });
    client.openSshSession.mockResolvedValue({
      operationId: "019d0000-0000-7000-8000-000000000442",
      idempotencyKey: "negotiated-open",
      openAttemptId: running.openAttemptId,
      stateRevision: running.stateRevision,
      session: running,
      attachment: attachment(),
    });
    const wrapper = mountPane();
    await flushPromises();

    document.querySelector<HTMLButtonElement>('[aria-label="View negotiated algorithms"]')?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("curve25519-sha256");
    expect(document.body.textContent).toContain("ssh-ed25519");
    expect(document.body.textContent).toContain("Policy revision");
    expect(document.body.textContent).toContain("2");
    expect(document.body.textContent).toContain("2026.08.29.1");
    wrapper.unmount();
  });

  it("opens an isolated host-key challenge with every fence and no local approval button", async () => {
    const awaiting = summary("awaitingHostKeyDecision", { channelId: null });
    const challenge = {
      challengeId: "019d0000-0000-7000-8000-000000000441",
      sessionId: awaiting.sessionId,
      generation: awaiting.generation,
      stateRevision: awaiting.stateRevision,
      endpoint: awaiting.endpoint!,
      keyAlgorithm: "ssh-ed25519",
      fingerprintSha256: "SHA256:test-fingerprint",
    };
    client.openSshSession.mockImplementation(async (_request, onEvent) => {
      onEvent(event(awaiting, { kind: "hostKeyChallenge", challenge }));
      return {
        operationId: "019d0000-0000-7000-8000-000000000442",
        idempotencyKey: "open",
        openAttemptId: awaiting.openAttemptId,
        stateRevision: awaiting.stateRevision,
        session: awaiting,
        attachment: attachment({ channelId: null }),
      };
    });
    client.decideSshHostKey.mockResolvedValue(details(summary("authenticating")));

    const wrapper = mountPane();
    await flushPromises();

    expect(client.openSshSession).toHaveBeenCalledWith({
      target,
      credentialRefId: "019d0000-0000-7000-8000-000000000413",
      pluginAuthorizationToken: null,
      viewId: paneId,
      rows: 28,
      cols: 96,
    }, expect.any(Function));
    expect(document.body.textContent).not.toContain("SHA256:test-fingerprint");
    expect(client.secureChallenge).toHaveBeenCalledWith({ kind: "sshHostKey", request: {
      sessionId: awaiting.sessionId,
      expectedGeneration: awaiting.generation,
      challengeId: challenge.challengeId,
      expectedStateRevision: challenge.stateRevision,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
    } });
    expect(client.decideSshHostKey).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("opens keyboard-interactive in the isolated window without receiving answers", async () => {
    const authenticating = summary("authenticating", {
      channelId: null,
      stateRevision: "8",
    });
    const keyboardChallenge = {
      challengeId: "019d0000-0000-7000-8000-000000000491",
      sessionId: authenticating.sessionId,
      generation: authenticating.generation,
      stateRevision: authenticating.stateRevision,
      routeStage: { kind: "target" } as const,
      credentialRefId: "019d0000-0000-7000-8000-000000000413",
      attemptIndex: 0,
      roundIndex: 1,
      name: "Verification",
      instructions: "Enter the account name and one-time code.",
      prompts: [
        { promptIndex: 0, text: "Account", echo: true, sensitive: false },
        { promptIndex: 1, text: "Code", echo: false, sensitive: true },
      ],
      expiresAtUnixMs: Date.now() + 120_000,
    };
    client.openSshSession.mockImplementation(async (_request, onEvent) => {
      onEvent(event(authenticating, {
        kind: "keyboardInteractiveChallengeChanged",
        challenge: keyboardChallenge,
      }));
      return {
        operationId: "019d0000-0000-7000-8000-000000000492",
        idempotencyKey: "kbi-open",
        openAttemptId: authenticating.openAttemptId,
        stateRevision: authenticating.stateRevision,
        session: authenticating,
        attachment: attachment({ channelId: null, stateRevision: "8" }),
      };
    });
    client.prepareSshKeyboardInteractiveAnswer.mockResolvedValue({
      answerRefId: "019d0000-0000-7000-8000-000000000493",
      expiresAtUnixMs: keyboardChallenge.expiresAtUnixMs,
    });
    client.respondSshKeyboardInteractive.mockResolvedValue(
      details(summary("authenticating", { channelId: null, stateRevision: "9" })),
    );

    const wrapper = mountPane();
    await flushPromises();
    expect(document.querySelector("#ssh-kbi-0")).toBeNull();
    expect(document.querySelector("#ssh-kbi-1")).toBeNull();
    expect(client.secureChallenge).toHaveBeenCalledWith({ kind: "sshKeyboard", request: {
      sessionId: authenticating.sessionId,
      expectedGeneration: authenticating.generation,
      challengeId: keyboardChallenge.challengeId,
      expectedStateRevision: keyboardChallenge.stateRevision,
      roundIndex: 1,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
    } });
    expect(client.prepareSshKeyboardInteractiveAnswer).not.toHaveBeenCalled();
    expect(client.respondSshKeyboardInteractive).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("acquires input ownership and forwards UTF-8/ANSI input plus fenced resize", async () => {
    const running = summary("running");
    client.openSshSession.mockResolvedValue({
      operationId: "019d0000-0000-7000-8000-000000000451",
      idempotencyKey: "open",
      openAttemptId: running.openAttemptId,
      stateRevision: running.stateRevision,
      session: running,
      attachment: attachment(),
    });
    const wrapper = mountPane();
    await flushPromises();

    expect(client.changeTerminalInputFocus).toHaveBeenLastCalledWith({
      expectedFocusEpoch: "1",
      target: {
        kind: "ssh",
        target: {
          sessionId: running.sessionId,
          expectedGeneration: "2",
          expectedStateRevision: "7",
          channelId: running.channelId,
          attachmentId: attachment().attachmentId,
          viewId: paneId,
        },
      },
    });
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");

    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("input", "\u0000é\u001b[A");
    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("resize", 33, 120);
    await flushPromises();

    expect(client.sendSshInput).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: "2",
      channelId: running.channelId,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
      focusEpoch: "2",
      leaseId: inputLease().leaseId,
      inputEpoch: "5",
      clientSeq: "1",
      bytes: [0, 195, 169, 27, 91, 65],
    });
    expect(client.resizeSshTerminal).toHaveBeenCalledWith(expect.objectContaining({
      resizeSeq: "1",
      rows: 33,
      cols: 120,
    }));
    wrapper.unmount();
  });

  it("handles rejected direct xterm input without retrying and requires an output check", async () => {
    const running = summary("running");
    client.openSshSession.mockResolvedValue({
      operationId: "019d0000-0000-7000-8000-000000000452",
      idempotencyKey: "open-input-rejection",
      openAttemptId: running.openAttemptId,
      stateRevision: running.stateRevision,
      session: running,
      attachment: attachment(),
    });
    client.sendSshInput.mockRejectedValueOnce(new Error("write result unavailable"));
    const wrapper = mountPane();
    await flushPromises();

    const terminal = wrapper.findComponent({ name: "NvxTerminalView" });
    terminal.vm.$emit("input", "whoami\r");
    await flushPromises();

    expect(client.sendSshInput).toHaveBeenCalledTimes(1);
    expect(wrapper.text()).not.toContain("The send result is uncertain. Check the terminal before retrying.");
    expect(useTipsStore().items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "error", title: "The send result is uncertain. Check the terminal before retrying." }),
    ]));
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("true");

    terminal.vm.$emit("input", "whoami\r");
    await flushPromises();
    expect(client.sendSshInput).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("connects xterm search requests and explicit selection state to the shared tools", async () => {
    const running = summary("running");
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running);
    await flushPromises();
    const terminal = wrapper.findComponent({ name: "NvxTerminalView" });

    terminal.vm.$emit("searchRequest");
    terminal.vm.$emit("selectionChange", true);
    await flushPromises();

    expect(wrapper.find("[role='search']").exists()).toBe(true);
    expect(wrapper.get("button[aria-label='Copy selected text']").attributes("disabled"))
      .toBeUndefined();
    wrapper.unmount();
  });

  it("keeps search field focus out of the SSH Channel input path", async () => {
    const running = summary("running");
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running);
    await flushPromises();
    writes.focus.mockClear();
    client.sendSshInput.mockClear();

    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("searchRequest");
    await flushPromises();
    await wrapper.get("[role='search'] input").trigger("focusin");
    await flushPromises();

    expect(writes.focus).not.toHaveBeenCalled();
    expect(client.sendSshInput).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reattaches a renderer, heartbeats it, and releases only the renderer binding", async () => {
    vi.useFakeTimers();
    const running = summary("running", { eventSeq: "9" });
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });

    const wrapper = mountPane(running);
    await flushPromises();

    expect(client.attachSshSession).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedStateRevision: running.stateRevision,
      viewId: paneId,
      afterOutputSeq: null,
    }, expect.any(Function));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(client.heartbeatSshAttachment).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedAttachmentRevision: attachment().attachmentRevision,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
    });

    wrapper.unmount();
    await flushPromises();
    expect(client.detachSshSession).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedStateRevision: running.stateRevision,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
      intent: "rendererUnavailable",
      confirmation: null,
    });
  });

  it("resumes after a stale heartbeat without replaying output already rendered", async () => {
    vi.useFakeTimers();
    const running = summary("running", { eventSeq: "9" });
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession
      .mockResolvedValueOnce({
        stateRevision: running.stateRevision,
        attachmentRevision: running.attachmentRevision,
        attachment: attachment(),
        replay: [{
          kind: "frame",
          payload: {
            sessionId: running.sessionId,
            generation: running.generation,
            channelId: running.channelId!,
            outputSeq: "1",
            bytes: [65],
          },
        }],
      })
      .mockResolvedValueOnce({
        stateRevision: running.stateRevision,
        attachmentRevision: "5",
        attachment: attachment({
          attachmentId: "019d0000-0000-7000-8000-000000000424",
          attachAttemptId: "019d0000-0000-7000-8000-000000000425",
          attachmentRevision: "5",
        }),
        replay: [{
          kind: "frame",
          payload: {
            sessionId: running.sessionId,
            generation: running.generation,
            channelId: running.channelId!,
            outputSeq: "2",
            bytes: [66],
          },
        }],
      });
    client.heartbeatSshAttachment.mockRejectedValueOnce(new Error("stale attachment"));

    const wrapper = mountPane(running);
    await flushPromises();
    expect(writes.bytes.mock.calls.map(([bytes]) => bytes)).toEqual([[65]]);

    await vi.advanceTimersByTimeAsync(10_000);
    await flushPromises();

    expect(client.attachSshSession).toHaveBeenNthCalledWith(2, {
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedStateRevision: running.stateRevision,
      viewId: paneId,
      afterOutputSeq: "1",
    }, expect.any(Function));
    expect(writes.bytes.mock.calls.map(([bytes]) => bytes)).toEqual([[65], [66]]);
    expect(writes.gap).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("does not carry an output cursor across a reconnected generation", async () => {
    vi.useFakeTimers();
    const running = summary("running", { eventSeq: "9" });
    const reconnected = summary("running", {
      generation: "3",
      stateRevision: "8",
      attachmentRevision: "5",
      eventSeq: "1",
      channelId: "019d0000-0000-7000-8000-000000000426",
    });
    client.getSshSession
      .mockResolvedValueOnce(details(running))
      .mockResolvedValueOnce(details(reconnected));
    client.attachSshSession
      .mockResolvedValueOnce({
        stateRevision: running.stateRevision,
        attachmentRevision: running.attachmentRevision,
        attachment: attachment(),
        replay: [{
          kind: "frame",
          payload: {
            sessionId: running.sessionId,
            generation: running.generation,
            channelId: running.channelId!,
            outputSeq: "7",
            bytes: [65],
          },
        }],
      })
      .mockResolvedValueOnce({
        stateRevision: reconnected.stateRevision,
        attachmentRevision: reconnected.attachmentRevision,
        attachment: attachment({
          attachmentId: "019d0000-0000-7000-8000-000000000427",
          attachAttemptId: "019d0000-0000-7000-8000-000000000428",
          generation: reconnected.generation,
          channelId: reconnected.channelId,
          stateRevision: reconnected.stateRevision,
          attachmentRevision: reconnected.attachmentRevision,
        }),
        replay: [{
          kind: "frame",
          payload: {
            sessionId: reconnected.sessionId,
            generation: reconnected.generation,
            channelId: reconnected.channelId!,
            outputSeq: "1",
            bytes: [66],
          },
        }],
      });
    client.heartbeatSshAttachment.mockRejectedValueOnce(new Error("stale attachment"));

    const wrapper = mountPane(running);
    await flushPromises();
    await vi.advanceTimersByTimeAsync(10_000);
    await flushPromises();

    expect(client.attachSshSession).toHaveBeenNthCalledWith(2, {
      sessionId: reconnected.sessionId,
      expectedGeneration: reconnected.generation,
      expectedStateRevision: reconnected.stateRevision,
      viewId: paneId,
      afterOutputSeq: null,
    }, expect.any(Function));
    expect(writes.bytes.mock.calls.map(([bytes]) => bytes)).toEqual([[65], [66]]);
    expect(writes.gap).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("registers focus and acquires a lease only when an inactive running Pane is activated", async () => {
    const running = summary("running", { eventSeq: "9" });
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running, false);
    await flushPromises();

    expect(client.changeTerminalInputFocus).not.toHaveBeenCalled();
    expect(focusedTerminalLabel.value).toBeNull();
    await wrapper.setProps({ active: true });
    (wrapper.vm as unknown as { activateFromTab(): void }).activateFromTab();
    await flushPromises();

    expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(1);
    expect(focusedTerminalLabel.value).toBe("Production");
    expect(writes.focus).toHaveBeenCalled();

    (wrapper.vm as unknown as { deactivateFromTab(): void }).deactivateFromTab();
    expect(focusedTerminalLabel.value).toBeNull();
    expect(await runInFocusedTerminal("uptime")).toBe("unavailable");
    wrapper.unmount();
  });

  it("reacquires an expired input lease while the active SSH Pane remains running", async () => {
    vi.useFakeTimers();
    const running = summary("running", { eventSeq: "9" });
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running);
    await flushPromises();
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");
    client.changeTerminalInputFocus.mockClear();
    client.renewSshInputLease.mockRejectedValueOnce(new Error("expired lease"));

    await vi.advanceTimersByTimeAsync(5_000);
    await flushPromises();

    expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(1);
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");
    wrapper.unmount();
  });

  it("merges replay with live output registered during attach without promoting a later snapshot watermark", async () => {
    const running = summary("running", { eventSeq: "4" });
    const attachResponse = {
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [{
        kind: "frame" as const,
        payload: {
          sessionId: running.sessionId,
          generation: running.generation,
          channelId: running.channelId!,
          outputSeq: "1",
          bytes: [65],
        },
      }],
    };
    const attachGate = deferred<typeof attachResponse>();
    let onEvent: ((value: SshSessionEvent) => void) | null = null;
    client.getSshSession
      .mockResolvedValueOnce(details(running))
      // The removed post-attach read used to return this higher eventSeq and
      // silently discard the live seq=5 frame that was already buffered.
      .mockResolvedValue(details(summary("running", { eventSeq: "5" })));
    client.attachSshSession.mockImplementation((_request, listener) => {
      onEvent = listener;
      return attachGate.promise;
    });

    const wrapper = mountPane(running);
    await vi.waitFor(() => expect(onEvent).not.toBeNull());
    attachGate.resolve(attachResponse);
    dispatchCapturedEvent(onEvent, event(running, {
      kind: "outputFrame",
      frame: {
        sessionId: running.sessionId,
        generation: running.generation,
        channelId: running.channelId!,
        outputSeq: "2",
        bytes: [66],
      },
    }, "5"));
    await flushPromises();
    dispatchCapturedEvent(onEvent, event(running, {
      kind: "outputFrame",
      frame: {
        sessionId: running.sessionId,
        generation: running.generation,
        channelId: running.channelId!,
        outputSeq: "3",
        bytes: [67],
      },
    }, "6"));
    await flushPromises();

    expect(client.getSshSession).toHaveBeenCalledTimes(1);
    expect(writes.bytes.mock.calls.map(([bytes]) => bytes)).toEqual([[65], [66], [67]]);
    expect(writes.gap).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("projects authoritative transport heartbeat status and applies fenced updates", async () => {
    const running = summary("running");
    let onEvent: ((value: SshSessionEvent) => void) | null = null;
    client.getSshSession.mockResolvedValue(details(running, {
      heartbeat: {
        policyRevision: "2",
        mode: "transportKeepalive",
        transports: [{
          routeStage: { kind: "target" },
          nextDueAtUnixMs: Date.now() + 30_000,
          lastSentAtUnixMs: Date.now() - 1_000,
          lastAckAtUnixMs: Date.now(),
          consecutiveFailures: 0,
        }],
        shell: null,
      },
    }));
    client.attachSshSession.mockImplementation(async (_request, listener) => {
      onEvent = listener;
      return {
        stateRevision: running.stateRevision,
        attachmentRevision: running.attachmentRevision,
        attachment: attachment(),
        replay: [],
      };
    });

    const wrapper = mountPane(running);
    await flushPromises();
    expect(wrapper.text()).toContain("SSH heartbeat · 1 routes");

    dispatchCapturedEvent(onEvent, event(running, {
      kind: "heartbeatChanged",
      heartbeat: {
        policyRevision: "2",
        mode: "transportKeepalive",
        transports: [{
          routeStage: { kind: "target" },
          nextDueAtUnixMs: Date.now() + 30_000,
          lastSentAtUnixMs: Date.now(),
          lastAckAtUnixMs: null,
          consecutiveFailures: 2,
        }],
        shell: null,
      },
    }));
    await flushPromises();
    expect(wrapper.text()).toContain("SSH heartbeat · 2 pending failures");

    dispatchCapturedEvent(onEvent, event(running, {
      kind: "stateChanged",
      previousState: "running",
      state: "disconnecting",
      closeReason: null,
      failureReason: null,
    }, "2"));
    await flushPromises();
    expect(wrapper.text()).not.toContain("SSH heartbeat");
    wrapper.unmount();
  });

  it("keeps generation fences and reports a real output gap while replaying attach-time events", async () => {
    const running = summary("running", { eventSeq: "4" });
    const attachResponse = {
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [{
        kind: "frame" as const,
        payload: {
          sessionId: running.sessionId,
          generation: running.generation,
          channelId: running.channelId!,
          outputSeq: "1",
          bytes: [65],
        },
      }],
    };
    const attachGate = deferred<typeof attachResponse>();
    let onEvent: ((value: SshSessionEvent) => void) | null = null;
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockImplementation((_request, listener) => {
      onEvent = listener;
      return attachGate.promise;
    });

    const wrapper = mountPane(running);
    await vi.waitFor(() => expect(onEvent).not.toBeNull());
    attachGate.resolve(attachResponse);
    const oldGeneration = summary("running", { generation: "1", eventSeq: "4" });
    dispatchCapturedEvent(onEvent, event(oldGeneration, {
      kind: "outputFrame",
      frame: {
        sessionId: running.sessionId,
        generation: oldGeneration.generation,
        channelId: running.channelId!,
        outputSeq: "2",
        bytes: [88],
      },
    }, "5"));
    dispatchCapturedEvent(onEvent, event(running, {
      kind: "outputFrame",
      frame: {
        sessionId: running.sessionId,
        generation: running.generation,
        channelId: running.channelId!,
        outputSeq: "3",
        bytes: [67],
      },
    }, "5"));
    await flushPromises();
    dispatchCapturedEvent(onEvent, event(running, {
      kind: "outputFrame",
      frame: {
        sessionId: running.sessionId,
        generation: running.generation,
        channelId: running.channelId!,
        outputSeq: "4",
        bytes: [68],
      },
    }, "6"));
    await flushPromises();

    expect(writes.bytes.mock.calls.map(([bytes]) => bytes)).toEqual([[65], [67], [68]]);
    expect(writes.gap).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("fails closed in a visible Failed state when opening cannot reach Core", async () => {
    client.openSshSession.mockRejectedValue(new Error("IPC unavailable"));
    const wrapper = mountPane();
    await flushPromises();

    expect(wrapper.text()).toContain(i18n.global.t("sshSession.states.failed"));
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("true");

    const running = summary("running");
    client.openSshSession.mockResolvedValue({
      operationId: "019d0000-0000-7000-8000-000000000461",
      idempotencyKey: "retry-open",
      openAttemptId: running.openAttemptId,
      stateRevision: running.stateRevision,
      session: running,
      attachment: attachment(),
    });
    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();

    expect(client.openSshSession).toHaveBeenCalledTimes(2);
    expect(wrapper.text()).toContain(i18n.global.t("sshSession.states.running"));
    expect(wrapper.get(".ssh-terminal-pane__state").text()).toBe(
      i18n.global.t("sshSession.states.running"),
    );
    wrapper.unmount();
  });

  it("requests typed authentication recovery when a fresh open cannot resolve credentials", async () => {
    client.openSshSession.mockRejectedValueOnce({
      code: "ssh_terminal.credential_unavailable",
      messageKey: "errors.sshSession.credentialUnavailable",
    });
    const wrapper = mountPane(null, true, false, "reconnect", null);
    await flushPromises();

    expect(wrapper.emitted("requestAuthenticationRecovery")?.[0]).toEqual([paneId, target]);
    expect(wrapper.text()).toContain(i18n.global.t("sshSession.states.failed"));
    wrapper.unmount();
  });

  it("disconnects explicitly and requests new authentication instead of retrying a missing credential", async () => {
    const reason: SshSessionFailureReason = {
      code: "credentialUnavailable",
      stage: "authentication",
      routeStage: null,
      algorithmNegotiation: null,
      retryStrategy: "chooseCredential",
      messageKey: "sshSession.failureFallback",
      diagnosticId: null,
    };
    const failed = summary("failed", { failureReason: reason });
    client.getSshSession.mockResolvedValue(details(failed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: failed.stateRevision,
      attachmentRevision: "3",
      attachment: attachment({ channelId: null }),
      replay: [],
    });
    const wrapper = mountPane(failed);
    await flushPromises();

    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();
    expect(client.reconnectSshSession).not.toHaveBeenCalled();
    expect(wrapper.emitted("requestCredential")?.[0]).toEqual([paneId, target]);
    wrapper.unmount();

    vi.clearAllMocks();
    const running = summary("running");
    client.getSshSession.mockResolvedValue(details(running));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    client.disconnectSshSession.mockResolvedValue(details(summary("disconnecting")));
    const runningWrapper = mountPane(running);
    await flushPromises();
    await runningWrapper.get(".ssh-terminal-pane__disconnect").trigger("click");
    await flushPromises();
    expect(client.disconnectSshSession).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedStateRevision: running.stateRevision,
    });
    runningWrapper.unmount();
  });

  it("requests Vault unlock instead of blindly retrying a locked proxy credential", async () => {
    const reason: SshSessionFailureReason = {
      code: "proxyCredentialLocked",
      stage: "routeIngress",
      routeStage: { kind: "ingress" },
      algorithmNegotiation: null,
      retryStrategy: "unlockVault",
      messageKey: "errors.sshSession.proxyCredentialLocked",
      diagnosticId: null,
    };
    const failed = summary("failed", { failureReason: reason });
    client.getSshSession.mockResolvedValue(details(failed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: failed.stateRevision,
      attachmentRevision: "3",
      attachment: attachment({ channelId: null }),
      replay: [],
    });
    const wrapper = mountPane(failed);
    await flushPromises();

    expect(wrapper.get(".ssh-terminal-pane__reconnect").text()).toContain(
      i18n.global.t("sshSession.unlockVault"),
    );
    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();

    expect(client.reconnectSshSession).not.toHaveBeenCalled();
    expect(wrapper.emitted("requestVaultUnlock")?.[0]).toEqual([paneId, target]);
    wrapper.unmount();
  });

  it("shows the exact one-based jump position and endpoint for a route failure", async () => {
    const reason: SshSessionFailureReason = {
      code: "jumpChannelFailed",
      stage: "routeIngress",
      routeStage: {
        kind: "jumpHost",
        hopIndex: 1,
        hostId: "019d0000-0000-7000-8000-000000000403",
        endpoint: {
          address: "jump.example",
          port: 2222,
          username: "deploy",
        },
      },
      algorithmNegotiation: null,
      retryStrategy: "retryOpen",
      messageKey: "errors.sshSession.jumpChannelFailed",
      diagnosticId: null,
    };
    const failed = summary("failed", { failureReason: reason });
    client.getSshSession.mockResolvedValue(details(failed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: failed.stateRevision,
      attachmentRevision: "3",
      attachment: attachment({ channelId: null }),
      replay: [],
    });

    const wrapper = mountPane(failed);
    await flushPromises();
    expect(wrapper.get(".ssh-terminal-pane__failure").text()).toContain(
      "Jump 2 (jump.example:2222)",
    );
    expect(wrapper.get(".ssh-terminal-pane__failure").text()).toContain(
      "could not open an SSH forwarding channel",
    );
    wrapper.unmount();
  });

  it("shows the structured algorithm negotiation category and candidate summaries", async () => {
    const reason: SshSessionFailureReason = {
      code: "algorithmNegotiationFailed",
      stage: "connecting",
      routeStage: { kind: "target" },
      algorithmNegotiation: {
        category: "hostKey",
        clientCandidates: ["ssh-ed25519"],
        serverCandidates: ["ssh-rsa"],
      },
      retryStrategy: "never",
      messageKey: "errors.sshSession.algorithmNegotiationFailed",
      diagnosticId: null,
    };
    const failed = summary("failed", { failureReason: reason });
    client.getSshSession.mockResolvedValue(details(failed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: failed.stateRevision,
      attachmentRevision: "3",
      attachment: attachment({ channelId: null }),
      replay: [],
    });

    const wrapper = mountPane(failed);
    await flushPromises();
    expect(document.body.textContent).toContain("Server host key");
    expect(document.body.textContent).toContain("ssh-ed25519");
    expect(document.body.textContent).toContain("ssh-rsa");
    wrapper.unmount();
  });

  it("refreshes after a rejected disconnect without leaving an unhandled action", async () => {
    const running = summary("running");
    const refreshed = summary("running", { stateRevision: "8" });
    client.getSshSession
      .mockResolvedValueOnce(details(running))
      .mockResolvedValueOnce(details(refreshed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    client.disconnectSshSession.mockRejectedValueOnce(new Error("stale disconnect fence"));
    const wrapper = mountPane(running);
    await flushPromises();

    await wrapper.get(".ssh-terminal-pane__disconnect").trigger("click");
    await flushPromises();

    expect(client.getSshSession).toHaveBeenCalledTimes(2);
    expect(wrapper.text()).toContain(i18n.global.t("sshSession.states.running"));
    expect(wrapper.get(".ssh-terminal-pane__disconnect").attributes("disabled"))
      .toBeUndefined();

    client.disconnectSshSession.mockResolvedValueOnce(details(summary("disconnecting")));
    await wrapper.get(".ssh-terminal-pane__disconnect").trigger("click");
    await flushPromises();
    expect(client.disconnectSshSession).toHaveBeenLastCalledWith({
      sessionId: refreshed.sessionId,
      expectedGeneration: refreshed.generation,
      expectedStateRevision: refreshed.stateRevision,
    });
    wrapper.unmount();
  });

  it("reconciles a server-side idle disconnect when the app returns to the foreground", async () => {
    const running = summary("running");
    const failed = summary("failed", {
      stateRevision: "8",
      failureReason: {
        code: "connectionLost",
        stage: "running",
        routeStage: null,
        algorithmNegotiation: null,
        retryStrategy: "retryOpen",
        messageKey: "errors.sshSession.connectionLost",
        diagnosticId: null,
      },
    });
    client.getSshSession
      .mockResolvedValueOnce(details(running))
      .mockResolvedValueOnce(details(failed, { inputLease: null }));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running);
    await flushPromises();

    await (wrapper.vm as unknown as {
      reconcileAfterForeground(): Promise<void>;
    }).reconcileAfterForeground();
    await flushPromises();

    expect(wrapper.get(".ssh-terminal-pane__state").text()).toBe(
      i18n.global.t("sshSession.states.failed"),
    );
    expect(wrapper.text()).toContain(i18n.global.t("errors.sshSession.connectionLost"));
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("true");
    wrapper.unmount();
  });

  it("reacquires input ownership after a foreground refresh finds the SSH session running", async () => {
    const running = summary("running");
    client.getSshSession.mockResolvedValue(details(running, { inputLease: null }));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running);
    await flushPromises();
    client.changeTerminalInputFocus.mockClear();

    await (wrapper.vm as unknown as {
      reconcileAfterForeground(): Promise<void>;
    }).reconcileAfterForeground();
    await flushPromises();

    expect(client.changeTerminalInputFocus).toHaveBeenCalledTimes(1);
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");
    wrapper.unmount();
  });

  it("propagates an unaccepted close disconnect so an optimistic Tab can be restored", async () => {
    const running = summary("running");
    const refreshed = summary("running", { stateRevision: "8" });
    client.getSshSession
      .mockResolvedValueOnce(details(running))
      .mockResolvedValueOnce(details(refreshed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    client.disconnectSshSession.mockRejectedValueOnce(new Error("stale disconnect fence"));
    const wrapper = mountPane(running);
    await flushPromises();

    await expect((wrapper.vm as unknown as {
      disconnectForClose(): Promise<void>;
    }).disconnectForClose()).rejects.toThrow("stale disconnect fence");
    expect(client.getSshSession).toHaveBeenCalledTimes(2);
    expect(wrapper.text()).toContain(i18n.global.t("sshSession.states.running"));
    wrapper.unmount();
  });

  it.each(["closed", "failed"] as const)("reconnects %s on input once, honoring active state, dialogs and preference", async (state) => {
    const closed = summary(state);
    client.getSshSession.mockResolvedValue(details(closed));
    client.attachSshSession.mockResolvedValue({ stateRevision: closed.stateRevision, attachmentRevision: "3", attachment: attachment({ channelId: null }), replay: [] });
    const pending = deferred<SshSessionDetails>();
    client.reconnectSshSession.mockReturnValue(pending.promise);
    const wrapper = mountPane(closed);
    await flushPromises();
    const view = wrapper.findComponent(terminalViewStub);
    const preferences = useTerminalPreferencesStore();
    expect(view.props("reconnectOnInput")).toBe(true);
    preferences.setInteraction({ ...preferences.preferences.interaction, sshReconnectOnInput: false });
    view.vm.$emit("reconnectRequest");
    await flushPromises();
    expect(client.reconnectSshSession).not.toHaveBeenCalled();
    preferences.setInteraction({ ...preferences.preferences.interaction, sshReconnectOnInput: true });
    await wrapper.setProps({ active: false });
    view.vm.$emit("reconnectRequest");
    await flushPromises();
    expect(client.reconnectSshSession).not.toHaveBeenCalled();
    await wrapper.setProps({ active: true });
    const modal = document.createElement("div");
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    document.body.append(modal);
    view.vm.$emit("reconnectRequest");
    await flushPromises();
    expect(client.reconnectSshSession).not.toHaveBeenCalled();
    modal.remove();
    view.vm.$emit("reconnectRequest");
    view.vm.$emit("reconnectRequest");
    await flushPromises();
    expect(client.reconnectSshSession).toHaveBeenCalledTimes(1);
    expect(view.props("readOnly")).toBe(true);
    pending.resolve(details(summary("connecting", { generation: "3", stateRevision: "8" })));
    await flushPromises();
    view.vm.$emit("reconnectRequest");
    await flushPromises();
    expect(client.reconnectSshSession).toHaveBeenCalledTimes(1);
    expect(client.sendSshInput).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it.each(["credential", "vaultUnlock"] as const)("routes input through existing %s recovery without replay", async (recovery) => {
    const wrapper = mountPane(null, true, true, recovery, null);
    await flushPromises();
    wrapper.findComponent(terminalViewStub).vm.$emit("reconnectRequest");
    await flushPromises();
    expect(wrapper.emitted(recovery === "credential" ? "requestCredential" : "requestVaultUnlock")).toEqual([[paneId, target]]);
    expect(client.reconnectSshSession).not.toHaveBeenCalled();
    expect(client.sendSshInput).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reconnects a closed session with a fresh generation and the current terminal size", async () => {
    const closed = summary("closed", { channelId: null });
    const reconnected = summary("connecting", { generation: "3", stateRevision: "8" });
    client.getSshSession.mockResolvedValue(details(closed));
    client.attachSshSession.mockResolvedValue({
      stateRevision: closed.stateRevision,
      attachmentRevision: "3",
      attachment: attachment({ channelId: null }),
      replay: [],
    });
    client.reconnectSshSession.mockResolvedValue(details(reconnected, {
      attachments: [attachment({ generation: "3", stateRevision: "8", channelId: null })],
    }));
    const wrapper = mountPane(closed);
    await flushPromises();

    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();
    expect(client.reconnectSshSession).toHaveBeenCalledWith({
      sessionId: closed.sessionId,
      expectedGeneration: "2",
      expectedStateRevision: "7",
      attachmentId: "019d0000-0000-7000-8000-000000000421",
      viewId: "019d0000-0000-7000-8000-000000000401",
      credentialRefId: null,
      rows: 28,
      cols: 96,
    });
    wrapper.unmount();
  });

  it("accepts the replacement Channel before acquiring input after reconnect", async () => {
    const closed = summary("closed", { channelId: null, eventSeq: "4" });
    const connecting = summary("connecting", {
      generation: "3",
      stateRevision: "8",
      attachmentRevision: "5",
      eventSeq: "1",
      channelId: null,
    });
    const running = summary("running", {
      generation: "3",
      stateRevision: "9",
      attachmentRevision: "6",
      eventSeq: "3",
      channelId: "019d0000-0000-7000-8000-000000000426",
    });
    let onEvent: ((value: SshSessionEvent) => void) | null = null;
    client.getSshSession.mockResolvedValue(details(closed));
    client.attachSshSession.mockImplementation(async (_request, listener) => {
      onEvent = listener;
      return {
        stateRevision: closed.stateRevision,
        attachmentRevision: closed.attachmentRevision,
        attachment: attachment({ channelId: null }),
        replay: [],
      };
    });
    client.reconnectSshSession.mockResolvedValue(details(connecting, {
      attachments: [attachment({
        generation: connecting.generation,
        stateRevision: connecting.stateRevision,
        attachmentRevision: connecting.attachmentRevision,
        channelId: null,
      })],
    }));
    const wrapper = mountPane(closed);
    await flushPromises();
    client.changeTerminalInputFocus.mockClear();

    await wrapper.get(".ssh-terminal-pane__reconnect").trigger("click");
    await flushPromises();
    dispatchCapturedEvent(onEvent, event(running, {
      kind: "attachmentChanged",
      change: "attached",
      attachmentRevision: running.attachmentRevision,
      attachment: attachment({
        generation: running.generation,
        stateRevision: running.stateRevision,
        attachmentRevision: running.attachmentRevision,
        channelId: running.channelId,
      }),
    }, "2"));
    dispatchCapturedEvent(onEvent, event(running, {
      kind: "stateChanged",
      previousState: "connecting",
      state: "running",
      closeReason: null,
      failureReason: null,
    }, "3"));
    await flushPromises();

    expect(client.changeTerminalInputFocus).toHaveBeenCalledWith({
      expectedFocusEpoch: "1",
      target: expect.objectContaining({
        kind: "ssh",
        target: expect.objectContaining({
          expectedGeneration: "3",
          channelId: running.channelId,
        }),
      }),
    });
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");
    wrapper.unmount();
  });

  it("shows login automation progress and takes input ownership through one fenced action", async () => {
    const automating = summary("automatingLogin", {
      channelId: "019d0000-0000-7000-8000-000000000414",
    });
    const progress = {
      policyRevision: "4",
      currentStepIndex: 0,
      totalSteps: 2,
      stepKind: "expect" as const,
      secretLabel: null,
      status: "running" as const,
      failureCode: null,
      startedAtUnixMs: 1,
      stepDeadlineUnixMs: Date.now() + 10_000,
    };
    client.getSshSession.mockResolvedValue(details(automating, {
      activeLoginAutomation: progress,
    }));
    client.attachSshSession.mockResolvedValue({
      stateRevision: automating.stateRevision,
      attachmentRevision: "3",
      attachment: attachment(),
      replay: [],
    });
    const running = summary("running", { stateRevision: "9" });
    client.takeoverSshLoginAutomation.mockResolvedValue({
      details: details(running, { inputLease: inputLease({ focusEpoch: "2" }) }),
      focus: {
        focusEpoch: "2",
        target: {
          sessionId: running.sessionId,
          expectedGeneration: running.generation,
          expectedStateRevision: running.stateRevision,
          channelId: running.channelId,
          attachmentId: attachment().attachmentId,
          viewId: paneId,
        },
        lease: inputLease({ focusEpoch: "2" }),
      },
    });

    const wrapper = mountPane(automating);
    await flushPromises();
    expect(document.body.textContent).toContain("Running post-login commands: step 1 of 2");
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("true");

    bodyButton(i18n.global.t("sshSession.loginAutomation.takeover"))?.click();
    await flushPromises();

    expect(client.takeoverSshLoginAutomation).toHaveBeenCalledWith({
      sessionId: automating.sessionId,
      expectedGeneration: automating.generation,
      expectedStateRevision: automating.stateRevision,
      expectedFocusEpoch: "1",
      channelId: automating.channelId,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
    });
    expect(document.body.textContent).not.toContain("Running post-login commands: step 1 of 2");
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");
    wrapper.unmount();
  });
});
