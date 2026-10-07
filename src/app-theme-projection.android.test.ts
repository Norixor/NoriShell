import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(...args: unknown[]) => Promise<unknown>>(() => Promise.resolve());
const mobile = { value: true };
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }));
vi.mock("./platform-host", () => ({ hasTauriIpc: () => true, isMobileLayout: () => mobile.value }));

const { BUILTIN_APP_THEMES } = await import("./app-theme");
const { publishAppThemeProjection } = await import("./app-theme-projection");

describe("Android system bars follow the app theme", () => {
  beforeEach(() => {
    invoke.mockClear();
    mobile.value = true;
    localStorage.clear();
  });

  it("sends the resolved appearance and header color once per change", () => {
    const projection = { light: BUILTIN_APP_THEMES.light, dark: BUILTIN_APP_THEMES.dark };
    document.documentElement.dataset.theme = "dark";
    publishAppThemeProjection(projection);
    publishAppThemeProjection(projection);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenLastCalledWith("android_set_system_bars", {
      request: expect.objectContaining({ dark: true, background: BUILTIN_APP_THEMES.dark.colors.bgSurface.toLowerCase() }),
    });
    document.documentElement.dataset.theme = "light";
    publishAppThemeProjection(projection);
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenLastCalledWith("android_set_system_bars", {
      request: expect.objectContaining({ dark: false, background: BUILTIN_APP_THEMES.light.colors.bgSurface.toLowerCase() }),
    });
  });

  it("contains a missing or failing command without breaking the projection", async () => {
    const projection = { light: BUILTIN_APP_THEMES.light, dark: BUILTIN_APP_THEMES.dark };
    document.documentElement.dataset.theme = "dark";
    invoke.mockImplementationOnce(() => { throw new Error("command not available"); });
    expect(() => publishAppThemeProjection(projection)).not.toThrow();
    invoke.mockImplementationOnce(() => Promise.reject(new Error("system_bars.unavailable")));
    publishAppThemeProjection(projection);
    await Promise.resolve();
    await Promise.resolve();
    // A failed sync is retried on the next publish instead of being deduplicated away.
    publishAppThemeProjection(projection);
    expect(invoke).toHaveBeenCalledTimes(3);
    expect(localStorage.getItem("norishell.app-theme.projection.v1")).not.toBeNull();
  });

  it("never calls the Android command on desktop", () => {
    mobile.value = false;
    document.documentElement.dataset.theme = "dark";
    publishAppThemeProjection({ light: BUILTIN_APP_THEMES.light, dark: { ...BUILTIN_APP_THEMES.dark } });
    expect(invoke).not.toHaveBeenCalled();
  });
});
