import { beforeEach, describe, expect, it, vi } from "vitest";

const clipboard = vi.hoisted(() => ({
  isTauri: vi.fn(),
  readNativeClipboardText: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: clipboard.isTauri }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: clipboard.readNativeClipboardText }));

import { readText } from "./platform-clipboard";

describe("platform clipboard", () => {
  beforeEach(() => {
    clipboard.isTauri.mockReset();
    clipboard.readNativeClipboardText.mockReset();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { readText: vi.fn() },
    });
  });

  it("uses the capability-gated native clipboard reader in Tauri", async () => {
    clipboard.isTauri.mockReturnValue(true);
    clipboard.readNativeClipboardText.mockResolvedValue("native text");

    await expect(readText()).resolves.toBe("native text");
    expect(clipboard.readNativeClipboardText).toHaveBeenCalledOnce();
    expect(navigator.clipboard.readText).not.toHaveBeenCalled();
  });

  it("keeps the browser clipboard reader for non-Tauri surfaces", async () => {
    clipboard.isTauri.mockReturnValue(false);
    vi.mocked(navigator.clipboard.readText).mockResolvedValue("browser text");

    await expect(readText()).resolves.toBe("browser text");
    expect(navigator.clipboard.readText).toHaveBeenCalledOnce();
    expect(clipboard.readNativeClipboardText).not.toHaveBeenCalled();
  });
});
