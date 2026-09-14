import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";
import NvxDesktopPasswordFields from "./NvxDesktopPasswordFields.vue";
import { desktopEn } from "../../locales/desktop";

const api = vi.hoisted(() => ({
  cancelHostCreatePassword: vi.fn(), createVault: vi.fn(), fetchVaultStatus: vi.fn(),
  stageHostCreatePassword: vi.fn(), unlockVault: vi.fn(),
}));
vi.mock("../../core-api/client", () => api);
function mountFields() {
  return mount(NvxDesktopPasswordFields, {
    props: { newProfile: true, modelValue: null, credentials: [], label: "Fixture desktop", busy: false },
    global: {
      plugins: [createI18n({ legacy: false, locale: "en", messages: { en: { desktop: desktopEn } } })],
      stubs: {
        Teleport: true,
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
  api.fetchVaultStatus.mockResolvedValue({ state: "unlocked" });
  api.stageHostCreatePassword.mockResolvedValue({ stagedPasswordId: "staged-fixture" });
  api.cancelHostCreatePassword.mockResolvedValue({ cancelled: true });
  api.createVault.mockResolvedValue({ state: "unlocked" });
  api.unlockVault.mockResolvedValue({ state: "unlocked" });
});
describe("desktop creation password lifecycle", () => {
  it("leaves an empty password optional without opening Vault or staging a secret", async () => {
    const wrapper = mountFields();
    expect(await wrapper.vm.prepare()).toBeNull();
    expect(api.fetchVaultStatus).not.toHaveBeenCalled();
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

  it("creates Missing Vault with confirmation before resuming the same save", async () => {
    api.fetchVaultStatus.mockResolvedValue({ state: "missing" });
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    const saving = wrapper.vm.prepare(); await flushPromises();
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    await wrapper.get("#desktop-save-vault-password").setValue("synthetic-vault-password");
    await wrapper.get("#desktop-save-vault-confirmation").setValue("different-password");
    const submit = wrapper.findAll("button").find((button) => button.text() === desktopEn.createVault)!;
    expect(submit.attributes("disabled")).toBeDefined();
    await wrapper.get("#desktop-save-vault-confirmation").setValue("synthetic-vault-password");
    expect(wrapper.findAll("button").find((button) => button.text() === desktopEn.createVault)!.attributes("disabled")).toBeUndefined();
    await wrapper.findAll("button").find((button) => button.text() === desktopEn.createVault)!.trigger("click"); await flushPromises();
    expect(api.createVault).toHaveBeenCalledWith("synthetic-vault-password", "synthetic-vault-password");
    expect(api.unlockVault).not.toHaveBeenCalled();
    expect(await saving).toEqual(expect.objectContaining({ stagedPasswordId: "staged-fixture" }));
    expect(wrapper.find("#desktop-save-vault-password").exists()).toBe(false);
    wrapper.unmount();
  });

  it("cancels Locked Vault without staging or saving the desktop password", async () => {
    api.fetchVaultStatus.mockResolvedValue({ state: "locked" });
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    const saving = wrapper.vm.prepare(); await flushPromises();
    expect(wrapper.find("#desktop-save-vault-confirmation").exists()).toBe(false);
    const cancel = wrapper.findAll("button").find((button) => button.text() === desktopEn.cancel)!;
    await cancel.trigger("click");
    expect(await saving).toBe(false);
    expect((wrapper.get("#desktop-password").element as HTMLInputElement).value).toBe("");
    expect(api.createVault).not.toHaveBeenCalled();
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("uses unlock for RequiresReload and keeps secrets cleared after Vault failure", async () => {
    api.fetchVaultStatus.mockResolvedValue({ state: "requiresReload" });
    api.unlockVault.mockRejectedValue(new Error("fixture rejection"));
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("synthetic-desktop-password");
    const saving = wrapper.vm.prepare(); await flushPromises();
    await wrapper.get("#desktop-save-vault-password").setValue("synthetic-vault-password");
    await wrapper.findAll("button").find((button) => button.text() === desktopEn.unlockVault)!.trigger("click");
    await flushPromises();
    expect(api.unlockVault).toHaveBeenCalledTimes(1);
    expect((wrapper.get("#desktop-save-vault-password").element as HTMLInputElement).value).toBe("");
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
    expect(await saving).toBe(false);
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

  it("bounds UTF-8 bytes before opening Vault", async () => {
    const wrapper = mountFields();
    await wrapper.get("#desktop-password").setValue("字".repeat(1500));
    expect(await wrapper.vm.prepare()).toBe(false);
    expect(api.fetchVaultStatus).not.toHaveBeenCalled();
    expect(api.stageHostCreatePassword).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
