import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import NvxWindowFrame from "./NvxWindowFrame.vue";

const platformWindowMocks = vi.hoisted(() => ({
  getNativeControlsInset: vi.fn<() => Promise<number | null>>(),
  setNativeHeaderHeight: vi.fn<(height: number) => Promise<void>>(),
}));

vi.mock("../../platform", () => ({
  detectDesktopPlatform: () => "macos",
}));

vi.mock("../../platform-window", () => platformWindowMocks);

describe("NvxWindowFrame macOS native-control inset", () => {
  beforeEach(() => {
    platformWindowMocks.getNativeControlsInset.mockReset();
    platformWindowMocks.setNativeHeaderHeight.mockReset();
    platformWindowMocks.setNativeHeaderHeight.mockResolvedValue();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("resynchronizes measured header height after zoom and layout changes, then disconnects", async () => {
    let resized: ResizeObserverCallback | undefined;
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) { resized = callback; }
      observe() {}
      disconnect = disconnect;
    });
    let height = 56;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
      () => ({ height }) as DOMRect,
    );
    const wrapper = mount(NvxWindowFrame, {
      props: { zoom: 1.25 },
      slots: { default: '<header class="nvx-app-header"></header>' },
    });
    await flushPromises();
    expect(platformWindowMocks.setNativeHeaderHeight).toHaveBeenLastCalledWith(70);
    await wrapper.setProps({ zoom: 0.8 });
    expect(platformWindowMocks.setNativeHeaderHeight).toHaveBeenLastCalledWith(44.800000000000004);
    height = 60;
    resized?.([], {} as ResizeObserver);
    expect(platformWindowMocks.setNativeHeaderHeight).toHaveBeenLastCalledWith(48);
    wrapper.unmount();
    expect(disconnect).toHaveBeenCalledOnce();
  });

  it("publishes a measured native-control inset for the header safe area", async () => {
    platformWindowMocks.getNativeControlsInset.mockResolvedValue(82);
    const wrapper = mount(NvxWindowFrame);

    await flushPromises();

    expect(wrapper.get(".nvx-window-frame").attributes("style")).toContain(
      "--nvx-window-native-controls-inset: 82px",
    );
    expect(wrapper.get(".nvx-window-frame").attributes("style")).toContain(
      "--nvx-layout-navigation-rail-width: 82px",
    );
  });

  it("leaves the CSS fallback active when AppKit measurement is unavailable", async () => {
    platformWindowMocks.getNativeControlsInset.mockResolvedValue(null);
    const wrapper = mount(NvxWindowFrame);

    await flushPromises();

    expect(wrapper.get(".nvx-window-frame").attributes("style")).toBeUndefined();
  });

  it("keeps native button clearance fixed in screen points as the WebView zooms", async () => {
    platformWindowMocks.getNativeControlsInset.mockResolvedValue(80);
    const wrapper = mount(NvxWindowFrame, { props: { zoom: 1.25 } });
    await flushPromises();
    expect(wrapper.attributes("style")).toContain("--nvx-window-native-controls-inset: 64px");
    await wrapper.setProps({ zoom: 0.8 });
    expect(wrapper.attributes("style")).toContain("--nvx-window-native-controls-inset: 100px");
  });
});
