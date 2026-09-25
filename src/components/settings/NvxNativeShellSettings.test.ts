import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { nativeShellSettingsEn } from "../../locales/native-shell-settings";

const api = vi.hoisted(() => ({
  clearNativeTerminalHistory: vi.fn(),
  getNativeTerminalSettings: vi.fn(),
  pauseNativeTerminalHistory: vi.fn(),
  replaceNativeTerminalSettings: vi.fn(),
}));

vi.mock("../../core-api/native-terminal", () => api);

import NvxNativeShellSettings from "./NvxNativeShellSettings.vue";

const snapshot = {
  settings: {
    historyEnabled: true,
    persistEncrypted: true,
    historyMaxEntries: 500,
    historyRetentionDays: 30,
    historyPaused: false,
    notificationsEnabled: false,
    notificationThresholdSeconds: 60,
  },
  settingsRevision: "7",
  historyAvailable: false,
  historyPersistenceFailed: false,
};

function mountSettings() {
  return mount(NvxNativeShellSettings, {
    attachTo: document.body,
    global: {
      plugins: [
        createPinia(),
        createI18n({ legacy: false, locale: "en", messages: { en: { nativeShellSettings: nativeShellSettingsEn } } }),
      ],
      stubs: { Teleport: true },
    },
  });
}

describe("NvxNativeShellSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getNativeTerminalSettings.mockResolvedValue(structuredClone(snapshot));
    api.pauseNativeTerminalHistory.mockResolvedValue(structuredClone(snapshot));
    api.clearNativeTerminalHistory.mockResolvedValue(undefined);
  });

  it("hydrates saved values on first load without marking the form dirty", async () => {
    api.getNativeTerminalSettings.mockResolvedValue({
      ...snapshot,
      settings: {
        ...snapshot.settings,
        notificationsEnabled: true,
        notificationThresholdSeconds: 120,
        historyMaxEntries: 800,
        historyRetentionDays: 14,
      },
    });
    const wrapper = mountSettings();
    await flushPromises();

    expect(wrapper.findAll('input[type="number"]').map(input => (input.element as HTMLInputElement).value))
      .toEqual(["800", "14"]);
    expect(wrapper.findAll('input[type="checkbox"]').map(input => (input.element as HTMLInputElement).checked))
      .toEqual([true, false, true]);
    const save = wrapper.findAll("button").find(button => button.text().includes("Save settings"));
    expect(save!.attributes("disabled")).toBeDefined();
    wrapper.unmount();
  });

  it("loads Core facts, preserves edits made during a save, and emits saved only after success", async () => {
    let resolveSave!: (value: typeof snapshot) => void;
    api.replaceNativeTerminalSettings.mockImplementation(() => new Promise((resolve) => { resolveSave = resolve; }));
    const wrapper = mountSettings();
    await flushPromises();

    expect(wrapper.text()).toContain("The Vault is missing, locked, or needs reload");
    const numericInputs = wrapper.findAll('input[type="number"]');
    await numericInputs[0]!.setValue("120");
    const save = wrapper.findAll("button").find((button) => button.text().includes("Save settings"));
    await save!.trigger("click");
    expect(api.replaceNativeTerminalSettings).toHaveBeenCalledWith(expect.objectContaining({
      historyMaxEntries: 120,
    }), "7");

    await numericInputs[0]!.setValue("180");
    resolveSave({
      ...snapshot,
      settings: { ...snapshot.settings, historyMaxEntries: 120 },
      settingsRevision: "8",
    });
    await flushPromises();

    expect((numericInputs[0]!.element as HTMLInputElement).value).toBe("180");
    expect(wrapper.emitted("saved")).toHaveLength(1);
    wrapper.unmount();
  });

  it("shows a retryable load error and only renders editable controls after a snapshot arrives", async () => {
    api.getNativeTerminalSettings.mockRejectedValueOnce(new Error("offline"));
    const wrapper = mountSettings();
    await flushPromises();

    expect(wrapper.text()).toContain("Native Shell settings could not be loaded.");
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(false);
    const retry = wrapper.findAll("button").find((button) => button.text().includes("Retry"));
    await retry!.trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Command history");
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("only clears after an explicit confirmation and keeps the dialog open on failure", async () => {
    api.getNativeTerminalSettings.mockResolvedValue({ ...snapshot, historyAvailable: true });
    const wrapper = mountSettings();
    await flushPromises();

    const clear = wrapper.findAll("button").find((button) => button.text().includes("Clear all history"));
    await clear!.trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Clear all command history?");
    expect(api.clearNativeTerminalHistory).not.toHaveBeenCalled();

    api.clearNativeTerminalHistory.mockRejectedValueOnce(new Error("unavailable"));
    const confirm = () => wrapper.findAll("button").find((button) => (
      button.text().includes("Clear all") && !button.text().includes("history")
    ));
    await confirm()!.trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Clear all command history?");

    await confirm()!.trigger("click");
    await flushPromises();
    expect(api.clearNativeTerminalHistory).toHaveBeenCalledWith(null);
    expect(document.body.textContent).not.toContain("Clear all command history?");
    wrapper.unmount();
  });

  it("keeps Resume now available while paused history remains readable", async () => {
    const paused = {
      ...snapshot,
      settings: { ...snapshot.settings, persistEncrypted: false, historyPaused: true },
      historyAvailable: true,
    };
    api.getNativeTerminalSettings.mockResolvedValue(paused);
    api.pauseNativeTerminalHistory.mockResolvedValue({
      ...paused,
      settings: { ...paused.settings, historyPaused: false },
      settingsRevision: "8",
    });
    const wrapper = mountSettings();
    await flushPromises();

    const resume = wrapper.findAll("button").find((button) => button.text().includes("Resume now"));
    expect(resume?.attributes("disabled")).toBeUndefined();
    await resume!.trigger("click");
    await flushPromises();
    expect(api.pauseNativeTerminalHistory).toHaveBeenCalledWith(false);
    wrapper.unmount();
  });
});
