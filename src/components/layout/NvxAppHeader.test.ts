import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../locales";
import NvxAppHeader from "./NvxAppHeader.vue";

const windowActionMocks = vi.hoisted(() => ({
  performWindowAction: vi.fn<() => Promise<void>>(),
  physicalWindowsCaptionHitRegion: vi.fn(() => null),
  setWindowsMaximizeHitRegion: vi.fn<() => Promise<void>>(),
}));

vi.mock("../../platform-window", () => windowActionMocks);

function mountHeader(platform: "macos" | "windows") {
  return mount(NvxAppHeader, {
    props: { platform },
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
    windowActionMocks.setWindowsMaximizeHitRegion.mockReset();
    windowActionMocks.setWindowsMaximizeHitRegion.mockResolvedValue();
  });

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
});
