import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { revealWindowAfterMount } from "./window-first-show";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.mocked(invoke).mockReset();
  vi.restoreAllMocks();
});

describe("first window reveal", () => {
  it("signals Core only after resolving body styles and layout", async () => {
    const steps: string[] = [];
    vi.spyOn(window, "getComputedStyle").mockImplementation(() => {
      steps.push("style");
      return { getPropertyValue: () => "#f7f8fa" } as unknown as CSSStyleDeclaration;
    });
    vi.spyOn(document.body, "getBoundingClientRect").mockImplementation(() => {
      steps.push("layout");
      return {} as DOMRect;
    });
    vi.mocked(invoke).mockImplementation(async () => {
      steps.push("ready");
    });

    await revealWindowAfterMount();

    expect(steps).toEqual(["style", "layout", "ready"]);
    expect(invoke).toHaveBeenCalledWith("window_renderer_ready");
  });

  it("leaves a failed ready signal to the native timeout fallback", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("WebView unavailable"));
    await expect(revealWindowAfterMount()).resolves.toBeUndefined();
  });

  it("waits for startup fonts before revealing", async () => {
    const originalFonts = Object.getOwnPropertyDescriptor(document, "fonts");
    let finishFonts!: () => void;
    Object.defineProperty(document, "fonts", {
      configurable: true,
      value: { ready: new Promise<void>((resolve) => { finishFonts = resolve; }) },
    });
    try {
      const reveal = revealWindowAfterMount();
      await Promise.resolve();
      expect(invoke).not.toHaveBeenCalled();
      finishFonts();
      await reveal;
      expect(invoke).toHaveBeenCalledWith("window_renderer_ready");
    } finally {
      if (originalFonts) Object.defineProperty(document, "fonts", originalFonts);
      else Reflect.deleteProperty(document, "fonts");
    }
  });
});
