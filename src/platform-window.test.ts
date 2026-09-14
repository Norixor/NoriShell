import { beforeEach, describe, expect, it, vi } from "vitest";

const windowMocks = vi.hoisted(() => ({
  isTauri: vi.fn(),
  getCurrentWindow: vi.fn(),
  invoke: vi.fn(),
  requestWindowClose: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: windowMocks.isTauri,
  invoke: windowMocks.invoke,
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: windowMocks.getCurrentWindow }));
vi.mock("./core-api/client", () => ({ requestWindowClose: windowMocks.requestWindowClose }));

import { performWindowAction, physicalWindowsCaptionHitRegion } from "./platform-window";

describe("Windows caption hit region", () => {
  it("converts the WebView maximize button rect to bounded physical pixels", () => {
    expect(
      physicalWindowsCaptionHitRegion(
        { left: 1142, top: 0, width: 46, height: 56 },
        1.5,
      ),
    ).toEqual({ x: 1713, y: 0, width: 69, height: 84 });
  });

  it("rejects unavailable or non-visible layout measurements", () => {
    expect(
      physicalWindowsCaptionHitRegion(
        { left: 0, top: 0, width: 0, height: 56 },
        2,
      ),
    ).toBeNull();
    expect(
      physicalWindowsCaptionHitRegion(
        { left: 10, top: 0, width: 46, height: 56 },
        Number.NaN,
      ),
    ).toBeNull();
  });
});

describe("native fullscreen window action", () => {
  const nativeWindow = {
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    isFullscreen: vi.fn<() => Promise<boolean>>(),
    setFullscreen: vi.fn<(fullscreen: boolean) => Promise<void>>(),
  };

  beforeEach(() => {
    windowMocks.isTauri.mockReset();
    windowMocks.isTauri.mockReturnValue(true);
    windowMocks.getCurrentWindow.mockReset();
    windowMocks.getCurrentWindow.mockReturnValue(nativeWindow);
    nativeWindow.isFullscreen.mockReset();
    nativeWindow.setFullscreen.mockReset();
    nativeWindow.setFullscreen.mockResolvedValue();
  });

  it.each([
    [true, false],
    [false, true],
  ])("sets fullscreen to %s when the native window reports %s", async (expected, current) => {
    nativeWindow.isFullscreen.mockResolvedValue(current);

    await performWindowAction("toggleFullscreen");

    expect(nativeWindow.isFullscreen).toHaveBeenCalledOnce();
    expect(nativeWindow.setFullscreen).toHaveBeenCalledWith(expected);
  });

  it("keeps the native rejection observable to the desktop action handler", async () => {
    nativeWindow.isFullscreen.mockResolvedValue(false);
    nativeWindow.setFullscreen.mockRejectedValue(new Error("fullscreen denied"));

    await expect(performWindowAction("toggleFullscreen")).rejects.toThrow("fullscreen denied");
  });
});
