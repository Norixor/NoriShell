import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { appThemeEn } from "../../locales/app-theme";
import { useAppThemeStore } from "../../stores/appTheme";
import { useUiStore } from "../../stores/ui";
import { listPluginThemes } from "../../core-api/app-theme";
import clear from "../../../examples/theme-plugins/clear/assets/theme.json";
import midnight from "../../../examples/theme-plugins/midnight/assets/theme.json";
import sand from "../../../examples/theme-plugins/sand/assets/theme.json";
import NvxAppThemeSettings from "./NvxAppThemeSettings.vue";

vi.mock("../../core-api/app-theme", () => ({ listPluginThemes: vi.fn().mockResolvedValue({ themes: [] }) }));
vi.mock("../../platform-file-export", () => ({ exportJsonFile: vi.fn().mockResolvedValue(true) }));
async function setup() {
  const pinia = createPinia();
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/", component: { template: "<div />" } }, { path: "/plugins", component: { template: "<div />" } }] });
  await router.push("/"); await router.isReady();
  const wrapper = mount(NvxAppThemeSettings, { attachTo: document.body, global: { plugins: [pinia, router, createI18n({ legacy: false, locale: "en", messages: { en: { appTheme: appThemeEn } } })] } });
  await flushPromises();
  return { wrapper, store: useAppThemeStore(pinia), ui: useUiStore(pinia) };
}
beforeEach(() => { localStorage.clear(); document.documentElement.removeAttribute("style"); });
afterEach(() => { document.body.innerHTML = ""; vi.restoreAllMocks(); });
describe("application appearance editor", () => {
  it("keeps edited colors in the draft until save and cancels without mutating the profile", async () => {
    const { wrapper, store } = await setup();
    const original = store.exportProfile();
    await wrapper.get("#theme-color-accent").setValue("#123456");
    expect(store.exportProfile()).toBe(original);
    await wrapper.findAll("button").find((button) => button.text() === "Discard changes")!.trigger("click");
    expect(store.exportProfile()).toBe(original);
    expect(wrapper.get<HTMLInputElement>("#theme-color-accent").element.value).toBe("#1f5fd2");
    await wrapper.get("#theme-color-accent").setValue("#123456");
    await wrapper.findAll("button").find((button) => button.text() === "Save changes")!.trigger("click");
    expect(store.profile.overrides["builtin:light"]?.colors?.accent).toBe("#123456");
    expect(wrapper.text()).toContain("Appearance saved.");
    wrapper.unmount();
  });
  it("blocks unreadable colors and retains the saved profile after a persistence failure", async () => {
    const { wrapper, store } = await setup();
    const original = store.exportProfile();
    await wrapper.get("#theme-color-textPrimary").setValue("#ffffff");
    expect(wrapper.findAll("button").find((button) => button.text() === "Save changes")!.attributes("disabled")).toBeDefined();
    expect(store.exportProfile()).toBe(original);
    await wrapper.findAll("button").find((button) => button.text() === "Discard changes")!.trigger("click");
    await wrapper.get("#theme-color-accent").setValue("#123456");
    vi.spyOn(localStorage, "setItem").mockImplementationOnce(() => { throw new Error("quota"); });
    await wrapper.findAll("button").find((button) => button.text() === "Save changes")!.trigger("click");
    expect(store.exportProfile()).toBe(original);
    expect(wrapper.text()).toContain("Appearance could not be saved");
    wrapper.unmount();
  });
  it("shows all three plugin themes together and applies the selected mode only on save", async () => {
    vi.mocked(listPluginThemes).mockResolvedValueOnce({ themes: [clear, midnight, sand].map((definition) => ({
      pluginId: `com.norishell.theme.${definition.id}`, packageHash: "a".repeat(64), version: "1.0.0", enabled: true, definition,
    })) } as Awaited<ReturnType<typeof listPluginThemes>>);
    const { wrapper, store, ui } = await setup();
    const cards = wrapper.findAll(".theme-choice");
    expect(cards).toHaveLength(5);
    expect(cards.slice(0, 3).map((card) => card.text())).toEqual([
      expect.stringContaining("Moss"), expect.stringContaining("Mulberry"), expect.stringContaining("Sand"),
    ]);
    const dark = cards.find((card) => card.text().includes("Mulberry"))!;
    await dark.trigger("click");
    expect(ui.themePreference).toBe("light");
    expect(store.profile.darkThemeId).toBe("builtin:dark");
    await wrapper.findAll("button").find((button) => button.text() === "Discard changes")!.trigger("click");
    expect(ui.themePreference).toBe("light");
    expect(dark.attributes("aria-pressed")).toBe("false");
    await dark.trigger("click");
    const persistMode = vi.spyOn(ui, "setThemePreference").mockReturnValueOnce(false);
    const save = wrapper.findAll("button").find((button) => button.text() === "Save changes")!;
    await save.trigger("click");
    expect(wrapper.text()).toContain("appearance mode could not be saved");
    expect(save.attributes("disabled")).toBeUndefined();
    expect(ui.themePreference).toBe("light");
    await save.trigger("click");
    expect(persistMode).toHaveBeenLastCalledWith("dark");
    expect(ui.themePreference).toBe("dark");
    expect(store.profile.darkThemeId).toContain("midnight");
    await cards.find((card) => card.text().includes("NoriShell Light"))!.trigger("click");
    expect(save.attributes("disabled")).toBeUndefined();
    await save.trigger("click");
    expect(ui.themePreference).toBe("light");
    expect(wrapper.findAll(".theme-choice")).toHaveLength(5);
    wrapper.unmount();
  });

});
