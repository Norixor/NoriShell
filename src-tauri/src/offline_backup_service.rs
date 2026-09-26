//! User-owned encrypted backups. The format is independent of plugin sync.

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_app_persistence::SshSyncChangeFence;
use norishell_core_api::{CoreApiError, ErrorCategory, RequestId, RetryStrategy, WireSequence};
use norishell_secret_vault::{SecretRef, VaultError};
use norishell_ssh_profile_sync::{
    MAX_OFFLINE_BACKUP_FILE_BYTES, PortableBundleV1, RecoveryPassword, decrypt_offline_backup,
    encrypt_offline_backup,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tauri::{AppHandle, Manager as _, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::oneshot;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::ssh_sync_exchange_local::{OfflineDuplicatePolicy, OfflineImportSelection};
use crate::{
    host_service::HostService,
    ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter,
    vault_service::{VaultEnvelopeImportError, VaultService, VaultServiceError},
};

#[cfg(unix)]
use std::fs::File;

const BACKUP_CONTENT_FORMAT: &str = "norishell-offline-content-v1";
const MAIN_WINDOW_LABEL: &str = "main";
const PENDING_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING: usize = 4;
const SECURE_PROMPT_LIFETIME: Duration = Duration::from_secs(180);
const MAX_SECURE_PASSWORD_BYTES: usize = 65_536;

#[derive(Debug, Serialize)]
#[serde(transparent)]
pub(crate) struct OfflineBackupCommandError(Box<CoreApiError>);

impl From<&str> for OfflineBackupCommandError {
    fn from(value: &str) -> Self {
        let (category, retry, message_key) = match value {
            "offline-backup-cancelled" => (
                ErrorCategory::Unavailable,
                RetryStrategy::Never,
                "offlineBackupErrors.cancelled",
            ),
            "offline-backup-empty-selection" | "offline-backup-invalid-selection" => (
                ErrorCategory::Validation,
                RetryStrategy::Never,
                "offlineBackup.invalidSelection",
            ),
            "offline-backup-invalid-password" => (
                ErrorCategory::Validation,
                RetryStrategy::Never,
                "offlineBackup.openPasswordLength",
            ),
            "offline-backup-invalid-content" | "offline-backup-open-failed" => (
                ErrorCategory::Validation,
                RetryStrategy::Never,
                "offlineBackup.openFailed",
            ),
            "offline-backup-too-many-open" => (
                ErrorCategory::Unavailable,
                RetryStrategy::AfterMilliseconds(1000),
                "offlineBackupErrors.tooManyOpen",
            ),
            "offline-backup-handle-expired" | "offline-backup-preview-expired" => (
                ErrorCategory::Conflict,
                RetryStrategy::RefreshSnapshot,
                "offlineBackup.previewFailed",
            ),
            "offline-backup-vault-already-exists" | "offline-backup-target-not-fresh" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "offlineBackup.targetNotFresh",
            ),
            "offline-backup-vault-locked" | "offline-backup-vault-unavailable" => (
                ErrorCategory::Permission,
                RetryStrategy::WaitForUser,
                "offlineBackup.vaultUnavailable",
            ),
            "offline-backup-vault-merge-uncertain" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "offlineBackup.vaultMergeUncertain",
            ),
            "offline-backup-vault-merge-failed" => (
                ErrorCategory::Internal,
                RetryStrategy::Never,
                "offlineBackup.vaultMergeFailed",
            ),
            "offline-backup-vault-restored-configs-failed" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "offlineBackup.vaultRestoredPartial",
            ),
            "offline-backup-vault-restore-failed" | "offline-backup-import-failed" => (
                ErrorCategory::Internal,
                RetryStrategy::Never,
                "offlineBackup.importFailed",
            ),
            "offline-backup-secure-denied" => (
                ErrorCategory::Permission,
                RetryStrategy::Never,
                "offlineBackup.secure.failed",
            ),
            "offline-backup-secure-expired" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "offlineBackupErrors.secureExpired",
            ),
            "offline-backup-preview-failed" => (
                ErrorCategory::Internal,
                RetryStrategy::RefreshSnapshot,
                "offlineBackup.previewFailed",
            ),
            "offline-backup-encryption-failed"
            | "offline-backup-export-failed"
            | "offline-backup-snapshot-failed"
            | "offline-backup-write-failed" => (
                ErrorCategory::Internal,
                RetryStrategy::Never,
                "offlineBackup.exportFailed",
            ),
            "offline-backup-snapshot-stale" => (
                ErrorCategory::Conflict,
                RetryStrategy::RefreshSnapshot,
                "offlineBackupErrors.snapshotChanged",
            ),
            "offline-backup-dialog-unavailable"
            | "offline-backup-secure-unavailable"
            | "offline-backup-unavailable" => (
                ErrorCategory::Unavailable,
                RetryStrategy::RefreshSnapshot,
                "offlineBackup.exportFailed",
            ),
            _ => {
                let diagnostic_id = Uuid::new_v4().to_string();
                eprintln!(
                    "offline backup command failed: diagnostic_id={diagnostic_id} reason={value}"
                );
                return Self(Box::new(CoreApiError::safe_internal(
                    RequestId::new(),
                    diagnostic_id,
                )));
            }
        };
        let mut error = crate::core_api_error::core_error(
            RequestId::new(),
            value,
            category,
            retry,
            message_key,
        );
        if matches!(
            value,
            "offline-backup-vault-merge-failed"
                | "offline-backup-vault-restore-failed"
                | "offline-backup-import-failed"
                | "offline-backup-preview-failed"
                | "offline-backup-encryption-failed"
                | "offline-backup-export-failed"
                | "offline-backup-snapshot-failed"
                | "offline-backup-write-failed"
        ) {
            let diagnostic_id = Uuid::new_v4().to_string();
            eprintln!(
                "offline backup operation failed: diagnostic_id={diagnostic_id} code={value}"
            );
            error.diagnostic_id = Some(diagnostic_id);
        }
        Self(error)
    }
}

impl From<String> for OfflineBackupCommandError {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OfflineBackupSelection {
    pub ssh: bool,
    pub desktop: bool,
    pub credentials: bool,
    pub vault: bool,
}

impl OfflineBackupSelection {
    fn validate(self) -> Result<(), &'static str> {
        if !self.ssh && !self.desktop && !self.vault {
            return Err("offline-backup-empty-selection");
        }
        if self.credentials && !self.ssh && !self.desktop {
            return Err("offline-backup-invalid-selection");
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OfflineBackupContents {
    format: String,
    pub selection: OfflineBackupSelection,
    pub portable: Option<PortableBundleV1>,
    pub vault_envelope: Option<String>,
}

impl OfflineBackupContents {
    pub(crate) fn new(
        selection: OfflineBackupSelection,
        portable: Option<PortableBundleV1>,
        vault_envelope: Option<&[u8]>,
    ) -> Result<Self, &'static str> {
        let contents = Self {
            format: BACKUP_CONTENT_FORMAT.to_owned(),
            selection,
            portable,
            vault_envelope: vault_envelope.map(|bytes| BASE64.encode(bytes)),
        };
        contents.validate()?;
        Ok(contents)
    }

    fn validate(&self) -> Result<(), &'static str> {
        self.selection.validate()?;
        if self.format != BACKUP_CONTENT_FORMAT
            || self.portable.is_some() != (self.selection.ssh || self.selection.desktop)
            || self.vault_envelope.is_some() != self.selection.vault
        {
            return Err("offline-backup-invalid-content");
        }
        if let Some(portable) = &self.portable {
            portable
                .validate()
                .map_err(|_| "offline-backup-invalid-content")?;
            if !self.selection.credentials
                && (!portable.secrets.is_empty() || !portable.objects.credentials.is_empty())
            {
                return Err("offline-backup-invalid-content");
            }
            if !self.selection.desktop && !portable.objects.desktop_profiles.is_empty() {
                return Err("offline-backup-invalid-content");
            }
        }
        if let Some(envelope) = &self.vault_envelope {
            if envelope.is_empty() {
                return Err("offline-backup-invalid-content");
            }
            let decoded = BASE64
                .decode(envelope)
                .map_err(|_| "offline-backup-invalid-content")?;
            if BASE64.encode(&decoded) != *envelope {
                return Err("offline-backup-invalid-content");
            }
        }
        Ok(())
    }

    pub(crate) fn vault_bytes(&self) -> Result<Option<Zeroizing<Vec<u8>>>, &'static str> {
        self.vault_envelope
            .as_ref()
            .map(|value| {
                BASE64
                    .decode(value)
                    .map(Zeroizing::new)
                    .map_err(|_| "offline-backup-invalid-content")
            })
            .transpose()
    }

    pub(crate) fn inventory(&self) -> OfflineBackupInventory {
        let portable = self.portable.as_ref();
        OfflineBackupInventory {
            // A desktop-only snapshot can carry SSH hosts as route or gateway
            // dependencies. Keep that implementation detail out of the user
            // inventory unless SSH roots were explicitly selected too.
            ssh: self.selection.ssh
                && portable.is_some_and(|bundle| !bundle.objects.hosts.is_empty()),
            desktop: self.selection.desktop
                && portable.is_some_and(|bundle| !bundle.objects.desktop_profiles.is_empty()),
            credentials: self.selection.credentials
                && portable.is_some_and(|bundle| !bundle.secrets.is_empty()),
            vault: self.selection.vault && self.vault_envelope.is_some(),
            host_count: if self.selection.ssh {
                portable.map_or(0, |bundle| bundle.objects.hosts.len())
            } else {
                0
            },
            desktop_count: if self.selection.desktop {
                portable.map_or(0, |bundle| bundle.objects.desktop_profiles.len())
            } else {
                0
            },
            credential_count: if self.selection.credentials {
                portable.map_or(0, |bundle| bundle.secrets.len())
            } else {
                0
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfflineBackupInventory {
    pub ssh: bool,
    pub desktop: bool,
    pub credentials: bool,
    pub vault: bool,
    pub host_count: usize,
    pub desktop_count: usize,
    pub credential_count: usize,
}

pub(crate) fn seal_contents(
    password: &RecoveryPassword,
    contents: &OfflineBackupContents,
) -> Result<Vec<u8>, &'static str> {
    contents.validate()?;
    let plaintext =
        Zeroizing::new(serde_json::to_vec(contents).map_err(|_| "offline-backup-invalid-content")?);
    encrypt_offline_backup(password, plaintext.as_slice())
        .map_err(|_| "offline-backup-encryption-failed")
}

pub(crate) fn open_contents(
    password: &RecoveryPassword,
    encoded: &[u8],
) -> Result<OfflineBackupContents, &'static str> {
    let plaintext =
        decrypt_offline_backup(password, encoded).map_err(|_| "offline-backup-open-failed")?;
    let contents: OfflineBackupContents = serde_json::from_slice(plaintext.as_slice())
        .map_err(|_| "offline-backup-invalid-content")?;
    contents.validate()?;
    Ok(contents)
}

struct PendingBackup {
    contents: OfflineBackupContents,
    archive_sha256: String,
    expires_at: Instant,
    prepared: Option<PreparedBackup>,
}

struct PreparedBackup {
    preview_handle: String,
    selection: OfflineBackupSelection,
    vault_mode: Option<OfflineVaultImportMode>,
    portable: bool,
    skipped_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OfflineVaultImportMode {
    Fresh,
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OfflineBackupSecureKind {
    Export,
    Open,
    RestoreVault,
    MergeVault,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfflineBackupSecurePrompt {
    id: String,
    kind: OfflineBackupSecureKind,
}

struct SecureBackupAnswer {
    password: Zeroizing<String>,
    password_confirmation: Zeroizing<String>,
    confirmed: bool,
}

struct PendingSecureBackupPrompt {
    prompt: OfflineBackupSecurePrompt,
    sender: oneshot::Sender<SecureBackupAnswer>,
}

#[derive(Clone, PartialEq, Eq)]
struct ExportSnapshotFence {
    database: SshSyncChangeFence,
    vault_id: Option<String>,
    vault_revision: Option<WireSequence>,
}

#[derive(Default)]
pub(crate) struct OfflineBackupService {
    pending: Mutex<BTreeMap<String, PendingBackup>>,
    secure_pending: Mutex<BTreeMap<String, PendingSecureBackupPrompt>>,
    operation: tokio::sync::Mutex<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenedOfflineBackup {
    handle: String,
    inventory: OfflineBackupInventory,
}

impl OfflineBackupService {
    fn stage(
        &self,
        contents: OfflineBackupContents,
        archive_sha256: String,
    ) -> Result<OpenedOfflineBackup, String> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        if pending.len() >= MAX_PENDING {
            return Err("offline-backup-too-many-open".into());
        }
        let handle = Uuid::new_v4().to_string();
        let inventory = contents.inventory();
        pending.insert(
            handle.clone(),
            PendingBackup {
                contents,
                archive_sha256,
                expires_at: Instant::now() + PENDING_LIFETIME,
                prepared: None,
            },
        );
        Ok(OpenedOfflineBackup { handle, inventory })
    }

    fn take(&self, handle: &str) -> Result<PendingBackup, String> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        pending
            .remove(handle)
            .ok_or_else(|| "offline-backup-handle-expired".to_owned())
    }

    fn take_prepared(&self, handle: &str, preview_handle: &str) -> Result<PendingBackup, String> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        let entry = pending
            .get(handle)
            .ok_or_else(|| "offline-backup-handle-expired".to_owned())?;
        let prepared = entry
            .prepared
            .as_ref()
            .ok_or_else(|| "offline-backup-preview-expired".to_owned())?;
        if prepared.preview_handle != preview_handle {
            return Err("offline-backup-preview-expired".into());
        }
        // Only consume the matching prepared preview. A secure prompt can
        // outlive a replacement preview, which must remain available to its
        // owner instead of being consumed by this stale apply request.
        pending
            .remove(handle)
            .ok_or_else(|| "offline-backup-handle-expired".to_owned())
    }

    fn vault_import_mode(
        &self,
        handle: &str,
        preview_handle: &str,
    ) -> Result<Option<OfflineVaultImportMode>, String> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        let entry = pending
            .get(handle)
            .ok_or_else(|| "offline-backup-handle-expired".to_owned())?;
        let prepared = entry
            .prepared
            .as_ref()
            .ok_or_else(|| "offline-backup-preview-expired".to_owned())?;
        if prepared.preview_handle != preview_handle {
            return Err("offline-backup-preview-expired".into());
        }
        Ok(prepared.vault_mode)
    }
}

fn secure_window_label(id: &str) -> String {
    format!("secure-backup-{id}")
}

fn secure_window_label_matches(id: &str, label: &str) -> bool {
    Uuid::parse_str(id).is_ok() && label == secure_window_label(id)
}

fn require_secure_window(window: &WebviewWindow, id: &str) -> Result<(), String> {
    if !secure_window_label_matches(id, window.label()) {
        return Err("offline-backup-secure-denied".into());
    }
    Ok(())
}

struct SecurePromptGuard {
    app: AppHandle,
    id: String,
}

impl Drop for SecurePromptGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self
            .app
            .state::<OfflineBackupService>()
            .secure_pending
            .lock()
        {
            pending.remove(&self.id);
        }
        if let Some(window) = self.app.get_webview_window(&secure_window_label(&self.id)) {
            let _ = window.destroy();
        }
    }
}

fn validate_secure_answer(
    kind: OfflineBackupSecureKind,
    answer: &SecureBackupAnswer,
) -> Result<(), String> {
    if answer.password.is_empty()
        || answer.password.len() > MAX_SECURE_PASSWORD_BYTES
        || answer.password_confirmation.len() > MAX_SECURE_PASSWORD_BYTES
    {
        return Err("offline-backup-secure-denied".into());
    }
    match kind {
        OfflineBackupSecureKind::Export => {
            if !answer.confirmed
                || answer.password.chars().count() < 8
                || answer.password.as_bytes() != answer.password_confirmation.as_bytes()
            {
                return Err("offline-backup-secure-denied".into());
            }
        }
        OfflineBackupSecureKind::Open if answer.password.len() < 8 => {
            return Err("offline-backup-secure-denied".into());
        }
        OfflineBackupSecureKind::RestoreVault | OfflineBackupSecureKind::MergeVault
            if !answer.confirmed =>
        {
            return Err("offline-backup-secure-denied".into());
        }
        OfflineBackupSecureKind::Open
        | OfflineBackupSecureKind::RestoreVault
        | OfflineBackupSecureKind::MergeVault => {}
    }
    Ok(())
}

async fn request_secure_password(
    kind: OfflineBackupSecureKind,
    owner: &WebviewWindow,
    app: &AppHandle,
    service: &OfflineBackupService,
) -> Result<Option<SecureBackupAnswer>, String> {
    if owner.label() != MAIN_WINDOW_LABEL {
        return Err("offline-backup-unavailable".into());
    }
    let id = Uuid::now_v7().to_string();
    let (sender, receiver) = oneshot::channel();
    service
        .secure_pending
        .lock()
        .map_err(|_| "offline-backup-secure-unavailable".to_owned())?
        .insert(
            id.clone(),
            PendingSecureBackupPrompt {
                prompt: OfflineBackupSecurePrompt {
                    id: id.clone(),
                    kind,
                },
                sender,
            },
        );
    let _guard = SecurePromptGuard {
        app: app.clone(),
        id: id.clone(),
    };
    let prompt_path = format!("secure-backup.html?prompt={id}");
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|url| url.join(&prompt_path).ok());
    let expected_query = format!("prompt={id}");
    let child = crate::secure_window_frame::apply_secure_window_frame(
        app,
        &secure_window_label(&id),
        WebviewWindowBuilder::new(
            app,
            secure_window_label(&id),
            WebviewUrl::App(prompt_path.into()),
        )
        .title("NoriShell"),
    )
    .inner_size(600.0, 460.0)
    .min_inner_size(480.0, 300.0)
    .on_navigation(move |url| {
        (cfg!(debug_assertions) && expected.as_ref().is_some_and(|expected| expected == url))
            || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost")
                    && url.port().is_none()))
                && url.path() == "/secure-backup.html"
                && url.query() == Some(expected_query.as_str()))
    })
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .build()
    .map_err(|_| "offline-backup-secure-unavailable".to_owned())?;
    let cancelled_app = app.clone();
    let cancelled_id = id.clone();
    child.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed)
            && let Ok(mut pending) = cancelled_app
                .state::<OfflineBackupService>()
                .secure_pending
                .lock()
        {
            pending.remove(&cancelled_id);
        }
    });
    let _ = crate::window_first_show::focus_if_revealed(&child);

    let mut receiver = receiver;
    let deadline = tokio::time::sleep(SECURE_PROMPT_LIFETIME);
    tokio::pin!(deadline);
    let mut owner_check = tokio::time::interval(Duration::from_millis(250));
    let owner_label = owner.label().to_owned();
    let owner_valid =
        || owner_label == MAIN_WINDOW_LABEL && app.get_webview_window(&owner_label).is_some();
    loop {
        tokio::select! {
            response = &mut receiver => return Ok(response.ok()),
            _ = &mut deadline => return Ok(None),
            _ = owner_check.tick() => if !owner_valid() { return Ok(None); },
        }
    }
}

fn capture_export_snapshot_fence(
    hosts: &HostService,
    vault: &VaultService,
) -> Result<ExportSnapshotFence, String> {
    let database = hosts
        .with_ssh_sync_repository(|repository| repository.ssh_sync_change_fence())
        .map_err(|_| "offline-backup-snapshot-failed".to_owned())?;
    let status = vault.status();
    Ok(ExportSnapshotFence {
        database,
        vault_id: status.vault_id,
        vault_revision: status.revision,
    })
}

fn export_snapshot_is_current(before: &ExportSnapshotFence, after: &ExportSnapshotFence) -> bool {
    before == after
}

#[tauri::command]
pub(crate) fn offline_backup_secure_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, OfflineBackupService>,
) -> Result<OfflineBackupSecurePrompt, OfflineBackupCommandError> {
    require_secure_window(&window, &id)?;
    service
        .secure_pending
        .lock()
        .map_err(|_| "offline-backup-secure-unavailable".to_owned())?
        .get(&id)
        .map(|entry| entry.prompt.clone())
        .ok_or_else(|| "offline-backup-secure-expired".into())
}

#[tauri::command]
pub(crate) fn offline_backup_secure_submit(
    id: String,
    password: String,
    password_confirmation: String,
    confirmed: bool,
    window: WebviewWindow,
    service: State<'_, OfflineBackupService>,
) -> Result<(), OfflineBackupCommandError> {
    require_secure_window(&window, &id)?;
    let kind = service
        .secure_pending
        .lock()
        .map_err(|_| "offline-backup-secure-unavailable".to_owned())?
        .get(&id)
        .map(|entry| entry.prompt.kind)
        .ok_or_else(|| "offline-backup-secure-expired".to_owned())?;
    let answer = SecureBackupAnswer {
        password: Zeroizing::new(password),
        password_confirmation: Zeroizing::new(password_confirmation),
        confirmed,
    };
    validate_secure_answer(kind, &answer)?;
    let pending = service
        .secure_pending
        .lock()
        .map_err(|_| "offline-backup-secure-unavailable".to_owned())?
        .remove(&id)
        .ok_or_else(|| "offline-backup-secure-expired".to_owned())?;
    pending
        .sender
        .send(answer)
        .map_err(|_| "offline-backup-secure-expired".into())
}

#[tauri::command]
pub(crate) fn offline_backup_secure_cancel(
    id: String,
    window: WebviewWindow,
    service: State<'_, OfflineBackupService>,
) -> Result<(), OfflineBackupCommandError> {
    require_secure_window(&window, &id)?;
    service
        .secure_pending
        .lock()
        .map_err(|_| "offline-backup-secure-unavailable".to_owned())?
        .remove(&id);
    Ok(())
}

#[tauri::command]
pub(crate) async fn offline_backup_discard(
    window: WebviewWindow,
    service: State<'_, OfflineBackupService>,
    adapter: State<'_, NoriShellSshSyncLocalAdapter>,
    handle: String,
) -> Result<(), OfflineBackupCommandError> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("offline-backup-unavailable".into());
    }
    let _operation = service.operation.lock().await;
    if let Ok(pending) = service.take(&handle)
        && let Some(prepared) = pending.prepared.filter(|prepared| prepared.portable)
    {
        adapter.discard_offline_import(&prepared.preview_handle);
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupImportPreview {
    handle: String,
    host_count: u32,
    desktop_profile_count: u32,
    credential_count: u32,
    duplicate_host_count: u32,
    duplicate_desktop_profile_count: u32,
    skipped_count: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupImportResult {
    host_count: u32,
    desktop_profile_count: u32,
    credential_count: u32,
    skipped_count: u32,
    vault_restored: bool,
}

fn import_selection_allowed(
    contents: &OfflineBackupContents,
    selection: OfflineBackupSelection,
) -> Result<(), String> {
    selection.validate().map_err(str::to_owned)?;
    let offered = contents.selection;
    if (selection.ssh && !offered.ssh)
        || (selection.desktop && !offered.desktop)
        || (selection.credentials && !offered.credentials)
        || (selection.vault && !offered.vault)
    {
        return Err("offline-backup-invalid-selection".into());
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn offline_backup_preview(
    window: WebviewWindow,
    service: State<'_, OfflineBackupService>,
    adapter: State<'_, NoriShellSshSyncLocalAdapter>,
    vault: State<'_, VaultService>,
    handle: String,
    selection: OfflineBackupSelection,
    duplicate_policy: OfflineDuplicatePolicy,
    vault_mode: Option<OfflineVaultImportMode>,
) -> Result<BackupImportPreview, OfflineBackupCommandError> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("offline-backup-unavailable".into());
    }
    let _operation = service.operation.lock().await;
    if selection.vault != vault_mode.is_some() {
        return Err("offline-backup-invalid-selection".into());
    }
    let vault_state = vault.status().state;
    let (bundle, digest, old_prepared) = {
        let mut pending = service
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        let entry = pending
            .get_mut(&handle)
            .ok_or_else(|| "offline-backup-handle-expired".to_owned())?;
        import_selection_allowed(&entry.contents, selection)?;
        match vault_mode {
            Some(OfflineVaultImportMode::Fresh)
                if vault_state != norishell_core_api::VaultState::Missing =>
            {
                return Err("offline-backup-vault-already-exists".into());
            }
            Some(OfflineVaultImportMode::Merge)
                if vault_state != norishell_core_api::VaultState::Unlocked =>
            {
                return Err("offline-backup-vault-locked".into());
            }
            _ => {}
        }
        let old = entry.prepared.take();
        (
            entry.contents.portable.clone(),
            entry.archive_sha256.clone(),
            old,
        )
    };
    if let Some(old) = old_prepared.filter(|prepared| prepared.portable) {
        adapter.discard_offline_import(&old.preview_handle);
    }
    let portable = selection.ssh || selection.desktop;
    let result = if portable {
        let bundle = bundle.ok_or_else(|| "offline-backup-invalid-content".to_owned())?;
        let preview = adapter
            .prepare_offline_import(
                bundle,
                &digest,
                OfflineImportSelection {
                    include_ssh: selection.ssh,
                    include_desktop: selection.desktop,
                    include_credentials: selection.credentials,
                    duplicate_policy,
                },
            )
            .map_err(|_| "offline-backup-preview-failed".to_owned())?;
        BackupImportPreview {
            handle: preview.handle,
            host_count: preview.host_count,
            desktop_profile_count: preview.desktop_profile_count,
            credential_count: preview.credential_count,
            duplicate_host_count: preview.duplicate_host_count,
            duplicate_desktop_profile_count: preview.duplicate_desktop_profile_count,
            skipped_count: preview.skipped_count,
        }
    } else {
        BackupImportPreview {
            handle: Uuid::new_v4().to_string(),
            host_count: 0,
            desktop_profile_count: 0,
            credential_count: 0,
            duplicate_host_count: 0,
            duplicate_desktop_profile_count: 0,
            skipped_count: 0,
        }
    };
    let mut pending = service
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = pending.get_mut(&handle) else {
        if portable {
            adapter.discard_offline_import(&result.handle);
        }
        return Err("offline-backup-handle-expired".into());
    };
    entry.prepared = Some(PreparedBackup {
        preview_handle: result.handle.clone(),
        selection,
        vault_mode,
        portable,
        skipped_count: result.skipped_count,
    });
    Ok(result)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn offline_backup_apply(
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, OfflineBackupService>,
    adapter: State<'_, NoriShellSshSyncLocalAdapter>,
    vault: State<'_, VaultService>,
    hosts: State<'_, HostService>,
    handle: String,
    preview_handle: String,
) -> Result<BackupImportResult, OfflineBackupCommandError> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("offline-backup-unavailable".into());
    }
    // Prompt before taking the prepared import. Cancellation must leave both
    // the archive handle and its portable restore plan available to retry.
    let vault_mode = service.vault_import_mode(&handle, &preview_handle)?;
    let secure_answer = if let Some(vault_mode) = vault_mode {
        request_secure_password(
            match vault_mode {
                OfflineVaultImportMode::Fresh => OfflineBackupSecureKind::RestoreVault,
                OfflineVaultImportMode::Merge => OfflineBackupSecureKind::MergeVault,
            },
            &window,
            &app,
            service.inner(),
        )
        .await?
        .ok_or_else(|| "offline-backup-cancelled".to_owned())
        .map(Some)?
    } else {
        None
    };
    let _operation = service.operation.lock().await;
    let pending = service.take_prepared(&handle, &preview_handle)?;
    let prepared = pending
        .prepared
        .ok_or_else(|| "offline-backup-preview-expired".to_owned())?;
    debug_assert_eq!(prepared.preview_handle, preview_handle);
    if prepared.vault_mode != vault_mode || prepared.selection.vault != vault_mode.is_some() {
        if prepared.portable {
            adapter.discard_offline_import(&prepared.preview_handle);
        }
        return Err("offline-backup-preview-expired".into());
    }
    let mut vault_restored = false;
    if let Some(vault_mode) = vault_mode {
        let envelope = pending
            .contents
            .vault_bytes()
            .map_err(str::to_owned)?
            .ok_or_else(|| "offline-backup-invalid-content".to_owned())?;
        let password = secure_answer
            .as_ref()
            .ok_or_else(|| "offline-backup-preview-expired".to_owned())?;
        let result = match vault_mode {
            OfflineVaultImportMode::Fresh => vault
                .import_encrypted_envelope_if_missing_with_fresh_target(
                    envelope.as_slice(),
                    password.password.as_bytes(),
                    |commit| {
                        hosts.with_ssh_sync_repository(|repository| {
                            repository.with_fresh_database_guard(commit)
                        })
                    },
                )
                .map(|_| ())
                .map_err(|failure| match failure {
                    VaultEnvelopeImportError::FreshTarget(
                        norishell_app_persistence::AppPersistenceError::DatabaseNotFresh,
                    ) => "offline-backup-target-not-fresh".to_owned(),
                    _ => "offline-backup-vault-restore-failed".to_owned(),
                }),
            OfflineVaultImportMode::Merge => {
                let reserved = hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.reserved_vault_secret_ref_ids()
                    })
                    .map_err(|_| "offline-backup-vault-merge-failed".to_owned())?
                    .into_iter()
                    .map(|id| SecretRef::parse(id.as_str()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "offline-backup-vault-merge-failed".to_owned())?;
                vault
                    .merge_encrypted_envelope(
                        envelope.as_slice(),
                        password.password.as_bytes(),
                        &reserved,
                    )
                    .map(|_| ())
                    .map_err(|error| match error {
                        VaultServiceError::Vault(VaultError::CommitStateUnknown(_))
                        | VaultServiceError::Vault(VaultError::ReloadRequired) => {
                            "offline-backup-vault-merge-uncertain".to_owned()
                        }
                        _ => "offline-backup-vault-merge-failed".to_owned(),
                    })
            }
        };
        if let Err(failure) = result {
            if prepared.portable {
                adapter.discard_offline_import(&preview_handle);
            }
            return Err(failure.into());
        }
        vault_restored = true;
    }
    let mut result = BackupImportResult {
        host_count: 0,
        desktop_profile_count: 0,
        credential_count: 0,
        skipped_count: prepared.skipped_count,
        vault_restored,
    };
    if prepared.portable {
        let applied = adapter
            .commit_offline_import(&preview_handle)
            .await
            .map_err(|_| {
                if vault_restored {
                    "offline-backup-vault-restored-configs-failed".to_owned()
                } else {
                    "offline-backup-import-failed".to_owned()
                }
            })?;
        result.host_count = applied.host_count;
        result.desktop_profile_count = applied.desktop_profile_count;
        result.credential_count = applied.credential_count;
    }
    Ok(result)
}

fn recovery_password(password: &[u8]) -> Result<RecoveryPassword, String> {
    RecoveryPassword::new(password.to_vec())
        .map_err(|_| "offline-backup-invalid-password".to_owned())
}

fn write_backup_atomically(path: PathBuf, encoded: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "offline-backup-write-failed".to_owned())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "offline-backup-write-failed".to_owned())?;
    temporary
        .write_all(encoded)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| "offline-backup-write-failed".to_owned())?;
    temporary
        .persist(&path)
        .map_err(|_| "offline-backup-write-failed".to_owned())?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "offline-backup-write-failed".to_owned())?;
    Ok(())
}

fn read_backup_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "offline-backup-open-failed".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "offline-backup-open-failed".to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_OFFLINE_BACKUP_FILE_BYTES as u64 {
        return Err("offline-backup-open-failed".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_OFFLINE_BACKUP_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "offline-backup-open-failed".to_owned())?;
    if bytes.len() > MAX_OFFLINE_BACKUP_FILE_BYTES {
        return Err("offline-backup-open-failed".into());
    }
    Ok(bytes)
}

async fn choose_backup_path(
    window: &WebviewWindow,
    export: bool,
) -> Result<Option<PathBuf>, String> {
    let (sender, receiver) = oneshot::channel();
    let dialog = window
        .app_handle()
        .dialog()
        .file()
        .set_parent(window)
        .add_filter("NoriShell Backup", &["norishell-backup"]);
    if export {
        dialog
            .set_file_name("NoriShell.norishell-backup")
            .save_file(move |path| {
                let _ = sender.send(path);
            });
    } else {
        dialog.pick_file(move |path| {
            let _ = sender.send(path);
        });
    }
    receiver
        .await
        .map_err(|_| "offline-backup-dialog-unavailable".to_owned())?
        .map(|path| {
            path.into_path()
                .map_err(|_| "offline-backup-dialog-unavailable".to_owned())
        })
        .transpose()
}

#[tauri::command]
pub(crate) async fn offline_backup_export(
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, OfflineBackupService>,
    adapter: State<'_, NoriShellSshSyncLocalAdapter>,
    vault: State<'_, VaultService>,
    hosts: State<'_, HostService>,
    selection: OfflineBackupSelection,
) -> Result<bool, OfflineBackupCommandError> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("offline-backup-unavailable".into());
    }
    selection.validate().map_err(str::to_owned)?;
    let Some(path) = choose_backup_path(&window, true).await? else {
        return Ok(false);
    };
    let Some(answer) = request_secure_password(
        OfflineBackupSecureKind::Export,
        &window,
        &app,
        service.inner(),
    )
    .await?
    else {
        return Ok(false);
    };
    let password = recovery_password(answer.password.as_bytes())?;
    let before = capture_export_snapshot_fence(hosts.inner(), vault.inner())?;
    let adapter = adapter.inner().clone();
    let vault_service = vault.inner().clone();
    let vault_after_snapshot = vault_service.clone();
    let encrypted = tauri::async_runtime::spawn_blocking(move || {
        let portable = if selection.ssh || selection.desktop {
            Some(
                adapter
                    .build_offline_portable_snapshot(
                        selection.ssh,
                        selection.desktop,
                        selection.credentials,
                    )
                    .map_err(|_| "offline-backup-snapshot-failed".to_owned())?
                    .bundle,
            )
        } else {
            None
        };
        let vault_envelope = if selection.vault {
            Some(
                vault_service
                    .export_encrypted_envelope()
                    .map_err(|_| "offline-backup-vault-unavailable".to_owned())?,
            )
        } else {
            None
        };
        let contents = OfflineBackupContents::new(selection, portable, vault_envelope.as_deref())
            .map_err(str::to_owned)?;
        seal_contents(&password, &contents)
            .map(Zeroizing::new)
            .map_err(str::to_owned)
    })
    .await
    .map_err(|_| "offline-backup-export-failed".to_owned())??;
    if !export_snapshot_is_current(
        &before,
        &capture_export_snapshot_fence(hosts.inner(), &vault_after_snapshot)?,
    ) {
        return Err("offline-backup-snapshot-stale".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        write_backup_atomically(path, encrypted.as_slice())
    })
    .await
    .map_err(|_| "offline-backup-export-failed".to_owned())??;
    Ok(true)
}

#[tauri::command]
pub(crate) async fn offline_backup_open(
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, OfflineBackupService>,
) -> Result<Option<OpenedOfflineBackup>, OfflineBackupCommandError> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("offline-backup-unavailable".into());
    }
    let Some(path) = choose_backup_path(&window, false).await? else {
        return Ok(None);
    };
    let Some(answer) = request_secure_password(
        OfflineBackupSecureKind::Open,
        &window,
        &app,
        service.inner(),
    )
    .await?
    else {
        return Ok(None);
    };
    let password = recovery_password(answer.password.as_bytes())?;
    let (contents, digest) = tauri::async_runtime::spawn_blocking(move || {
        let bytes = read_backup_bounded(&path)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let contents = open_contents(&password, &bytes).map_err(str::to_owned)?;
        Ok::<_, String>((contents, digest))
    })
    .await
    .map_err(|_| "offline-backup-open-failed".to_owned())??;
    service
        .stage(contents, digest)
        .map(Some)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_ssh_profile_sync::{
        BundleSchema, PortableHost, PortableObjectId, PortableObjects,
    };

    #[test]
    fn partial_vault_restore_is_not_a_generic_import_failure() {
        let partial =
            OfflineBackupCommandError::from("offline-backup-vault-restored-configs-failed");
        let failed = OfflineBackupCommandError::from("offline-backup-import-failed");
        assert_eq!(
            partial.0.code,
            "offline-backup-vault-restored-configs-failed"
        );
        assert_eq!(partial.0.message_key, "offlineBackup.vaultRestoredPartial");
        assert_ne!(partial.0.message_key, failed.0.message_key);
    }

    fn answer(password: &str, confirmation: &str, confirmed: bool) -> SecureBackupAnswer {
        SecureBackupAnswer {
            password: Zeroizing::new(password.to_owned()),
            password_confirmation: Zeroizing::new(confirmation.to_owned()),
            confirmed,
        }
    }

    fn portable_bundle_with_host_dependency() -> PortableBundleV1 {
        let dependency = PortableHost {
            id: PortableObjectId::new(),
            label: "desktop route dependency".to_owned(),
            address: "gateway.example.test".to_owned(),
            port: 22,
            username: None,
            favorite: false,
            tags: std::collections::BTreeSet::new(),
            identity_id: None,
            route_id: PortableObjectId::new(),
            authentication_plan_id: PortableObjectId::new(),
            algorithm_policy_id: PortableObjectId::new(),
            heartbeat_policy_id: PortableObjectId::new(),
            monitoring_policy_id: PortableObjectId::new(),
            login_automation_id: None,
        };
        PortableBundleV1 {
            schema: BundleSchema::V3,
            selected_categories: None,
            revision: 1,
            objects: PortableObjects {
                hosts: vec![dependency],
                ..PortableObjects::default()
            },
            secrets: Vec::new(),
            skipped_machine_bound: Vec::new(),
            tombstones: Vec::new(),
            preferences: None,
            update_times: Vec::new(),
            preference_update_times: Default::default(),
        }
    }

    #[test]
    fn vault_only_round_trip_is_detected_after_decryption() {
        let password = RecoveryPassword::new(b"backup-password-2026".to_vec()).unwrap();
        let selection = OfflineBackupSelection {
            ssh: false,
            desktop: false,
            credentials: false,
            vault: true,
        };
        let contents = OfflineBackupContents::new(selection, None, Some(b"encrypted vault"))
            .expect("valid content");
        let encrypted = seal_contents(&password, &contents).expect("seal backup");
        assert!(!String::from_utf8_lossy(&encrypted).contains("encrypted vault"));
        let opened = open_contents(&password, &encrypted).expect("open backup");
        assert!(opened.inventory().vault);
        assert!(!opened.inventory().ssh);
        assert_eq!(
            opened.vault_bytes().unwrap().unwrap().as_slice(),
            b"encrypted vault"
        );
    }

    #[test]
    fn rejects_missing_or_unselected_sections() {
        let selection = OfflineBackupSelection {
            ssh: true,
            desktop: false,
            credentials: false,
            vault: false,
        };
        assert!(OfflineBackupContents::new(selection, None, None).is_err());
        let selection = OfflineBackupSelection {
            ssh: false,
            desktop: false,
            credentials: true,
            vault: false,
        };
        assert!(OfflineBackupContents::new(selection, None, None).is_err());
    }

    #[test]
    fn inventory_does_not_offer_desktop_dependencies_as_ssh() {
        let contents = OfflineBackupContents {
            format: BACKUP_CONTENT_FORMAT.to_owned(),
            selection: OfflineBackupSelection {
                ssh: false,
                desktop: true,
                credentials: false,
                vault: false,
            },
            portable: Some(portable_bundle_with_host_dependency()),
            vault_envelope: None,
        };
        let inventory = contents.inventory();
        assert!(!inventory.ssh);
        assert_eq!(inventory.host_count, 0);
        assert!(!inventory.desktop);
        assert_eq!(inventory.desktop_count, 0);
    }

    #[test]
    fn secure_prompt_requires_the_exact_generated_window_label() {
        let id = Uuid::new_v4().to_string();
        assert!(secure_window_label_matches(&id, &secure_window_label(&id)));
        assert!(!secure_window_label_matches(&id, "main"));
        assert!(!secure_window_label_matches(
            "not-a-prompt",
            "secure-backup-not-a-prompt"
        ));
    }

    #[test]
    fn secure_answers_enforce_each_prompt_contract() {
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::Export,
                &answer("七个字符不够啊", "七个字符不够啊", true),
            )
            .is_err()
        );
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::Export,
                &answer("八个字符已经够了", "八个字符已经够了", true),
            )
            .is_ok()
        );
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::Export,
                &answer("twelve-bytes", "twelve-bytes", true),
            )
            .is_ok()
        );
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::Export,
                &answer("twelve-bytes", "different-value", true),
            )
            .is_err()
        );
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::RestoreVault,
                &answer("existing-vault-password", "", false),
            )
            .is_err()
        );
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::MergeVault,
                &answer("existing-vault-password", "", false),
            )
            .is_err()
        );
        assert!(
            validate_secure_answer(
                OfflineBackupSecureKind::Open,
                &answer("backup-password", "", false),
            )
            .is_ok()
        );
    }

    #[test]
    fn vault_restore_prompt_preflight_keeps_the_matching_prepared_backup() {
        let service = OfflineBackupService::default();
        let opened = service
            .stage(
                OfflineBackupContents {
                    format: BACKUP_CONTENT_FORMAT.to_owned(),
                    selection: OfflineBackupSelection {
                        ssh: false,
                        desktop: false,
                        credentials: false,
                        vault: true,
                    },
                    portable: None,
                    vault_envelope: Some("opaque-vault-envelope".to_owned()),
                },
                "0".repeat(64),
            )
            .expect("stage vault backup");
        let preview_handle = "vault-preview".to_owned();
        let prepared = PreparedBackup {
            preview_handle: preview_handle.clone(),
            selection: OfflineBackupSelection {
                ssh: false,
                desktop: false,
                credentials: false,
                vault: true,
            },
            vault_mode: Some(OfflineVaultImportMode::Fresh),
            portable: false,
            skipped_count: 0,
        };
        service
            .pending
            .lock()
            .expect("pending lock")
            .get_mut(&opened.handle)
            .expect("staged backup")
            .prepared = Some(prepared);
        assert!(
            service
                .vault_import_mode(&opened.handle, &preview_handle)
                .expect("prepared vault restore")
                == Some(OfflineVaultImportMode::Fresh)
        );
        assert!(
            service
                .pending
                .lock()
                .expect("pending lock")
                .contains_key(&opened.handle)
        );
    }

    #[test]
    fn export_snapshot_fence_rejects_database_or_vault_changes() {
        let before = ExportSnapshotFence {
            database: SshSyncChangeFence {
                business_generation: 10,
            },
            vault_id: Some("vault-a".to_owned()),
            vault_revision: Some(WireSequence::new(7)),
        };
        assert!(export_snapshot_is_current(&before, &before));

        let mut changed_database = before.clone();
        changed_database.database.business_generation += 1;
        assert!(!export_snapshot_is_current(&before, &changed_database));

        let mut changed_vault = before.clone();
        changed_vault.vault_revision = Some(WireSequence::new(8));
        assert!(!export_snapshot_is_current(&before, &changed_vault));

        let mut changed_vault_id = before.clone();
        changed_vault_id.vault_id = Some("vault-b".to_owned());
        assert!(!export_snapshot_is_current(&before, &changed_vault_id));
    }
}
