import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { defineComponent, h } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  TelnetSessionAttachment,
  TelnetSessionSummary,
} from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import { resetTerminalInputFocusForTests } from "../../terminal-input-target";

const client = vi.hoisted(() => ({
  attachTelnetSession: vi.fn(),
  changeTerminalInputFocus: vi.fn(),
  detachTelnetSession: vi.fn(),
  disconnectTelnetSession: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
  heartbeatTelnetAttachment: vi.fn(),
  openTelnetSession: vi.fn(),
  reconnectTelnetSession: vi.fn(),
  renewTelnetInputLease: vi.fn(),
  resizeTelnetTerminal: vi.fn(),
  sendTelnetInput: vi.fn(),
}));

vi.mock("../../core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("../../core-api/client")>(),
  ...client,
}));

import NvxTelnetTerminalPane from "./NvxTelnetTerminalPane.vue";

const session: TelnetSessionSummary = {
  sessionId: "019d0000-0000-7000-8000-000000000401",
  openAttemptId: "019d0000-0000-7000-8000-000000000402",
  endpoint: { address: "legacy.example.test", port: 23 },
  generation: "1",
  stateRevision: "2",
  attachmentRevision: "1",
  eventSeq: "1",
  socketId: "019d0000-0000-7000-8000-000000000403",
  state: "running",
  attachmentCount: 1,
  closeReason: null,
  failureReason: null,
  createdAtUnixMs: 1,
  updatedAtUnixMs: 2,
};

const attachment: TelnetSessionAttachment = {
  attachmentId: "019d0000-0000-7000-8000-000000000404",
  attachAttemptId: "019d0000-0000-7000-8000-000000000405",
  sessionId: session.sessionId,
  generation: session.generation,
  socketId: session.socketId,
  viewId: "019d0000-0000-7000-8000-000000000406",
  stateRevision: session.stateRevision,
  attachmentRevision: session.attachmentRevision,
  attachedAtUnixMs: 1,
};

const TerminalViewStub = defineComponent({
  name: "NvxTerminalView",
  emits: ["input", "resize", "selectionChange", "searchRequest"],
  setup(_, { expose }) {
    expose({
      dimensions: () => ({ rows: 24, cols: 80 }),
      focus: vi.fn(),
      fit: vi.fn(),
      writeBytes: vi.fn(),
      writeGap: vi.fn(),
    });
    return () => h("div", { class: "terminal-view-stub" });
  },
});

describe("NvxTelnetTerminalPane", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetTerminalInputFocusForTests();
    i18n.global.locale.value = "zh-CN";
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({
      focusEpoch: "0",
      target: null,
      lease: null,
    });
    client.openTelnetSession.mockResolvedValue({ session, attachment });
    client.heartbeatTelnetAttachment.mockResolvedValue(attachment);
    client.detachTelnetSession.mockResolvedValue({
      session,
      remainingAttachmentCount: 0,
    });
    client.changeTerminalInputFocus.mockResolvedValue({
      focusEpoch: "1",
      target: {
        kind: "telnet",
        target: {
          sessionId: session.sessionId,
          expectedGeneration: session.generation,
          expectedStateRevision: session.stateRevision,
          socketId: session.socketId,
          attachmentId: attachment.attachmentId,
          viewId: attachment.viewId,
        },
      },
      lease: null,
    });
  });

  it("opens only with the exact three-risk acknowledgement and no SSH credential fields", async () => {
    const wrapper = mount(NvxTelnetTerminalPane, {
      props: {
        paneId: attachment.viewId,
        label: "legacy.example.test:23",
        endpoint: session.endpoint,
        existingSession: null,
        active: true,
        canSplitHorizontal: true,
        canSplitVertical: true,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: { NvxTerminalView: TerminalViewStub },
      },
    });
    await flushPromises();

    expect(client.openTelnetSession).toHaveBeenCalledTimes(1);
    const request = client.openTelnetSession.mock.calls[0]?.[0];
    expect(request.endpoint).toEqual(session.endpoint);
    expect(request.riskConfirmation).toEqual({
      endpoint: session.endpoint,
      acceptsCleartextTransport: true,
      acceptsMissingServerIdentity: true,
      acceptsObservationAndTampering: true,
    });
    expect(request).not.toHaveProperty("credentialRefId");
    expect(wrapper.text()).toContain("Telnet 明文连接");
    wrapper.unmount();
    await flushPromises();
  });

  it("keeps restored history offline until all three risks are accepted again", async () => {
    const wrapper = mount(NvxTelnetTerminalPane, {
      props: {
        paneId: attachment.viewId,
        label: "legacy.example.test:23",
        endpoint: session.endpoint,
        existingSession: null,
        deferredStart: true,
        active: true,
        canSplitHorizontal: true,
        canSplitVertical: true,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: { NvxTerminalView: TerminalViewStub },
      },
    });
    await flushPromises();

    expect(client.openTelnetSession).not.toHaveBeenCalled();
    const reconnectButton = wrapper.findAll("button")
      .find((button) => button.text().includes(i18n.global.t("telnetSession.reconnect")));
    expect(reconnectButton).toBeDefined();
    await reconnectButton?.trigger("click");
    await flushPromises();

    expect(client.openTelnetSession).not.toHaveBeenCalled();
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptCleartext"));
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptMissingIdentity"));
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptTampering"));
    wrapper.unmount();
  });

  it("keeps all three risk labels visible when reconnecting", async () => {
    const wrapper = mount(NvxTelnetTerminalPane, {
      props: {
        paneId: attachment.viewId,
        label: "legacy.example.test:23",
        endpoint: session.endpoint,
        existingSession: {
          ...session,
          socketId: null,
          state: "closed",
          closeReason: "userRequested",
        },
        active: true,
        canSplitHorizontal: true,
        canSplitVertical: true,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: { NvxTerminalView: TerminalViewStub },
      },
    });
    await flushPromises();
    await wrapper.findAll("button")
      .find((button) => button.text().includes(i18n.global.t("telnetSession.reconnect")))
      ?.trigger("click");
    await flushPromises();

    const dialog = document.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptCleartext"));
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptMissingIdentity"));
    expect(dialog?.textContent).toContain(i18n.global.t("telnetSession.acceptTampering"));
    expect(dialog?.querySelectorAll(".nvx-checkbox__label")).toHaveLength(3);
    wrapper.unmount();
  });

  it("delegates an explicit close through the Telnet disconnect fence", async () => {
    const closed = { ...session, state: "closed" as const, socketId: null };
    client.disconnectTelnetSession.mockResolvedValue(closed);
    const wrapper = mount(NvxTelnetTerminalPane, {
      props: {
        paneId: attachment.viewId,
        label: "legacy.example.test:23",
        endpoint: session.endpoint,
        existingSession: null,
        active: true,
        canSplitHorizontal: true,
        canSplitVertical: true,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: { NvxTerminalView: TerminalViewStub },
      },
    });
    await flushPromises();
    await (wrapper.vm as unknown as { disconnectForClose(): Promise<void> }).disconnectForClose();

    expect(client.disconnectTelnetSession).toHaveBeenCalledWith({
      sessionId: session.sessionId,
      expectedGeneration: session.generation,
      expectedStateRevision: session.stateRevision,
    });
    wrapper.unmount();
    await flushPromises();
  });

  it("uses the stable localized runtime failure key", async () => {
    const wrapper = mount(NvxTelnetTerminalPane, {
      props: {
        paneId: attachment.viewId,
        label: "legacy.example.test:23",
        endpoint: session.endpoint,
        existingSession: {
          ...session,
          state: "failed",
          socketId: null,
          failureReason: {
            code: "connectTimeout",
            messageKey: "telnetSession.failures.connectTimeout",
            diagnosticId: null,
          },
        },
        active: false,
        canSplitHorizontal: true,
        canSplitVertical: true,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: { NvxTerminalView: TerminalViewStub },
      },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("Telnet 直连超时");
    wrapper.unmount();
  });

  it("unregisters the input target when the Pane becomes inactive", async () => {
    const wrapper = mount(NvxTelnetTerminalPane, {
      props: {
        paneId: attachment.viewId,
        label: "legacy.example.test:23",
        endpoint: session.endpoint,
        existingSession: null,
        active: true,
        canSplitHorizontal: true,
        canSplitVertical: true,
      },
      global: {
        plugins: [createPinia(), i18n],
        stubs: { NvxTerminalView: TerminalViewStub },
      },
    });
    await flushPromises();
    client.changeTerminalInputFocus.mockClear();

    await wrapper.setProps({ active: false });
    await flushPromises();

    expect(client.changeTerminalInputFocus).toHaveBeenCalledWith({
      expectedFocusEpoch: "1",
      target: null,
    });
    client.changeTerminalInputFocus.mockClear();
    (wrapper.vm as unknown as { activateFromTab(): void }).activateFromTab();
    await flushPromises();
    expect(client.changeTerminalInputFocus).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
