import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { terminalInteractionEn } from "../../locales/terminal-interaction";
import { useTerminalPreferencesStore } from "../../stores/terminalPreferences";
import { useTipsStore } from "../../stores/tips";
import { NvxButton, NvxInput, NvxSelect } from "../ui";
import NvxTerminalInteractionSettings from "./NvxTerminalInteractionSettings.vue";

const coreApi = vi.hoisted(() => ({ listHostCatalog: vi.fn() }));
vi.mock("../../core-api/client", () => ({ listHostCatalog: coreApi.listHostCatalog }));

function setup() {
  const pinia = createPinia();
  const wrapper = mount(NvxTerminalInteractionSettings, {
    global: { plugins: [pinia, createI18n({ legacy: false, locale: "en", messages: { en: { terminalInteraction: terminalInteractionEn } } })] },
  });
  return { wrapper, store: useTerminalPreferencesStore(pinia), tips: useTipsStore(pinia) };
}

describe("NvxTerminalInteractionSettings", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
    coreApi.listHostCatalog.mockResolvedValue([]);
  });
  function button(wrapper: ReturnType<typeof setup>["wrapper"], label: string) {
    return wrapper.findAllComponents(NvxButton).find((item) => item.text() === label)!;
  }
  it("keeps a complete draft and persists it only after explicit save", async () => {
    const { wrapper, store } = setup();
    const inputs = wrapper.findAllComponents(NvxInput);
    inputs[0]!.vm.$emit("update:modelValue", "9000");
    inputs[1]!.vm.$emit("update:modelValue", "3");
    wrapper.findAllComponents(NvxSelect)[0]!.vm.$emit("update:modelValue", "100");
    wrapper.findAllComponents(NvxSelect)[1]!.vm.$emit("update:modelValue", "address");
    wrapper.findAllComponents(NvxSelect)[2]!.vm.$emit("update:modelValue", "true");
    wrapper.findAllComponents(NvxSelect)[3]!.vm.$emit("update:modelValue", "paste");
    await wrapper.vm.$nextTick();
    expect(store.preferences.interaction.scrollback).toBe(5_000);
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.interaction).toMatchObject({ scrollback: 9_000, scrollSensitivity: 3, smoothScrollDuration: 100, doubleClickSelection: "address", copyOnSelect: true, rightClickBehavior: "paste", optionAsMetaLeft: false, optionAsMetaRight: false, backspaceMode: "del", bellMode: "off", linksEnabled: true });
    wrapper.unmount();
  });
  it("shows an inline error and keeps the saved value when a numeric draft is outside its bounds", async () => {
    const { wrapper, store } = setup();
    wrapper.findAllComponents(NvxInput)[0]!.vm.$emit("update:modelValue", "999");
    await wrapper.vm.$nextTick();
    expect(store.preferences.interaction.scrollback).toBe(5_000);
    expect(wrapper.text()).toContain("Enter an integer from 1,000 to 100,000.");
    expect(button(wrapper, "Save changes").props("disabled")).toBe(true);
    wrapper.unmount();
  });
  it("keeps the unsaved draft visible when local storage fails", async () => {
    const { wrapper, store, tips } = setup();
    wrapper.findAllComponents(NvxInput)[0]!.vm.$emit("update:modelValue", "9000");
    await wrapper.vm.$nextTick();
    const storageSpy = vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.interaction.scrollback).toBe(5_000);
    expect(wrapper.findAllComponents(NvxInput)[0]!.props("modelValue")).toBe("9000");
    expect(tips.items[0]?.title).toBe("Terminal interaction settings were not saved. Previous settings are unchanged.");
    storageSpy.mockRestore();
    wrapper.unmount();
  });

  it("loads a minimal Host catalog and explicitly saves a local keyboard override", async () => {
    coreApi.listHostCatalog.mockResolvedValue([{
      host: { hostId: "host-a", label: "Acceptance host", address: "acceptance.example.test" },
    }]);
    const { wrapper, store } = setup();
    await flushPromises();
    const select = (label: string) => wrapper.findAllComponents(NvxSelect)
      .filter((item) => item.props("ariaLabel") === label)
      .at(-1)!;

    select("Apply to").vm.$emit("update:modelValue", "host-a");
    await wrapper.vm.$nextTick();
    select("Host input mode").vm.$emit("update:modelValue", "override");
    await wrapper.vm.$nextTick();
    select("Left Option key").vm.$emit("update:modelValue", "true");
    select("Backspace sends").vm.$emit("update:modelValue", "bs");
    await wrapper.vm.$nextTick();

    const saveButtons = wrapper.findAllComponents(NvxButton).filter((item) => item.text() === "Save changes");
    await saveButtons.at(-1)!.trigger("click");
    expect(store.preferences.hostKeyboard).toEqual({
      "host-a": {
        mode: "override",
        keyboard: { optionAsMetaLeft: true, optionAsMetaRight: false, backspaceMode: "bs" },
      },
    });
    wrapper.unmount();
  });

  it("keeps global settings usable and reports a catalog load failure", async () => {
    coreApi.listHostCatalog.mockRejectedValue(new Error("offline"));
    const { wrapper, store } = setup();
    await flushPromises();
    expect(wrapper.text()).toContain("Could not load the Host list. Global terminal interaction settings remain available.");
    wrapper.findAllComponents(NvxInput)[0]!.vm.$emit("update:modelValue", "9000");
    await wrapper.vm.$nextTick();
    await button(wrapper, "Save changes").trigger("click");
    expect(store.preferences.interaction.scrollback).toBe(9_000);
    wrapper.unmount();
  });
});
