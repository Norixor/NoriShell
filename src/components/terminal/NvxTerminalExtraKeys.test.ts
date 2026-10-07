import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import type { Ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

const inputTarget = vi.hoisted(() => ({
  canSend: null as unknown as Ref<boolean>,
  applicationMode: false,
  send: vi.fn(async () => "sent" as "sent" | "unavailable" | "failed"),
}));
vi.mock("../../terminal-input-target", async () => {
  const { ref } = await import("vue");
  inputTarget.canSend = ref(false);
  return {
    canSendKeysToFocusedTerminal: inputTarget.canSend,
    focusedTerminalUsesApplicationCursorKeys: () => inputTarget.applicationMode,
    sendKeysToFocusedTerminal: inputTarget.send,
  };
});

import { i18n } from "../../locales";
import { useTipsStore } from "../../stores/tips";
import { clearStickyModifiers } from "../../terminal/extra-keys";
import NvxTerminalExtraKeys from "./NvxTerminalExtraKeys.vue";

function mountKeys() {
  return mount(NvxTerminalExtraKeys, { global: { plugins: [createPinia(), i18n] } });
}

describe("NvxTerminalExtraKeys", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    inputTarget.canSend.value = false;
    inputTarget.applicationMode = false;
    inputTarget.send.mockReset().mockResolvedValue("sent");
    clearStickyModifiers();
    i18n.global.locale.value = "en";
  });

  it("disables every key until a writable Pane holds the focus", async () => {
    const wrapper = mountKeys();
    const buttons = wrapper.findAll("button");
    expect(buttons).toHaveLength(16);
    expect(buttons.every((button) => button.attributes("disabled") !== undefined)).toBe(true);
    await buttons[2]!.trigger("click");
    expect(inputTarget.send).not.toHaveBeenCalled();
    inputTarget.canSend.value = true;
    await flushPromises();
    expect(wrapper.findAll("button").every((button) => button.attributes("disabled") === undefined)).toBe(true);
  });

  it("sends encoded sequences through the focused terminal and keeps the xterm focus", async () => {
    inputTarget.canSend.value = true;
    const wrapper = mountKeys();
    const escape = wrapper.find('button[aria-label="Esc"]');
    const pointerDown = new Event("pointerdown", { cancelable: true, bubbles: true });
    escape.element.dispatchEvent(pointerDown);
    expect(pointerDown.defaultPrevented).toBe(true);
    await escape.trigger("click");
    expect(inputTarget.send).toHaveBeenLastCalledWith("\x1b");
    inputTarget.applicationMode = true;
    await wrapper.find('button[aria-label="Up arrow"]').trigger("click");
    expect(inputTarget.send).toHaveBeenLastCalledWith("\x1bOA");
    const ctrl = wrapper.find('button[aria-label="Ctrl: applies to the next key"]');
    await ctrl.trigger("click");
    expect(ctrl.attributes("aria-pressed")).toBe("true");
    await wrapper.find('button[aria-label="Left arrow"]').trigger("click");
    expect(inputTarget.send).toHaveBeenLastCalledWith("\x1b[1;5D");
    expect(ctrl.attributes("aria-pressed")).toBe("false");
    await wrapper.find('button[aria-label="Pipe"]').trigger("click");
    expect(inputTarget.send).toHaveBeenLastCalledWith("|");
  });

  it("reports a send that did not reach the terminal", async () => {
    inputTarget.canSend.value = true;
    inputTarget.send.mockResolvedValue("failed");
    const wrapper = mountKeys();
    await wrapper.find('button[aria-label="Tab"]').trigger("click");
    await flushPromises();
    const tips = useTipsStore();
    expect(tips.items.map((item) => item.title)).toEqual(["The key could not be sent. Check the terminal before retrying."]);
  });

  it("localizes the row in Chinese", () => {
    i18n.global.locale.value = "zh-CN";
    const wrapper = mountKeys();
    expect(wrapper.attributes("aria-label")).toBe("终端辅助按键");
    expect(wrapper.find('button[aria-label="上方向键"]').exists()).toBe(true);
  });
});
