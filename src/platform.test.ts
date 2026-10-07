import { describe, expect, it } from "vitest";

import { detectDesktopPlatform, shortcutProfilePlatform } from "./platform";

const ANDROID_UA = "Mozilla/5.0 (Linux; Android 14; Pixel 8 Build/AP2A) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Mobile Safari/537.36";
const LINUX_UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36";
const MAC_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WINDOWS_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0";

describe("detectDesktopPlatform", () => {
  it("recognizes Android before the generic Linux match", () => {
    expect(detectDesktopPlatform(ANDROID_UA)).toBe("android");
    expect(detectDesktopPlatform(LINUX_UA)).toBe("linux");
  });

  it("keeps the desktop platforms unchanged", () => {
    expect(detectDesktopPlatform(MAC_UA)).toBe("macos");
    expect(detectDesktopPlatform(WINDOWS_UA)).toBe("windows");
    expect(detectDesktopPlatform("Mozilla/5.0 (Unknown)")).toBe("other");
  });

  it("maps Android and Linux to the Ctrl shortcut profile", () => {
    expect(shortcutProfilePlatform("android")).toBe("windows");
    expect(shortcutProfilePlatform("linux")).toBe("windows");
    expect(shortcutProfilePlatform("macos")).toBe("macos");
  });
});
