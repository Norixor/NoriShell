import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { defineComponent, h } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../locales";
import { resetTerminalInputFocusForTests } from "../../terminal-input-target";
import type { PluginTerminalSessionEvent, PluginTerminalSessionSummary, PluginTerminalSessionAttachment } from "../../core-api/plugin-terminal";

const api = vi.hoisted(() => ({
  claimPluginProtocolLaunch: vi.fn(), openPluginTerminalSession: vi.fn(), attachPluginTerminalSession: vi.fn(),
  detachPluginTerminalSession: vi.fn(), disconnectPluginTerminalSession: vi.fn(), heartbeatPluginAttachment: vi.fn(),
  fetchPluginTerminalSessionSnapshot: vi.fn(), reconnectPluginTerminalSession: vi.fn(), renewPluginInputLease: vi.fn(),
  resizePluginTerminal: vi.fn(), sendPluginInput: vi.fn(),
}));
const focus = vi.hoisted(() => ({ changeTerminalInputFocus: vi.fn(), fetchTerminalInputFocusSnapshot: vi.fn() }));
vi.mock("../../core-api/plugin-terminal", () => api);
vi.mock("../../core-api/client", async (original) => ({ ...await original<typeof import("../../core-api/client")>(), ...focus }));
import NvxPluginTerminalPane from "./NvxPluginTerminalPane.vue";
const profile = { pluginId: "test.provider", providerId: "serial", schemaHash: "hash", configuration: { baud: 115200 } };
const session: PluginTerminalSessionSummary = {
  ...profile, sessionId: "session", tabId: "tab", paneId: "pane", label: "Device",
  generation: "1", stateRevision: "2", attachmentRevision: "1", eventSeq: "1",
  streamId: "stream", state: "running", attachmentCount: 1, cleanupBlocked: false, failureReason: null,
};
const attachment: PluginTerminalSessionAttachment = {
  sessionId: "session", attachmentId: "attachment", generation: "1", streamId: "stream", viewId: "pane",
  stateRevision: "2", attachmentRevision: "1",
};
const launch = { launchId: "launch", pluginId: profile.pluginId, providerId: profile.providerId,
  label: "Device", tabId: "tab", paneId: "pane", revision: "1", claimed: false, expiresAtUnixMs: Date.now() + 60_000 };
const view = { writeBytes: vi.fn(), writeGap: vi.fn(), focus: vi.fn(), fit: vi.fn(), dimensions: () => ({ rows: 24, cols: 80 }) };
const TerminalStub = defineComponent({
  name: "NvxTerminalView", props: { readOnly: Boolean }, emits: ["input", "resize"],
  setup(_, { expose }) { expose(view); return () => h("div", { class: "nvx-terminal-view" }); },
});
function render(props = {}) {
  return mount(NvxPluginTerminalPane, {
    props: { paneId: "pane", tabId: "tab", label: "Device", profile, existingSession: null,
      active: true, canSplitHorizontal: true, canSplitVertical: true, ...props },
    global: { plugins: [createPinia(), i18n], stubs: { NvxTerminalView: TerminalStub, NvxTerminalTools: true, NvxTerminalPaneControls: true } },
  });
}
function event(eventSeq: string, bytes: number[], generation = "1"): PluginTerminalSessionEvent {
  return { session: { ...session, generation }, sessionId: "session", generation, stateRevision: "2", eventSeq: (BigInt(eventSeq) + 1n).toString(),
    payload: { kind: "output", frame: { sessionId: "session", generation, streamId: "stream", outputSeq: eventSeq, bytes } } };
}
let onEvent: (event: PluginTerminalSessionEvent) => void;
beforeEach(() => {
  vi.clearAllMocks(); resetTerminalInputFocusForTests(); i18n.global.locale.value = "en";
  api.openPluginTerminalSession.mockResolvedValue({ session });
  api.claimPluginProtocolLaunch.mockResolvedValue({ session });
  api.attachPluginTerminalSession.mockImplementation(async (_, callback) => {
    onEvent = callback;
    return { session, attachment, replay: [] };
  });
  api.heartbeatPluginAttachment.mockResolvedValue(attachment);
  api.fetchPluginTerminalSessionSnapshot.mockResolvedValue({ sessions: [session], snapshotRevision: "1" });
  api.detachPluginTerminalSession.mockResolvedValue({ session: { ...session, state: "closed" }, remainingAttachmentCount: 0 });
  focus.fetchTerminalInputFocusSnapshot.mockResolvedValue({ focusEpoch: "0", target: null, lease: null });
  focus.changeTerminalInputFocus.mockImplementation(async ({ target }) => ({ focusEpoch: "1", target,
    lease: target ? { kind: "plugin", lease: { leaseId: "lease", sessionId: "session", generation: "1", streamId: "stream",
      attachmentId: "attachment", viewId: "pane", focusEpoch: "1", inputEpoch: "1", expiresAtUnixMs: Date.now() + 60_000 } } : null,
  }));
});

describe("provider terminal Pane", () => {
  it("claims a launch after mounting and attaches the resulting independent session", async () => {
    const wrapper = render({ launch, profile: null });
    await flushPromises();
    expect(api.claimPluginProtocolLaunch).toHaveBeenCalledWith(launch);
    expect(api.openPluginTerminalSession).not.toHaveBeenCalled();
    expect(api.attachPluginTerminalSession).toHaveBeenCalledWith(expect.objectContaining({ sessionId: "session", viewId: "pane" }), expect.any(Function));
    expect(focus.changeTerminalInputFocus).toHaveBeenCalledWith(expect.objectContaining({ target: { kind: "plugin", target: expect.objectContaining({ streamId: "stream" }) } }));
    wrapper.unmount();
  });
  it("keeps a historical profile disconnected until explicit reconnect", async () => {
    const wrapper = render({ deferredStart: true }); await flushPromises();
    expect(api.openPluginTerminalSession).not.toHaveBeenCalled();
    await wrapper.findAll("button").find((button) => button.text().includes("Reconnect"))!.trigger("click");
    await flushPromises(); expect(api.openPluginTerminalSession).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
  it("replays raw bytes once and drops old-generation events without advancing the cursor", async () => {
    api.attachPluginTerminalSession.mockImplementation(async (_, callback) => {
      onEvent = callback;
      return { session, attachment, replay: [{ kind: "frame", payload: { sessionId: "session", generation: "1", streamId: "stream", outputSeq: "1", bytes: [27, 91, 109] } }] };
    });
    const wrapper = render({ existingSession: session }); await flushPromises();
    onEvent(event("1", [27, 91, 109])); onEvent(event("99", [88], "0")); onEvent(event("2", [65]));
    expect(view.writeBytes.mock.calls).toEqual([[[27, 91, 109]], [[65]]]);
    wrapper.unmount();
  });
  it("sends input and resize only with the canonical plugin focus and complete lease fence", async () => {
    const wrapper = render(); await flushPromises();
    const terminal = wrapper.findComponent(TerminalStub);
    terminal.vm.$emit("input", "é"); terminal.vm.$emit("resize", 40, 120); await flushPromises();
    expect(api.sendPluginInput).toHaveBeenCalledWith(expect.objectContaining({ sessionId: "session", expectedGeneration: "1", expectedStateRevision: "2", streamId: "stream", attachmentId: "attachment", viewId: "pane", leaseId: "lease", focusEpoch: "1", inputEpoch: "1", clientSeq: "1", bytes: [195, 169] }));
    expect(api.resizePluginTerminal).toHaveBeenCalledWith(expect.objectContaining({ focusEpoch: "1", leaseId: "lease", rows: 40, cols: 120 }));
    await wrapper.setProps({ active: false }); terminal.vm.$emit("input", "blocked"); await flushPromises();
    expect(api.sendPluginInput).toHaveBeenCalledTimes(1); wrapper.unmount();
  });
  it("detaches through the atomic last-attachment decision on explicit close", async () => {
    const wrapper = render(); await flushPromises();
    await (wrapper.vm as unknown as { disconnectForClose(): Promise<void> }).disconnectForClose();
    expect(api.detachPluginTerminalSession).toHaveBeenCalledWith(expect.objectContaining({ intent: "userClose", disconnectIfLast: true, expectedAttachmentRevision: "1" }));
    expect(api.disconnectPluginTerminalSession).not.toHaveBeenCalled(); wrapper.unmount();
  });
  it("keeps an attached pane close as userClose when Vue unmounts before its first microtask", async () => {
    const wrapper = render(); await flushPromises();
    const closing = (wrapper.vm as unknown as { disconnectForClose(): Promise<void> }).disconnectForClose();
    wrapper.unmount();
    await closing;

    expect(api.detachPluginTerminalSession).toHaveBeenCalledTimes(1);
    expect(api.detachPluginTerminalSession).toHaveBeenCalledWith(expect.objectContaining({
      intent: "userClose", disconnectIfLast: true, attachmentId: "attachment",
    }));
    expect(api.detachPluginTerminalSession).not.toHaveBeenCalledWith(expect.objectContaining({ intent: "rendererUnavailable" }));
    expect(api.disconnectPluginTerminalSession).not.toHaveBeenCalled();
  });
  it("keeps an optimistic tab close as userClose when unmount races an opening session", async () => {
    let resolveOpen!: (value: { session: PluginTerminalSessionSummary }) => void;
    api.openPluginTerminalSession.mockImplementation(() => new Promise<{ session: PluginTerminalSessionSummary }>((resolve) => { resolveOpen = resolve; }));
    const wrapper = render(); await flushPromises();
    expect(api.openPluginTerminalSession).toHaveBeenCalledTimes(1);

    const closing = (wrapper.vm as unknown as { disconnectForClose(): Promise<void> }).disconnectForClose();
    wrapper.unmount();
    resolveOpen({ session });
    await closing;

    expect(api.detachPluginTerminalSession).toHaveBeenCalledTimes(1);
    expect(api.detachPluginTerminalSession).toHaveBeenCalledWith(expect.objectContaining({
      intent: "userClose", disconnectIfLast: true, attachmentId: "attachment",
    }));
    expect(api.detachPluginTerminalSession).not.toHaveBeenCalledWith(expect.objectContaining({ intent: "rendererUnavailable" }));
  });
  it("keeps an optimistic tab close as userClose while an attachment is pending", async () => {
    type AttachResponse = { session: PluginTerminalSessionSummary; attachment: PluginTerminalSessionAttachment; replay: [] };
    let resolveAttach!: (value: AttachResponse) => void;
    api.attachPluginTerminalSession.mockImplementation((_, callback) => {
      onEvent = callback;
      return new Promise<AttachResponse>((resolve) => { resolveAttach = resolve; });
    });
    const wrapper = render({ existingSession: session }); await flushPromises();
    expect(api.attachPluginTerminalSession).toHaveBeenCalledTimes(1);

    const closing = (wrapper.vm as unknown as { disconnectForClose(): Promise<void> }).disconnectForClose();
    wrapper.unmount();
    resolveAttach({ session, attachment, replay: [] });
    await closing;

    expect(api.detachPluginTerminalSession).toHaveBeenCalledTimes(1);
    expect(api.detachPluginTerminalSession).toHaveBeenCalledWith(expect.objectContaining({
      intent: "userClose", disconnectIfLast: true, attachmentId: "attachment",
    }));
  });
  it("releases an ordinary renderer unmount without requesting a user close", async () => {
    const wrapper = render(); await flushPromises();
    wrapper.unmount(); await flushPromises();
    expect(api.detachPluginTerminalSession).toHaveBeenCalledWith(expect.objectContaining({
      intent: "rendererUnavailable", disconnectIfLast: false,
    }));
  });
  it("reattaches incrementally after a lost attachment heartbeat", async () => {
    const wrapper = render(); await flushPromises(); onEvent(event("1", [65]));
    api.heartbeatPluginAttachment.mockRejectedValueOnce(new Error("reaped"));
    await (wrapper.vm as unknown as { reconcileAfterForeground(): Promise<void> }).reconcileAfterForeground();
    expect(api.attachPluginTerminalSession).toHaveBeenLastCalledWith(expect.objectContaining({ afterOutputSeq: "1" }), expect.any(Function));
    wrapper.unmount();
  });
  it("renders an explicit output gap once before the resumed frame", async () => {
    const wrapper = render(); await flushPromises();
    onEvent({ session, sessionId: "session", generation: "1", stateRevision: "2", eventSeq: "2",
      payload: { kind: "outputGap", gap: { sessionId: "session", generation: "1", streamId: "stream", droppedFromOutputSeq: "1", resumesAtOutputSeq: "8", reason: "ringBufferOverflow" } } });
    onEvent(event("8", [65]));
    expect(view.writeGap).toHaveBeenCalledTimes(1); expect(view.writeBytes).toHaveBeenCalledWith([65]);
    wrapper.unmount();
  });
  it("recovers channel overruns with a fenced replay from the last rendered output", async () => {
    const wrapper = render(); await flushPromises();
    onEvent(event("1", [65]));
    onEvent({ session, sessionId: "session", generation: "1", stateRevision: "2", eventSeq: "3",
      payload: { kind: "outputGap", gap: { sessionId: "session", generation: "1", streamId: "stream",
        droppedFromOutputSeq: "0", resumesAtOutputSeq: "0", reason: "eventOverrun" } } });
    await flushPromises();
    expect(api.attachPluginTerminalSession).toHaveBeenCalledTimes(2);
    expect(api.attachPluginTerminalSession).toHaveBeenLastCalledWith(expect.objectContaining({
      sessionId: "session", expectedGeneration: "1", streamId: "stream", afterOutputSeq: "1",
    }), expect.any(Function));
    expect(api.openPluginTerminalSession).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
  it("keeps failed cleanup actionable and retries the actual disconnect", async () => {
    const blocked = { ...session, state: "failed", cleanupBlocked: true };
    api.disconnectPluginTerminalSession.mockResolvedValue({ ...session, state: "closed", cleanupBlocked: false });
    const wrapper = render({ existingSession: blocked }); await flushPromises();
    expect(wrapper.text()).toContain("has not released its resources");
    expect(wrapper.findAll("button").some((button) => button.text().includes("Reconnect"))).toBe(false);
    await (wrapper.vm as unknown as { disconnectForClose(): Promise<void> }).disconnectForClose();
    expect(api.disconnectPluginTerminalSession).toHaveBeenCalledTimes(1); wrapper.unmount();
  });
  it("recovers a lost claim response from the authoritative session snapshot", async () => {
    api.claimPluginProtocolLaunch.mockRejectedValueOnce(new Error("response lost"));
    const wrapper = render({ launch, profile: null }); await flushPromises();
    expect(api.fetchPluginTerminalSessionSnapshot).toHaveBeenCalledTimes(1);
    expect(api.attachPluginTerminalSession).toHaveBeenCalledTimes(1);
    expect(api.openPluginTerminalSession).not.toHaveBeenCalled(); wrapper.unmount();
  });

});
