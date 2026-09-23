import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({
  getPluginIsolatedSurfaceContent: vi.fn(),
  invokePluginIsolatedBridge: vi.fn(),
}));
vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => false,
  convertFileSrc: (token: string, protocol: string) => `${protocol}://localhost/${token}`,
}));
import PluginIsolatedWrapper from "./PluginIsolatedWrapper.vue";

describe("PluginIsolatedWrapper bootstrap", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.history.replaceState(null, "", "/?surfaceId=surface-one&channelNonce=private-nonce");
    i18n.global.locale.value = "en";
  });
  afterEach(() => window.history.replaceState(null, "", "/"));

  it.each(["en", "zh-CN"] as const)("localizes rejected content without exposing the Core error in %s", async (locale) => {
    i18n.global.locale.value = locale;
    client.getPluginIsolatedSurfaceContent.mockRejectedValue(new Error("private-token-and-nonce"));
    const wrapper = mount(PluginIsolatedWrapper, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get('[role="status"]').text()).toContain("surface-content");
    expect(wrapper.text()).not.toContain("plugins.isolated.");
    expect(wrapper.text()).not.toContain("private-token-and-nonce");
    expect(wrapper.find("iframe").exists()).toBe(false);
    wrapper.unmount();
  });

  it("rejects a response for another surface before loading plugin content", async () => {
    client.getPluginIsolatedSurfaceContent.mockResolvedValue({ surfaceId: "other", documentToken: "private-token", nextSequence: 1 });
    const wrapper = mount(PluginIsolatedWrapper, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("surface-identity");
    expect(wrapper.find("iframe").exists()).toBe(false);
    wrapper.unmount();
  });

  it("loads the Core document only in a script-only sandbox", async () => {
    client.getPluginIsolatedSurfaceContent.mockResolvedValue({ surfaceId: "surface-one", documentToken: "document-token", nextSequence: 1 });
    client.invokePluginIsolatedBridge.mockResolvedValue({ nextSequence: 2, pending: null, reply: null });
    const wrapper = mount(PluginIsolatedWrapper, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(client.getPluginIsolatedSurfaceContent).toHaveBeenCalledWith("surface-one", "private-nonce");
    expect(wrapper.get("iframe").attributes()).toMatchObject({
      src: "norishell-plugin://localhost/document-token", sandbox: "allow-scripts", referrerpolicy: "no-referrer", title: "Plugin surface",
    });
    expect(wrapper.find(".nvx-app-header").exists()).toBe(true);
    expect(wrapper.get("iframe").element.contains(wrapper.get(".nvx-app-header").element)).toBe(false);
    wrapper.unmount();
  });
});
