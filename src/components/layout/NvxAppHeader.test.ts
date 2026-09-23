import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import NvxAppHeader from "./NvxAppHeader.vue";

const windowActionMocks = vi.hoisted(() => ({
  invoke: vi.fn<() => Promise<void>>(),
  isResizable: vi.fn<() => Promise<boolean>>(),
  performWindowAction: vi.fn<() => Promise<void>>(),
  physicalWindowsCaptionHitRegion: vi.fn(() => null),
  setWindowsMaximizeHitRegion: vi.fn<() => Promise<void>>(),
}));

vi.mock("../../platform-window", () => windowActionMocks);
vi.mock("@tauri-apps/api/core", () => ({ invoke: windowActionMocks.invoke }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ isResizable: windowActionMocks.isResizable }) }));

function mountHeader(platform: "macos" | "windows", standalone = false) {
  return mount(NvxAppHeader, {
    props: { platform, standalone },
    global: {
      plugins: [createPinia(), i18n],
      stubs: {
        RouterLink: {
          template: "<a><slot /></a>",
        },
      },
    },
  });
}

describe("NvxAppHeader platform frame", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
    windowActionMocks.performWindowAction.mockReset();
    windowActionMocks.performWindowAction.mockResolvedValue();
    windowActionMocks.invoke.mockReset();
    windowActionMocks.invoke.mockResolvedValue();
    windowActionMocks.isResizable.mockReset();
    windowActionMocks.isResizable.mockResolvedValue(true);
    windowActionMocks.physicalWindowsCaptionHitRegion.mockClear();
    windowActionMocks.setWindowsMaximizeHitRegion.mockReset();
    windowActionMocks.setWindowsMaximizeHitRegion.mockResolvedValue();
  });
  afterEach(() => { vi.unstubAllGlobals(); });

  it("reserves a passive native-control inset on macOS without rendering caption buttons", () => {
    const wrapper = mountHeader("macos");

    expect(wrapper.find(".nvx-app-header__native-controls-inset").exists()).toBe(true);
    expect(wrapper.find(".nvx-window-controls").exists()).toBe(false);
    const brandMark = wrapper.get<HTMLImageElement>(".nvx-app-header__brand-mark");
    expect(brandMark.attributes("src")).toContain("norishell-mark.svg");
    expect(brandMark.attributes("width")).toBe("28");
    expect(brandMark.attributes("height")).toBe("28");
    expect(brandMark.attributes("draggable")).toBe("false");
  });

  it("uses Tauri deep drag regions for nested passive header surfaces", () => {
    const wrapper = mountHeader("macos");

    expect(wrapper.get(".nvx-app-header").attributes("data-tauri-drag-region")).toBe("deep");
    expect(wrapper.get(".nvx-app-header__brand").attributes("data-tauri-drag-region")).toBe(
      "deep",
    );
    expect(wrapper.get("#nvx-terminal-tabs-host").attributes("data-tauri-drag-region")).toBe(
      "deep",
    );
  });

  it("reserves the header main area for terminal tabs without project context", () => {
    const wrapper = mountHeader("macos");

    expect(wrapper.find("#nvx-terminal-tabs-host").exists()).toBe(true);
    expect(wrapper.find(".nvx-app-header__context").exists()).toBe(false);
    expect(wrapper.text()).not.toContain("Choose Project");
    expect(wrapper.text()).not.toContain("Workspace");
  });

  it("leaves drag and double-click window gestures to Tauri instead of duplicating them", async () => {
    const wrapper = mountHeader("macos");
    const brand = wrapper.get(".nvx-app-header__brand");

    await brand.trigger("click");
    await brand.trigger("dblclick");
    expect(windowActionMocks.performWindowAction).not.toHaveBeenCalled();
  });

  it("renders only the Windows caption-control component on Windows", () => {
    const wrapper = mountHeader("windows");

    expect(wrapper.find(".nvx-app-header__native-controls-inset").exists()).toBe(false);
    expect(wrapper.find(".nvx-window-controls").exists()).toBe(true);
    expect(
      wrapper.get(".nvx-window-controls").attributes("data-tauri-drag-region"),
    ).toBe("false");
    expect(wrapper.findAll(".nvx-window-controls button")).toHaveLength(3);
    for (const button of wrapper.findAll(".nvx-window-controls button")) {
      expect(
        button.element.closest("[data-tauri-drag-region]")
          ?.getAttribute("data-tauri-drag-region"),
      ).toBe("false");
    }
  });

  it("routes standalone caption actions to the caller-bound command without targeting main", async () => {
    const wrapper = mountHeader("windows", true);
    await flushPromises();
    const buttons = wrapper.findAll(".nvx-window-controls button");
    for (const [index, action] of ["minimize", "maximize", "close"].entries()) {
      await buttons[index]!.trigger("click");
      expect(windowActionMocks.invoke).toHaveBeenLastCalledWith("window_standalone_action", { action });
    }
    expect(windowActionMocks.performWindowAction).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("retains the main window's resource-aware close path", async () => {
    const wrapper = mountHeader("windows");
    await wrapper.findAll(".nvx-window-controls button")[2]!.trigger("click");
    expect(windowActionMocks.performWindowAction).toHaveBeenCalledWith("close");
    expect(windowActionMocks.invoke).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it.each(["fixed", "unavailable"])("disables maximization and clears Snap targeting for a %s standalone window", async (state) => {
    let measure: FrameRequestCallback | undefined;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { measure = callback; return 1; });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    if (state === "fixed") windowActionMocks.isResizable.mockResolvedValue(false);
    else windowActionMocks.isResizable.mockRejectedValue(new Error("window unavailable"));
    const wrapper = mountHeader("windows", true);
    expect(wrapper.get("[data-windows-maximize]").attributes("disabled")).toBeDefined();
    await flushPromises();
    measure?.(0);
    await flushPromises();
    expect(wrapper.get("[data-windows-maximize]").attributes("disabled")).toBeDefined();
    expect(windowActionMocks.setWindowsMaximizeHitRegion).toHaveBeenLastCalledWith(null);
    expect(windowActionMocks.physicalWindowsCaptionHitRegion).not.toHaveBeenCalled();
    await wrapper.get("[data-windows-maximize]").trigger("click");
    expect(windowActionMocks.invoke).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
