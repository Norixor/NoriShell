import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginSpecialPermissionSnapshot } from "../core-api/generated/core-api";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({ getPluginSpecialPermission: vi.fn(), decidePluginSpecialPermission: vi.fn() }));
const windowApi = vi.hoisted(() => ({ close: vi.fn() }));
vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => windowApi }));
import SecurePluginPermission from "./SecurePluginPermission.vue";

const snapshot: PluginSpecialPermissionSnapshot = {
  approvalId: "019d0000-0000-7000-8000-000000000999", approvalStateVersion: "1", expiresAtUnixMs: 1n,
  pluginId: "org.example.fixture", pluginName: "<script>fixture</script>", publisher: "Example",
  artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64), version: "1.0.0",
  target: { kind: "preparedPackage", preparationId: "fixture-preparation", expectedPackageSha256: "b".repeat(64), expectedStateVersion: null },
  requestedCapability: "remoteInspect", publisherVerified: false,
  specialGrants: [{ capability: "remoteInspect", granted: false }, { capability: "remoteExecRequest", granted: false }],
  hosts: [],
};

describe("SecurePluginPermission installation confirmation", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    i18n.global.locale.value = "en";
    client.getPluginSpecialPermission.mockResolvedValue(snapshot);
    client.decidePluginSpecialPermission.mockResolvedValue({ approvalId: snapshot.approvalId, decision: "approve" });
  });
  afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });

  it("preselects only the requested capability and writes nothing until explicit confirmation", async () => {
    const wrapper = mount(SecurePluginPermission, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.find("script").exists()).toBe(false);
    expect(wrapper.text()).toContain(snapshot.packageSha256);
    const checkboxes = wrapper.findAll<HTMLInputElement>('input[type="checkbox"]');
    expect(checkboxes.map((checkbox) => checkbox.element.checked)).toEqual([true, false]);
    expect(client.decidePluginSpecialPermission).not.toHaveBeenCalled();
    await wrapper.get("footer button:last-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginSpecialPermission).toHaveBeenCalledWith({
      approvalId: snapshot.approvalId, expectedApprovalStateVersion: "1", decision: "approve",
      specialGrants: [{ capability: "remoteInspect", granted: true }, { capability: "remoteExecRequest", granted: false }], hostSelections: [],
    });
    expect(wrapper.text()).toContain("approved for this package");
    expect(wrapper.text()).not.toContain("has been disabled");
    wrapper.unmount();
  });

  it("keeps rejection explicit and presents an expired decision as a failure", async () => {
    client.decidePluginSpecialPermission.mockRejectedValue(new Error("expired"));
    const wrapper = mount(SecurePluginPermission, { global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.get("footer button:first-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginSpecialPermission).toHaveBeenCalledWith(expect.objectContaining({ decision: "reject" }));
    expect(wrapper.text()).toContain("expired");
    expect(windowApi.close).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
