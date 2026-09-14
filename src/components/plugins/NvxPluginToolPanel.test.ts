import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginExtensionTargetContext, PluginUiContribution } from "../../core-api/generated/core-api";
import { i18n } from "../../locales";
import { usePluginExtensionsStore } from "../../stores/pluginExtensions";
import NvxPluginToolPanel from "./NvxPluginToolPanel.vue";
import NvxPluginUiDocument from "./NvxPluginUiDocument.vue";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));

function fixtures(context: PluginExtensionTargetContext): PluginUiContribution[] {
  return ["Docker", "Disk", "Network", "Processes", "Services", "Crontab", "Logs", "Nginx", "Tunnels"].map((name) => ({
    pluginId: `test.${name.toLowerCase()}`, pluginName: name, icon: "server",
    artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64), instanceGeneration: "1", stateVersion: "1", contributionRevision: "1", target: context,
    document: { schemaVersion: 1, rootNodeId: "root", nodes: [{ kind: "button", nodeId: "root", actionId: "refresh", label: `${name} refresh`, icon: "refresh", variant: "primary", disabled: false }] },
  }));
}
function setupStore() {
  const store = usePluginExtensionsStore();
  const acquire = vi.spyOn(store, "acquireTarget").mockImplementation(async (targetId, instanceKey, displayLabel) => ({ key: `${targetId}\0${instanceKey}`, context: { targetId, contextHandle: instanceKey, targetRevision: "1", surfaceKind: "sidebar", displayLabel: displayLabel ?? null } }));
  const release = vi.spyOn(store, "releaseTarget").mockResolvedValue();
  vi.spyOn(store, "loadTargetContributions").mockImplementation(async (context) => fixtures(context));
  vi.spyOn(store, "forContext").mockImplementation((context) => context ? fixtures(context) : []);
  return { store, acquire, release };
}
describe("single terminal plugin panel", () => {
  beforeEach(() => { setActivePinia(createPinia()); i18n.global.locale.value = "en"; localStorage.clear(); });
  afterEach(() => vi.restoreAllMocks());
  it("searches nine installed tools while rendering only the selected document", async () => {
    setupStore();
    const wrapper = mount(NvxPluginToolPanel, { props: { modelValue: true, targetId: "terminal.tools", instanceKey: "host|pane|ssh|session|1", contextLabel: "Production SSH" }, global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.findAllComponents(NvxPluginUiDocument)).toHaveLength(1);
    await wrapper.get("[data-plugin-picker-trigger]").trigger("click");
    await wrapper.get("input").setValue("nginx");
    expect(wrapper.findAll("[role=option]")).toHaveLength(1);
    await wrapper.get("[role=option]").trigger("click");
    expect(wrapper.findAllComponents(NvxPluginUiDocument)).toHaveLength(1);
    expect(wrapper.getComponent(NvxPluginUiDocument).props("contribution").pluginId).toBe("test.nginx");
    expect(wrapper.text()).toContain("Production SSH");
    wrapper.unmount();
  });
  it("keeps operation documents unavailable without a running SSH session", async () => {
    const { store } = setupStore();
    const invoke = vi.spyOn(store, "invokeAction");
    const wrapper = mount(NvxPluginToolPanel, { props: { modelValue: true, targetId: "terminal.tools", instanceKey: "|empty|ssh|none|0", contextLabel: "Local shell", available: false }, global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("Select a running SSH terminal");
    expect(wrapper.findAllComponents(NvxPluginUiDocument)).toHaveLength(0);
    expect(invoke).not.toHaveBeenCalled();
    await wrapper.setProps({ available: true });
    expect(wrapper.findAllComponents(NvxPluginUiDocument)).toHaveLength(1);
    wrapper.unmount();
  });
  it("replaces the lease and document context when the focused split or generation changes", async () => {
    const { acquire, release } = setupStore();
    const wrapper = mount(NvxPluginToolPanel, { props: { modelValue: true, targetId: "terminal.tools", instanceKey: "host-a|pane-a|ssh|session-a|1", contextLabel: "Host A" }, global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.setProps({ instanceKey: "host-b|pane-b|ssh|session-b|2", contextLabel: "Host B" });
    await flushPromises();
    expect(release).toHaveBeenCalledWith(expect.objectContaining({ key: "terminal.tools\0host-a|pane-a|ssh|session-a|1" }));
    expect(acquire).toHaveBeenLastCalledWith("terminal.tools", "host-b|pane-b|ssh|session-b|2", "Host B");
    expect(wrapper.getComponent(NvxPluginUiDocument).props("contribution").target.contextHandle).toBe("host-b|pane-b|ssh|session-b|2");
    expect(wrapper.text()).not.toContain("Host A");
    wrapper.unmount();
  });
  it("lets keyboard resizing persist a bounded host-owned width", async () => {
    setupStore();
    const wrapper = mount(NvxPluginToolPanel, { props: { modelValue: true, targetId: "terminal.tools", instanceKey: "context", contextLabel: "Host" }, global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.get("[role=separator]").trigger("keydown", { key: "End" });
    expect(localStorage.getItem("norishell.pluginTools.width.v1")).toBe("720");
    await wrapper.get("[role=separator]").trigger("keydown", { key: "ArrowLeft" });
    expect(wrapper.get("[role=separator]").attributes("aria-valuenow")).toBe("720");
    wrapper.unmount();
  });
});
