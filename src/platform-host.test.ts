import { afterEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({ tauri: false }));
const platform = vi.hoisted(() => ({ value: "macos" as "macos" | "windows" | "linux" | "android" | "other" }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => native.tauri }));
vi.mock("./platform", () => ({ detectDesktopPlatform: () => platform.value }));

import { hasNativeWindowChrome, hasTauriIpc, isMobileLayout, workspaceHostMode } from "./platform-host";

afterEach(() => { native.tauri = false; platform.value = "macos"; });

describe("platform-host", () => {
  it.each(["macos", "windows", "linux"] as const)("keeps native Tabs and window chrome on %s desktop windows", (value) => {
    native.tauri = true;
    platform.value = value;
    expect(hasTauriIpc()).toBe(true);
    expect(isMobileLayout()).toBe(false);
    expect(workspaceHostMode()).toBe("nativeTabs");
    expect(hasNativeWindowChrome()).toBe(true);
  });

  it("hosts Tabs in the single document on Android while keeping IPC", () => {
    native.tauri = true;
    platform.value = "android";
    expect(hasTauriIpc()).toBe(true);
    expect(isMobileLayout()).toBe(true);
    expect(workspaceHostMode()).toBe("singleDocument");
    expect(hasNativeWindowChrome()).toBe(false);
  });

  it("treats the browser preview as a single document without chrome or IPC", () => {
    platform.value = "macos";
    expect(hasTauriIpc()).toBe(false);
    expect(workspaceHostMode()).toBe("singleDocument");
    expect(hasNativeWindowChrome()).toBe(false);
    expect(isMobileLayout()).toBe(false);
  });

  it("never derives the mobile layout from the viewport width", () => {
    native.tauri = true;
    platform.value = "windows";
    Object.defineProperty(window, "innerWidth", { value: 360, configurable: true });
    expect(isMobileLayout()).toBe(false);
    expect(workspaceHostMode()).toBe("nativeTabs");
  });
});
