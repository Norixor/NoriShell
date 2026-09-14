import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { SshSyncSecurePrompt } from "../core-api/generated/core-api";
import { i18n } from "../locales";

const client = vi.hoisted(() => ({
  decideSshSyncSecurePrompt: vi.fn(),
  getSshSyncSecurePrompt: vi.fn(),
}));
const windowApi = vi.hoisted(() => ({ close: vi.fn() }));

vi.mock("../core-api/client", () => client);
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => windowApi,
}));

import SecureSshSync from "./SecureSshSync.vue";

const authorizePrompt: SshSyncSecurePrompt = {
  promptId: "019d0000-0000-7000-8000-000000000901",
  pluginId: "org.norixor",
  profileId: "norixor-production",
  remoteOrigin: "https://api.norixor.org",
  kind: "authorizeProvider",
  oauth: {
    authorizationUrl: "https://norixor.org/oauth/authorize",
    tokenUrl: "https://api.norixor.org/oauth/token",
    revokeUrl: "https://api.norixor.org/oauth/revoke",
    resourceOrigins: ["https://api.norixor.org"],
  },
  desktopProfiles: [], desktopProfileCount: 0, remoteDesktopProfileCount: 0,
  hosts: [],
  hostCount: 0,
  credentialCount: 0,
  conflictCount: 0,
  updateCount: 0,
  deleteCount: 0,
  remoteHostCount: 0,
  remoteCredentialCount: 0,
  localComparedAtUnixMs: null,
  remoteUpdatedAtUnixMs: null,
  differences: [],
  differenceTotalCount: 0,
  differenceOmittedCount: 0,
};

describe("SecureSshSync", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    i18n.global.locale.value = "zh-CN";
    client.getSshSyncSecurePrompt.mockResolvedValue(authorizePrompt);
  });

  it("keeps the localized protected identity in the window header and actions after the review", async () => {
    const wrapper = mount(SecureSshSync, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    await flushPromises();

    expect(wrapper.find(".nvx-secure-window__chrome .nvx-secure-window__protected").text()).toBe("受保护的窗口");
    expect(wrapper.find(".nvx-secure-window__intro").text()).toContain("授权同步服务");
    expect(wrapper.find(".nvx-secure-window__body").text()).toContain("https://api.norixor.org");
    expect(wrapper.find(".nvx-secure-window__body footer").exists()).toBe(false);
    expect(wrapper.find(".nvx-secure-window__actions").exists()).toBe(true);
    expect(wrapper.findAll(".nvx-secure-window__actions button").map((button) => button.text()))
      .toEqual(["取消", "仅批准本次"]);

    wrapper.unmount();
  });

  it("selects every portable credential by default and excludes device-bound credentials", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt,
      kind: "selectBackup",
      oauth: null,
      hosts: [{
        hostId: "019d0000-0000-7000-8000-000000000911",
        label: "Production",
        endpoint: "10.0.0.8:22",
        credentials: [{
          credentialRefId: "019d0000-0000-7000-8000-000000000912",
          label: "Private key",
          methodLabel: "Private key",
          machineBound: false,
        }, {
          credentialRefId: "019d0000-0000-7000-8000-000000000913",
          label: "Local Agent",
          methodLabel: "SSH Agent",
          machineBound: true,
        }],
      }],
    } satisfies SshSyncSecurePrompt);
    client.decideSshSyncSecurePrompt.mockResolvedValue({ accepted: true });
    const wrapper = mount(SecureSshSync, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    await flushPromises();

    await wrapper.find(".nvx-secure-window__actions button:last-child").trigger("click");
    await flushPromises();
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({
      selectedHostIds: ["019d0000-0000-7000-8000-000000000911"],
      selectedCredentialRefIds: ["019d0000-0000-7000-8000-000000000912"],
      vaultPassword: null,
    }));
    wrapper.unmount();
  });

  it("requires an explicit conflict direction before approving once", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt,
      kind: "resolveConflicts",
      oauth: null,
      conflictCount: 2,
    } satisfies SshSyncSecurePrompt);
    client.decideSshSyncSecurePrompt.mockResolvedValue({ accepted: true });
    const wrapper = mount(SecureSshSync, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const buttons = wrapper.findAll(".nvx-secure-window__actions button");
    expect(buttons.map((button) => button.text())).toEqual(["取消", "仅批准本次"]);
    expect(buttons[1]?.attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("保留本地");
    expect(wrapper.text()).toContain("使用云端");
    await wrapper.find('input[type="radio"][value="useRemote"]').setValue();
    expect(buttons[1]?.attributes("disabled")).toBeUndefined();
    await buttons[1]?.trigger("click");
    await flushPromises();
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({
      decision: "useRemote",
      selectedHostIds: [],
      selectedCredentialRefIds: [],
      vaultPassword: null,
    }));
    wrapper.unmount();
  });

  it("shows concrete differences and keeps direction choices inside the protected window", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt,
      kind: "chooseSyncDirection",
      oauth: null,
      hostCount: 2,
      credentialCount: 1,
      remoteHostCount: 1,
      remoteCredentialCount: 1,
      localComparedAtUnixMs: 1_788_400_000_000,
      remoteUpdatedAtUnixMs: 1_788_399_000_000,
      differences: [{
        kind: "host",
        change: "changed",
        label: "prod-db-01",
        localSummary: "deploy@10.10.10.21:22",
        remoteSummary: "deploy@10.10.10.22:22",
      }],
      differenceTotalCount: 1,
    } satisfies SshSyncSecurePrompt);
    client.decideSshSyncSecurePrompt.mockResolvedValue({ accepted: true });
    const wrapper = mount(SecureSshSync, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("prod-db-01");
    expect(wrapper.text()).toContain("deploy@10.10.10.21:22");
    expect(wrapper.text()).toContain("deploy@10.10.10.22:22");
    expect(wrapper.text()).toContain("内容不同");
    expect(wrapper.text()).toContain("目标端独有的项目也会被删除");
    expect(wrapper.text()).toContain("不是数据最后修改时间");
    expect(wrapper.findAll(".secure-sync__times p").map((item) => item.text()))
      .toEqual(["2 个 SSH 主机 · 0 个远程桌面 · 1 个凭据", "1 个 SSH 主机 · 0 个远程桌面 · 1 个凭据"]);
    expect(wrapper.findAll(".nvx-secure-window__actions button").map((button) => button.text()))
      .toEqual(["取消", "仅批准本次"]);
    expect(wrapper.find('.nvx-secure-window__actions button:last-child').attributes("disabled")).toBeDefined();
    expect(wrapper.text()).toContain("本机覆盖云端");
    expect(wrapper.text()).toContain("云端覆盖本机");
    await wrapper.find('input[type="radio"][value="keepLocal"]').setValue();
    await wrapper.find('.nvx-secure-window__actions button:last-child').trigger("click");
    await flushPromises();
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({ decision: "keepLocal" }));
    wrapper.unmount();
  });

  it("hides encrypted secret summaries and distinguishes missing items and unknown timestamps", async () => {
    i18n.global.locale.value = "en";
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt,
      kind: "chooseSyncDirection",
      localComparedAtUnixMs: null,
      remoteUpdatedAtUnixMs: null,
      differences: [{
        kind: "encryptedSecret",
        change: "changed",
        label: "Server password",
        localSummary: "SECRET_LOCAL_MUST_NOT_RENDER",
        remoteSummary: "SECRET_REMOTE_MUST_NOT_RENDER",
      }, {
        kind: "host",
        change: "localOnly",
        label: "Local server",
        localSummary: "deploy@local:22",
        remoteSummary: null,
      }],
      differenceTotalCount: 202,
      differenceOmittedCount: 200,
    } satisfies SshSyncSecurePrompt);
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();

    expect(wrapper.text()).not.toContain("SECRET_");
    expect(wrapper.text()).toContain("Changed · contents hidden");
    expect(wrapper.text()).toContain("Not present");
    expect(wrapper.text()).toContain("No timestamp available");
    expect(wrapper.text()).toContain("200 more differences");
    expect(wrapper.text()).not.toContain("plugins.sshSync.");
    wrapper.unmount();
  });

  it("keeps a close action when the prompt fails to load", async () => {
    client.getSshSyncSecurePrompt.mockRejectedValue(new Error("expired"));
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();

    const buttons = wrapper.findAll("footer button");
    expect(buttons.map((button) => button.text())).toEqual(["关闭"]);
    await buttons[0]?.trigger("click");
    expect(windowApi.close).toHaveBeenCalledOnce();
    expect(client.decideSshSyncSecurePrompt).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("clears entered secrets after a rejected decision and allows Escape to close", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt,
      kind: "unlockSynchronizedVault",
      oauth: null,
    } satisfies SshSyncSecurePrompt);
    client.decideSshSyncSecurePrompt.mockRejectedValue(new Error("state changed"));
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.find("input").setValue("example-vault-password");
    await wrapper.find("footer .nvx-button--primary").trigger("click");
    await flushPromises();

    expect(wrapper.find("input").exists()).toBe(false);
    expect(wrapper.findAll("footer button").map((button) => button.text())).toEqual(["关闭"]);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flushPromises();
    expect(windowApi.close).toHaveBeenCalledOnce();
    wrapper.unmount();
  });

  it("localizes the password controls and never submits a short password with Enter", async () => {
    i18n.global.locale.value = "en";
    client.getSshSyncSecurePrompt.mockResolvedValue({ ...authorizePrompt, kind: "unlockSynchronizedVault", oauth: null });
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.find(".nvx-secure-window__protected").text()).toBe("Protected window");
    expect(wrapper.find("label").attributes("for")).toBe("sync-vault-password");
    await wrapper.get("input").setValue("short");
    await wrapper.get("input").trigger("keydown", { key: "Enter" });
    expect(client.decideSshSyncSecurePrompt).not.toHaveBeenCalled();
    await wrapper.get('[aria-label="Show password"]').trigger("click");
    expect(wrapper.get("input").attributes("type")).toBe("text");
    await wrapper.get('[aria-label="Hide password"]').trigger("click");
    expect(wrapper.get("input").attributes("type")).toBe("password");
    expect(wrapper.text()).not.toContain("plugins.sshSync.");
    wrapper.unmount();
  });

  it("creates a missing local Vault only with matching UTF-8 passwords and clears both fields before continuing", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({ ...authorizePrompt, kind: "createLocalVault", oauth: null });
    let resolve!: (value: { accepted: boolean }) => void;
    client.decideSshSyncSecurePrompt.mockImplementation(() => new Promise((done) => { resolve = done; }));
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();

    expect(wrapper.text()).toContain("本机 Vault 未创建；已有其他设备请使用相同 Vault 密码；仅创建本机 Vault，之后继续操作。");
    const fields = wrapper.findAll('input[type="password"]');
    expect(fields).toHaveLength(2);
    await fields[0]!.setValue("密码密码");
    await fields[1]!.setValue("不匹配的密码");
    expect(wrapper.find("footer .nvx-button--primary").attributes("disabled")).toBeDefined();
    await fields[1]!.setValue("密码密码");
    expect(wrapper.find("footer .nvx-button--primary").attributes("disabled")).toBeUndefined();
    await wrapper.find("footer .nvx-button--primary").trigger("click");

    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({
      vaultPassword: "密码密码",
      vaultPasswordConfirmation: "密码密码",
    }));
    expect(fields.every((field) => (field.element as HTMLInputElement).value === "")).toBe(true);
    resolve({ accepted: true });
    await flushPromises();
    wrapper.unmount();
  });

  it("uses the Vault password to recover a synchronized key without submitting a confirmation", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({ ...authorizePrompt, kind: "recoverSynchronizedKey", oauth: null });
    client.decideSshSyncSecurePrompt.mockResolvedValue({ accepted: true });
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();

    expect(wrapper.findAll('input[type="password"]')).toHaveLength(1);
    await wrapper.get('input[type="password"]').setValue("example-vault-password");
    await wrapper.find("footer .nvx-button--primary").trigger("click");
    await flushPromises();
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({
      vaultPassword: "example-vault-password",
      vaultPasswordConfirmation: undefined,
    }));
    wrapper.unmount();
  });

  it("rejects repeated decisions while a direction is being submitted", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({ ...authorizePrompt, kind: "chooseSyncDirection" });
    let finish: (() => void) | undefined;
    client.decideSshSyncSecurePrompt.mockImplementation(() => new Promise<{ accepted: boolean }>((resolve) => { finish = () => resolve({ accepted: true }); }));
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.find('input[type="radio"][value="useRemote"]').setValue();
    await wrapper.find("footer button:last-child").trigger("click");
    expect(wrapper.findAll("footer button").every((button) => button.attributes("disabled") !== undefined)).toBe(true);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledOnce();
    expect(windowApi.close).not.toHaveBeenCalled();
    finish?.();
    await flushPromises();
    wrapper.unmount();
  });

  it("does not report an unaccepted decision as confirmed", async () => {
    client.decideSshSyncSecurePrompt.mockResolvedValue({ accepted: false });
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();
    await wrapper.find("footer .nvx-button--primary").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("SSH 同步请求不可用");
    expect(wrapper.text()).not.toContain("受保护的数据交换已确认");
    expect(wrapper.findAll("footer button").map((button) => button.text())).toEqual(["关闭"]);
    wrapper.unmount();
  });

  it("requires an explicit destructive confirmation before resetting remote data", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt,
      kind: "resetRemote",
      oauth: null,
    } satisfies SshSyncSecurePrompt);
    client.decideSshSyncSecurePrompt.mockResolvedValue({ accepted: true });
    const wrapper = mount(SecureSshSync, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("此操作无法撤销");
    expect(wrapper.text()).toContain("不会删除本机 Host、凭据、Vault 或同步密钥");
    const confirm = wrapper.find(".nvx-secure-window__actions button:last-child");
    expect(confirm.text()).toBe("仅批准本次");
    expect(confirm.classes()).toContain("nvx-button--danger");
    await confirm.trigger("click");
    await flushPromises();
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({
      decision: "approve",
      selectedHostIds: [],
      selectedCredentialRefIds: [],
      vaultPassword: null,
    }));
    wrapper.unmount();
  });
  it("selects a desktop separately from its saved password", async () => {
    client.getSshSyncSecurePrompt.mockResolvedValue({
      ...authorizePrompt, kind: "selectBackup", oauth: null,
      desktopProfiles: [{ profileId: "desktop-a", label: "RDP fixture", protocol: "rdp", address: "desktop.example", port: 3389, username: "user", domain: "TEST",
        credentials: [{ credentialRefId: "credential-a", label: "Saved password", methodLabel: "Password", machineBound: false }] }],
    } satisfies SshSyncSecurePrompt);
    const wrapper = mount(SecureSshSync, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("RDP fixture");
    const boxes = wrapper.findAll('input[type="checkbox"]');
    expect(boxes).toHaveLength(2);
    await boxes[1]!.setValue(false);
    await wrapper.find("footer .nvx-button--primary").trigger("click");
    await flushPromises();
    expect(client.decideSshSyncSecurePrompt).toHaveBeenCalledWith(expect.objectContaining({
      selectedDesktopProfileIds: ["desktop-a"], selectedCredentialRefIds: [], selectedHostIds: [],
    }));
    wrapper.unmount();
  });

});
