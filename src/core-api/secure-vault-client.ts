import { invoke } from "@tauri-apps/api/core";
import { secureInvoke } from "../secure-transport";

export type SecureVaultMode = "ensureUnlocked" | "unlockSavedLocal" | "enableAutoUnlock" | "enableLocalAutoUnlock";

export interface SecureVaultPrompt {
  id: string;
  kind: "create" | "unlock" | "enableAutoUnlock" | "enableLocalAutoUnlock";
  canReset: boolean;
}
export function requestSecureVault(mode: SecureVaultMode): Promise<boolean> {
  // Callable from the main window and from Core-owned surfaces (credential prompt, editors).
  return secureInvoke<boolean>("secure_vault_open", { mode });
}
export const secureVaultClient = {
  get: (id: string) => secureInvoke<SecureVaultPrompt>("secure_vault_get", { id }),
  submit: (id: string, password: string, passwordConfirmation: string, confirmed: boolean) =>
    secureInvoke<void>("secure_vault_submit", { id, password, passwordConfirmation, confirmed }),
  cancel: (id: string) => secureInvoke<void>("secure_vault_cancel", { id }),
  reset: (id: string, phrase: string, confirmed: boolean) =>
    secureInvoke<void>("secure_vault_reset", { id, phrase, confirmed }),
};

/** Resolve Vault dependencies in Core, including proxy and jump-host credentials. */
export function ensureHostVault(hostId: string, expectedHostStateVersion: string, includeOptionalCredentials = false): Promise<boolean> {
  return invoke<boolean>("secure_vault_ensure_for_host", { hostId, expectedHostStateVersion, includeOptionalCredentials });
}
