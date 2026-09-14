import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../locales";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import NvxPluginSettingsDialog from "./NvxPluginSettingsDialog.vue";

const client = vi.hoisted(() => ({
  getPluginSettings: vi.fn(), replacePluginSettings: vi.fn(), resetPluginSettings: vi.fn(),
  parseCoreApiError: (value: unknown) => value,
}));
vi.mock("../../core-api/client", () => client);
enableAutoUnmount(afterEach);
const snapshot = () => ({
  pluginId: "test.status", packageSha256: "a".repeat(64), installedStateVersion: "2",
  schemaSha256: "b".repeat(64), revision: "3",
  schema: { schemaVersion: 1, fields: [
    { key: "visible", type: "boolean", label: { en: "Show terminal status", "zh-CN": "显示终端状态条" }, default: true },
    { key: "limit", type: "number", label: { en: "Limit", "zh-CN": "数量限制" }, default: 5, min: 1, max: 10 },
  ] }, values: { visible: true, limit: 5 },
});
function render() {
  return mount(NvxPluginSettingsDialog, { props: { pluginId: "test.status", pluginName: "Status" },
    attachTo: document.body, global: { plugins: [i18n], stubs: { Teleport: true } } });
}
beforeEach(() => {
  setActivePinia(createPinia());
  vi.clearAllMocks();
  i18n.global.locale.value = "en";
  client.getPluginSettings.mockResolvedValue(snapshot());
  vi.spyOn(usePluginExtensionsStore(), "refreshPluginContributions").mockResolvedValue();
});
afterEach(() => { document.body.innerHTML = ""; vi.restoreAllMocks(); });

describe("plugin settings", () => {
  it("keeps a local draft until saving the exact settings fence and refreshes mounted contexts", async () => {
    const wrapper = render();
    await flushPromises();
    await wrapper.get('input[type="checkbox"]').setValue(false);
    expect(client.replacePluginSettings).not.toHaveBeenCalled();
    client.replacePluginSettings.mockResolvedValue({ ...snapshot(), revision: "4", values: { visible: false, limit: 5 } });
    await wrapper.findAll("button").find((button) => button.text() === "Save settings")!.trigger("click");
    await flushPromises();
    expect(client.replacePluginSettings).toHaveBeenCalledExactlyOnceWith({
      pluginId: "test.status", expectedPackageSha256: "a".repeat(64), expectedInstalledStateVersion: "2",
      expectedSchemaSha256: "b".repeat(64), expectedRevision: "3", values: { visible: false, limit: 5 },
    });
    expect(usePluginExtensionsStore().refreshPluginContributions).toHaveBeenCalledWith("test.status");
    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("keeps conflict drafts visible and requires explicit reload", async () => {
    const wrapper = render();
    await flushPromises();
    await wrapper.get('input[type="checkbox"]').setValue(false);
    client.replacePluginSettings.mockRejectedValue({ code: "plugin.settings_conflict" });
    await wrapper.findAll("button").find((button) => button.text() === "Save settings")!.trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("Reload the latest settings");
    expect(wrapper.get('input[type="checkbox"]').element).toHaveProperty("checked", false);
    expect(wrapper.emitted("close")).toBeUndefined();
    await wrapper.findAll("button").find((button) => button.text() === "Reload settings")!.trigger("click");
    await flushPromises();
    expect(wrapper.get('input[type="checkbox"]').element).toHaveProperty("checked", true);
  });

  it("restores defaults through reset and localizes plugin field labels", async () => {
    i18n.global.locale.value = "zh-CN";
    const wrapper = render();
    await flushPromises();
    expect(wrapper.text()).toContain("显示终端状态条");
    client.resetPluginSettings.mockResolvedValue({ ...snapshot(), revision: "4" });
    await wrapper.findAll("button").find((button) => button.text() === "恢复默认设置")!.trigger("click");
    await flushPromises();
    expect(client.resetPluginSettings).toHaveBeenCalledOnce();
    expect(client.resetPluginSettings.mock.calls[0]![0]).not.toHaveProperty("values");
    expect(wrapper.emitted("close")).toBeUndefined();
  });

  it("rejects invalid numeric drafts and cancellation never writes", async () => {
    const wrapper = render();
    await flushPromises();
    await wrapper.get('input[type="number"]').setValue(11);
    expect(wrapper.findAll("button").find((button) => button.text() === "Save settings")!.attributes("disabled")).toBeDefined();
    await wrapper.findAll("button").find((button) => button.text() === "Cancel")!.trigger("click");
    expect(client.replacePluginSettings).not.toHaveBeenCalled();
    expect(client.resetPluginSettings).not.toHaveBeenCalled();
    expect(wrapper.emitted("close")).toHaveLength(1);
  });
});
