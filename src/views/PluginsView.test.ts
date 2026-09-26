import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginLocalPackagePreview, PluginOperationSummary } from "../core-api/generated/core-api";
import { i18n } from "../locales";

enableAutoUnmount(afterEach);

const client = vi.hoisted(() => ({
  cancelPluginOperation: vi.fn(),
  cancelLocalPluginPackage: vi.fn(),
  disablePlugin: vi.fn(),
  enablePlugin: vi.fn(),
  getPluginOperation: vi.fn(),
  getPluginReadiness: vi.fn(),
  installLocalPlugin: vi.fn(),
  invokePluginContribution: vi.fn(),
  listInstalledPlugins: vi.fn(),
  listPluginAudit: vi.fn(),
  listPluginContributions: vi.fn(),
  listPluginNavigation: vi.fn(),
  listPendingPluginTerminalInput: vi.fn(),
  openPluginSpecialPermission: vi.fn(),
  openPluginTerminalInput: vi.fn(),
  prepareLocalPluginPackage: vi.fn(),
  preparePluginContributionCopy: vi.fn(),
  replacePluginCapabilityGrants: vi.fn(),
  setPluginSafeModeNextStart: vi.fn(),
  uninstallPlugin: vi.fn(),
}));

vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => false }));

import PluginsView from "./PluginsView.vue";

const packagePreview: PluginLocalPackagePreview = {
  preparationId: "019d0000-0000-7000-8000-000000000800",
  pluginId: "com.norishell.example",
  name: "Local Example",
  author: "Example Author",
  version: "1.2.0",
  packageSize: 2048n,
  packageSha256: "a".repeat(64),
  capabilities: ["uiPanel"],
  currentVersion: "1.1.0",
  currentStateVersion: "5",
  priorPackageSha256: null,
  retainedCapabilityGrants: [{ capability: "uiPanel", granted: true }],
  approvedSpecialGrants: [],
  specialPermissionExpiresAtUnixMs: null,
  publisherVerified: false,
};

const installedPlugin = {
  pluginId: packagePreview.pluginId,
  name: packagePreview.name,
  publisher: packagePreview.author,
  packageKind: "wasm" as const,
  artifactFingerprintSha256: "c".repeat(64),
  packageSha256: "d".repeat(64),
  activeVersion: "1.1.0",
  capabilities: ["uiPanel"] as const,
  grants: [{ capability: "uiPanel" as const, granted: true }],
  hasSettings: false,
  state: "enabled" as const,
  stateVersion: "5",
  installedAtUnixMs: 1n,
  updatedAtUnixMs: 2n,
};

const successfulInstall: PluginOperationSummary = {
  operationId: "019d0000-0000-7000-8000-000000000806",
  pluginId: packagePreview.pluginId,
  kind: "update",
  state: "succeeded",
  progressPercent: 100,
  errorCode: null,
  startedAtUnixMs: 1n,
  updatedAtUnixMs: 2n,
};

async function mountPlugins() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/settings", component: { template: "<div>Settings</div>" } },
      { path: "/plugins", component: PluginsView },
    ],
  });
  await router.push("/plugins");
  await router.isReady();
  const wrapper = mount(PluginsView, {
    attachTo: document.body,
    global: { plugins: [createPinia(), router, i18n], stubs: { teleport: true } },
  });
  await flushPromises();
  return wrapper;
}

describe("PluginsView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    client.cancelLocalPluginPackage.mockResolvedValue(undefined);
    client.getPluginReadiness.mockResolvedValue({
      ready: true,
      protocolMajor: 1,
      protocolMinor: 3,
      safeModeActive: false,
      safeModeNextStart: false,
    });
    client.installLocalPlugin.mockResolvedValue(successfulInstall);
    client.disablePlugin.mockResolvedValue({ ...installedPlugin, state: "disabled", stateVersion: "6" });
    client.enablePlugin.mockResolvedValue({ ...installedPlugin, activeVersion: packagePreview.version, state: "enabled", stateVersion: "7" });
    client.listInstalledPlugins.mockResolvedValue([installedPlugin]);
    client.listPluginAudit.mockResolvedValue([]);
    client.listPluginContributions.mockResolvedValue([]);
    client.listPluginNavigation.mockResolvedValue([]);
    client.listPendingPluginTerminalInput.mockResolvedValue([]);
    client.prepareLocalPluginPackage.mockResolvedValue(packagePreview);
  });

  it("shows installed plugins and a local ZIP import entry without a marketplace", async () => {
    const wrapper = await mountPlugins();

    expect(wrapper.text()).toContain("Installed plugins");
    expect(wrapper.text()).toContain("Import ZIP");
    expect(wrapper.text()).toContain("Import a newer version of the same plugin to update it safely.");
    expect(wrapper.text()).not.toContain("Plugin Marketplace");
    expect(wrapper.text()).not.toContain("Refresh catalog");
  });

  it("previews a manually selected newer ZIP and submits it through the local install path", async () => {
    const wrapper = await mountPlugins();

    const importButton = wrapper.findAll("button").find((button) => button.text().includes("Import ZIP"));
    expect(importButton).toBeDefined();
    await importButton!.trigger("click");
    await flushPromises();

    expect(client.prepareLocalPluginPackage).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("1.1.0 → 1.2.0");
    expect(wrapper.text()).toContain("Confirm update");

    const updateButton = wrapper.findAll("button").find((button) => button.text().includes("Confirm Update"));
    expect(updateButton).toBeDefined();
    await updateButton!.trigger("click");
    await flushPromises();

    expect(client.installLocalPlugin).toHaveBeenCalledWith({
      preparationId: packagePreview.preparationId,
      expectedPackageSha256: packagePreview.packageSha256,
      expectedStateVersion: "6",
      capabilityGrants: [{ capability: "uiPanel", granted: false }],
    });
  });

  it("shows a same-version package replacement in the existing import confirmation", async () => {
    client.prepareLocalPluginPackage.mockResolvedValue({
      ...packagePreview,
      version: installedPlugin.activeVersion,
      currentVersion: installedPlugin.activeVersion,
      priorPackageSha256: installedPlugin.packageSha256,
      retainedCapabilityGrants: [],
    });
    const wrapper = await mountPlugins();
    const importButton = wrapper.findAll("button").find((button) => button.text().includes("Import ZIP"));
    await importButton!.trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Replace the package for this version");
    expect(wrapper.text()).toContain(installedPlugin.packageSha256);
    expect(wrapper.text()).toContain(packagePreview.packageSha256);
    expect(wrapper.findAll("button").filter((button) => button.text().includes("Confirm Update"))).toHaveLength(1);
  });

  it("discloses the retained package identity when reinstalling after uninstall", async () => {
    client.listInstalledPlugins.mockResolvedValue([]);
    client.prepareLocalPluginPackage.mockResolvedValue({
      ...packagePreview,
      currentVersion: null,
      currentStateVersion: null,
      priorPackageSha256: installedPlugin.packageSha256,
    });
    const wrapper = await mountPlugins();
    const importButton = wrapper.findAll("button").find((button) => button.text().includes("Import ZIP"));
    await importButton!.trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Replace the package for this version");
    expect(wrapper.text()).toContain("Private storage and sign-in state remain bound to the old package identity");
    expect(wrapper.text()).toContain(installedPlugin.packageSha256);
    expect(wrapper.findAll("button").filter((button) => button.text().includes("Confirm Install"))).toHaveLength(1);
  });
});
