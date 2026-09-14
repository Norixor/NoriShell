import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import {
  MAX_TERMINAL_URL_LENGTH,
  findTerminalHttpLinks,
  openSafeTerminalHttpUrl,
  safeTerminalHttpUrl,
  terminalLinkModifierPressed,
} from "./terminal-links";

describe("terminal HTTP link policy", () => {
  it("finds only safe HTTP(S) link snapshots and removes prose punctuation", () => {
    expect(findTerminalHttpLinks("Read https://example.com/a?q=1, then http://localhost:3000.")).toEqual([
      { url: "https://example.com/a?q=1", start: 5, end: 30 },
      { url: "http://localhost:3000", start: 37, end: 58 },
    ]);
  });

  it("rejects non-http, userinfo, controls, and oversized candidates", () => {
    expect(safeTerminalHttpUrl("ssh://example.com")).toBeNull();
    expect(safeTerminalHttpUrl("https://user:pass@example.com")).toBeNull();
    expect(safeTerminalHttpUrl("https://@example.com")).toBeNull();
    expect(safeTerminalHttpUrl("https://%40example.com")).toBeNull();
    expect(safeTerminalHttpUrl("https://example.com/a b")).toBeNull();
    expect(safeTerminalHttpUrl("https://example.com/\u0000x")).toBeNull();
    expect(safeTerminalHttpUrl("https://example.com/%0a")).toBeNull();
    expect(safeTerminalHttpUrl("https://example.com/%E2%80%AEhidden")).toBeNull();
    expect(safeTerminalHttpUrl("https://example.com/%zz")).toBeNull();
    expect(safeTerminalHttpUrl(`https://example.com/${"a".repeat(MAX_TERMINAL_URL_LENGTH)}`)).toBeNull();
  });

  it("requires the platform modifier and sends only the validated click snapshot to the opener", async () => {
    expect(terminalLinkModifierPressed(new MouseEvent("click", { metaKey: true }), "macos")).toBe(true);
    expect(terminalLinkModifierPressed(new MouseEvent("click", { ctrlKey: true }), "macos")).toBe(false);
    expect(terminalLinkModifierPressed(new MouseEvent("click", { ctrlKey: true }), "windows")).toBe(true);
    const opener = vi.fn().mockResolvedValue(undefined);
    await expect(openSafeTerminalHttpUrl("https://example.com/", opener)).resolves.toBe(true);
    expect(opener).toHaveBeenCalledOnce();
    expect(opener).toHaveBeenCalledWith("https://example.com/");
    await expect(openSafeTerminalHttpUrl("file:///tmp/no", opener)).resolves.toBe(false);
    expect(opener).toHaveBeenCalledOnce();
  });
});
