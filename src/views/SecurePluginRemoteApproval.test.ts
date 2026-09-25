import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginRemoteApprovalPrompt } from "../core-api/generated/core-api";
import NvxSelect from "../components/ui/NvxSelect.vue";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({ getPluginRemoteApproval: vi.fn(), decidePluginRemoteApproval: vi.fn(), submitPluginCredentialInput: vi.fn() }));
const windowApi = vi.hoisted(() => ({ close: vi.fn() }));
vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => windowApi }));

import SecurePluginRemoteApproval from "./SecurePluginRemoteApproval.vue";

const prompt: PluginRemoteApprovalPrompt = {
  approvalId: "019d0000-0000-7000-8000-000000000901", pluginId: "test.operations", pluginName: "Operations", locale: "en",
  hostLabel: "Production", endpoint: "root@host.test:22", stateVersion: "1", expiresAtUnixMs: 1n,
  content: { kind: "execute", command: "printf '<img src=x onerror=alert(1)>'", stdin: "0 0 * * * true\n", reason: "Update the selected task" },
  rememberPolicy: "exactOperation",
};

describe("SecurePluginRemoteApproval", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    client.getPluginRemoteApproval.mockResolvedValue(prompt);
    client.decidePluginRemoteApproval.mockResolvedValue(undefined);
    client.submitPluginCredentialInput.mockResolvedValue(undefined);
    windowApi.close.mockResolvedValue(undefined);
  });

  it("renders exact execution as inert text with decisions outside the scrolling body", async () => {
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("main").attributes("data-plugin-protected")).toBeDefined();
    // Untrusted prompt text lives below the shared, trusted brand header.
    expect(wrapper.get(".nvx-secure-window__content").find("img").exists()).toBe(false);
    expect(wrapper.findAll("pre")[0]?.text()).toContain("<img src=x onerror=alert(1)>");
    expect(wrapper.get(".secure-remote__body").find("footer").exists()).toBe(false);
    await wrapper.get("footer button:last-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginRemoteApproval).toHaveBeenCalledWith({ approvalId: prompt.approvalId, expectedStateVersion: "1", decision: "approve", policy: "once", expiry: "unlimited" });
    wrapper.unmount();
  });

  it("shows the exact forward being stopped and returns a one-time rejection", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({ ...prompt, content: {
      kind: "forwardStop", reason: "Stop the selected listener", forwardHandle: "a".repeat(64),
      actualBind: { address: "127.0.0.1", port: 43001 },
      rule: { kind: "remote", remoteBindAddress: "127.0.0.1", remoteListenPort: 0, localTargetHost: "localhost", localTargetPort: 8080 },
    } });
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("h1").text()).toBe("Stop this forward");
    expect(wrapper.text()).toContain("127.0.0.1:43001");
    expect(wrapper.text()).toContain("localhost:8080");
    expect(wrapper.find("input").exists()).toBe(false);
    await wrapper.get("footer button:first-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginRemoteApproval).toHaveBeenCalledWith({ approvalId: prompt.approvalId, expectedStateVersion: "1", decision: "reject", policy: "once", expiry: "unlimited" });
    wrapper.unmount();
  });

  it("renders protected network-access details without treating them as a forward rule", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({
      ...prompt,
      content: {
        kind: "access",
        operation: "networkRequest",
        details: "UDP 198.51.100.24:443\nReceive up to 4 KiB",
        reason: "Query the selected resolver",
      },
    } as unknown as PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("h1").text()).toBe("Review network access");
    expect(wrapper.text()).toContain("Network request");
    expect(wrapper.text()).toContain("Network access target");
    expect(wrapper.text()).not.toContain("Current SSH connection");
    expect(wrapper.text()).toContain("Query the selected resolver");
    expect(wrapper.find("pre").text()).toContain("UDP 198.51.100.24:443");
    expect(wrapper.find(".secure-remote__route").exists()).toBe(false);
    expect(wrapper.text()).toContain("Production");
    expect(wrapper.text()).toContain("This request can send or receive data at the listed addresses.");
    wrapper.unmount();
  });

  it("identifies a fresh SSH execution separately from the user's terminal", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({ ...prompt, content: {
      kind: "access", operation: "remoteExecute", details: "printf test", reason: "inspect",
    } } satisfies PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("Independent SSH execution");
    expect(wrapper.text()).not.toContain("Network access target");
    expect(wrapper.text()).toContain("This opens a separate SSH connection");
    expect(wrapper.text()).not.toContain("This request can send or receive data at the listed addresses.");
    wrapper.unmount();
  });

  it("labels a local executable approval with the local-user risk instead of network access", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({
      ...prompt,
      hostLabel: "Local process: cat",
      endpoint: "Local process: cat",
      content: {
        kind: "access",
        operation: "localExecute",
        details: "This local program runs as the current user.",
        reason: "isolated:tools:processStart",
      },
    } satisfies PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("h1").text()).toBe("Review local program execution");
    expect(wrapper.text()).toContain("Operations requests local program execution.");
    expect(wrapper.text()).toContain("This starts a local program with your current user authority.");
    expect(wrapper.text()).not.toContain("This request can send or receive data at the listed addresses.");
    expect(wrapper.text()).toContain("Local execution context");
    wrapper.unmount();
  });

  it("labels file access with its local-file risk instead of a network warning", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({
      ...prompt,
      hostLabel: "Selected local file",
      endpoint: "/tmp/report.txt",
      content: {
        kind: "access",
        operation: "fileAccess",
        details: "Selected directory scope",
        reason: "open-report",
      },
    } satisfies PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("h1").text()).toBe("Review local file access");
    expect(wrapper.text()).toContain("Operations requests access to the selected local file or directory.");
    expect(wrapper.text()).toContain("Selected local file or directory");
    expect(wrapper.text()).not.toContain("Current SSH connection");
    expect(wrapper.text()).not.toContain("requests access to the current connection");
    expect(wrapper.text()).toContain("This request can access the selected local file or directory.");
    expect(wrapper.text()).not.toContain("This request can send or receive data at the listed addresses.");
    wrapper.unmount();
  });

  it.each(["sftpRead", "sftpWrite"] as const)("discloses independent remote file scope for %s", async (operation) => {
    client.getPluginRemoteApproval.mockResolvedValue({
      ...prompt,
      content: { kind: "access", operation, details: "/srv/plugin-data", reason: "sftpOpen" },
    } satisfies PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("h1").text()).toBe("Review remote SFTP access");
    expect(wrapper.text()).toContain("Independent SFTP connection");
    expect(wrapper.text()).toContain("not a server sandbox");
    expect(wrapper.text()).not.toContain("selected local file");
    wrapper.unmount();
  });

  it("sends plugin credentials only through the protected input endpoint and clears the field", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({ ...prompt, rememberPolicy: "unavailable", content: {
      kind: "credential", label: "Service token", target: { origin: "https://api.example.test", injection: { kind: "bearer" } },
    } } satisfies PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.findComponent(NvxSelect).exists()).toBe(false);
    expect(wrapper.get("footer button:last-child").attributes("disabled")).toBeDefined();
    await wrapper.get("input[type=password]").setValue("test-only-token");
    await wrapper.get("footer button:last-child").trigger("click");
    await flushPromises();
    expect(client.submitPluginCredentialInput).toHaveBeenCalledWith({ approvalId: prompt.approvalId, expectedStateVersion: "1", secret: "test-only-token" });
    expect(client.decidePluginRemoteApproval).not.toHaveBeenCalled();
    expect((wrapper.get("input[type=password]").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });

  it("requires a distinct confirmed Vault creation and never offers remembered approval", async () => {
    client.getPluginRemoteApproval.mockResolvedValue({ ...prompt, rememberPolicy: "unavailable", content: { kind: "vaultAccess", create: true } } satisfies PluginRemoteApprovalPrompt);
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.get("h1").text()).toBe("Create your local Vault");
    expect(wrapper.findComponent(NvxSelect).exists()).toBe(false);
    await wrapper.get("#plugin-credential-secret").setValue("七个字符不够啊");
    expect(wrapper.get("#plugin-credential-secret").attributes("aria-invalid")).toBe("true");
    expect(wrapper.text()).toContain("at least 8 characters");
    await wrapper.get("#plugin-credential-secret").setValue("test-vault-password");
    await wrapper.get("#plugin-vault-confirmation").setValue("wrong-password");
    expect(wrapper.get("footer button:last-child").attributes("disabled")).toBeDefined();
    await wrapper.get("#plugin-vault-confirmation").setValue("test-vault-password");
    await wrapper.get("footer button:last-child").trigger("click");
    await flushPromises();
    expect(client.submitPluginCredentialInput).toHaveBeenCalledWith({ approvalId: prompt.approvalId, expectedStateVersion: "1", secret: "test-vault-password", confirmation: "test-vault-password" });
    expect((wrapper.get("#plugin-vault-confirmation").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });

  it("remembers only an exact operation and resets a new unavailable approval to once", async () => {
    const wrapper = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    wrapper.getComponent(NvxSelect).vm.$emit("update:modelValue", "always");
    await wrapper.vm.$nextTick();
    expect(wrapper.get("footer button:last-child").text()).toBe("Allow and remember");
    await wrapper.get("footer button:last-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginRemoteApproval).toHaveBeenLastCalledWith(expect.objectContaining({ policy: "always" }));
    wrapper.unmount();

    client.getPluginRemoteApproval.mockResolvedValue({ ...prompt, approvalId: "019d0000-0000-7000-8000-000000000902", rememberPolicy: "unavailable" });
    const next = mount(SecurePluginRemoteApproval, { global: { plugins: [i18n] } });
    await flushPromises();
    const options = next.getComponent(NvxSelect).props("options");
    if (!Array.isArray(options) || options[1] === undefined) throw new Error("missing always option");
    expect(options[1]).toMatchObject({ disabled: true });
    await next.get("footer button:first-child").trigger("click");
    await flushPromises();
    expect(client.decidePluginRemoteApproval).toHaveBeenLastCalledWith(expect.objectContaining({ policy: "once" }));
    next.unmount();
  });
});
