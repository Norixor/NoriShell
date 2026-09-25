import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  LocalSessionAttachment,
  LocalSessionDetails,
  LocalSessionInputLease,
  LocalSessionState,
  LocalSessionSummary,
} from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import { useTipsStore } from "../../stores/tips";
import { resetTerminalInputFocusForTests } from "../../terminal-input-target";

const client = vi.hoisted(() => ({
  attachLocalSession: vi.fn(),
  changeTerminalInputFocus: vi.fn(),
  detachLocalSession: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
  getLocalSession: vi.fn(),
  heartbeatLocalAttachment: vi.fn(),
  openLocalSession: vi.fn(),
  renewLocalInputLease: vi.fn(),
  resizeLocalTerminal: vi.fn(),
  sendLocalInput: vi.fn(),
  terminateLocalSession: vi.fn(),
}));

vi.mock("../../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../core-api/client")>();
  return { ...actual, ...client };
});

import NvxLocalTerminalPane from "./NvxLocalTerminalPane.vue";

const paneId = "019d0000-0000-7000-8000-000000001001";
let viewDimensions = { rows: 31, cols: 101 };
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
    terminalLabel: { type: String, default: "" },
    gapLabel: { type: String, default: "" },
    shellPromptKey: { type: String, default: null },
  },
  emits: ["input", "resize", "searchRequest", "selectionChange"],
  setup(props, { expose }) {
    expose({
      writeBytes: writes.bytes,
      writeGap: writes.gap,
      dimensions: () => viewDimensions,
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
  state: LocalSessionState,
  overrides: Partial<LocalSessionSummary> = {},
): LocalSessionSummary {
  return {
    sessionId: "019d0000-0000-7000-8000-000000001011",
    openAttemptId: "019d0000-0000-7000-8000-000000001012",
    shellName: "zsh",
    generation: "1",
    stateRevision: "3",
    attachmentRevision: "2",
    eventSeq: "0",
    ptyId: state === "running" ? "019d0000-0000-7000-8000-000000001013" : null,
    state,
    exit: null,
    failureReason: null,
    attachmentCount: 1,
    createdAtUnixMs: 1,
    updatedAtUnixMs: 2,
    ...overrides,
  };
}

function attachment(overrides: Partial<LocalSessionAttachment> = {}): LocalSessionAttachment {
  return {
    attachmentId: "019d0000-0000-7000-8000-000000001021",
    attachAttemptId: "019d0000-0000-7000-8000-000000001022",
    sessionId: "019d0000-0000-7000-8000-000000001011",
    generation: "1",
    ptyId: "019d0000-0000-7000-8000-000000001013",
    viewId: paneId,
    stateRevision: "3",
    attachmentRevision: "2",
    attachedAtUnixMs: 2,
    ...overrides,
  };
}

function lease(overrides: Partial<LocalSessionInputLease> = {}): LocalSessionInputLease {
  return {
    leaseId: "019d0000-0000-7000-8000-000000001031",
    sessionId: "019d0000-0000-7000-8000-000000001011",
    generation: "1",
    attachmentId: attachment().attachmentId,
    viewId: paneId,
    focusEpoch: "1",
    inputEpoch: "4",
    expiresAtUnixMs: Date.now() + 60_000,
    ...overrides,
  };
}

function details(session: LocalSessionSummary): LocalSessionDetails {
  return { session, attachments: [attachment()], inputLease: null };
}

function mountPane(
  existingSession: LocalSessionSummary | null = null,
  active = true,
  deferredStart = false,
  visible = true,
) {
  const host = document.createElement("div");
  document.body.append(host);
  return mount(NvxLocalTerminalPane, {
    attachTo: host,
    props: {
      paneId,
      label: "Local Shell",
      existingSession,
      deferredStart,
      active,
      visible,
      canSplitHorizontal: true,
      canSplitVertical: true,
      canSplitWorkspaceRight: true,
    },
    global: { plugins: [createPinia(), i18n], stubs: { NvxTerminalView: terminalViewStub } },
  });
}

describe("NvxLocalTerminalPane", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    viewDimensions = { rows: 31, cols: 101 };
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
        lease: target?.kind === "local"
          ? { kind: "local", lease: lease({ focusEpoch }) }
          : null,
      };
    });
    client.heartbeatLocalAttachment.mockResolvedValue(attachment());
    client.renewLocalInputLease.mockResolvedValue(lease());
    client.sendLocalInput.mockResolvedValue(undefined);
    client.resizeLocalTerminal.mockResolvedValue(undefined);
    client.detachLocalSession.mockResolvedValue({
      kind: "detached",
      session: summary("closed"),
      remainingAttachmentCount: 0,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = "";
  });

  it("starts a restored local Pane with a fresh PTY", async () => {
    const running = summary("running");
    client.openLocalSession.mockResolvedValue({ session: running, attachment: attachment() });
    const wrapper = mountPane(null, true, false);
    await flushPromises();

    expect(client.openLocalSession).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("accepts fresh output sequence numbers after restarting a failed Shell", async () => {
    let onEvent!: (event: unknown) => void;
    const oldSession = summary("running");
    client.openLocalSession.mockImplementationOnce((_request, handler) => {
      onEvent = handler;
      return Promise.resolve({ session: oldSession, attachment: attachment() });
    });
    const wrapper = mountPane();
    await flushPromises();
    const output = (current: LocalSessionSummary, sequence: string, byte: number) => ({
      schemaVersion: 1,
      sessionId: current.sessionId,
      generation: current.generation,
      stateRevision: current.stateRevision,
      eventSeq: sequence,
      occurredAtUnixMs: 1,
      payload: { kind: "outputFrame", frame: {
        sessionId: current.sessionId, generation: current.generation,
        ptyId: current.ptyId, outputSeq: sequence, bytes: [byte],
      } },
    });
    onEvent(output(oldSession, "10", 65));
    onEvent({
      schemaVersion: 1, sessionId: oldSession.sessionId,
      generation: oldSession.generation, stateRevision: "4", eventSeq: "11",
      occurredAtUnixMs: 2,
      payload: { kind: "stateChanged", previousState: "running", state: "failed", exit: null, failureReason: null },
    });
    await flushPromises();

    const nextSession = summary("running", {
      sessionId: "019d0000-0000-7000-8000-000000001031",
      ptyId: "019d0000-0000-7000-8000-000000001033",
    });
    client.openLocalSession.mockImplementationOnce((_request, handler) => {
      onEvent = handler;
      return Promise.resolve({
        session: nextSession,
        attachment: attachment({ sessionId: nextSession.sessionId, ptyId: nextSession.ptyId }),
      });
    });
    const restart = wrapper.findAll("button").find(button => button.text().includes("Restart"));
    expect(restart).toBeDefined();
    await restart!.trigger("click");
    await flushPromises();
    onEvent(output(nextSession, "1", 66));
    await flushPromises();
    expect(writes.bytes).toHaveBeenLastCalledWith([66]);
    wrapper.unmount();
  });

  it("opens the default PTY and sends fenced input and resize through the shared focus broker", async () => {
    const running = summary("running");
    client.openLocalSession.mockResolvedValue({ session: running, attachment: attachment() });
    const wrapper = mountPane();
    await flushPromises();

    expect(client.openLocalSession).toHaveBeenCalledWith({
      viewId: paneId,
      rows: 31,
      cols: 101,
    }, expect.any(Function));
    expect(client.changeTerminalInputFocus).toHaveBeenLastCalledWith({
      expectedFocusEpoch: "1",
      target: {
        kind: "local",
        target: {
          sessionId: running.sessionId,
          expectedGeneration: running.generation,
          expectedStateRevision: running.stateRevision,
          ptyId: running.ptyId,
          attachmentId: attachment().attachmentId,
          viewId: paneId,
        },
      },
    });
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");

    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("input", "é\u001b[A");
    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("resize", 40, 120);
    await flushPromises();

    expect(client.sendLocalInput).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: running.sessionId,
      ptyId: running.ptyId,
      focusEpoch: "2",
      clientSeq: "1",
      bytes: [195, 169, 27, 91, 65],
    }));
    expect(client.resizeLocalTerminal).toHaveBeenCalledWith(expect.objectContaining({
      resizeSeq: "2",
      rows: 40,
      cols: 120,
    }));
    wrapper.unmount();
  });

  it("handles rejected direct xterm input without retrying and requires an output check", async () => {
    const running = summary("running");
    client.openLocalSession.mockResolvedValue({ session: running, attachment: attachment() });
    client.sendLocalInput.mockRejectedValueOnce(new Error("write result unavailable"));
    const wrapper = mountPane();
    await flushPromises();

    const terminal = wrapper.findComponent({ name: "NvxTerminalView" });
    terminal.vm.$emit("input", "pwd\r");
    await flushPromises();

    expect(client.sendLocalInput).toHaveBeenCalledTimes(1);
    expect(wrapper.text()).not.toContain("The send result is uncertain. Check the terminal before retrying.");
    expect(useTipsStore().items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "error", title: "The send result is uncertain. Check the terminal before retrying." }),
    ]));
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("true");

    terminal.vm.$emit("input", "pwd\r");
    await flushPromises();
    expect(client.sendLocalInput).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("replays the newest terminal size after a hidden local Tab becomes visible", async () => {
    const running = summary("running");
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running, false, false, false);
    await flushPromises();

    viewDimensions = { rows: 42, cols: 132 };
    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("resize", 42, 132);
    await flushPromises();
    expect(client.resizeLocalTerminal).not.toHaveBeenCalled();

    await wrapper.setProps({ active: true, visible: true });
    (wrapper.vm as unknown as { activateFromTab(): void }).activateFromTab();
    await flushPromises();

    expect(client.resizeLocalTerminal).toHaveBeenCalledWith(expect.objectContaining({
      rows: 42,
      cols: 132,
    }));
    wrapper.unmount();
  });

  it("reasserts the current size when a visible local view returns without a new fit event", async () => {
    const running = summary("running");
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running, false);
    await flushPromises();
    client.resizeLocalTerminal.mockClear();

    await wrapper.setProps({ visible: false });
    await wrapper.setProps({ visible: true });
    await flushPromises();

    expect(client.resizeLocalTerminal).toHaveBeenCalledWith(expect.objectContaining({
      rows: 31,
      cols: 101,
      attachmentId: attachment().attachmentId,
    }));
    expect(client.sendLocalInput).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("resizes a visible local Pane even while another Pane owns input focus", async () => {
    const running = summary("running");
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running, false);
    await flushPromises();

    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("resize", 42, 132);
    await flushPromises();

    expect(client.resizeLocalTerminal).toHaveBeenCalledWith(expect.objectContaining({
      rows: 42,
      cols: 132,
      attachmentId: attachment().attachmentId,
    }));
    expect(client.changeTerminalInputFocus).not.toHaveBeenCalledWith(expect.objectContaining({
      target: expect.objectContaining({ kind: "local" }),
    }));
    wrapper.unmount();
  });

  it("sends geometry changes for the active local attachment before input focus is granted", async () => {
    const running = summary("running");
    client.changeTerminalInputFocus.mockImplementation(() => new Promise(() => undefined));
    client.openLocalSession.mockResolvedValue({ session: running, attachment: attachment() });
    const wrapper = mountPane();
    await flushPromises();

    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("resize", 35, 116);
    await flushPromises();

    expect(client.resizeLocalTerminal).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedStateRevision: running.stateRevision,
      ptyId: running.ptyId,
      attachmentId: attachment().attachmentId,
      viewId: paneId,
      resizeSeq: "2",
      rows: 35,
      cols: 116,
    });
    expect(client.sendLocalInput).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("connects xterm search requests and explicit selection state to the shared tools", async () => {
    const running = summary("running");
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
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

  it("keeps search field focus out of the local PTY input path", async () => {
    const running = summary("running");
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    const wrapper = mountPane(running);
    await flushPromises();
    writes.focus.mockClear();
    client.sendLocalInput.mockClear();

    wrapper.findComponent({ name: "NvxTerminalView" }).vm.$emit("searchRequest");
    await flushPromises();
    await wrapper.get("[role='search'] input").trigger("focusin");
    await flushPromises();

    expect(writes.focus).not.toHaveBeenCalled();
    expect(client.sendLocalInput).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("reattaches, replays raw output, heartbeats, and releases only the renderer binding", async () => {
    vi.useFakeTimers();
    const running = summary("running", { eventSeq: "5" });
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [{
        kind: "frame",
        payload: {
          sessionId: running.sessionId,
          generation: running.generation,
          ptyId: running.ptyId!,
          outputSeq: "1",
          bytes: [27, 91, 51, 49, 109],
        },
      }],
    });
    const wrapper = mountPane(running);
    await flushPromises();

    expect(writes.bytes).toHaveBeenCalledWith([27, 91, 51, 49, 109]);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(client.heartbeatLocalAttachment).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: running.sessionId,
      attachmentId: attachment().attachmentId,
    }));

    wrapper.unmount();
    await flushPromises();
    expect(client.detachLocalSession).toHaveBeenCalledWith(expect.objectContaining({
      intent: "rendererUnavailable",
      confirmation: null,
    }));
  });

  it("resumes after a stale heartbeat without replaying output already rendered", async () => {
    vi.useFakeTimers();
    const running = summary("running", { eventSeq: "5" });
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession
      .mockResolvedValueOnce({
        stateRevision: running.stateRevision,
        attachmentRevision: running.attachmentRevision,
        attachment: attachment(),
        replay: [{
          kind: "frame",
          payload: {
            sessionId: running.sessionId,
            generation: running.generation,
            ptyId: running.ptyId!,
            outputSeq: "1",
            bytes: [65],
          },
        }],
      })
      .mockResolvedValueOnce({
        stateRevision: running.stateRevision,
        attachmentRevision: "4",
        attachment: attachment({
          attachmentId: "019d0000-0000-7000-8000-000000001024",
          attachAttemptId: "019d0000-0000-7000-8000-000000001025",
          attachmentRevision: "4",
        }),
        replay: [{
          kind: "frame",
          payload: {
            sessionId: running.sessionId,
            generation: running.generation,
            ptyId: running.ptyId!,
            outputSeq: "2",
            bytes: [66],
          },
        }],
      });
    client.heartbeatLocalAttachment.mockRejectedValueOnce(new Error("stale attachment"));

    const wrapper = mountPane(running);
    await flushPromises();
    expect(writes.bytes.mock.calls.map(([bytes]) => bytes)).toEqual([[65]]);

    await vi.advanceTimersByTimeAsync(10_000);
    await flushPromises();

    expect(client.attachLocalSession).toHaveBeenNthCalledWith(2, {
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

  it("terminates an active local process explicitly before the parent removes its tab", async () => {
    const running = summary("running");
    client.getLocalSession.mockResolvedValue(details(running));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: running.stateRevision,
      attachmentRevision: running.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    client.terminateLocalSession.mockResolvedValue(summary("stopping"));
    const wrapper = mountPane(running);
    await flushPromises();

    await (wrapper.vm as unknown as { terminateForClose(): Promise<void> }).terminateForClose();
    expect(client.terminateLocalSession).toHaveBeenCalledWith({
      sessionId: running.sessionId,
      expectedGeneration: running.generation,
      expectedStateRevision: running.stateRevision,
    });
    wrapper.unmount();
  });

  it("keeps process-cleanup failures actionable and retries explicit termination", async () => {
    const cleanupFailed = summary("failed", {
      failureReason: {
        code: "processCleanupFailed",
        messageKey: "errors.localTerminal.processCleanupFailed",
        diagnosticId: null,
      },
    });
    client.getLocalSession.mockResolvedValue(details(cleanupFailed));
    client.attachLocalSession.mockResolvedValue({
      stateRevision: cleanupFailed.stateRevision,
      attachmentRevision: cleanupFailed.attachmentRevision,
      attachment: attachment(),
      replay: [],
    });
    client.terminateLocalSession.mockResolvedValue(summary("stopping"));
    const wrapper = mountPane(cleanupFailed);
    await flushPromises();

    const terminateButton = wrapper.findAll("button")
      .find((candidate) => candidate.text().includes("Terminate"));
    expect(terminateButton).toBeDefined();
    await terminateButton!.trigger("click");
    await flushPromises();
    expect(client.terminateLocalSession).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: cleanupFailed.sessionId,
      expectedStateRevision: cleanupFailed.stateRevision,
    }));
    wrapper.unmount();
  });

  it("preserves control events when bounded attach buffering drops output", async () => {
    let resolveOpen!: (value: {
      session: LocalSessionSummary;
      attachment: LocalSessionAttachment;
    }) => void;
    let onEvent!: (event: unknown) => void;
    client.openLocalSession.mockImplementation((_request, eventHandler) => {
      onEvent = eventHandler;
      return new Promise((resolve) => { resolveOpen = resolve; });
    });
    const wrapper = mountPane();
    await flushPromises();

    const starting = summary("starting", { stateRevision: "1", ptyId: null });
    for (let index = 1; index <= 300; index += 1) {
      onEvent({
        schemaVersion: 1,
        sessionId: starting.sessionId,
        generation: starting.generation,
        stateRevision: "1",
        eventSeq: String(index),
        occurredAtUnixMs: index,
        payload: {
          kind: "outputFrame",
          frame: {
            sessionId: starting.sessionId,
            generation: starting.generation,
            ptyId: attachment().ptyId!,
            outputSeq: String(index),
            bytes: [index % 256],
          },
        },
      });
    }
    onEvent({
      schemaVersion: 1,
      sessionId: starting.sessionId,
      generation: starting.generation,
      stateRevision: "2",
      eventSeq: "301",
      occurredAtUnixMs: 301,
      payload: {
        kind: "stateChanged",
        previousState: "starting",
        state: "running",
        exit: null,
        failureReason: null,
      },
    });
    onEvent({
      schemaVersion: 1,
      sessionId: starting.sessionId,
      generation: starting.generation,
      stateRevision: "2",
      eventSeq: "302",
      occurredAtUnixMs: 302,
      payload: {
        kind: "attachmentChanged",
        change: "attached",
        attachmentRevision: "2",
        attachment: attachment(),
      },
    });
    resolveOpen({ session: starting, attachment: attachment() });
    await flushPromises();

    expect(writes.gap).toHaveBeenCalled();
    expect(wrapper.get(".terminal-view-stub").attributes("data-read-only")).toBe("false");
    wrapper.unmount();
  });
});
