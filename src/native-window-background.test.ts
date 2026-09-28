import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  window: vi.fn<(color: number[]) => Promise<void>>(async () => undefined),
  webview: vi.fn<(color: number[]) => Promise<void>>(async () => undefined),
}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setBackgroundColor: native.window }) }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ setBackgroundColor: native.webview }) }));

import { startNativeBackgroundSync } from "./native-window-background";
import { afterNextPaint, parseHexRgb } from "./workspace-tab-paint";

const root = document.documentElement;
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("native background colour", () => {
  let stop: (() => void) | null = null;

  beforeEach(() => {
    native.window.mockReset().mockResolvedValue(undefined);
    native.webview.mockReset().mockResolvedValue(undefined);
    root.style.setProperty("--nvx-color-bg-canvas", "#f7f8fa");
  });
  afterEach(() => {
    stop?.();
    stop = null;
    root.style.removeProperty("--nvx-color-bg-canvas");
  });

  it("paints the window and its WebView with the opaque canvas colour and follows theme changes", async () => {
    stop = startNativeBackgroundSync("window-and-webview");
    await flush();
    expect(native.window).toHaveBeenCalledWith([247, 248, 250, 255]);
    expect(native.webview).toHaveBeenCalledWith([247, 248, 250, 255]);
    root.style.setProperty("--nvx-color-bg-canvas", "#101217");
    root.dataset.theme = "dark";
    await flush();
    expect(native.webview).toHaveBeenLastCalledWith([16, 18, 23, 255]);
    expect(native.window).toHaveBeenLastCalledWith([16, 18, 23, 255]);
    // An unrelated root style change does not rewrite an unchanged colour.
    root.style.setProperty("--nvx-unrelated", "1");
    await flush();
    expect(native.webview).toHaveBeenCalledTimes(2);
    delete root.dataset.theme;
  });

  it("sets only its own WebView for a Tab child", async () => {
    stop = startNativeBackgroundSync("webview");
    await flush();
    expect(native.webview).toHaveBeenCalledWith([247, 248, 250, 255]);
    expect(native.window).not.toHaveBeenCalled();
  });

  it("retries a colour whose native write failed on the next change", async () => {
    native.webview.mockRejectedValueOnce(new Error("denied"));
    stop = startNativeBackgroundSync("webview");
    await flush();
    root.dataset.theme = "light";
    await flush();
    expect(native.webview).toHaveBeenCalledTimes(2);
    delete root.dataset.theme;
  });
});

describe("paint helpers", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

  it("parses only #rrggbb canvas colours", () => {
    expect(parseHexRgb(" #0A0b10 ")).toEqual([10, 11, 16]);
    expect(parseHexRgb("#fff")).toBeNull();
    expect(parseHexRgb("rgb(1, 2, 3)")).toBeNull();
  });

  it("resolves after two animation frames, or after the timeout when frames stop", async () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frames.push(callback); return frames.length; });
    let done = false;
    const painted = afterNextPaint(50).then(() => { done = true; });
    frames.shift()!(0);
    await Promise.resolve();
    expect(done).toBe(false);
    frames.shift()!(16);
    await painted;
    expect(done).toBe(true);

    // A minimized window delivers no frames.
    vi.stubGlobal("requestAnimationFrame", () => 0);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    let stalled = false;
    const bounded = afterNextPaint(50).then(() => { stalled = true; });
    await vi.advanceTimersByTimeAsync(49);
    expect(stalled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await bounded;
    expect(stalled).toBe(true);
  });
});
