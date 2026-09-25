import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import { i18n } from "../locales";
import SecureVault from "./SecureVault.vue";
const mocks = vi.hoisted(() => ({ get: vi.fn(), submit: vi.fn(), cancel: vi.fn(), reset: vi.fn() }));
vi.mock("../core-api/secure-vault-client", () => ({ secureVaultClient: mocks }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ startDragging: vi.fn() }) }));
function render() {
  return mount(SecureVault, { global: { plugins: [createI18n({ legacy: false, locale: "en", missingWarn: false, fallbackWarn: false, messages: { en: {} } })] } });
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.submit.mockReturnValue(new Promise(() => undefined));
  mocks.cancel.mockResolvedValue(undefined);
  mocks.reset.mockReturnValue(new Promise(() => undefined));
});
describe("isolated Vault interaction", () => {
  it("explains the eight-character rule while the create action is disabled", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "create" });
    const wrapper = render();
    await flushPromises();
    const inputs = wrapper.findAll('input[type="password"]');
    await inputs[0]!.setValue("七个字符不够啊");
    expect(inputs[0]!.attributes("aria-invalid")).toBe("true");
    expect(wrapper.text()).toContain("sshTerminal.vaultPasswordTooShort");
    expect(wrapper.findAll("button").at(-1)!.attributes("disabled")).toBeDefined();
    await inputs[0]!.setValue("八个字符已经够了");
    await inputs[1]!.setValue("八个字符已经够了");
    expect(wrapper.findAll("button").at(-1)!.attributes("disabled")).toBeUndefined();
    wrapper.unmount();
  });
  it("requires matching create confirmation and clears secrets before the reply", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "create" });
    const wrapper = render();
    await flushPromises();
    const inputs = wrapper.findAll('input[type="password"]');
    await inputs[0]!.setValue("密码密码密码密码");
    await inputs[1]!.setValue("different");
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).not.toHaveBeenCalled();
    await inputs[1]!.setValue("密码密码密码密码");
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).toHaveBeenCalledWith("", "密码密码密码密码", "密码密码密码密码", false);
    expect(inputs.every((input) => (input.element as HTMLInputElement).value === "")).toBe(true);
    wrapper.unmount();
  });
  it("requires a separate explicit automatic-unlock confirmation", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "enableAutoUnlock" });
    const wrapper = render();
    await flushPromises();
    await wrapper.find('input[type="password"]').setValue("synthetic-test-password");
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).not.toHaveBeenCalled();
    await wrapper.find('input[type="checkbox"]').setValue(true);
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).toHaveBeenCalledWith("", "synthetic-test-password", "", true);
    wrapper.unmount();
  });
  it("requires a separate explicit confirmation before storing the Vault password locally", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "enableLocalAutoUnlock" });
    const wrapper = render();
    await flushPromises();
    expect(wrapper.text()).toContain("sshSettings.vault.enableLocalDialog.securityNotice");
    await wrapper.find('input[type="password"]').setValue("synthetic-test-password");
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).not.toHaveBeenCalled();
    await wrapper.find('input[type="checkbox"]').setValue(true);
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).toHaveBeenCalledWith("", "synthetic-test-password", "", true);
    wrapper.unmount();
  });
  it("cancels without submitting any password", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "unlock" });
    const wrapper = render();
    await flushPromises();
    await wrapper.find('input[type="password"]').setValue("synthetic-test-password");
    const cancel = wrapper.findAll("button").find((button) => button.text() === "sshSettings.vault.enableDialog.cancel")!;
    await cancel.trigger("click");
    expect(mocks.cancel).toHaveBeenCalledWith("");
    expect(mocks.submit).not.toHaveBeenCalled();
    expect((wrapper.find('input[type="password"]').element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });
  it("requires a typed phrase and explicit acknowledgement before resetting a locked Vault", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "unlock", canReset: true });
    const wrapper = render();
    await flushPromises();
    await wrapper.find('input[type="password"]').setValue("wrong-password");
    await wrapper.findAll("button").find((button) => button.text() === "sshHosts.vault.reset.open")!.trigger("click");
    expect(wrapper.find('input[type="password"]').exists()).toBe(false);
    const reset = wrapper.findAll("button").find((button) => button.text() === "sshHosts.vault.reset.delete")!;
    expect(reset.attributes("disabled")).toBeDefined();
    await wrapper.find('input[type="text"]').setValue("reset");
    await wrapper.find('input[type="checkbox"]').setValue(true);
    expect(reset.attributes("disabled")).toBeDefined();
    await wrapper.find('input[type="text"]').setValue("RESET");
    expect(reset.attributes("disabled")).toBeUndefined();
    await reset.trigger("click");
    expect(mocks.reset).toHaveBeenCalledWith("", "RESET", true);
    expect(mocks.submit).not.toHaveBeenCalled();
    expect((wrapper.find('input[type="text"]').element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });
  it("does not offer reset outside an eligible unlock prompt", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "unlock", canReset: false });
    const wrapper = render();
    await flushPromises();
    expect(wrapper.text()).not.toContain("sshHosts.vault.reset.open");
    wrapper.unmount();
  });
  it("shows the translated Vault hint in the protected window", async () => {
    i18n.global.locale.value = "zh-CN";
    mocks.get.mockResolvedValue({ id: "fixture", kind: "unlock", canReset: true });
    const wrapper = mount(SecureVault, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("请输入 Vault 密码。");
    expect(wrapper.text()).not.toContain("sshHosts.quickConnect");
    wrapper.unmount();
    i18n.global.locale.value = "en";
  });
});
