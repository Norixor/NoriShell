import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../locales";
import NvxStandaloneHeader from "./NvxStandaloneHeader.vue";

const native = vi.hoisted(() => ({ platform: "windows", invoke: vi.fn(), requestClose: vi.fn() }));
vi.mock("../../platform", () => ({ detectDesktopPlatform: () => native.platform }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: native.invoke }));
vi.mock("../../secure-transport", () => ({ requestSecureWindowClose: native.requestClose }));
vi.mock("./NvxWindowFrame.vue", () => ({ default: { template: "<div><slot /></div>" } }));

function header() {
  return mount(NvxStandaloneHeader, { slots: { default: "Editor" }, global: { plugins: [i18n] } });
}

describe("standalone header native gestures", () => {
  beforeEach(() => {
    native.platform = "windows";
    native.invoke.mockReset();
    native.invoke.mockResolvedValue(undefined);
    native.requestClose.mockReset();
  });

  it("keeps the desktop brand chrome and has no back button on desktop", () => {
    const wrapper = header();
    expect(wrapper.find(".nvx-app-header__brand").exists()).toBe(true);
    expect(wrapper.find(".nvx-standalone-header__back").exists()).toBe(false);
    wrapper.unmount();
  });

  it("renders a compact phone app bar whose back button runs the back-key close path on Android", async () => {
    native.platform = "android";
    const wrapper = header();
    expect(wrapper.find(".nvx-app-header").exists()).toBe(false);
    expect(wrapper.classes()).toContain("nvx-standalone-header--mobile");
    expect(wrapper.get(".nvx-standalone-header__title").text()).toBe("Editor");
    await wrapper.get(".nvx-standalone-header__back").trigger("click");
    expect(native.requestClose).toHaveBeenCalledOnce();
    expect(native.invoke).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("drags and maximizes only the calling Windows window", async () => {
    const wrapper = header();
    await wrapper.get(".nvx-standalone-header__title").trigger("mousedown", { button: 0, detail: 1 });
    expect(native.invoke).toHaveBeenLastCalledWith("window_standalone_action", { action: "drag" });
    await wrapper.get(".nvx-standalone-header__title").trigger("mousedown", { button: 0, detail: 2 });
    expect(native.invoke).toHaveBeenLastCalledWith("window_standalone_action", { action: "maximize" });
    native.invoke.mockClear();
    await wrapper.get(".nvx-window-controls button").trigger("mousedown", { button: 0, detail: 1 });
    expect(native.invoke).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("allows macOS to cancel double-click maximization by moving before mouseup", async () => {
    native.platform = "macos";
    const wrapper = header();
    const title = wrapper.get(".nvx-standalone-header__title");
    await title.trigger("mousedown", { button: 0, detail: 2, clientX: 80, clientY: 20 });
    await title.trigger("mouseup", { button: 0, detail: 2, clientX: 81, clientY: 20 });
    expect(native.invoke).not.toHaveBeenCalled();
    await title.trigger("mousedown", { button: 0, detail: 2, clientX: 80, clientY: 20 });
    await title.trigger("mouseup", { button: 0, detail: 2, clientX: 80, clientY: 20 });
    expect(native.invoke).toHaveBeenCalledWith("window_standalone_action", { action: "maximize" });
    wrapper.unmount();
  });
});
