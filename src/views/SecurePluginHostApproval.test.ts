import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import NvxSelect from "../components/ui/NvxSelect.vue";
import type { PluginHostApprovalSummary } from "../core-api/generated/core-api";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({ getPluginHostApproval: vi.fn(), decidePluginHostApproval: vi.fn() }));
const windowApi = vi.hoisted(() => ({ close: vi.fn() }));
vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => windowApi }));

import SecurePluginHostApproval from "./SecurePluginHostApproval.vue";

const approval = {
  approvalId: "019d0000-0000-7000-8000-000000000904", pluginId: "test.operations", pluginName: "Operations", kind: "mutation",
  hostLabel: "Production", endpoint: "root@host.test:22", reason: "Update connection settings", mutationPatch: { label: "Production 2" }, sessionKind: null,
  expiresAtUnixMs: 1n, stateVersion: "1", rememberPolicy: "storageUnavailable",
} as PluginHostApprovalSummary;

describe("SecurePluginHostApproval", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
    vi.clearAllMocks();
    client.getPluginHostApproval.mockResolvedValue(approval);
    client.decidePluginHostApproval.mockResolvedValue({ decision: "reject" });
    windowApi.close.mockResolvedValue(undefined);
  });

  it("disables remembered Host approval and always rejects with a one-time policy", async () => {
    const wrapper = mount(SecurePluginHostApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("This can change Host configuration or affect later connections.");
    const options = wrapper.getComponent(NvxSelect).props("options");
    if (!Array.isArray(options) || options[1] === undefined) throw new Error("missing always option");
    expect(options[1]).toMatchObject({ disabled: true });
    await wrapper.get("footer button:first-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginHostApproval).toHaveBeenCalledWith(expect.objectContaining({ decision: "reject", policy: "once" }));
    wrapper.unmount();
  });
});
