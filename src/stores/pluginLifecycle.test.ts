import { createPinia, disposePinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { disablePlugin, enablePlugin, listPluginUiContributions, replacePluginCapabilityGrants } from "../core-api/client";
import type { InstalledPluginSummary } from "../core-api/generated/core-api";
import { usePluginExtensionsStore } from "./pluginExtensions";
import { usePluginsStore } from "./plugins";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("../plugins/hostDomBroker", () => ({ invalidatePluginHostDom: vi.fn() }));
vi.mock("../core-api/client", async (importOriginal) => ({
  ...await importOriginal<typeof import("../core-api/client")>(),
  uninstallPlugin: vi.fn(async () => ({
    operationId: "remove", pluginId: "com.norishell.workflow-demo", kind: "uninstall",
    state: "succeeded", progressPercent: 100, errorCode: null, startedAtUnixMs: 0n, updatedAtUnixMs: 0n,
  })),
  listInstalledPlugins: vi.fn(async () => []),
  listPluginAudit: vi.fn(async () => []),
  getPluginReadiness: vi.fn(async () => ({ ready: true })),
  enablePlugin: vi.fn(),
  disablePlugin: vi.fn(),
  replacePluginCapabilityGrants: vi.fn(),
  listPluginContributions: vi.fn(async () => []),
  listPendingPluginTerminalInput: vi.fn(async () => []),
  listPluginNavigation: vi.fn(async () => []),
  listPluginUiContributions: vi.fn(async () => []),
  listPluginExtensionTargets: vi.fn(async () => [{
    targetId: "app.header.actions", surfaceKind: "toolbar", requiredCapability: "uiPanel",
    contextual: false, acceptsForms: true,
  }]),
  openPluginTargetContext: vi.fn(async () => ({
    targetId: "app.header.actions", surfaceKind: "toolbar", contextHandle: "header-context",
    targetRevision: "1", displayLabel: null,
  })),
}));

const plugin: InstalledPluginSummary = {
  pluginId: "com.norishell.workflow-demo", name: "Workflow Demo", publisher: "NoriShell",
  packageKind: "wasm",
  artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64), activeVersion: "1.0.0",
  capabilities: ["uiPanel"], grants: [], hasSettings: false, state: "disabled", stateVersion: "1",
  installedAtUnixMs: 0n, updatedAtUnixMs: 0n,
};

describe("plugin lifecycle mounted contributions", () => {
  let pinia: ReturnType<typeof createPinia>;
  beforeEach(() => {
    vi.clearAllMocks();
    pinia = createPinia();
    setActivePinia(pinia);
    vi.mocked(enablePlugin).mockResolvedValue({ ...plugin, state: "enabled", stateVersion: "2" });
    vi.mocked(disablePlugin).mockResolvedValue({ ...plugin, stateVersion: "3" });
    vi.mocked(replacePluginCapabilityGrants).mockResolvedValue({ ...plugin, stateVersion: "4" });
  });
  afterEach(() => disposePinia(pinia));

  it.each(["enable", "disable", "permissions", "uninstall"] as const)("refreshes an already mounted empty target after %s", async (operation) => {
    const extensions = usePluginExtensionsStore();
    const lease = await extensions.acquireTarget("app.header.actions", "header");
    await extensions.loadTargetContributions(lease.context);
    expect(extensions.forContext(lease.context)).toEqual([]);
    vi.mocked(listPluginUiContributions).mockClear();
    const plugins = usePluginsStore();
    const result = operation === "uninstall"
      ? await plugins.remove(plugin, false)
      : operation === "permissions"
        ? await plugins.replaceGrants(plugin, [])
        : await plugins.setEnabled(plugin, operation === "enable");
    expect(result).not.toBeNull();
    expect(listPluginUiContributions).toHaveBeenCalledExactlyOnceWith({ target: lease.context });
  });
});
