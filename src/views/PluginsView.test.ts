import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import * as tauriCore from "@tauri-apps/api/core";
import { createPinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  PluginLocalPackagePreview,
  PluginOperationSummary,
  PluginTerminalInputProposal,
} from "../core-api/generated/core-api";
import { i18n } from "../locales";

enableAutoUnmount(afterEach);

const client = vi.hoisted(() => ({
  cancelPluginOperation: vi.fn(),
  cancelLocalPluginPackage: vi.fn(),
  prepareCatalogPluginPackage: vi.fn(),
  openPluginSpecialPermission: vi.fn(),
  openPluginTerminalInput: vi.fn(),
  disablePlugin: vi.fn(),
  enablePlugin: vi.fn(),
  fetchPluginCatalogSnapshot: vi.fn(),
  readPluginIcon: vi.fn().mockResolvedValue({ dataUrl: null }),
  getPluginOperation: vi.fn(),
  getPluginSettings: vi.fn(),
  getPluginReadiness: vi.fn(),
  installLocalPlugin: vi.fn(),
  installCatalogPlugin: vi.fn(),
  invokePluginContribution: vi.fn(),
  listInstalledPlugins: vi.fn(),
  listPluginContributions: vi.fn(),
  listPluginAudit: vi.fn(),
  listPluginOperationPermissions: vi.fn(),
  listPendingPluginTerminalInput: vi.fn(),
  preparePluginContributionCopy: vi.fn(),
  prepareLocalPluginPackage: vi.fn(),
  refreshPluginCatalog: vi.fn(),
  replacePluginCapabilityGrants: vi.fn(),
  revokePluginOperationPermission: vi.fn(),
  clearPluginOperationPermissions: vi.fn(),
  setPluginSafeModeNextStart: vi.fn(),
  uninstallPlugin: vi.fn(),
}));

vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/core", { spy: true });

import PluginsView from "./PluginsView.vue";
import { usePluginsStore } from "../stores/plugins";
import { useTipsStore } from "../stores/tips";
import { usePluginIconsStore } from "../stores/pluginIcons";

const packagePreview: PluginLocalPackagePreview = {
  preparationId: "019d0000-0000-7000-8000-000000000800",
  pluginId: "com.norishell.example",
  name: "Local Example",
  author: "Example Author",
  version: "1.2.0",
  packageSize: 2048n,
  packageSha256: "a".repeat(64),
  capabilities: ["uiPanel", "terminalRequestInput"],
  currentVersion: null,
  currentStateVersion: null,
  retainedCapabilityGrants: [],
  approvedSpecialGrants: [],
  specialPermissionExpiresAtUnixMs: null,
  publisherVerified: false,
};

const proposal: PluginTerminalInputProposal = {
  approvalId: "019d0000-0000-7000-8000-000000000801",
  pluginId: packagePreview.pluginId,
  pluginName: packagePreview.name,
  publisher: packagePreview.author,
  artifactFingerprintSha256: "SHA256:artifact",
  packageSha256: packagePreview.packageSha256,
  instanceGeneration: "1",
  hostLabel: "Production API",
  endpoint: "ops@example.test:22",
  sessionId: "019d0000-0000-7000-8000-000000000802",
  generation: "2",
  channelId: "019d0000-0000-7000-8000-000000000803",
  attachmentId: "019d0000-0000-7000-8000-000000000804",
  viewId: "019d0000-0000-7000-8000-000000000805",
  focusEpoch: "6",
  inputEpoch: "7",
  payload: "printf '<script>not markup</script>'",
  appendEnter: true,
  payloadSha256: "b".repeat(64),
  expiresAtUnixMs: 1000n,
  stateVersion: "8",
  rememberPolicy: "unavailable",
};

const successfulInstall: PluginOperationSummary = {
  operationId: "019d0000-0000-7000-8000-000000000806",
  pluginId: packagePreview.pluginId,
  kind: "install",
  state: "succeeded",
  progressPercent: 100,
  errorCode: null,
  startedAtUnixMs: 1n,
  updatedAtUnixMs: 2n,
};

const catalogEntry = {
  pluginId: "dev.norishell.terminal-enhancer",
  name: "NoriShell Terminal Enhancer",
  publisher: "NoriShell",
  version: "1.3.0",
  protocolMajor: 1,
  protocolMinor: 3,
  platform: "desktop",
  architectures: ["universal"],
  packageUrl: "https://plugins.example.test/terminal-enhancer.zip",
  packageSize: 4096n,
  packageSha256: "e".repeat(64),
  publisherKeyBase64: "fixture",
  publisherSignatureBase64: "fixture",
  capabilities: ["uiPanel", "terminalMetadata", "terminalRequestInput", "sshSync"] as const,
  minimumAppVersion: "1.0.0",
  publishedAtUnixMs: 1_788_192_000_000n,
  compatibility: "compatible" as const,
};

const enabledPlugin = {
  pluginId: packagePreview.pluginId,
  name: packagePreview.name,
  publisher: packagePreview.author,
  artifactFingerprintSha256: "c".repeat(64),
  activeVersion: "1.1.0",
  packageSha256: "d".repeat(64),
  capabilities: packagePreview.capabilities,
  grants: packagePreview.capabilities.map((capability) => ({ capability, granted: false })),
  state: "enabled" as const,
  stateVersion: "5",
  installedAtUnixMs: 1n,
  updatedAtUnixMs: 2n,
};

const installedCatalogPlugin = {
  ...enabledPlugin,
  pluginId: catalogEntry.pluginId,
  name: catalogEntry.name,
  publisher: catalogEntry.publisher,
  activeVersion: "1.2.0",
  capabilities: [...catalogEntry.capabilities],
  grants: catalogEntry.capabilities.map((capability) => ({
    capability,
    granted: capability !== "terminalRequestInput" && capability !== "sshSync",
  })),
  state: "disabled" as const,
};

function buttonByText(text: string) {
  return Array.from(document.body.querySelectorAll<HTMLButtonElement>("button"))
    .find((button) => button.textContent?.trim() === text);
}

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
    global: { plugins: [createPinia(), router, i18n] },
  });
  await flushPromises();
  return wrapper;
}

async function openInstalledPlugins() {
  await buttonByText("Installed plugins")?.click();
  await flushPromises();
}

describe("PluginsView", () => {
  it("falls back after image errors in marketplace list, details and installed rows", async () => {
    client.listInstalledPlugins.mockResolvedValueOnce([installedCatalogPlugin]);
    const wrapper = await mountPlugins();
    const native = vi.spyOn(tauriCore, "isTauri").mockReturnValue(true);
    try {
      const icons = usePluginIconsStore();
      const source = "data:image/png;base64,broken";
      client.readPluginIcon.mockResolvedValue({ dataUrl: source });
      await icons.load(catalogEntry.pluginId, "catalog", "1");
      await flushPromises();
      const listImage = wrapper.get(".marketplace-plugin__icon img");
      expect(listImage.attributes("src")).toBe(source);
      await listImage.trigger("error");
      expect(wrapper.get(".marketplace-plugin__icon img").attributes("src")).toContain("ssh-toolkit");
      await icons.load(catalogEntry.pluginId, "catalog", "1", true);
      await flushPromises();
      const detailImage = wrapper.findAll(".marketplace-inspector img").find((img) => img.attributes("src") === source);
      expect(detailImage).toBeDefined();
      await detailImage!.trigger("error");
      expect(icons.imageFor(catalogEntry.pluginId, "catalog")).toBeUndefined();
      await openInstalledPlugins();
      await icons.load(catalogEntry.pluginId, "installed", "hash");
      await flushPromises();
      await wrapper.get(".plugin-table__icon img").trigger("error");
      expect(wrapper.find(".plugin-table__icon img").exists()).toBe(false);
      expect(wrapper.find(".plugin-table__icon svg").exists()).toBe(true);
    } finally {
      native.mockRestore();
      client.readPluginIcon.mockResolvedValue({ dataUrl: null });
    }
  });

  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    client.prepareLocalPluginPackage.mockResolvedValue(packagePreview);
    client.cancelLocalPluginPackage.mockResolvedValue(undefined);
    client.openPluginSpecialPermission.mockResolvedValue(undefined);
    client.prepareCatalogPluginPackage.mockImplementation(async (input: { pluginId: string; version: string; expectedStateVersion: string | null }) => {
      const snapshot = await client.fetchPluginCatalogSnapshot();
      const entry = snapshot.entries.find((item: { pluginId: string }) => item.pluginId === input.pluginId);
      return { ...packagePreview, pluginId: entry.pluginId, name: entry.name, author: entry.publisher,
        version: entry.version, packageSha256: entry.packageSha256, capabilities: [...entry.capabilities],
        currentVersion: input.expectedStateVersion ? installedCatalogPlugin.activeVersion : null,
        currentStateVersion: input.expectedStateVersion, publisherVerified: true,
        retainedCapabilityGrants: entry.retainedCapabilityGrants ?? [],
      };
    });
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "fixture-1",
      verifiedAtUnixMs: 1_788_192_000_000n,
      entries: [catalogEntry],
    });
    client.listInstalledPlugins.mockResolvedValue([]);
    client.listPluginContributions.mockResolvedValue([{
      pluginId: packagePreview.pluginId,
      pluginName: packagePreview.name,
      artifactFingerprintSha256: "c".repeat(64),
      packageSha256: "d".repeat(64),
      instanceGeneration: "1",
      stateVersion: "3",
      contributionRevision: "1",
      slot: "pluginsPage",
      nodes: [
        { kind: "text", text: "<b>literal plugin text</b>" },
        { kind: "status", label: "Ready", tone: "success" },
        { kind: "action", actionId: "generateUuid", label: "Generate UUID" },
      ],
    }]);
    client.listPluginAudit.mockResolvedValue([]);
    client.listPluginOperationPermissions.mockResolvedValue({ permissions: [], policyRevision: null });
    client.revokePluginOperationPermission.mockResolvedValue(undefined);
    client.clearPluginOperationPermissions.mockResolvedValue(undefined);
    client.listPendingPluginTerminalInput.mockResolvedValue([proposal]);
    client.getPluginReadiness.mockResolvedValue({
      ready: true,
      trustedRootCount: 0,
      protocolMajor: 1,
      protocolMinor: 3,
      safeModeActive: false,
      safeModeNextStart: false,
    });
    client.installLocalPlugin.mockResolvedValue(successfulInstall);
    client.installCatalogPlugin.mockResolvedValue(successfulInstall);
    client.refreshPluginCatalog.mockResolvedValue({
      ...successfulInstall,
      kind: "catalogRefresh",
      pluginId: null,
    });
    client.invokePluginContribution.mockResolvedValue({
      pluginId: packagePreview.pluginId,
      pluginName: packagePreview.name,
      artifactFingerprintSha256: "c".repeat(64),
      packageSha256: "d".repeat(64),
      instanceGeneration: "1",
      stateVersion: "3",
      contributionRevision: "2",
      slot: "pluginsPage",
      nodes: [
        { kind: "text", text: "UUID: 019d0000-0000-7000-8000-000000000900" },
        { kind: "action", actionId: "generateUuid", label: "Generate UUID" },
      ],
    });
    client.openPluginTerminalInput.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows the catalog failure reason and retries the online refresh", async () => {
    const wrapper = await mountPlugins();
    const store = usePluginsStore();
    store.catalogErrorCode = "catalogSignatureInvalid";
    await flushPromises();
    const notice = wrapper.get(".marketplace-state");
    expect(notice.text()).toContain("catalog signature could not be verified");
    expect(notice.text()).not.toContain("Check the network");
    client.refreshPluginCatalog.mockClear();
    await notice.get("button").trigger("click");
    await flushPromises();
    expect(client.refreshPluginCatalog).toHaveBeenCalledOnce();
    expect(store.catalogErrorCode).toBeNull();
  });

  it("removes the details column when closed and restores it on selection", async () => {
    const wrapper = await mountPlugins();
    await wrapper.get(".marketplace-inspector__close").trigger("click");
    expect(wrapper.find(".marketplace-inspector").exists()).toBe(false);
    expect(wrapper.get(".plugin-marketplace").classes()).toContain("plugin-marketplace--without-inspector");
    await wrapper.get(".marketplace-plugin").trigger("click");
    expect(wrapper.find(".marketplace-inspector").exists()).toBe(true);
    expect(wrapper.get(".plugin-marketplace").classes()).not.toContain("plugin-marketplace--without-inspector");
    wrapper.unmount();
  });

  it("opens the marketplace by default and switches to installed plugin management", async () => {
    const wrapper = await mountPlugins();

    expect(wrapper.text()).toContain("Plugin Marketplace");
    expect(wrapper.text()).toContain("NoriShell Terminal Enhancer");
    expect(wrapper.text()).toContain("Ordinary capabilities");
    expect(wrapper.text()).toContain("Terminal metadata");
    expect(wrapper.text()).toContain("Special permissions");
    expect(wrapper.text()).toContain("Request terminal input");
    expect(wrapper.text()).not.toContain("Add a terminal side panel");
    expect(wrapper.text()).not.toContain("New: tab groups and workspace persistence");
    expect(wrapper.text()).toContain("The publisher has not provided release notes for this version.");
    expect(buttonByText("Install")?.disabled).toBe(false);
    const selectedMarketPlugin = wrapper.find(".marketplace-plugin--selected");
    expect(selectedMarketPlugin.find(".marketplace-plugin__icon img").attributes("src")).toContain("ssh-toolkit");
    expect(selectedMarketPlugin.findAll(".marketplace-plugin__platforms img")).toHaveLength(3);
    expect(selectedMarketPlugin.text()).not.toContain("Mac Win Linux");
    expect(wrapper.text()).not.toContain("NoriShell does not verify author identity");
    expect(wrapper.text()).not.toContain("Review Request");

    await openInstalledPlugins();

    expect(wrapper.text()).toContain("Installed plugins");
    expect(wrapper.text()).toContain("Review Request");
    wrapper.unmount();
  });

  it("opens the native ZIP flow and confirms capabilities before local install", async () => {
    const wrapper = await mountPlugins();

    await buttonByText("Import ZIP")?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("Local Example");
    expect(document.body.textContent).toContain("Example Author");
    expect(document.body.textContent).toContain(packagePreview.packageSha256);
    await buttonByText("Confirm Install")?.click();
    await flushPromises();

    expect(client.installLocalPlugin).toHaveBeenCalledWith({
      preparationId: packagePreview.preparationId,
      expectedPackageSha256: packagePreview.packageSha256,
      expectedStateVersion: packagePreview.currentStateVersion,
      capabilityGrants: [
        { capability: "uiPanel", granted: false },
        { capability: "terminalRequestInput", granted: false },
      ],
    });
    wrapper.unmount();
  });

  it.each(["remoteInspect", "networkDomain", "localFiles", "deviceSerial", "terminalProvider", "credentialsPlugin", "sftpWrite"] as const)("requires a matching protected decision before granting %s at install", async (capability) => {
    const preview = { ...packagePreview, capabilities: ["uiPanel", capability] as const };
    client.prepareLocalPluginPackage.mockResolvedValue(preview);
    const wrapper = await mountPlugins();
    await buttonByText("Import ZIP")?.click();
    await flushPromises();
    const checkbox = document.body.querySelectorAll<HTMLInputElement>('[role="dialog"] input[type="checkbox"]')[1]!;
    expect(checkbox.disabled).toBe(false);
    checkbox.click();
    await flushPromises();
    expect(checkbox.checked).toBe(false);
    expect(client.openPluginSpecialPermission).toHaveBeenCalledWith({
      target: { kind: "preparedPackage", preparationId: preview.preparationId,
        expectedPackageSha256: preview.packageSha256, expectedStateVersion: null },
      requestedCapability: capability,
    });
    expect(buttonByText("Confirm Install")?.disabled).toBe(true);
    const approved = { ...preview, capabilities: [...preview.capabilities],
      approvedSpecialGrants: [{ capability, granted: true }],
      specialPermissionExpiresAtUnixMs: BigInt(Date.now() + 60_000) };
    const store = usePluginsStore();
    const mismatched = { kind: "preparedPackage" as const, preview: { ...approved, packageSha256: "f".repeat(64) } };
    store.applySpecialPermissionOutcome(mismatched);
    window.dispatchEvent(new CustomEvent("norishell:plugin-special-permission-changed", { detail: mismatched }));
    await flushPromises();
    expect(checkbox.checked).toBe(false);
    expect(buttonByText("Confirm Install")?.disabled).toBe(true);
    const decision = { kind: "preparedPackage" as const, preview: approved };
    store.applySpecialPermissionOutcome(decision);
    window.dispatchEvent(new CustomEvent("norishell:plugin-special-permission-changed", { detail: decision }));
    await flushPromises();
    expect(checkbox.checked).toBe(true);
    expect(buttonByText("Confirm Install")?.disabled).toBe(false);
    await buttonByText("Confirm Install")?.click();
    await flushPromises();
    expect(client.installLocalPlugin).toHaveBeenCalledWith(expect.objectContaining({ capabilityGrants: [
      { capability: "uiPanel", granted: false }, { capability, granted: true },
    ] }));
    wrapper.unmount();
  });

  it("removes expired approval from the draft and requires confirmation again", async () => {
    vi.useFakeTimers();
    client.prepareLocalPluginPackage.mockResolvedValue({ ...packagePreview,
      capabilities: ["uiPanel", "remoteInspect"],
      approvedSpecialGrants: [{ capability: "remoteInspect", granted: true }],
      specialPermissionExpiresAtUnixMs: BigInt(Date.now() + 1000),
    });
    const wrapper = await mountPlugins();
    await buttonByText("Import ZIP")?.click();
    await flushPromises();
    const checkbox = document.body.querySelectorAll<HTMLInputElement>('[role="dialog"] input[type="checkbox"]')[1]!;
    expect(checkbox.checked).toBe(true);
    vi.advanceTimersByTime(1001);
    await flushPromises();
    expect(checkbox.checked).toBe(false);
    expect(buttonByText("Confirm Install")?.disabled).toBe(true);
    expect(document.body.textContent).toContain("approval has expired");
    expect(client.installLocalPlugin).not.toHaveBeenCalled();
    checkbox.click();
    await flushPromises();
    const store = usePluginsStore();
    const cancelled = { kind: "preparedPackage" as const, preview: { ...store.preparedPackage!,
      approvedSpecialGrants: [],
    } };
    store.applySpecialPermissionOutcome(cancelled);
    window.dispatchEvent(new CustomEvent("norishell:plugin-special-permission-changed", { detail: cancelled }));
    await flushPromises();
    expect(buttonByText("Confirm Install")?.disabled).toBe(true);
    wrapper.unmount();
  });

  it("discards the consumed preparation after a failed installation", async () => {
    client.installLocalPlugin.mockResolvedValue({ ...successfulInstall, state: "failed", errorCode: "installConflict" });
    const wrapper = await mountPlugins();
    await buttonByText("Import ZIP")?.click();
    await flushPromises();
    await buttonByText("Confirm Install")?.click();
    await flushPromises();
    expect(usePluginsStore().preparedPackage).toBeNull();
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    wrapper.unmount();
  });

  it("prepares a verified catalog package and waits for the shared installation confirmation", async () => {
    const wrapper = await mountPlugins();
    await buttonByText("Install")?.click();
    await flushPromises();
    expect(client.prepareCatalogPluginPackage).toHaveBeenCalledWith({
      pluginId: catalogEntry.pluginId, version: catalogEntry.version, expectedStateVersion: null,
    });
    expect(client.installLocalPlugin).not.toHaveBeenCalled();
    expect(client.installCatalogPlugin).not.toHaveBeenCalled();
    await buttonByText("Confirm Install")?.click();
    await flushPromises();
    expect(client.installLocalPlugin).toHaveBeenCalledWith({
      preparationId: packagePreview.preparationId, expectedPackageSha256: catalogEntry.packageSha256,
      expectedStateVersion: null,
      capabilityGrants: catalogEntry.capabilities.map((capability) => ({ capability, granted: false })),
    });
    wrapper.unmount();
  });

  it("does not fall back to the visual fixture when the real catalog fails", async () => {
    client.fetchPluginCatalogSnapshot.mockRejectedValueOnce(new Error("offline"));

    const wrapper = await mountPlugins();

    expect(wrapper.text()).toContain("The operation failed without a recognized reason.");
    expect(wrapper.text()).not.toContain("NoriShell Terminal Enhancer");
    expect(buttonByText("Retry")).toBeDefined();
    wrapper.unmount();
  });

  it("marks the current catalog version as already installed", async () => {
    client.listInstalledPlugins.mockResolvedValueOnce([{
      ...enabledPlugin,
      pluginId: catalogEntry.pluginId,
      name: catalogEntry.name,
      activeVersion: catalogEntry.version,
      packageSha256: catalogEntry.packageSha256,
    }]);

    const wrapper = await mountPlugins();

    expect(buttonByText("Installed")?.disabled).toBe(true);
    expect(wrapper.find(".marketplace-plugin__version").text()).toContain("Enabled");
    wrapper.unmount();
  });

  it("shows only the newest catalog version for each plugin", async () => {
    client.fetchPluginCatalogSnapshot.mockResolvedValueOnce({
      catalogRevision: "fixture-versions",
      verifiedAtUnixMs: 1_788_192_000_000n,
      entries: [
        { ...catalogEntry, version: "1.0.1", publishedAtUnixMs: 1_788_191_000_000n },
        catalogEntry,
      ],
    });

    const wrapper = await mountPlugins();

    expect(wrapper.findAll(".marketplace-plugin")).toHaveLength(1);
    expect(wrapper.find(".marketplace-plugin__version").text()).toContain(catalogEntry.version);
    expect(wrapper.text()).not.toContain("1.0.1");
    wrapper.unmount();
  });

  it("shows Update in the marketplace only for a newer catalog version", async () => {
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);

    const wrapper = await mountPlugins();

    expect(buttonByText("Update")?.disabled).toBe(false);
    await buttonByText("Update")?.click();
    await flushPromises();
    expect(client.installCatalogPlugin).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain("Confirm update");
    await buttonByText("Confirm Update")?.click();
    await flushPromises();
    expect(client.installLocalPlugin).toHaveBeenCalledWith({
      preparationId: packagePreview.preparationId,
      expectedPackageSha256: catalogEntry.packageSha256,
      expectedStateVersion: installedCatalogPlugin.stateVersion,
      capabilityGrants: [
        { capability: "uiPanel", granted: false },
        { capability: "terminalMetadata", granted: false },
        { capability: "terminalRequestInput", granted: false },
        { capability: "sshSync", granted: false },
      ],
    });
    wrapper.unmount();
  });

  it("shows Update in the installed list and preserves existing grants", async () => {
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);

    const wrapper = await mountPlugins();
    await openInstalledPlugins();

    expect(buttonByText("Update")?.disabled).toBe(false);
    await buttonByText("Update")?.click();
    await flushPromises();
    expect(client.installCatalogPlugin).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain("Confirm update");
    await buttonByText("Confirm Update")?.click();
    await flushPromises();
    expect(client.installLocalPlugin).toHaveBeenCalledWith({
      preparationId: packagePreview.preparationId,
      expectedPackageSha256: catalogEntry.packageSha256,
      expectedStateVersion: installedCatalogPlugin.stateVersion,
      capabilityGrants: catalogEntry.capabilities.map((capability) => ({ capability, granted: false })),
    });
    wrapper.unmount();
  });

  it("shows Core-retained grants without asking again and reviews only additions", async () => {
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "retained-grants",
      verifiedAtUnixMs: 1_788_192_000_000n,
      entries: [{
        ...catalogEntry,
        retainedCapabilityGrants: [
          { capability: "uiPanel", granted: true },
          { capability: "sshSync", granted: true },
          { capability: "terminalMetadata", granted: false },
        ],
      }],
    });
    const wrapper = await mountPlugins();
    await buttonByText("Update")?.click();
    await flushPromises();
    const dialog = document.body.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain("No repeat approval is needed");
    expect(dialog?.textContent).toContain("Allowed · retained");
    expect(dialog?.textContent).toContain("Not allowed · retained");
    expect(dialog?.querySelectorAll('input[type="checkbox"]')).toHaveLength(1);
    await buttonByText("Confirm Update")?.click();
    await flushPromises();
    // Core revalidates and migrates existing grants; the main window cannot mint special permissions through an update request.
    expect(client.installLocalPlugin.mock.calls[0]?.[0].capabilityGrants)
      .toEqual(catalogEntry.capabilities.map((capability) => ({ capability, granted: false })));
    wrapper.unmount();
  });

  it("opens the protected terminal approval window without rendering payload in main", async () => {
    const wrapper = await mountPlugins();
    await openInstalledPlugins();

    await buttonByText("Review Request")?.click();
    await flushPromises();
    expect(document.body.textContent).not.toContain(proposal.payload);
    expect(client.openPluginTerminalInput).toHaveBeenCalledWith({
      approvalId: proposal.approvalId,
      expectedStateVersion: proposal.stateVersion,
    });
    wrapper.unmount();
  });

  it("keeps an unchanged update free of capability checkboxes", async () => {
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "unchanged-grants",
      verifiedAtUnixMs: 1_788_192_000_000n,
      entries: [{ ...catalogEntry, retainedCapabilityGrants: installedCatalogPlugin.grants }],
    });
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update")?.click();
    await flushPromises();
    const dialog = document.body.querySelector('[role="dialog"]');
    expect(dialog?.querySelectorAll('input[type="checkbox"]')).toHaveLength(0);
    expect(dialog?.textContent).toContain("No repeat approval is needed");
    expect(dialog?.textContent).not.toContain("Capabilities are denied by default");
    wrapper.unmount();
  });

  it("renders declarative contribution text through host components", async () => {
    const wrapper = await mountPlugins();
    await openInstalledPlugins();

    expect(wrapper.text()).toContain("<b>literal plugin text</b>");
    expect(wrapper.text()).toContain("Ready");
    expect(document.body.querySelector("b")).toBeNull();
    wrapper.unmount();
  });

  it("invokes a host-rendered action with the exact contribution fence", async () => {
    const wrapper = await mountPlugins();
    await openInstalledPlugins();

    await buttonByText("Generate UUID")?.click();
    await flushPromises();

    expect(client.invokePluginContribution).toHaveBeenCalledWith({
      pluginId: packagePreview.pluginId,
      artifactFingerprintSha256: "c".repeat(64),
      expectedPackageSha256: "d".repeat(64),
      instanceGeneration: "1",
      expectedStateVersion: "3",
      expectedContributionRevision: "1",
      slot: "pluginsPage",
      actionId: "generateUuid",
    });
    expect(wrapper.text()).toContain("019d0000-0000-7000-8000-000000000900");
    wrapper.unmount();
  });

  it("updates an enabled plugin without uninstalling and restores its enabled state", async () => {
    vi.useFakeTimers();
    const updatePreview = {
      ...packagePreview,
      version: "1.2.0",
      currentVersion: enabledPlugin.activeVersion,
      currentStateVersion: enabledPlugin.stateVersion,
    };
    const disabledBeforeUpdate = { ...enabledPlugin, state: "disabled" as const, stateVersion: "6" };
    const installedUpdate = {
      ...disabledBeforeUpdate,
      activeVersion: updatePreview.version,
      packageSha256: updatePreview.packageSha256,
      stateVersion: "7",
    };
    const enabledUpdate = { ...installedUpdate, state: "enabled" as const, stateVersion: "8" };
    client.prepareLocalPluginPackage.mockResolvedValue(updatePreview);
    client.listInstalledPlugins
      .mockResolvedValueOnce([enabledPlugin])
      .mockResolvedValueOnce([installedUpdate]);
    client.disablePlugin.mockResolvedValue(disabledBeforeUpdate);
    client.installLocalPlugin.mockResolvedValue({ ...successfulInstall, kind: "update" });
    client.enablePlugin.mockResolvedValue(enabledUpdate);

    const wrapper = await mountPlugins();
    await buttonByText("Import ZIP")?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("1.1.0 → 1.2.0");
    await buttonByText("Confirm Update")?.click();
    await flushPromises();

    expect(client.disablePlugin).toHaveBeenCalledWith({
      pluginId: enabledPlugin.pluginId,
      expectedStateVersion: "5",
    });
    expect(client.installLocalPlugin).toHaveBeenCalledWith({
      preparationId: updatePreview.preparationId,
      expectedPackageSha256: updatePreview.packageSha256,
      expectedStateVersion: "6",
      capabilityGrants: updatePreview.capabilities.map((capability) => ({ capability, granted: false })),
    });
    expect(client.enablePlugin).toHaveBeenCalledWith({
      pluginId: enabledPlugin.pluginId,
      expectedStateVersion: "7",
    });
    expect(client.uninstallPlugin).not.toHaveBeenCalled();
    expect(useTipsStore().items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "success", title: "Plugin state updated" }),
    ]));

    vi.advanceTimersByTime(4_000);
    await flushPromises();
    expect(useTipsStore().items).toEqual([]);
    expect(wrapper.text()).not.toContain("Operation status");
    wrapper.unmount();
  });

  it("restores the old enabled plugin when an update fails", async () => {
    const updatePreview = {
      ...packagePreview,
      currentVersion: enabledPlugin.activeVersion,
      currentStateVersion: enabledPlugin.stateVersion,
    };
    const disabledBeforeUpdate = { ...enabledPlugin, state: "disabled" as const, stateVersion: "6" };
    client.prepareLocalPluginPackage.mockResolvedValue(updatePreview);
    client.listInstalledPlugins.mockResolvedValue([enabledPlugin]);
    client.disablePlugin.mockResolvedValue(disabledBeforeUpdate);
    client.installLocalPlugin.mockResolvedValue({
      ...successfulInstall,
      kind: "update",
      state: "failed",
      errorCode: "installConflict",
    });
    client.enablePlugin.mockResolvedValue({ ...enabledPlugin, stateVersion: "7" });

    const wrapper = await mountPlugins();
    await buttonByText("Import ZIP")?.click();
    await flushPromises();
    await buttonByText("Confirm Update")?.click();
    await flushPromises();

    expect(client.enablePlugin).toHaveBeenCalledWith({
      pluginId: enabledPlugin.pluginId,
      expectedStateVersion: "6",
    });
    expect(client.uninstallPlugin).not.toHaveBeenCalled();
    expect(useTipsStore().items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "error", title: "Plugin operation did not complete" }),
    ]));
    wrapper.unmount();
  });
  it("opens declared settings from a disabled installed plugin without enabling it", async () => {
    client.listInstalledPlugins.mockResolvedValue([{ ...installedCatalogPlugin, hasSettings: true }]);
    client.getPluginSettings.mockResolvedValue(null);
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await wrapper.get('button[aria-label="More plugin actions"]').trigger("click");
    await buttonByText("Settings")?.click();
    await flushPromises();
    expect(client.getPluginSettings).toHaveBeenCalledWith({ pluginId: installedCatalogPlugin.pluginId });
    expect(document.body.textContent).toContain("This plugin does not declare any settings.");
    expect(client.enablePlugin).not.toHaveBeenCalled();
    expect(client.disablePlugin).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("opens remembered operation approvals from an installed plugin's existing action menu", async () => {
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    client.listPluginOperationPermissions.mockResolvedValue({
      policyRevision: "policy-1",
      permissions: [{
        permissionId: "permission-1", pluginId: installedCatalogPlugin.pluginId, operation: "remoteExecute",
        actionLabel: "Restart Nginx", targetLabel: "Production", createdAtUnixMs: 1n,
        expiresAtUnixMs: null,
      }],
    });
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await wrapper.get('button[aria-label="More plugin actions"]').trigger("click");
    await buttonByText("Operation approvals")?.click();
    await flushPromises();
    expect(client.listPluginOperationPermissions).toHaveBeenCalledWith(installedCatalogPlugin.pluginId);
    expect(document.body.textContent).toContain("Restart Nginx");
    wrapper.unmount();
  });

  it("opens real catalog versions and prepares the explicitly selected compatible release", async () => {
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "versions", verifiedAtUnixMs: 1n,
      entries: [catalogEntry, { ...catalogEntry, version: "1.0.1", capabilities: ["uiPanel"] }],
    });
    const wrapper = await mountPlugins();
    await wrapper.get(".marketplace-version-link").trigger("click");
    expect(wrapper.findAll(".marketplace-version")).toHaveLength(2);
    const oldRelease = wrapper.findAll(".marketplace-version").find((item) => item.get("strong").text() === "1.0.1")!;
    await oldRelease.trigger("click");
    expect(oldRelease.attributes("aria-pressed")).toBe("true");
    await buttonByText("Install")?.click();
    await flushPromises();
    expect(client.prepareCatalogPluginPackage).toHaveBeenCalledWith({ pluginId: catalogEntry.pluginId, version: "1.0.1", expectedStateVersion: null });
    wrapper.unmount();
  });

  it("shows unknown capabilities without offering installation", async () => {
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "future-capability", verifiedAtUnixMs: 1n,
      entries: [{ ...catalogEntry, protocolMinor: 14, compatibility: "protocolIncompatible",
        unsupportedCapabilities: ["futureHardwareAccess"] }],
    });
    const wrapper = await mountPlugins();
    expect(wrapper.get(".marketplace-inspector").text()).toContain("futureHardwareAccess");
    expect(wrapper.text()).toContain("Capabilities requiring a newer app");
    const install = buttonByText("Install");
    expect(!install || install.disabled).toBe(true);
    expect(client.prepareCatalogPluginPackage).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("renders signed release metadata and changes the details with the selected version", async () => {
    const details = {
      description: "Connect the account and synchronize SSH profiles.",
      releaseNotes: ["<b>Publisher release note</b>"],
      extensionTargets: ["app.navigation", "app.page", "future.target"],
      releasePublishedAtUnixMs: 1_788_192_000_000n,
    };
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "signed-details", verifiedAtUnixMs: 1n,
      entries: [{ ...catalogEntry, details }, {
        ...catalogEntry, version: "1.0.1", publishedAtUnixMs: 0n,
        details: { ...details, description: "Earlier account integration.", releaseNotes: ["Initial account release."], releasePublishedAtUnixMs: null },
      }],
    });
    const wrapper = await mountPlugins();
    expect(wrapper.get(".marketplace-inspector__description").text()).toBe(details.description);
    expect(wrapper.get(".marketplace-release-list").text()).toBe(details.releaseNotes[0]);
    expect(wrapper.find(".marketplace-release-list b").exists()).toBe(false);
    expect(wrapper.get(".marketplace-target-list").text()).toContain("Add a navigation entry");
    expect(wrapper.get(".marketplace-target-list").text()).toContain("Provide a plugin page");
    expect(wrapper.get(".marketplace-target-list").text()).toContain("future.target");
    expect(wrapper.get(".marketplace-release-meta time").attributes("datetime")).toBe("2026-08-31");
    await wrapper.get(".marketplace-version-link").trigger("click");
    await wrapper.findAll(".marketplace-version").find((item) => item.get("strong").text() === "1.0.1")!.trigger("click");
    expect(wrapper.get(".marketplace-inspector__description").text()).toBe("Earlier account integration.");
    expect(wrapper.get(".marketplace-release-list").text()).toBe("Initial account release.");
    expect(wrapper.find(".marketplace-release-meta time").exists()).toBe(false);
    wrapper.unmount();
  });

  it("explains every declared permission and offers installed management from details", async () => {
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    const wrapper = await mountPlugins();
    const help = wrapper.get(".marketplace-learn-permissions");
    await help.trigger("click");
    expect(help.attributes("aria-expanded")).toBe("true");
    expect(wrapper.findAll(".marketplace-permission-description")).toHaveLength(catalogEntry.capabilities.length);
    expect(wrapper.find(".marketplace-permission-description").text()).not.toContain("plugins.capabilities");
    await buttonByText("Manage installed plugin")?.click();
    await flushPromises();
    expect(wrapper.find(".plugin-marketplace").exists()).toBe(false);
    expect(document.body.textContent).toContain(installedCatalogPlugin.name);
    wrapper.unmount();
  });

  it("keeps package preparation busy until the review preview arrives", async () => {
    let finishPreparation!: (preview: PluginLocalPackagePreview) => void;
    client.prepareCatalogPluginPackage.mockImplementationOnce(() => new Promise((resolve) => { finishPreparation = resolve; }));
    const wrapper = await mountPlugins();
    await buttonByText("Install")?.click();
    await flushPromises();
    expect(wrapper.get('button[aria-label="Preparing package…"]').attributes("disabled")).toBeDefined();
    expect(client.prepareCatalogPluginPackage).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).not.toContain("Confirm install");
    finishPreparation({ ...packagePreview, pluginId: catalogEntry.pluginId });
    await flushPromises();
    expect(wrapper.find('button[aria-label="Preparing package…"]').exists()).toBe(false);
    expect(document.body.textContent).toContain("Confirm install");
    wrapper.unmount();
  });

  it("distinguishes a same-version package mismatch and prevents a catalog downgrade", async () => {
    client.listInstalledPlugins.mockResolvedValue([{ ...installedCatalogPlugin, activeVersion: catalogEntry.version }]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "versions", verifiedAtUnixMs: 1n,
      entries: [catalogEntry, { ...catalogEntry, version: "1.0.1" }],
    });
    const wrapper = await mountPlugins();
    expect(buttonByText("Installed package differs")?.disabled).toBe(true);
    await wrapper.get(".marketplace-version-link").trigger("click");
    await wrapper.findAll(".marketplace-version").find((item) => item.get("strong").text() === "1.0.1")!.trigger("click");
    expect(buttonByText("Newer version installed")?.disabled).toBe(true);
    expect(client.prepareCatalogPluginPackage).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("keeps Norixor first in the marketplace and uses its built-in description only when the catalog omits one", async () => {
    const norixor = {
      ...catalogEntry,
      pluginId: "org.norixor",
      name: "Norixor Account",
      version: "1.0.0",
    };
    const later = { ...catalogEntry, pluginId: "dev.norishell.later", name: "Later plugin", version: "1.0.1" };
    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "norixor-first", verifiedAtUnixMs: 1n,
      entries: [catalogEntry, norixor, later],
    });

    const wrapper = await mountPlugins();
    expect(wrapper.findAll(".marketplace-plugin").map((item) => item.text())).toEqual(expect.arrayContaining([
      expect.stringContaining("Norixor Account"),
      expect.stringContaining("NoriShell Terminal Enhancer"),
      expect.stringContaining("Later plugin"),
    ]));
    expect(wrapper.findAll(".marketplace-plugin")[0]?.text()).toContain("Norixor Account");
    expect(wrapper.get(".marketplace-inspector__description").text()).toContain("Sign in to a Norixor account");
    wrapper.unmount();

    client.fetchPluginCatalogSnapshot.mockResolvedValue({
      catalogRevision: "norixor-description", verifiedAtUnixMs: 1n,
      entries: [{ ...norixor, details: { description: "Publisher supplied description", releaseNotes: [], extensionTargets: [], releasePublishedAtUnixMs: null } }, catalogEntry, later],
    });
    const described = await mountPlugins();
    expect(described.get(".marketplace-inspector__description").text()).toBe("Publisher supplied description");
    described.unmount();
  });

  it("keeps the installed Norixor plugin first and includes its product description", async () => {
    const norixor = { ...installedCatalogPlugin, pluginId: "org.norixor", name: "Norixor Account" };
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin, norixor]);
    const wrapper = await mountPlugins();
    await openInstalledPlugins();

    expect(wrapper.findAll(".plugin-table__row")[0]?.text()).toContain("Norixor Account");
    expect(wrapper.text()).toContain("Sign in to a Norixor account");
    wrapper.unmount();
  });

  it("uses tips for an uninstall result without rendering an operation status card", async () => {
    client.listInstalledPlugins.mockResolvedValue([enabledPlugin]);
    client.uninstallPlugin.mockResolvedValue({ ...successfulInstall, kind: "uninstall" });
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await wrapper.get('button[aria-label="More plugin actions"]').trigger("click");
    await buttonByText("Uninstall")?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("Also delete plugin data");
    await buttonByText("Uninstall Plugin")?.click();
    await flushPromises();

    expect(useTipsStore().items).toEqual(expect.arrayContaining([
      expect.objectContaining({ tone: "success", message: "Uninstalled Local Example." }),
    ]));
    expect(wrapper.text()).not.toContain("Operation status");
    wrapper.unmount();
  });

  it("updates the frozen compatible candidates in order with retained grants", async () => {
    const secondPlugin = { ...installedCatalogPlugin, pluginId: "dev.norishell.second", name: "Second plugin", activeVersion: "1.0.0" };
    const secondEntry = { ...catalogEntry, pluginId: secondPlugin.pluginId, name: secondPlugin.name, version: "1.1.0" };
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin, secondPlugin]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({ catalogRevision: "batch-success", verifiedAtUnixMs: 1n, entries: [catalogEntry, secondEntry] });
    client.prepareCatalogPluginPackage.mockImplementation(async (input: { pluginId: string; version: string; expectedStateVersion: string | null }) => {
      const plugin = input.pluginId === secondPlugin.pluginId ? secondPlugin : installedCatalogPlugin;
      const entry = input.pluginId === secondPlugin.pluginId ? secondEntry : catalogEntry;
      return { ...packagePreview, pluginId: plugin.pluginId, name: plugin.name, author: plugin.publisher,
        version: entry.version, packageSha256: entry.packageSha256, capabilities: [...plugin.capabilities],
        currentVersion: plugin.activeVersion, currentStateVersion: plugin.stateVersion,
        retainedCapabilityGrants: plugin.grants, publisherVerified: true };
    });
    client.installLocalPlugin.mockImplementation(async (input: { preparationId: string }) => ({
      ...successfulInstall, operationId: `${input.preparationId}-operation`, pluginId: input.preparationId.includes("800") ? installedCatalogPlugin.pluginId : secondPlugin.pluginId, kind: "update",
    }));

    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (2)")?.click();
    await flushPromises();

    expect(client.prepareCatalogPluginPackage.mock.calls.map((call) => call[0].pluginId)).toEqual([installedCatalogPlugin.pluginId, secondPlugin.pluginId]);
    expect(client.installLocalPlugin).toHaveBeenCalledTimes(2);
    expect(client.installLocalPlugin.mock.calls[0]?.[0].capabilityGrants).toEqual(installedCatalogPlugin.grants);
    expect(client.installLocalPlugin.mock.calls[1]?.[0].capabilityGrants).toEqual(secondPlugin.grants);
    expect(wrapper.text()).toContain("2 updated · 0 failed · 0 skipped");
    wrapper.unmount();
  });

  it("pauses the batch for changed capabilities, skips a cancelled item, and continues", async () => {
    const changed = { ...installedCatalogPlugin, pluginId: "dev.norishell.changed", name: "Changed plugin", activeVersion: "1.0.0", capabilities: ["uiPanel", "terminalMetadata"] as const };
    const secondPlugin = { ...installedCatalogPlugin, pluginId: "dev.norishell.second", name: "Second plugin", activeVersion: "1.0.0" };
    const changedEntry = { ...catalogEntry, pluginId: changed.pluginId, name: changed.name, version: "1.1.0", capabilities: [...changed.capabilities] };
    const secondEntry = { ...catalogEntry, pluginId: secondPlugin.pluginId, name: secondPlugin.name, version: "1.1.0" };
    client.listInstalledPlugins.mockResolvedValue([changed, secondPlugin]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({ catalogRevision: "batch-permissions", verifiedAtUnixMs: 1n, entries: [changedEntry, secondEntry] });
    client.prepareCatalogPluginPackage.mockImplementation(async (input: { pluginId: string }) => {
      const plugin = input.pluginId === changed.pluginId ? changed : secondPlugin;
      const entry = input.pluginId === changed.pluginId ? changedEntry : secondEntry;
      return { ...packagePreview, preparationId: `${packagePreview.preparationId}-${input.pluginId}`, pluginId: plugin.pluginId, name: plugin.name,
        version: entry.version, packageSha256: entry.packageSha256, capabilities: [...plugin.capabilities], currentVersion: plugin.activeVersion,
        currentStateVersion: plugin.stateVersion, publisherVerified: true,
        retainedCapabilityGrants: input.pluginId === changed.pluginId ? [{ capability: "uiPanel", granted: false }] : plugin.grants };
    });

    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (2)")?.click();
    await flushPromises();
    expect(document.body.textContent).toContain("Confirm update");
    await buttonByText("Cancel")?.click();
    await flushPromises();

    expect(client.installLocalPlugin).toHaveBeenCalledTimes(1);
    expect(client.installLocalPlugin.mock.calls[0]?.[0].expectedPackageSha256).toBe(secondEntry.packageSha256);
    expect(wrapper.text()).toContain("1 updated · 0 failed · 1 skipped");
    wrapper.unmount();
  });

  it("continues after a preparation failure and keeps the localized failure in the batch summary", async () => {
    const secondPlugin = { ...installedCatalogPlugin, pluginId: "dev.norishell.second", name: "Second plugin", activeVersion: "1.0.0" };
    const secondEntry = { ...catalogEntry, pluginId: secondPlugin.pluginId, name: secondPlugin.name, version: "1.1.0" };
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin, secondPlugin]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({ catalogRevision: "batch-failure", verifiedAtUnixMs: 1n, entries: [catalogEntry, secondEntry] });
    client.prepareCatalogPluginPackage
      .mockRejectedValueOnce({ errorCode: "publisherChanged" })
      .mockResolvedValueOnce({ ...packagePreview, pluginId: secondPlugin.pluginId, name: secondPlugin.name, version: secondEntry.version,
        packageSha256: secondEntry.packageSha256, capabilities: [...secondPlugin.capabilities], currentVersion: secondPlugin.activeVersion,
        currentStateVersion: secondPlugin.stateVersion, publisherVerified: true, retainedCapabilityGrants: secondPlugin.grants });
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (2)")?.click();
    await flushPromises();

    expect(client.prepareCatalogPluginPackage).toHaveBeenCalledTimes(2);
    expect(client.installLocalPlugin).toHaveBeenCalledTimes(1);
    expect(wrapper.text()).toContain("1 updated · 1 failed · 0 skipped");
    expect(wrapper.text()).toContain("This package has a different signing identity from the installed plugin.");
    wrapper.unmount();
  });

  it("stops after an in-flight preparation returns and discards it without installing", async () => {
    let finishPreparation!: (preview: PluginLocalPackagePreview) => void;
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    client.prepareCatalogPluginPackage.mockImplementationOnce(() => new Promise((resolve) => { finishPreparation = resolve; }));
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (1)")?.click();
    await flushPromises();
    await buttonByText("Stop remaining updates")?.click();
    finishPreparation({ ...packagePreview, pluginId: installedCatalogPlugin.pluginId, name: installedCatalogPlugin.name,
      version: catalogEntry.version, packageSha256: catalogEntry.packageSha256, capabilities: [...installedCatalogPlugin.capabilities],
      currentVersion: installedCatalogPlugin.activeVersion, currentStateVersion: installedCatalogPlugin.stateVersion,
      retainedCapabilityGrants: installedCatalogPlugin.grants, publisherVerified: true });
    await flushPromises();

    expect(client.cancelLocalPluginPackage).toHaveBeenCalledWith(expect.any(String));
    expect(client.installLocalPlugin).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("does not advance a batch after the plugins page unmounts", async () => {
    let finishPreparation!: (preview: PluginLocalPackagePreview) => void;
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    client.prepareCatalogPluginPackage.mockImplementationOnce(() => new Promise((resolve) => { finishPreparation = resolve; }));
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (1)")?.click();
    await flushPromises();
    wrapper.unmount();
    finishPreparation({ ...packagePreview, pluginId: installedCatalogPlugin.pluginId, name: installedCatalogPlugin.name,
      version: catalogEntry.version, packageSha256: catalogEntry.packageSha256, capabilities: [...installedCatalogPlugin.capabilities],
      currentVersion: installedCatalogPlugin.activeVersion, currentStateVersion: installedCatalogPlugin.stateVersion,
      retainedCapabilityGrants: installedCatalogPlugin.grants, publisherVerified: true });
    await flushPromises();

    expect(client.cancelLocalPluginPackage).toHaveBeenCalledWith(expect.any(String));
    expect(client.installLocalPlugin).not.toHaveBeenCalled();
  });

  it("does not expose the capability confirmation while a retained-grant batch install is in flight", async () => {
    let finishInstall!: (operation: PluginOperationSummary) => void;
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin]);
    client.prepareCatalogPluginPackage.mockResolvedValue({ ...packagePreview, pluginId: installedCatalogPlugin.pluginId, name: installedCatalogPlugin.name,
      version: catalogEntry.version, packageSha256: catalogEntry.packageSha256, capabilities: [...installedCatalogPlugin.capabilities],
      currentVersion: installedCatalogPlugin.activeVersion, currentStateVersion: installedCatalogPlugin.stateVersion,
      retainedCapabilityGrants: installedCatalogPlugin.grants, publisherVerified: true });
    client.installLocalPlugin.mockImplementationOnce(() => new Promise((resolve) => { finishInstall = resolve; }));
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (1)")?.click();
    await flushPromises();

    expect(client.installLocalPlugin).toHaveBeenCalledTimes(1);
    expect(buttonByText("Confirm Update")).toBeUndefined();
    finishInstall({ ...successfulInstall, kind: "update" });
    await flushPromises();
    expect(client.installLocalPlugin).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("continues after a definite installation failure but disables all-updates when nothing is newer", async () => {
    const secondPlugin = { ...installedCatalogPlugin, pluginId: "dev.norishell.second", name: "Second plugin", activeVersion: "1.0.0" };
    const secondEntry = { ...catalogEntry, pluginId: secondPlugin.pluginId, name: secondPlugin.name, version: "1.1.0" };
    client.listInstalledPlugins.mockResolvedValue([installedCatalogPlugin, secondPlugin]);
    client.fetchPluginCatalogSnapshot.mockResolvedValue({ catalogRevision: "batch-install-failure", verifiedAtUnixMs: 1n, entries: [catalogEntry, secondEntry] });
    client.prepareCatalogPluginPackage.mockImplementation(async (input: { pluginId: string }) => {
      const plugin = input.pluginId === secondPlugin.pluginId ? secondPlugin : installedCatalogPlugin;
      const entry = input.pluginId === secondPlugin.pluginId ? secondEntry : catalogEntry;
      return { ...packagePreview, preparationId: `${packagePreview.preparationId}-${input.pluginId}`, pluginId: plugin.pluginId, name: plugin.name,
        version: entry.version, packageSha256: entry.packageSha256, capabilities: [...plugin.capabilities], currentVersion: plugin.activeVersion,
        currentStateVersion: plugin.stateVersion, publisherVerified: true, retainedCapabilityGrants: plugin.grants };
    });
    client.installLocalPlugin.mockRejectedValueOnce({ errorCode: "installConflict" }).mockResolvedValueOnce({ ...successfulInstall, kind: "update" });
    const wrapper = await mountPlugins();
    await openInstalledPlugins();
    await buttonByText("Update all (2)")?.click();
    await flushPromises();

    expect(client.installLocalPlugin).toHaveBeenCalledTimes(2);
    expect(wrapper.text()).toContain("1 updated · 1 failed · 0 skipped");
    wrapper.unmount();

    client.listInstalledPlugins.mockResolvedValue([{ ...installedCatalogPlugin, activeVersion: catalogEntry.version }]);
    const current = await mountPlugins();
    await openInstalledPlugins();
    expect(buttonByText("Update all (0)")?.disabled).toBe(true);
    expect(client.prepareCatalogPluginPackage).toHaveBeenCalledTimes(2);
    current.unmount();
  });

});
