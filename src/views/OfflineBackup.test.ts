import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

import { i18n } from "../locales";
import { useTipsStore } from "../stores/tips";

const client = vi.hoisted(() => ({
  exportBackup: vi.fn(),
  openBackup: vi.fn(),
  previewBackup: vi.fn(),
  applyBackup: vi.fn(),
  discardBackup: vi.fn(),
  secureVault: vi.fn(),
  vaultStatus: vi.fn(),
}));

vi.mock("../core-api/offline-backup-client", () => ({
  exportOfflineBackup: client.exportBackup,
  openOfflineBackup: client.openBackup,
  previewOfflineBackup: client.previewBackup,
  applyOfflineBackup: client.applyBackup,
  discardOfflineBackup: client.discardBackup,
}));
vi.mock("../core-api/secure-vault-client", () => ({ requestSecureVault: client.secureVault }));
vi.mock("../core-api/client", () => ({ fetchVaultStatus: client.vaultStatus }));

import OfflineBackup from "./OfflineBackup.vue";

function mountBackup() {
  i18n.global.locale.value = "en";
  const pinia = createPinia();
  setActivePinia(pinia);
  return mount(OfflineBackup, { global: { plugins: [i18n, pinia] } });
}

describe("OfflineBackup", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    client.exportBackup.mockResolvedValue(true);
    client.discardBackup.mockResolvedValue(undefined);
    client.secureVault.mockResolvedValue(true);
    client.vaultStatus.mockResolvedValue({ state: "unlocked" });
  });

  it("exports connection metadata by default without opting into secrets", async () => {
    const wrapper = mountBackup();
    const button = wrapper.findAll("button").find((item) => item.text() === "Choose location and export");
    await button!.trigger("click");
    await flushPromises();

    expect(client.exportBackup).toHaveBeenCalledWith(
      { ssh: true, desktop: true, credentials: false, vault: false },
    );
    expect(wrapper.find("input[type=password]").exists()).toBe(false);
    wrapper.unmount();
  });

  it("reports a failed backup open through the shared Tips store", async () => {
    client.openBackup.mockRejectedValue(new Error("fixture failure"));
    const wrapper = mountBackup();
    await wrapper.findAll("button").find((item) => item.text() === "Choose backup file")!.trigger("click");
    await flushPromises();

    const message = i18n.global.t("offlineBackup.openFailed");
    expect(useTipsStore().items[0]).toEqual(expect.objectContaining({
      scope: "offline-backup", tone: "error", title: message,
    }));
    expect(wrapper.text()).not.toContain(message);
    useTipsStore().clearAll();
    wrapper.unmount();
  });

  it("offers only detected import categories and binds the preview to the selection", async () => {
    client.openBackup.mockResolvedValue({
      handle: "opened-file",
      inventory: {
        ssh: true,
        desktop: false,
        credentials: true,
        vault: true,
        hostCount: 2,
        desktopCount: 0,
        credentialCount: 1,
      },
    });
    client.previewBackup.mockResolvedValue({
      handle: "preview-handle",
      hostCount: 2,
      desktopProfileCount: 0,
      credentialCount: 0,
      duplicateHostCount: 0,
      duplicateDesktopProfileCount: 0,
      skippedCount: 0,
    });
    const wrapper = mountBackup();
    await wrapper.findAll("button").find((item) => item.text() === "Choose backup file")!.trigger("click");
    await flushPromises();

    expect(wrapper.findAll("fieldset")[1]?.text()).toContain("SSH configurations");
    expect(wrapper.findAll("fieldset")[1]?.text()).not.toContain("Remote desktop configurations");
    await wrapper.findAll("button").find((item) => item.text() === "Preview import")!.trigger("click");
    await flushPromises();
    expect(client.previewBackup).toHaveBeenCalledWith(
      "opened-file",
      { ssh: true, desktop: false, credentials: false, vault: false },
      "skip",
      null,
    );
    expect(client.secureVault).not.toHaveBeenCalled();
    wrapper.unmount();
    await flushPromises();
    expect(client.discardBackup).toHaveBeenCalledWith("opened-file");
  });

  it("previews full Vault contents as a merge when the local Vault is unlocked", async () => {
    client.openBackup.mockResolvedValue({
      handle: "opened-file",
      inventory: {
        ssh: false, desktop: false, credentials: false, vault: true,
        hostCount: 0, desktopCount: 0, credentialCount: 0,
      },
    });
    client.previewBackup.mockResolvedValue({
      handle: "preview-handle", hostCount: 0, desktopProfileCount: 0,
      credentialCount: 0, duplicateHostCount: 0, duplicateDesktopProfileCount: 0,
      skippedCount: 0,
    });
    const wrapper = mountBackup();
    await wrapper.findAll("button").find((item) => item.text() === "Choose backup file")!.trigger("click");
    await flushPromises();
    await wrapper.findAll("input[type=checkbox]").at(-1)!.setValue(true);
    await wrapper.findAll("button").find((item) => item.text() === "Preview import")!.trigger("click");
    await flushPromises();
    expect(client.previewBackup).toHaveBeenCalledWith(
      "opened-file",
      { ssh: false, desktop: false, credentials: false, vault: true },
      "skip",
      "merge",
    );
    expect(wrapper.text()).toContain("merged into the unlocked Vault");
    wrapper.unmount();
  });
});
