import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginTerminalInputProposal } from "../core-api/generated/core-api";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({ getPluginTerminalInput: vi.fn(), decidePluginTerminalInput: vi.fn() }));
const windowApi = vi.hoisted(() => ({ close: vi.fn() }));
vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => windowApi }));

import SecurePluginTerminalInput from "./SecurePluginTerminalInput.vue";

const approval = {
  approvalId: "019d0000-0000-7000-8000-000000000903", pluginId: "test.operations", pluginName: "Operations", publisher: "NoriShell tests",
  artifactFingerprintSha256: "a".repeat(64), packageSha256: "b".repeat(64), instanceGeneration: "1", hostLabel: "Production", endpoint: "root@host.test:22",
  sessionId: "session", generation: "1", channelId: "channel", attachmentId: "attachment", viewId: "view", focusEpoch: "1", inputEpoch: "1",
  payload: "systemctl restart nginx", appendEnter: true, payloadSha256: "c".repeat(64), expiresAtUnixMs: 1n, stateVersion: "1", rememberPolicy: "exactOperation",
} as PluginTerminalInputProposal;

describe("SecurePluginTerminalInput", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
    vi.clearAllMocks();
    client.getPluginTerminalInput.mockResolvedValue(approval);
    client.decidePluginTerminalInput.mockResolvedValue({ decision: "approve" });
    windowApi.close.mockResolvedValue(undefined);
  });

  it("shows the terminal-state risk and sends the remembered policy only when selected", async () => {
    const wrapper = mount(SecurePluginTerminalInput, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("It is not a safe command.");
    await wrapper.get('.nvx-secure-window__decision input[value="always"]').setValue();
    await wrapper.vm.$nextTick();
    expect(wrapper.get(".nvx-secure-window__action-hint").text()).toContain("commands remain specific");
    await wrapper.get("footer button:last-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginTerminalInput).toHaveBeenCalledWith(expect.objectContaining({ decision: "approve", policy: "always" }));
    wrapper.unmount();
  });
});
