import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createI18n } from "vue-i18n";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { appThemeEn } from "../../locales/app-theme";
import { useAppThemeStore } from "../../stores/appTheme";
import NvxAppThemeSettings from "./NvxAppThemeSettings.vue";

vi.mock("../../core-api/app-theme", () => ({ listPluginThemes: vi.fn().mockResolvedValue({ themes: [] }) }));
vi.mock("../../platform-file-export", () => ({ exportJsonFile: vi.fn().mockResolvedValue(true) }));
async function setup() {
  const pinia = createPinia();
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/", component: { template: "<div />" } }, { path: "/plugins", component: { template: "<div />" } }] });
  await router.push("/"); await router.isReady();
  const wrapper = mount(NvxAppThemeSettings, { attachTo: document.body, global: { plugins: [pinia, router, createI18n({ legacy: false, locale: "en", messages: { en: { appTheme: appThemeEn } } })] } });
  await flushPromises();
  return { wrapper, store: useAppThemeStore(pinia) };
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
    vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    await wrapper.findAll("button").find((button) => button.text() === "Save changes")!.trigger("click");
    expect(store.exportProfile()).toBe(original);
    expect(wrapper.text()).toContain("Appearance could not be saved");
    wrapper.unmount();
  });
});
