import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";
import NvxDesktopPasswordFields from "./NvxDesktopPasswordFields.vue";
import { desktopEn } from "../../locales/desktop";

const api = vi.hoisted(() => ({
  cancelHostCreatePassword: vi.fn(), stageHostCreatePassword: vi.fn(),
}));
const secureVault = vi.hoisted(() => ({ requestSecureVault: vi.fn() }));
vi.mock("../../core-api/client", () => api);
vi.mock("../../core-api/secure-vault-client", () => secureVault);
function mountFields() {
  return mount(NvxDesktopPasswordFields, {
    props: { newProfile: true, modelValue: null, credentials: [], label: "Fixture desktop", busy: false },
    global: {
      plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: desktopEn } } })],
      stubs: {
        NvxSelect: { props: ["modelValue", "options", "disabled"], emits: ["update:modelValue"], template: '<select :value="modelValue" :disabled="disabled" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="option in options" :key="option.value" :value="option.value">{{ option.label }}</option></select>' },
      },
    },
  });
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
beforeEach(() => {
  vi.resetAllMocks();
  secureVault.requestSecureVault.mockResolvedValue(true);
  api.stageHostCreatePassword.mockResolvedValue({ stagedPasswordId: "staged-fixture" });
  api.cancelHostCreatePassword.mockResolvedValue({ cancelled: true });
});
describe("desktop creation password lifecycle", () => {
  it("leaves an empty password optional without opening a protected Vault window or staging a secret", async () => {
    const wrapper = mountFields();
    expect(await wrapper.vm.prepare()).toBeNull();
    expect(secureVault.requestSecureVault).not.toHaveBeenCalled();
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
    expect(api.cancelHostCreatePassword).not.toHaveBeenCalled();
  });

  it("clears input before the stage acknowledgement and returns only an opaque stage", async () => {
    const waiting = deferred<{ stagedPasswordId: string }>();
    api.stageHostCreatePassword.mockReturnValue(waiting.promise);
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    const saving = wrapper.vm.prepare();
    await flushPromises();
    expect((wrapper.get("#desktop-password").element as HTMLInputElement).value).toBe("");
    expect(api.stageHostCreatePassword).toHaveBeenCalledTimes(1);
    expect(api.stageHostCreatePassword.mock.calls[0]![0].operationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    waiting.resolve({ stagedPasswordId: "staged-fixture" });
    const stage = await saving;
    expect(stage).toEqual({ operationId: expect.any(String), idempotencyKey: expect.any(String), stagedPasswordId: "staged-fixture" });
    expect(JSON.stringify(stage)).not.toContain("synthetic-desktop-password");
    expect(await wrapper.vm.prepare()).toEqual(stage);
    expect(api.stageHostCreatePassword).toHaveBeenCalledTimes(1);
    wrapper.vm.acceptSaved(); wrapper.unmount();
    expect(api.cancelHostCreatePassword).not.toHaveBeenCalled();
  });

  it("uses the protected Vault window before staging and never renders Vault password fields here", async () => {
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    expect(await wrapper.vm.prepare()).toEqual(expect.objectContaining({ stagedPasswordId: "staged-fixture" }));
    expect(secureVault.requestSecureVault).toHaveBeenCalledWith("ensureUnlocked");
    expect(wrapper.find("#desktop-save-vault-password").exists()).toBe(false);
    expect(wrapper.find("#desktop-save-vault-confirmation").exists()).toBe(false);
    wrapper.unmount();
  });

  it("cancels the protected Vault window without staging or saving the desktop password", async () => {
    secureVault.requestSecureVault.mockResolvedValue(false);
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    expect(await wrapper.vm.prepare()).toBe(false);
    expect((wrapper.get("#desktop-password").element as HTMLInputElement).value).toBe("");
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("keeps secrets cleared when the protected Vault window rejects", async () => {
    secureVault.requestSecureVault.mockRejectedValue(new Error("fixture rejection"));
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    expect(await wrapper.vm.prepare()).toBe(false);
    expect(secureVault.requestSecureVault).toHaveBeenCalledTimes(1);
    expect((wrapper.get("#desktop-password").element as HTMLInputElement).value).toBe("");
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("cleans a failed stage by its original operation even if the response was lost", async () => {
    api.stageHostCreatePassword.mockRejectedValue(new Error("fixture lost response"));
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    expect(await wrapper.vm.prepare()).toBe(false);
    const submitted = api.stageHostCreatePassword.mock.calls[0]![0];
    expect(api.cancelHostCreatePassword).toHaveBeenCalledWith({ operationId: submitted.operationId, idempotencyKey: submitted.idempotencyKey });
    expect(wrapper.text()).toContain(desktopEn.passwordSaveFailed);
    wrapper.unmount();
  });

  it("never treats a cleared failed password as an explicit choice to save without one", async () => {
    api.stageHostCreatePassword.mockRejectedValue(new Error("fixture lost response"));
    api.cancelHostCreatePassword.mockRejectedValue(new Error("fixture Vault locked"));
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    expect(await wrapper.vm.prepare()).toBe(false);
    expect(await wrapper.vm.prepare()).toBe(false);
    expect(api.stageHostCreatePassword).toHaveBeenCalledTimes(1);
    api.cancelHostCreatePassword.mockResolvedValue({ cancelled: true });
    expect(await wrapper.vm.prepare()).toBe(false);
    await wrapper.get("select").setValue(""); await flushPromises();
    expect(await wrapper.vm.prepare()).toBeNull();
    wrapper.unmount();
  });

  it("cleans a late stage after unmount with the original pair, not a reset draft", async () => {
    const waiting = deferred<{ stagedPasswordId: string }>();
    api.stageHostCreatePassword.mockReturnValue(waiting.promise);
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    const saving = wrapper.vm.prepare(); await flushPromises();
    const submitted = api.stageHostCreatePassword.mock.calls[0]![0];
    wrapper.unmount(); await flushPromises();
    waiting.resolve({ stagedPasswordId: "late-stage" });
    expect(await saving).toBe(false);
    expect(api.cancelHostCreatePassword).toHaveBeenLastCalledWith({ operationId: submitted.operationId, idempotencyKey: submitted.idempotencyKey });
    expect(api.cancelHostCreatePassword).toHaveBeenCalledTimes(2);
  });

  it("clears staged password when switching to ask-on-connect", async () => {
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    const stage = await wrapper.vm.prepare();
    await wrapper.get("select").setValue(""); await flushPromises();
    expect(api.cancelHostCreatePassword).toHaveBeenCalledWith(expect.objectContaining(stage === false || !stage ? {} : { operationId: stage.operationId, idempotencyKey: stage.idempotencyKey }));
    expect(wrapper.find("#desktop-password").exists()).toBe(false);
    expect(await wrapper.vm.prepare()).toBeNull();
    wrapper.unmount();
  });

  it("reports password staging as dirty and awaits its cleanup before allowing the editor to close", async () => {
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    expect(wrapper.emitted("dirty-change")?.at(-1)).toEqual([true]);
    const stage = await wrapper.vm.prepare();
    expect(stage).toEqual(expect.objectContaining({ stagedPasswordId: "staged-fixture" }));
    if (!stage) throw new Error("expected staged password");

    expect(await wrapper.vm.discard()).toBe(true);
    expect(api.cancelHostCreatePassword).toHaveBeenCalledWith(expect.objectContaining({
      operationId: stage.operationId,
      idempotencyKey: stage.idempotencyKey,
    }));
    expect(wrapper.emitted("dirty-change")?.at(-1)).toEqual([false]);
    wrapper.unmount();
  });

  it("bounds UTF-8 bytes before opening the protected Vault window", async () => {
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("字".repeat(1500));
    expect(await wrapper.vm.prepare()).toBe(false);
    expect(secureVault.requestSecureVault).not.toHaveBeenCalled();
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
