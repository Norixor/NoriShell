import { invoke } from "@tauri-apps/api/core";

export interface OfflineBackupSelection {
  ssh: boolean;
  desktop: boolean;
  credentials: boolean;
  vault: boolean;
}

export interface OfflineBackupInventory extends OfflineBackupSelection {
  hostCount: number;
  desktopCount: number;
  credentialCount: number;
}

export interface OpenedOfflineBackup {
  handle: string;
  inventory: OfflineBackupInventory;
}

export type OfflineDuplicatePolicy = "skip" | "copy";
export type OfflineVaultImportMode = "fresh" | "merge";

export interface OfflineBackupPreview {
  handle: string;
  hostCount: number;
  desktopProfileCount: number;
  credentialCount: number;
  duplicateHostCount: number;
  duplicateDesktopProfileCount: number;
  skippedCount: number;
}

export interface OfflineBackupApplyResult {
  hostCount: number;
  desktopProfileCount: number;
  credentialCount: number;
  skippedCount: number;
  vaultRestored: boolean;
}

export function exportOfflineBackup(
  selection: OfflineBackupSelection,
): Promise<boolean> {
  return invoke<boolean>("offline_backup_export", { selection });
}

export function openOfflineBackup(): Promise<OpenedOfflineBackup | null> {
  return invoke<OpenedOfflineBackup | null>("offline_backup_open");
}

export function previewOfflineBackup(
  handle: string,
  selection: OfflineBackupSelection,
  duplicatePolicy: OfflineDuplicatePolicy,
  vaultMode: OfflineVaultImportMode | null,
): Promise<OfflineBackupPreview> {
  return invoke<OfflineBackupPreview>("offline_backup_preview", { handle, selection, duplicatePolicy, vaultMode });
}

export function applyOfflineBackup(
  handle: string,
  previewHandle: string,
): Promise<OfflineBackupApplyResult> {
  return invoke<OfflineBackupApplyResult>("offline_backup_apply", { handle, previewHandle });
}

export interface SecureBackupPrompt {
  id: string;
  kind: "export" | "open" | "restoreVault" | "mergeVault";
}

export const secureBackupClient = {
  get: (id: string) => invoke<SecureBackupPrompt>("offline_backup_secure_get", { id }),
  submit: (id: string, password: string, passwordConfirmation: string, confirmed: boolean) =>
    invoke<void>("offline_backup_secure_submit", { id, password, passwordConfirmation, confirmed }),
  cancel: (id: string) => invoke<void>("offline_backup_secure_cancel", { id }),
};

export function discardOfflineBackup(handle: string): Promise<void> {
  return invoke<void>("offline_backup_discard", { handle });
}
