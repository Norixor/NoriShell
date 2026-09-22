import { invoke } from "@tauri-apps/api/core";

export type SecureCredentialKind = "password" | "privateKey";

export interface SecureCredentialRequest {
  kind: SecureCredentialKind;
  label: string;
  // A null identity prepares a short-lived credential for one connection.
  identityId: string | null;
  // Replacement keeps the existing opaque reference and is validated again by Core.
  credentialRefId?: string | null;
  expectedStateVersion?: string | null;
}

export interface SecureCredentialPrompt {
  id: string;
  kind: SecureCredentialKind;
  label: string;
  persistent: boolean;
  replacement: boolean;
}

/**
 * Opens Core's isolated credential prompt. The only result is an opaque
 * credential reference; password, private-key material, and passphrases
 * never cross back into the main WebView.
 */
export function requestSecureCredential(request: SecureCredentialRequest): Promise<string | null> {
  return invoke<string | null>("secure_credential_open", { request });
}

export const secureCredentialClient = {
  get: (id: string) => invoke<SecureCredentialPrompt>("secure_credential_get", { id }),
  submit: (id: string, secret: string, passphrase: string) => invoke<void>("secure_credential_submit", {
    id,
    secret,
    passphrase,
  }),
  cancel: (id: string) => invoke<void>("secure_credential_cancel", { id }),
};
