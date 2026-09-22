import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import SecureVault from "./SecureVault.vue";
const mocks = vi.hoisted(() => ({ get: vi.fn(), submit: vi.fn(), cancel: vi.fn() }));
vi.mock("../core-api/secure-vault-client", () => ({ secureVaultClient: mocks }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ startDragging: vi.fn() }) }));
function render() {
  return mount(SecureVault, { global: { plugins: [createI18n({ legacy: false, locale: "en", missingWarn: false, fallbackWarn: false, messages: { en: {} } })] } });
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.submit.mockReturnValue(new Promise(() => undefined));
  mocks.cancel.mockResolvedValue(undefined);
});
describe("isolated Vault interaction", () => {
  it("requires matching create confirmation and clears secrets before the reply", async () => {
    mocks.get.mockResolvedValue({ id: "fixture", kind: "create" });
    const wrapper = render();
    await flushPromises();
    const inputs = wrapper.findAll('input[type="password"]');
    await inputs[0]!.setValue("密码密码");
    await inputs[1]!.setValue("different");
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).not.toHaveBeenCalled();
    await inputs[1]!.setValue("密码密码");
    await wrapper.find("form").trigger("submit");
    expect(mocks.submit).toHaveBeenCalledWith("", "密码密码", "密码密码", false);
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
});
