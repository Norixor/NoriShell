import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import SecureCredential from "./SecureCredential.vue";

const credential = vi.hoisted(() => ({ get: vi.fn(), submit: vi.fn(), cancel: vi.fn() }));
const vault = vi.hoisted(() => vi.fn());
vi.mock("../core-api/secure-credential-client", () => ({ secureCredentialClient: credential }));
vi.mock("../core-api/secure-vault-client", () => ({ requestSecureVault: vault }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ startDragging: vi.fn() }) }));

function render() {
  return mount(SecureCredential, {
    global: {
      plugins: [createI18n({
        legacy: false,
        locale: "en",
        missingWarn: false,
        fallbackWarn: false,
        messages: { en: {} },
      })],
    },
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  credential.submit.mockReturnValue(new Promise(() => undefined));
  credential.cancel.mockResolvedValue(undefined);
  vault.mockResolvedValue(true);
});

describe("isolated SSH credential interaction", () => {
  it("submits a one-time password and clears it before Core receives a result", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "password", label: "ops@example.test", persistent: false, replacement: false });
    const wrapper = render();
    await flushPromises();
    const password = wrapper.get("#secure-credential-password");
    await password.setValue("synthetic-test-password");
    await wrapper.find("form").trigger("submit");

    expect(credential.submit).toHaveBeenCalledWith("", "synthetic-test-password", "");
    expect((password.element as HTMLInputElement).value).toBe("");
    expect(vault).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("uses the Vault prompt before submitting a persistent private key", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "privateKey", label: "ops@example.test", persistent: true, replacement: false });
    const wrapper = render();
    await flushPromises();
    const key = wrapper.get("#secure-credential-private-key");
    const passphrase = wrapper.get("#secure-credential-passphrase");
    await key.setValue("-----BEGIN OPENSSH PRIVATE KEY-----");
    await passphrase.setValue("synthetic-passphrase");
    await wrapper.find("form").trigger("submit");
    await flushPromises();

    expect(vault).toHaveBeenCalledWith("ensureUnlocked");
    expect(credential.submit).toHaveBeenCalledWith("", "-----BEGIN OPENSSH PRIVATE KEY-----", "synthetic-passphrase");
    expect((key.element as HTMLTextAreaElement).value).toBe("");
    expect((passphrase.element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });

  it("does not submit when the nested Vault prompt is cancelled", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "password", label: "ops@example.test", persistent: true, replacement: false });
    vault.mockResolvedValue(false);
    const wrapper = render();
    await flushPromises();
    await wrapper.get("#secure-credential-password").setValue("synthetic-test-password");
    await wrapper.find("form").trigger("submit");
    await flushPromises();

    expect(credential.submit).not.toHaveBeenCalled();
    expect((wrapper.get("#secure-credential-password").element as HTMLInputElement).value)
      .toBe("synthetic-test-password");
    wrapper.unmount();
  });

  it("cancels without submitting secret material", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "password", label: "ops@example.test", persistent: false, replacement: false });
    const wrapper = render();
    await flushPromises();
    await wrapper.get("#secure-credential-password").setValue("synthetic-test-password");
    await wrapper.findAll("button").find((button) => button.text() === "sshTerminal.cancel")!.trigger("click");

    expect(credential.cancel).toHaveBeenCalledWith("");
    expect(credential.submit).not.toHaveBeenCalled();
    expect((wrapper.get("#secure-credential-password").element as HTMLInputElement).value).toBe("");
    wrapper.unmount();
  });
});
