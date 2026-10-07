import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createI18n } from "vue-i18n";
import { secureWindowEn } from "../locales/secure-windows";
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
        // Only the secure-window strings are loaded: failure wording is asserted, everything else may render as keys.
        messages: { en: { secureWindow: secureWindowEn } },
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
    // While Core answers only the key text stays in this surface; the passphrase is gone.
    expect((key.element as HTMLTextAreaElement).value).toBe("-----BEGIN OPENSSH PRIVATE KEY-----");
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

  it.each([
    ["credential_transient.passphrase_required", "secureWindow.credential.passphraseRequired"],
    ["credential_transient.incorrect_passphrase", "secureWindow.credential.incorrectPassphrase"],
  ])("reports %s on the passphrase field and focuses it for the retry", async (code, messageKey) => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "privateKey", label: "ops@example.test", persistent: false, replacement: false });
    credential.submit.mockRejectedValueOnce({ code, category: "validation", retryStrategy: "waitForUser", messageKey, params: {}, requestId: null, diagnosticId: null, conflict: null });
    const wrapper = render();
    await flushPromises();
    await wrapper.get("#secure-credential-private-key").setValue("synthetic-private-key");
    await wrapper.get("#secure-credential-passphrase").setValue("synthetic-wrong-passphrase");
    await wrapper.find("form").trigger("submit");
    await flushPromises();
    await flushPromises();

    expect(document.activeElement?.id ?? "").not.toBe("secure-credential-private-key");
    expect(messageKey).toMatch(/^secureWindow\.credential\./);
    // Only the passphrase must be entered again; the key text is kept for the retry.
    expect((wrapper.get("#secure-credential-private-key").element as HTMLTextAreaElement).value).toBe("synthetic-private-key");
    expect((wrapper.get("#secure-credential-passphrase").element as HTMLInputElement).value).toBe("");
    expect(wrapper.get("#secure-credential-passphrase").attributes("aria-invalid")).toBe("true");
    expect(wrapper.find(".nvx-field__error").exists()).toBe(true);
    wrapper.unmount();
  });

  it("clears the kept key text once Core accepts the credential", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "privateKey", label: "ops@example.test", persistent: false, replacement: false });
    credential.submit.mockResolvedValueOnce(undefined);
    const wrapper = render();
    await flushPromises();
    await wrapper.get("#secure-credential-private-key").setValue("synthetic-private-key");
    await wrapper.find("form").trigger("submit");
    await flushPromises();
    expect((wrapper.get("#secure-credential-private-key").element as HTMLTextAreaElement).value).toBe("");
    wrapper.unmount();
  });

  it("clears the kept key text when the prompt lifetime ends or the window closes", async () => {
    vi.useFakeTimers();
    try {
      credential.get.mockResolvedValue({ id: "fixture", kind: "privateKey", label: "ops@example.test", persistent: false, replacement: false });
      credential.submit.mockRejectedValueOnce({ code: "credential_transient.incorrect_passphrase", category: "validation", retryStrategy: "waitForUser", messageKey: "secureWindow.credential.incorrectPassphrase", params: {}, requestId: null, diagnosticId: null, conflict: null });
      const wrapper = render();
      await flushPromises();
      await wrapper.get("#secure-credential-private-key").setValue("synthetic-private-key");
      await wrapper.find("form").trigger("submit");
      await flushPromises();
      const key = wrapper.get("#secure-credential-private-key").element as HTMLTextAreaElement;
      expect(key.value).toBe("synthetic-private-key");
      vi.advanceTimersByTime(180_000);
      await flushPromises();
      expect(key.value).toBe("");
      await wrapper.get("#secure-credential-private-key").setValue("synthetic-private-key");
      window.dispatchEvent(new Event("pagehide"));
      await flushPromises();
      expect(key.value).toBe("");
      wrapper.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it("clears both fields when the private key itself is invalid", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "privateKey", label: "ops@example.test", persistent: false, replacement: false });
    credential.submit.mockRejectedValueOnce({ code: "credential_transient.invalid_request", category: "validation", retryStrategy: "never", messageKey: "secureWindow.credential.invalidMaterial", params: {}, requestId: null, diagnosticId: null, conflict: null });
    const wrapper = render();
    await flushPromises();
    await wrapper.get("#secure-credential-private-key").setValue("not-a-key");
    await wrapper.find("form").trigger("submit");
    await flushPromises();

    expect((wrapper.get("#secure-credential-private-key").element as HTMLTextAreaElement).value).toBe("");
    expect(wrapper.get("#secure-credential-passphrase").attributes("aria-invalid")).not.toBe("true");
    // The prompt kind, not the shared Core code, names the problem: a key file that is not a usable key.
    expect(wrapper.text()).toContain("The private key is invalid or corrupted. Check that it is a complete OpenSSH private key.");
    expect(wrapper.text()).not.toContain("The password is invalid");
    wrapper.unmount();
  });

  it("keeps the password wording for invalid material on a password prompt", async () => {
    credential.get.mockResolvedValue({ id: "fixture", kind: "password", label: "ops@example.test", persistent: false, replacement: false });
    credential.submit.mockRejectedValueOnce({ code: "credential_transient.invalid_material", category: "validation", retryStrategy: "never", messageKey: "secureWindow.credential.invalidMaterial", params: {}, requestId: null, diagnosticId: null, conflict: null });
    const wrapper = render();
    await flushPromises();
    await wrapper.get("#secure-credential-password").setValue("x");
    await wrapper.find("form").trigger("submit");
    await flushPromises();
    expect(wrapper.text()).toContain("The password is invalid. Check it and try again.");
    expect(wrapper.text()).not.toContain("private key is invalid");
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
