import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  isTauri: () => true,
  Channel: class<T> {
    onmessage: ((message: T) => void) | null = null;
  },
}));

import {
  decidePluginTerminalInput,
  installLocalPlugin,
  invokePluginContribution,
  listPluginAudit,
  listPluginContributions,
  listPluginOperationPermissions,
  preparePluginContributionCopy,
  prepareLocalPluginPackage,
  revokePluginOperationPermission,
  clearPluginOperationPermissions,
  setPluginLocale,
  uninstallPlugin,
} from "./client";

describe("plugin Core client boundaries", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauri.invoke.mockResolvedValue({});
  });

  it("uses the Core-owned system picker without exposing a path", async () => {
    await prepareLocalPluginPackage();

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_local_package_prepare", {
      request: { meta: { requestId: expect.any(String) } },
    });
  });

  it("mints hash-bound local install and uninstall operations", async () => {
    await installLocalPlugin({
      preparationId: "019d0000-0000-7000-8000-000000000700",
      expectedPackageSha256: "a".repeat(64),
      expectedStateVersion: "7",
      capabilityGrants: [{ capability: "terminalObserve", granted: true }],
    });
    await uninstallPlugin({
      pluginId: "com.norishell.example",
      expectedStateVersion: "8",
      deleteData: false,
    });

    const installRequest = tauri.invoke.mock.calls[0]?.[1].request;
    const uninstallRequest = tauri.invoke.mock.calls[1]?.[1].request;
    expect(tauri.invoke.mock.calls.map(([command]) => command)).toEqual([
      "plugin_local_install",
      "plugin_uninstall",
    ]);
    expect(installRequest).toMatchObject({
      preparationId: "019d0000-0000-7000-8000-000000000700",
      expectedPackageSha256: "a".repeat(64),
      expectedStateVersion: "7",
    });
    expect(installRequest.idempotencyKey).toBe(`plugin-local-install-${installRequest.operationId}`);
    expect(uninstallRequest.idempotencyKey).toBe(`plugin-uninstall-${uninstallRequest.operationId}`);
  });

  it("reads Core-owned declarative contribution projections", async () => {
    await listPluginContributions();

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_contribution_list", {
      request: { meta: { requestId: expect.any(String) } },
    });
  });

  it("reads a bounded Core-owned plugin audit projection", async () => {
    await listPluginAudit(50);

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_audit_list", {
      request: {
        meta: { requestId: expect.any(String) },
        limit: 50,
      },
    });
  });

  it("manages remembered operation approvals through the main Core command surface", async () => {
    await listPluginOperationPermissions("com.norishell.utility-demo");
    await revokePluginOperationPermission({
      pluginId: "com.norishell.utility-demo",
      permissionId: "019d0000-0000-7000-8000-000000000702",
      expectedPolicyRevision: "9",
    });
    await clearPluginOperationPermissions({
      pluginId: "com.norishell.utility-demo",
      expectedPolicyRevision: "10",
    });

    expect(tauri.invoke.mock.calls.map(([command]) => command)).toEqual([
      "plugin_operation_permissions_list",
      "plugin_operation_permission_revoke",
      "plugin_operation_permissions_clear",
    ]);
    expect(tauri.invoke.mock.calls[0]?.[1].request).toMatchObject({
      pluginId: "com.norishell.utility-demo",
    });
    expect(tauri.invoke.mock.calls[1]?.[1].request).toMatchObject({
      permissionId: "019d0000-0000-7000-8000-000000000702",
      expectedPolicyRevision: "9",
    });
    expect(tauri.invoke.mock.calls[2]?.[1].request).toMatchObject({
      expectedPolicyRevision: "10",
    });
  });

  it("invokes one revision-bound host-rendered plugin action", async () => {
    await invokePluginContribution({
      pluginId: "com.norishell.utility-demo",
      artifactFingerprintSha256: "a".repeat(64),
      expectedPackageSha256: "b".repeat(64),
      instanceGeneration: "4",
      expectedStateVersion: "6",
      expectedContributionRevision: "7",
      slot: "terminalToolbar",
      actionId: "generateUuid",
    });

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_contribution_invoke", {
      request: expect.objectContaining({
        pluginId: "com.norishell.utility-demo",
        artifactFingerprintSha256: "a".repeat(64),
        expectedPackageSha256: "b".repeat(64),
        instanceGeneration: "4",
        expectedStateVersion: "6",
        expectedContributionRevision: "7",
        slot: "terminalToolbar",
        actionId: "generateUuid",
      }),
    });
  });

  it("persists the application locale for plugin startup", async () => {
    await setPluginLocale("en");

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_locale_set", {
      request: {
        meta: { requestId: expect.any(String) },
        locale: "en",
      },
    });
  });

  it("prepares a revision-bound clipboard value without a plugin browser capability", async () => {
    await preparePluginContributionCopy({
      pluginId: "com.norishell.utility-demo",
      artifactFingerprintSha256: "a".repeat(64),
      expectedPackageSha256: "b".repeat(64),
      instanceGeneration: "4",
      expectedStateVersion: "6",
      expectedContributionRevision: "7",
      slot: "terminalToolbar",
      copyId: "copyUuid",
    });

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_contribution_copy", {
      request: expect.objectContaining({
        pluginId: "com.norishell.utility-demo",
        slot: "terminalToolbar",
        copyId: "copyUuid",
      }),
    });
  });

  it("forwards the full one-time terminal approval fence", async () => {
    await decidePluginTerminalInput({
      approvalId: "019d0000-0000-7000-8000-000000000701",
      decision: "approve",
      expectedStateVersion: "12",
      policy: "once",
      expiry: "unlimited",
    });

    expect(tauri.invoke).toHaveBeenCalledWith("plugin_terminal_input_decide", {
      request: expect.objectContaining({
        approvalId: "019d0000-0000-7000-8000-000000000701",
        decision: "approve",
        expectedStateVersion: "12",
        policy: "once",
        expiry: "unlimited",
      }),
    });
  });
});
