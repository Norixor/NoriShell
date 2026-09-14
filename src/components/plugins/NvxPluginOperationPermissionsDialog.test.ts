import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginOperationPermissionList } from "../../core-api/generated/core-api";
import { i18n } from "../../locales";

const client = vi.hoisted(() => ({
  listPluginOperationPermissions: vi.fn(),
  revokePluginOperationPermission: vi.fn(),
  clearPluginOperationPermissions: vi.fn(),
}));
vi.mock("../../core-api/client", () => client);

import NvxPluginOperationPermissionsDialog from "./NvxPluginOperationPermissionsDialog.vue";

const snapshot = {
  policyRevision: "policy-1",
  permissions: [{
    permissionId: "permission-1", pluginId: "test.operations", operation: "remoteExecute",
    actionLabel: "Restart Nginx", targetLabel: "Production · root@host.test:22", createdAtUnixMs: 1_700_000_000_000n,
    expiresAtUnixMs: null,
  }],
} as PluginOperationPermissionList;

describe("NvxPluginOperationPermissionsDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    client.listPluginOperationPermissions.mockResolvedValue(snapshot);
    client.revokePluginOperationPermission.mockResolvedValue(undefined);
    client.clearPluginOperationPermissions.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows the exact operation and refreshes with the current policy revision after revocation", async () => {
    client.listPluginOperationPermissions
      .mockResolvedValueOnce(snapshot)
      .mockResolvedValueOnce({ permissions: [], policyRevision: "policy-2" });
    const wrapper = mount(NvxPluginOperationPermissionsDialog, {
      props: { pluginId: "test.operations", pluginName: "Operations" },
      global: { plugins: [i18n] },
      attachTo: document.body,
    });
    await flushPromises();
    expect(document.body.textContent).toContain("Remote operation");
    expect(document.body.textContent).toContain("Restart Nginx");
    expect(document.body.textContent).toContain("Production · root@host.test:22");
    expect(document.body.textContent).toContain("Until revoked");
    document.body.querySelector<HTMLButtonElement>("tbody button")!.click();
    await flushPromises();
    expect(client.revokePluginOperationPermission).toHaveBeenCalledWith({
      pluginId: "test.operations", permissionId: "permission-1", expectedPolicyRevision: "policy-1",
    });
    expect(document.body.textContent).toContain("No remembered operation approvals");
    wrapper.unmount();
  });

  it("labels a remembered network request in the management table", async () => {
    client.listPluginOperationPermissions.mockResolvedValue({
      policyRevision: "policy-1",
      permissions: [{
        ...snapshot.permissions[0]!,
        operation: "networkRequest",
        actionLabel: "workflow:github.repository.read:github.repository.request",
      }],
    } as unknown as PluginOperationPermissionList);
    const wrapper = mount(NvxPluginOperationPermissionsDialog, {
      props: { pluginId: "test.operations", pluginName: "Operations" },
      global: { plugins: [i18n] },
      attachTo: document.body,
    });
    await flushPromises();
    expect(document.body.textContent).toContain("Network request");
    expect(document.body.textContent).toContain("workflow:github.repository.read:github.repository.request");
    wrapper.unmount();
  });

  it("keeps a failed action retryable and clears all only with the loaded revision", async () => {
    client.revokePluginOperationPermission.mockRejectedValueOnce(new Error("conflict"));
    const wrapper = mount(NvxPluginOperationPermissionsDialog, {
      props: { pluginId: "test.operations", pluginName: "Operations" },
      global: { plugins: [i18n] },
      attachTo: document.body,
    });
    await flushPromises();
    document.body.querySelector<HTMLButtonElement>("tbody button")!.click();
    await flushPromises();
    expect(document.body.textContent).toContain("Operation approvals could not be updated");
    expect(document.body.querySelector<HTMLButtonElement>("tbody button")!.disabled).toBe(false);
    const clear = Array.from(document.body.querySelectorAll<HTMLButtonElement>("footer button"))
      .find((button) => button.textContent?.trim() === "Revoke all");
    clear!.click();
    await flushPromises();
    expect(client.clearPluginOperationPermissions).toHaveBeenCalledWith({
      pluginId: "test.operations", expectedPolicyRevision: "policy-1",
    });
    wrapper.unmount();
  });

  it("marks an expired approval while keeping revocation available", async () => {
    client.listPluginOperationPermissions.mockResolvedValue({
      ...snapshot,
      permissions: [{ ...snapshot.permissions[0]!, expiresAtUnixMs: 1n }],
    } satisfies PluginOperationPermissionList);
    const wrapper = mount(NvxPluginOperationPermissionsDialog, {
      props: { pluginId: "test.operations", pluginName: "Operations" },
      global: { plugins: [i18n] },
      attachTo: document.body,
    });
    await flushPromises();
    expect(document.body.textContent).toContain("Expired");
    expect(document.body.querySelector<HTMLButtonElement>("tbody button")!.disabled).toBe(false);
    wrapper.unmount();
  });
});
