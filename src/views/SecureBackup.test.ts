import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../locales";

const secure = vi.hoisted(() => ({ get: vi.fn(), submit: vi.fn(), cancel: vi.fn() }));
vi.mock("../core-api/offline-backup-client", () => ({ secureBackupClient: secure }));

import SecureBackup from "./SecureBackup.vue";

describe("SecureBackup", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.history.replaceState({}, "", "/secure-backup.html?prompt=operation-1");
    i18n.global.locale.value = "en";
    secure.get.mockResolvedValue({ id: "operation-1", kind: "export" });
    secure.submit.mockResolvedValue(undefined);
  });

  it("collects and confirms a backup password in the isolated prompt", async () => {
    const wrapper = mount(SecureBackup, { global: { plugins: [i18n] } });
    await flushPromises();
    const submit = wrapper.findAll("button").find((button) => button.text() === "Continue")!;
    expect(submit.attributes("disabled")).toBeDefined();
    await wrapper.find("#secure-backup-password").setValue("backup-password-2026");
    await wrapper.find("#secure-backup-confirmation").setValue("backup-password-2026");
    await wrapper.find("input[type=checkbox]").setValue(true);
    await submit.trigger("click");
    await flushPromises();

    expect(secure.submit).toHaveBeenCalledWith(
      "operation-1",
      "backup-password-2026",
      "backup-password-2026",
      true,
    );
    expect((wrapper.find("#secure-backup-password").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });

  it("shows why a short export password cannot continue", async () => {
    const wrapper = mount(SecureBackup, { global: { plugins: [i18n] } });
    await flushPromises();
    const password = wrapper.find("#secure-backup-password");
    await password.setValue("七个字符不够啊");
    expect(password.attributes("aria-invalid")).toBe("true");
    expect(wrapper.text()).toContain("at least 8 characters");
    await password.setValue("八个字符已经够了");
    await wrapper.find("#secure-backup-confirmation").setValue("八个字符已经够了");
    await wrapper.find("input[type=checkbox]").setValue(true);
    expect(wrapper.findAll("button").at(-1)!.attributes("disabled")).toBeUndefined();
    wrapper.unmount();
  });

  it("explains that merging keeps the current Vault and requires the source password", async () => {
    secure.get.mockResolvedValue({ id: "operation-1", kind: "mergeVault" });
    const wrapper = mount(SecureBackup, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.text()).toContain("current Vault password will not change");
    expect(wrapper.text()).toContain("Other feature settings will not be restored automatically");
    expect(wrapper.findAll("button").at(-1)!.attributes("disabled")).toBeDefined();
    wrapper.unmount();
  });
});
