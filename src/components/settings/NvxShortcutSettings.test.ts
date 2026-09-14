import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { createPinia, setActivePinia } from "pinia";

import { shortcutMessages } from "../../locales/shortcuts";
import { useShortcutsStore } from "../../stores/shortcuts";
import { useTipsStore } from "../../stores/tips";

const exporter = vi.hoisted(() => ({ exportJsonFile: vi.fn() }));
vi.mock("../../platform-file-export", () => exporter);
import NvxShortcutSettings from "./NvxShortcutSettings.vue";

describe("NvxShortcutSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("filters command rows and records a binding without overwriting another action", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const i18n = createI18n({
      legacy: false,
      locale: "en",
      messages: shortcutMessages,
    });
    const wrapper = mount(NvxShortcutSettings, {
      attachTo: document.body,
      global: {
        plugins: [pinia, i18n],
        stubs: { Teleport: true },
      },
    });

    expect(wrapper.text()).toContain("Keyboard Shortcuts");
    const search = wrapper.find("input[aria-label='Search actions']");
    await search.setValue("quick commands");
    expect(wrapper.text()).toContain("Toggle Quick Commands");
    expect(wrapper.text()).not.toContain("Open server overview");

    await search.setValue("");
    const record = wrapper.findAll("button").find((button) => button.text().includes("Record"));
    expect(record).toBeTruthy();
    await record!.trigger("click");
    const recorder = wrapper.find(".nvx-shortcut-settings__recorder");
    await recorder.trigger("keydown", { key: "G", code: "KeyG", metaKey: true });
    const save = wrapper.findAll("button").find((button) => button.text().includes("Save shortcut"));
    expect(save).toBeTruthy();
    await save!.trigger("click");

    expect(useShortcutsStore().macosBindings["navigation.overview"]).toBe("Meta+KeyG");
    expect(useShortcutsStore().macosBindings["navigation.terminal"]).toBe("Meta+Shift+KeyT");
    wrapper.unmount();
  });

  it("does not report an export when the native save panel is cancelled and reports write failures", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const i18n = createI18n({ legacy: false, locale: "en", messages: shortcutMessages });
    const wrapper = mount(NvxShortcutSettings, { global: { plugins: [pinia, i18n], stubs: { Teleport: true } } });
    const exportButton = wrapper.findAll("button").find((button) => button.text().includes("Export"));
    if (!exportButton) throw new Error("Missing export button");

    exporter.exportJsonFile.mockResolvedValueOnce(false);
    await exportButton.trigger("click");
    await flushPromises();
    expect(exporter.exportJsonFile).toHaveBeenCalledWith("shortcuts", expect.any(String));
    expect(useTipsStore(pinia).items).toHaveLength(0);

    exporter.exportJsonFile.mockRejectedValueOnce(new Error("write failed"));
    await exportButton.trigger("click");
    await flushPromises();
    expect(useTipsStore(pinia).items[0]?.title).toBe("The shortcut configuration could not be exported.");
    wrapper.unmount();
  });

  it("prevents a second export while the native save panel is still open", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const i18n = createI18n({ legacy: false, locale: "en", messages: shortcutMessages });
    const wrapper = mount(NvxShortcutSettings, { global: { plugins: [pinia, i18n], stubs: { Teleport: true } } });
    const exportButton = wrapper.findAll("button").find((button) => button.text().includes("Export"));
    if (!exportButton) throw new Error("Missing export button");
    let resolveExport: ((value: boolean) => void) | undefined;
    exporter.exportJsonFile.mockImplementationOnce(() => new Promise<boolean>((resolve) => { resolveExport = resolve; }));

    await exportButton.trigger("click");
    await exportButton.trigger("click");
    expect(exporter.exportJsonFile).toHaveBeenCalledOnce();
    expect(exportButton.attributes("disabled")).toBeDefined();

    resolveExport?.(false);
    await flushPromises();
    expect(exportButton.attributes("disabled")).toBeUndefined();
    wrapper.unmount();
  });

  it("does not publish a result after the settings view is unmounted", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const i18n = createI18n({ legacy: false, locale: "en", messages: shortcutMessages });
    const wrapper = mount(NvxShortcutSettings, { global: { plugins: [pinia, i18n], stubs: { Teleport: true } } });
    const exportButton = wrapper.findAll("button").find((button) => button.text().includes("Export"));
    if (!exportButton) throw new Error("Missing export button");
    let resolveExport: ((value: boolean) => void) | undefined;
    exporter.exportJsonFile.mockImplementationOnce(() => new Promise<boolean>((resolve) => { resolveExport = resolve; }));

    await exportButton.trigger("click");
    wrapper.unmount();
    resolveExport?.(true);
    await flushPromises();
    expect(useTipsStore(pinia).items).toHaveLength(0);
  });
});
