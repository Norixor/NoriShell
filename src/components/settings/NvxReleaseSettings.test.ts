import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { releasesEn } from "../../locales/releases";

const api = vi.hoisted(() => ({
  getVersion: vi.fn(),
  invoke: vi.fn(),
  openUrl: vi.fn(),
}));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: api.getVersion }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: api.openUrl }));

import NvxReleaseSettings from "./NvxReleaseSettings.vue";

function mountSettings() {
  return mount(NvxReleaseSettings, {
    global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { releases: releasesEn } } })] },
  });
}

function button(wrapper: ReturnType<typeof mountSettings>, text: string) {
  const found = wrapper.findAll("button").find((item) => item.text().includes(text));
  if (!found) throw new Error(`Missing button: ${text}`);
  return found;
}

describe("release settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getVersion.mockResolvedValue("0.1.0");
    api.invoke.mockResolvedValue({
      currentVersion: "0.1.0",
      status: "upToDate",
      latestVersion: null,
      releaseUrl: "https://github.com/Norixor/NoriShell/releases",
    });
    api.openUrl.mockResolvedValue(undefined);
  });

  it("does not check GitHub on mount and shows the packaged version", async () => {
    const wrapper = mountSettings();
    await flushPromises();
    expect(api.invoke).not.toHaveBeenCalled();
    expect(wrapper.get("h2").text()).toBe("NoriShell");
    expect(wrapper.text()).toContain("github.com/Norixor/NoriShell");
    expect(wrapper.text()).toContain("Current version 0.1.0");
    expect(wrapper.text()).toContain("No update check has run yet");
  });

  it("opens only the fixed project page from About", async () => {
    const wrapper = mountSettings();
    await flushPromises();
    await button(wrapper, "Open GitHub").trigger("click");
    await flushPromises();
    expect(api.openUrl).toHaveBeenCalledWith("https://github.com/Norixor/NoriShell");
  });

  it("shows a newer version and opens only the fixed Releases page", async () => {
    api.invoke.mockResolvedValueOnce({
      currentVersion: "0.1.0-beta.1",
      status: "updateAvailable",
      latestVersion: "0.1.0",
      releaseUrl: "https://example.invalid/untrusted-release-url",
    });
    const wrapper = mountSettings();
    await flushPromises();
    await button(wrapper, "Check for updates").trigger("click");
    await flushPromises();
    expect(api.invoke).toHaveBeenCalledWith("release_check");
    expect(wrapper.text()).toContain("Version 0.1.0 is available");
    await button(wrapper, "Download on GitHub").trigger("click");
    await flushPromises();
    expect(api.openUrl).toHaveBeenCalledWith("https://github.com/Norixor/NoriShell/releases");
  });

  it("distinguishes an empty published list from an up-to-date release", async () => {
    api.invoke.mockResolvedValueOnce({
      currentVersion: "0.1.0",
      status: "noRelease",
      latestVersion: null,
      releaseUrl: "https://github.com/Norixor/NoriShell/releases",
    });
    const wrapper = mountSettings();
    await flushPromises();
    await button(wrapper, "Check for updates").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("No published release is available on GitHub Releases");
    expect(wrapper.text()).not.toContain("You are up to date");
  });
});
