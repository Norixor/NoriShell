import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  changeTerminalInputFocus: vi.fn(),
  fetchTerminalInputFocusSnapshot: vi.fn(),
}));
const clipboard = vi.hoisted(() => ({ readText: vi.fn() }));

vi.mock("../../core-api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../core-api/client")>();
  return { ...actual, ...client };
});
vi.mock("../../platform-clipboard", () => ({ readText: clipboard.readText }));

import { i18n } from "../../locales";
import {
  focusTerminalInputTarget,
  registerTerminalInputTarget,
  resetTerminalInputFocusForTests,
} from "../../terminal-input-target";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import NvxTerminalPasteGuard from "./NvxTerminalPasteGuard.vue";

function target(id: string) {
  return {
    kind: "ssh" as const,
    target: {
      sessionId: `019d0000-0000-7000-8000-000000000${id}1`,
      expectedGeneration: "1",
      expectedStateRevision: "1",
      channelId: `019d0000-0000-7000-8000-000000000${id}2`,
      attachmentId: `019d0000-0000-7000-8000-000000000${id}3`,
      viewId: `019d0000-0000-7000-8000-000000000${id}4`,
    },
  };
}

function dialogButton(label: string) {
  return Array.from(document.querySelectorAll<HTMLButtonElement>(".nvx-dialog__actions button"))
    .find((button) => button.textContent?.includes(label));
}

describe("NvxTerminalPasteGuard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clipboard.readText.mockReset();
    localStorage.clear();
    resetTerminalInputFocusForTests();
    i18n.global.locale.value = "en";
    client.fetchTerminalInputFocusSnapshot.mockResolvedValue({ focusEpoch: "0", target: null, lease: null });
    client.changeTerminalInputFocus.mockImplementation(async ({ expectedFocusEpoch, target: next }) => ({
      focusEpoch: (BigInt(expectedFocusEpoch) + 1n).toString(), target: next, lease: null,
    }));
  });

  afterEach(() => document.querySelectorAll(".nvx-dialog__backdrop").forEach((element) => element.remove()));

  it("cancels a reviewed paste without sending any bytes", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    useTerminalPreferencesStore(pinia).setPasteWarning("always");
    const pane = target("41");
    const send = vi.fn().mockResolvedValue(undefined);
    registerTerminalInputTarget({
      id: "pane-a", label: () => "Pane A", focusTarget: () => pane,
      applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send,
    });
    await focusTerminalInputTarget("pane-a");
    const wrapper = mount(NvxTerminalPasteGuard, {
      attachTo: document.body,
      props: { paneId: "pane-a", bracketed: () => false },
      global: { plugins: [pinia, i18n] },
    });

    await (wrapper.vm as unknown as { requestPaste(text: string): Promise<void> }).requestPaste("echo one");
    expect(dialogButton(i18n.global.t("terminalEnhancements.paste.cancel"))).toBeTruthy();
    await dialogButton(i18n.global.t("terminalEnhancements.paste.cancel"))?.click();
    await flushPromises();

    expect(send).not.toHaveBeenCalled();
    expect(wrapper.emitted("pasted")).toBeUndefined();
    wrapper.unmount();
  });

  it("never redirects a confirmed paste after focus or generation changes", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    useTerminalPreferencesStore(pinia).setPasteWarning("always");
    const first = target("51");
    const second = target("61");
    const firstSend = vi.fn().mockResolvedValue(undefined);
    const secondSend = vi.fn().mockResolvedValue(undefined);
    registerTerminalInputTarget({ id: "pane-a", label: () => "Pane A", focusTarget: () => first, applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send: firstSend });
    registerTerminalInputTarget({ id: "pane-b", label: () => "Pane B", focusTarget: () => second, applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send: secondSend });
    await focusTerminalInputTarget("pane-a");
    const wrapper = mount(NvxTerminalPasteGuard, {
      attachTo: document.body,
      props: { paneId: "pane-a", bracketed: () => true },
      global: { plugins: [pinia, i18n] },
    });

    await (wrapper.vm as unknown as { requestPaste(text: string): Promise<void> }).requestPaste("echo one\necho two");
    await focusTerminalInputTarget("pane-b");
    await focusTerminalInputTarget("pane-a");
    await dialogButton(i18n.global.t("terminalEnhancements.paste.confirm"))?.click();
    await flushPromises();
    expect(firstSend).not.toHaveBeenCalled();
    expect(secondSend).not.toHaveBeenCalled();

    await (wrapper.vm as unknown as { requestPaste(text: string): Promise<void> }).requestPaste("echo one\necho two");
    first.target.expectedGeneration = "2";
    await dialogButton(i18n.global.t("terminalEnhancements.paste.confirm"))?.click();
    await flushPromises();
    expect(firstSend).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("uses bracketed-paste framing without adding Enter", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    useTerminalPreferencesStore(pinia).setPasteWarning("always");
    const pane = target("71");
    const send = vi.fn().mockResolvedValue(undefined);
    registerTerminalInputTarget({ id: "pane-a", label: () => "Pane A", focusTarget: () => pane, applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send });
    await focusTerminalInputTarget("pane-a");
    const wrapper = mount(NvxTerminalPasteGuard, {
      attachTo: document.body,
      props: { paneId: "pane-a", bracketed: () => true },
      global: { plugins: [pinia, i18n] },
    });

    await (wrapper.vm as unknown as { requestPaste(text: string): Promise<void> }).requestPaste("echo one\necho two");
    await dialogButton(i18n.global.t("terminalEnhancements.paste.confirm"))?.click();
    await flushPromises();
    expect(send).toHaveBeenCalledWith("\u001b[200~echo one\recho two\u001b[201~");
    expect(send.mock.calls[0]?.[0]).not.toContain("\r\r");
    wrapper.unmount();
  });

  it("releases the captured input ticket when clipboard access fails", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const pane = target("81");
    const send = vi.fn().mockResolvedValue(undefined);
    registerTerminalInputTarget({ id: "pane-a", label: () => "Pane A", focusTarget: () => pane, applyFocusLease: vi.fn(), canAcceptInput: () => true, canAcceptRawInput: () => true, send });
    await focusTerminalInputTarget("pane-a");
    clipboard.readText.mockRejectedValueOnce(new Error("clipboard denied"));
    const wrapper = mount(NvxTerminalPasteGuard, {
      attachTo: document.body,
      props: { paneId: "pane-a", bracketed: () => false },
      global: { plugins: [pinia, i18n] },
    });

    await (wrapper.vm as unknown as { pasteFromClipboard(): Promise<void> }).pasteFromClipboard();
    expect(send).not.toHaveBeenCalled();
    expect(wrapper.find(".nvx-dialog__backdrop").exists()).toBe(false);

    await (wrapper.vm as unknown as { requestPaste(text: string): Promise<void> }).requestPaste("echo next");
    expect(send).toHaveBeenCalledWith("echo next");
    wrapper.unmount();
  });
});
