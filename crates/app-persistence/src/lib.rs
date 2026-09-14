//! Application-level SQLite persistence for SSH-first non-secret metadata.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{
    AgentIdentityKind, AgentIdentitySource, AlgorithmCategory, AlgorithmCompatibilityException,
    AlgorithmPolicySummary, AuthenticationMethodKind, AuthenticationPlanMode,
    AuthenticationPlanSummary, CredentialKind, CredentialRefDetails, CredentialRefId,
    CredentialRefSummary, DesktopProfile, DiskResourceId, HeartbeatPolicy, HeartbeatPolicySummary,
    HostCatalogEntry, HostCatalogSort, HostConfiguredCreateRequest, HostConfiguredCreateResponse,
    HostConnectionConfigSummary, HostCreateLoginAutomationStep, HostCreatePasswordCancelResponse,
    HostCreatePasswordStageId, HostGroupId, HostGroupSummary, HostId, HostOrganizationSummary,
    HostSummary, HostTagId, HostTagSummary, IdentityDeleteImpact, IdentityDeleteResponse,
    IdentityId, IdentitySummary, KnownHostDeleteResponse, KnownHostId, KnownHostSummary,
    LoginAutomationSecretCancelResponse, LoginAutomationSecretStageId, LoginAutomationStepInput,
    LoginAutomationStepSummary, LoginAutomationSummary, MonitoringPolicy, MonitoringPolicySummary,
    NetworkResourceId, OperationId, PluginCapability, PluginId, PluginInstallState,
    PluginOperationId, PluginOperationKind, PluginOperationState, ProxyDnsMode, ProxyEndpoint,
    RecentConnectionSummary, RouteIngress, RoutePlanSummary, SecretRefId, ShellHeartbeatLineEnding,
    SshAgentScope, SshCertificateMetadata, SshCertificateType, TerminalWorkspaceLayout,
    TerminalWorkspaceLayoutSnapshot, WireSequence,
};
use norishell_ssh_domain::{Endpoint, EndpointError, ssh_sha256_fingerprint};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use thiserror::Error;

mod desktop_preferences;
mod desktop_profiles;
mod forward_rules;
pub use desktop_preferences::*;
pub use desktop_profiles::validate_desktop_profile;
mod migrations;
mod plugin_credentials;
mod plugin_operation_permissions;
mod plugin_permissions;
mod plugin_private_storage;
mod plugin_settings;
mod plugin_tasks;
pub use plugin_credentials::*;
pub use plugin_operation_permissions::*;
pub use plugin_permissions::*;
pub use plugin_private_storage::*;
pub use plugin_settings::*;
pub use plugin_tasks::*;

#[cfg(test)]
use migrations::{
    migrate_v2_to_v3, migrate_v3_to_v4, migrate_v4_to_v5, migrate_v5_to_v6, migrate_v6_to_v7,
};

const SCHEMA_VERSION: i64 = 42;
const ROOT_DISK_RESOURCE_ID: &str = "root";
const AGGREGATE_NON_LOOPBACK_NETWORK_RESOURCE_ID: &str = "aggregateNonLoopback";
const MAX_TERMINAL_WORKSPACE_LAYOUT_BYTES: usize = 256 * 1024;

const DEFAULT_ALGORITHM_POLICY_ID: &str = "secure-default";
const MAX_AUTHENTICATION_CREDENTIALS: usize = 16;
const MAX_JUMP_HOPS: usize = 5;
const MAX_AUTOMATION_STEPS: usize = 32;
const MAX_HOST_TAGS: usize = 32;
const MAX_RECENT_CONNECTIONS: u16 = 100;
const LOGIN_AUTOMATION_SECRET_STAGE_TTL_MS: i64 = 10 * 60 * 1_000;
const HOST_CREATE_PASSWORD_STAGE_TTL_MS: i64 = 10 * 60 * 1_000;
const MAX_SSH_SYNC_PROFILES_PER_PLUGIN: usize = 1_024;
const MAX_SSH_SYNC_SCOPE_HOSTS: usize = 1_024;
const MAX_SSH_SYNC_SCOPE_CREDENTIALS: usize = 4_096;
const MAX_SSH_SYNC_SCOPE_DESKTOP_PROFILES: usize = 1_024;
const MAX_SSH_SYNC_KEY_ENVELOPE_BYTES: usize = 128 * 1_024;
const MAX_SSH_SYNC_OBJECT_MAPPINGS_PER_BATCH: usize = 8_192;
const MAX_SSH_SYNC_OBJECT_MAPPINGS_PER_OWNER: usize = 65_536;
const MAX_SSH_SYNC_OWNED_DELTA_OBJECTS: usize = 8_192;
const MAX_SSH_SYNC_VAULT_GC_ACK: usize = 4_096;
const MAX_SSH_SYNC_DELETE_REFS: usize = 8_192;
const MAX_SSH_SYNC_SCOPE_MEMBERSHIPS: usize = 65_536;

/// Removes objects introduced after a migration-test fixture's claimed
/// version, so lowering `user_version` cannot leave a newer schema behind.
#[cfg(test)]
pub(crate) fn remove_schema_added_after_fixture_version(
    connection: &Connection,
    fixture_version: i64,
) -> rusqlite::Result<()> {
    if fixture_version < 42 {
        connection.execute_batch(
            "PRAGMA defer_foreign_keys = ON;
             BEGIN IMMEDIATE;
             DELETE FROM ssh_sync_scope_memberships WHERE object_kind = 'desktop_profile';
             DELETE FROM ssh_sync_object_mappings WHERE object_kind = 'desktop_profile';
             ALTER TABLE ssh_sync_profile_states
               DROP COLUMN custom_desktop_profile_ids_json;

             ALTER TABLE ssh_sync_object_mappings RENAME TO ssh_sync_object_mappings_v42;
             CREATE TABLE ssh_sync_object_mappings (
               plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(
                 length(signer_fingerprint_sha256) = 64
                 AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                 AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
               ),
               profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
               object_kind TEXT NOT NULL CHECK(
                 object_kind IN ('host', 'identity', 'credential', 'secret')
               ),
               portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
               local_object_id TEXT NOT NULL CHECK(length(local_object_id) = 36),
               created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
               updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
               FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
                 REFERENCES ssh_sync_profile_states(
                   plugin_id, signer_fingerprint_sha256, profile_id
                 ) ON DELETE CASCADE,
               PRIMARY KEY(
                 plugin_id, signer_fingerprint_sha256, profile_id,
                 object_kind, portable_object_id
               ),
               UNIQUE(
                 plugin_id, signer_fingerprint_sha256, profile_id,
                 object_kind, local_object_id
               )
             ) STRICT;
             INSERT INTO ssh_sync_object_mappings
               (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                portable_object_id, local_object_id, created_at_ms, updated_at_ms)
             SELECT plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                    portable_object_id, local_object_id, created_at_ms, updated_at_ms
             FROM ssh_sync_object_mappings_v42;
             DROP TABLE ssh_sync_object_mappings_v42;
             CREATE INDEX ssh_sync_object_mappings_plugin
               ON ssh_sync_object_mappings(plugin_id, signer_fingerprint_sha256, profile_id);

             ALTER TABLE ssh_sync_scope_memberships RENAME TO ssh_sync_scope_memberships_v42;
             CREATE TABLE ssh_sync_scope_memberships (
               plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 160),
               signer_fingerprint_sha256 TEXT NOT NULL CHECK(
                 length(signer_fingerprint_sha256) = 64
                 AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
                 AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
               ),
               profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 160),
               object_kind TEXT NOT NULL CHECK(
                 object_kind IN ('host', 'identity', 'credential', 'secret')
               ),
               portable_object_id TEXT NOT NULL CHECK(length(portable_object_id) = 36),
               membership TEXT NOT NULL CHECK(membership IN ('included', 'excluded')),
               updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0),
               FOREIGN KEY(plugin_id, signer_fingerprint_sha256, profile_id)
                 REFERENCES ssh_sync_profile_states(
                   plugin_id, signer_fingerprint_sha256, profile_id
                 ) ON DELETE CASCADE,
               PRIMARY KEY(
                 plugin_id, signer_fingerprint_sha256, profile_id,
                 object_kind, portable_object_id
               )
             ) STRICT;
             INSERT INTO ssh_sync_scope_memberships
               (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                portable_object_id, membership, updated_at_ms)
             SELECT plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                    portable_object_id, membership, updated_at_ms
             FROM ssh_sync_scope_memberships_v42;
             DROP TABLE ssh_sync_scope_memberships_v42;
             COMMIT;",
        )?;
    }
    if fixture_version < 41 {
        connection.execute_batch("DROP TABLE IF EXISTS desktop_preferences;")?;
    }
    if fixture_version < 40 {
        connection.execute_batch("DROP TABLE IF EXISTS desktop_profile_password_receipts;")?;
    }
    if fixture_version < 39 {
        // Reverse the v39 additions before lowering user_version. Keeping a
        // newer column or CHECK constraint in a historical fixture makes the
        // forward migration test accidentally exercise a hybrid schema.
        connection.execute_batch(
            "ALTER TABLE plugin_operation_permissions DROP COLUMN expires_at_unix_ms;
             DROP INDEX IF EXISTS plugin_workflow_task_steps_task_updated;
             ALTER TABLE plugin_workflow_task_steps
               RENAME TO plugin_workflow_task_steps_v39;
             CREATE TABLE plugin_workflow_task_steps (
               task_id TEXT NOT NULL REFERENCES plugin_workflow_tasks(task_id) ON DELETE CASCADE,
               step_id TEXT NOT NULL CHECK(length(step_id) BETWEEN 1 AND 64),
               method TEXT NOT NULL CHECK(method IN
                 ('serial_devices', 'serial_open', 'serial_send', 'protocol_open', 'describe',
                  'permissions', 'permission_request', 'permissions_forget', 'resources_list',
                  'resource_close', 'subscription_start', 'timer_start', 'resource_events',
                  'network_start', 'network_send', 'remote_exec_start', 'remote_exec_send',
                  'process_start', 'process_send', 'sftp_open', 'sftp', 'file_pick', 'file',
                  'credential', 'storage', 'terminal_request_input')),
               state TEXT NOT NULL CHECK(state IN
                 ('dispatching', 'succeeded', 'failed', 'needs_user_action', 'outcome_unknown',
                  'interrupted')),
               outcome_unknown INTEGER NOT NULL CHECK(outcome_unknown IN (0, 1)),
               state_version INTEGER NOT NULL CHECK(state_version >= 1),
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               PRIMARY KEY(task_id, step_id)
             ) STRICT;
             INSERT INTO plugin_workflow_task_steps
               (task_id, step_id, method, state, outcome_unknown, state_version,
                created_at_ms, updated_at_ms)
             SELECT task_id, step_id, method, state, outcome_unknown, state_version,
                    created_at_ms, updated_at_ms
             FROM plugin_workflow_task_steps_v39;
             DROP TABLE plugin_workflow_task_steps_v39;
             CREATE INDEX plugin_workflow_task_steps_task_updated
               ON plugin_workflow_task_steps(task_id, updated_at_ms DESC, step_id DESC);",
        )?;
    }
    if fixture_version < 38 {
        connection.execute_batch(
            "DROP TABLE IF EXISTS plugin_workflow_task_steps;
             DROP TABLE IF EXISTS plugin_workflow_tasks;",
        )?;
    }
    if fixture_version < 35 {
        connection.execute_batch("DROP TABLE IF EXISTS plugin_credentials;")?;
    }
    if fixture_version < 32 {
        connection.execute_batch(
            "DROP TABLE IF EXISTS plugin_private_storage;
             DROP TABLE IF EXISTS plugin_operation_permissions;
             DROP TABLE IF EXISTS plugin_operation_approval_policies;",
        )?;
    }
    Ok(())
}

const CREDENTIAL_RECORD_COLUMNS: &str = "credential_refs.id, credential_refs.identity_id, credential_refs.kind, \
     password_secret_slot.secret_ref_id, private_key_secret_slot.secret_ref_id, \
     passphrase_secret_slot.secret_ref_id, credential_refs.priority, credential_refs.label, \
     credential_private_key_details.public_key_algorithm, credential_private_key_details.public_key_fingerprint, \
     credential_ssh_agent_details.public_key_blob, credential_ssh_agent_details.public_key_algorithm, \
     credential_ssh_agent_details.public_key_fingerprint, credential_ssh_agent_details.agent_scope, \
     credential_ssh_agent_details.identity_kind, credential_ssh_agent_details.hardware_application, \
     credential_ssh_agent_details.certificate_blob, \
     credential_ssh_agent_details.certificate_algorithm, credential_ssh_agent_details.certificate_fingerprint, \
     credential_ssh_agent_details.certificate_ca_public_key_fingerprint, \
     credential_ssh_agent_details.certificate_serial, \
     credential_ssh_agent_details.certificate_key_id, \
     credential_ssh_agent_details.certificate_valid_principals_json, \
     credential_ssh_agent_details.certificate_type, \
     credential_ssh_agent_details.certificate_valid_after_unix_seconds, \
     credential_ssh_agent_details.certificate_valid_before_unix_seconds, \
     credential_ssh_agent_details.certificate_critical_options_json, \
     credential_ssh_agent_details.certificate_extensions_json, \
     credential_refs.state_version, credential_refs.import_operation_id, \
     credential_refs.import_idempotency_key, credential_refs.import_state, \
     credential_keyboard_interactive_details.max_rounds";

const CREDENTIAL_RECORD_JOINS: &str = " LEFT JOIN credential_secret_slots AS password_secret_slot
       ON password_secret_slot.credential_ref_id = credential_refs.id
      AND password_secret_slot.slot_kind = 'password'
     LEFT JOIN credential_secret_slots AS private_key_secret_slot
       ON private_key_secret_slot.credential_ref_id = credential_refs.id
      AND private_key_secret_slot.slot_kind = 'private_key'
     LEFT JOIN credential_secret_slots AS passphrase_secret_slot
       ON passphrase_secret_slot.credential_ref_id = credential_refs.id
      AND passphrase_secret_slot.slot_kind = 'passphrase'
     LEFT JOIN credential_private_key_details
       ON credential_private_key_details.credential_ref_id = credential_refs.id
     LEFT JOIN credential_keyboard_interactive_details
       ON credential_keyboard_interactive_details.credential_ref_id = credential_refs.id
     LEFT JOIN credential_ssh_agent_details
       ON credential_ssh_agent_details.credential_ref_id = credential_refs.id";

pub type Result<T> = std::result::Result<T, AppPersistenceError>;

#[derive(Debug, Error)]
pub enum AppPersistenceError {
    #[error("invalid SSH metadata: {0}")]
    InvalidInput(&'static str),
    #[error("the requested SSH metadata record does not exist")]
    NotFound,
    #[error("the SSH metadata record changed; refresh and retry")]
    Conflict,
    #[error(
        "the idempotency key or operation id was already used for a different credential import"
    )]
    IdempotencyConflict,
    #[error("the stored SSH metadata is invalid")]
    InvalidStoredData,
    #[error("the login automation secret staging state must be reloaded")]
    RequiresReload,
    #[error("the SSH sync restore commit outcome is unknown; reconcile before retrying")]
    RestoreCommitUnknown,
    #[error("the known host key does not match the trusted key")]
    KnownHostMismatch {
        trusted_fingerprint: String,
        observed_fingerprint: String,
    },
    #[error("the known host key algorithm does not match the trusted endpoint pin")]
    KnownHostAlgorithmChanged {
        trusted_algorithm: String,
        observed_algorithm: String,
    },
    #[error("application database schema {0} is newer than this build")]
    UnsupportedSchema(i64),
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedHostKey {
    pub normalized_address: String,
    pub port: u16,
    pub key_algorithm: String,
    pub public_key_base64: String,
    pub fingerprint_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownHostObservation {
    Unknown(ObservedHostKey),
    Trusted(KnownHostSummary),
    Mismatch {
        trusted: KnownHostSummary,
        observed: ObservedHostKey,
    },
    AlgorithmChanged {
        trusted: KnownHostSummary,
        observed: ObservedHostKey,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialImportState {
    Pending,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginOperationPhase {
    Resolve,
    Download,
    VerifyCatalog,
    VerifyPackage,
    AwaitingCapabilities,
    Staged,
    FilesystemActivated,
    DatabaseCommitted,
    ReconcileRequired,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCatalogTrustRecord {
    pub root_key_id: String,
    pub sequence: u64,
    pub payload_sha256: String,
    pub catalog_signature_base64: String,
    pub catalog_revision: String,
    pub expires_at_unix_ms: i64,
    pub verified_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCatalogEntryRecord {
    pub plugin_id: PluginId,
    pub version: String,
    pub name: String,
    pub publisher: String,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub platform: String,
    pub architectures: Vec<String>,
    pub package_url: String,
    pub package_size: u64,
    pub package_sha256: String,
    pub publisher_key_base64: String,
    pub publisher_signature_base64: String,
    pub capabilities: Vec<PluginCapability>,
    pub raw_capabilities: Vec<String>,
    pub unsupported_manifest: bool,
    pub minimum_app_version: String,
    pub published_at_unix_ms: i64,
    pub details: Option<norishell_core_api::PluginCatalogReleaseDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstalledRecord {
    pub plugin_id: PluginId,
    pub name: String,
    pub publisher: String,
    pub signer_fingerprint_sha256: String,
    pub active_version: String,
    pub package_sha256: String,
    pub capabilities: Vec<PluginCapability>,
    pub state: PluginInstallState,
    pub state_version: WireSequence,
    pub installed_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstalledVersionPublisher {
    pub publisher_key_base64: String,
    pub publisher_signature_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOperationRecord {
    pub operation_id: PluginOperationId,
    pub plugin_id: Option<PluginId>,
    pub kind: PluginOperationKind,
    pub state: PluginOperationState,
    pub phase: PluginOperationPhase,
    pub idempotency_key: String,
    pub request_fingerprint_sha256: String,
    pub candidate_version: Option<String>,
    pub expected_active_version: Option<String>,
    pub error_code: Option<String>,
    pub state_version: WireSequence,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCapabilityGrantRecord {
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
    pub major_version: u64,
    pub capability: PluginCapability,
    pub granted: bool,
    pub state_version: WireSequence,
    pub binding: Option<PluginPermissionBinding>,
}

/// Exact package and host-security contract under which a plugin permission
/// decision is effective. Rows without a binding are retained only as legacy
/// history and must never be carried to another package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPermissionBinding {
    pub artifact_sha256: String,
    pub app_version_major: u64,
    pub app_version_minor: u64,
    pub secure_surface_contract_revision: u64,
}

type PluginPermissionScopeBindingRow = (i64, Option<String>, Option<i64>, Option<i64>, Option<i64>);

#[derive(Debug)]
pub struct PluginActivationPermissions<'a> {
    pub protocol_major: u64,
    pub binding: &'a PluginPermissionBinding,
    pub previous_binding: Option<&'a PluginPermissionBinding>,
    pub expected_previous_grant_state_version: Option<WireSequence>,
    pub expected_previous_scope_state_version: Option<WireSequence>,
    pub grants: &'a [(PluginCapability, bool)],
    pub carried_host_scope_capabilities: &'a [PluginCapability],
    /// A protected candidate decision replaces the carried scopes atomically.
    pub approved_host_scopes: Option<&'a [(HostId, PluginCapability)]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAuditRecord {
    pub audit_id: u64,
    pub plugin_id: Option<String>,
    pub operation_id: Option<String>,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
    pub created_at_unix_ms: i64,
}

/// Internal credential metadata. Secret references never cross the Core API boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    pub credential_ref_id: CredentialRefId,
    pub identity_id: IdentityId,
    pub method: AuthenticationMethodKind,
    pub details: CredentialRecordDetails,
    pub priority: u32,
    pub label: String,
    pub state_version: WireSequence,
    pub import_operation_id: Option<OperationId>,
    pub import_idempotency_key: Option<String>,
    pub import_state: CredentialImportState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialRecordDetails {
    Password {
        secret_ref_id: SecretRefId,
    },
    PrivateKey {
        secret_ref_id: SecretRefId,
        passphrase_secret_ref_id: Option<SecretRefId>,
        public_key_algorithm: Option<String>,
        public_key_fingerprint: Option<String>,
    },
    KeyboardInteractive {
        max_rounds: u8,
    },
    SshAgent {
        public_key_blob: Vec<u8>,
        public_key_algorithm: String,
        public_key_fingerprint: String,
        scope: SshAgentScope,
    },
    Certificate {
        certificate: Box<SshCertificateMetadata>,
        scope: SshAgentScope,
    },
    HardwareKey {
        public_key_blob: Vec<u8>,
        public_key_algorithm: String,
        public_key_fingerprint: String,
        application: String,
        scope: SshAgentScope,
    },
}

/// One read-transaction snapshot used by Core to resolve a saved Host into an
/// immutable connection operation. It is internal to the Rust runtime and may
/// contain opaque secret references that must never cross the public API.
pub struct HostConnectionSnapshot {
    pub host: HostSummary,
    pub identity: Option<IdentitySummary>,
    pub config: HostConnectionConfigSummary,
    pub credentials: Vec<CredentialRecord>,
    /// Internal execution snapshot. Unlike the public summary, SendSecret
    /// steps retain only their opaque SecretRef so the Core actor can borrow
    /// the value directly from the unlocked Vault.
    pub login_automation: LoginAutomationExecutionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAutomationExecutionPlan {
    pub revision: WireSequence,
    pub confirmed_revision: Option<WireSequence>,
    pub enabled: bool,
    pub steps: Vec<LoginAutomationStepInput>,
}

pub struct AppRepository {
    path: PathBuf,
    connection: Connection,
    login_automation_secret_stages_poisoned: bool,
    host_create_password_stages_poisoned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginAutomationSecretStageState {
    PendingVault,
    Staged,
    CleanupPending,
    Cancelled,
    Consumed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LoginAutomationSecretStageRecord {
    pub staged_secret_id: LoginAutomationSecretStageId,
    pub secret_ref_id: SecretRefId,
    pub host_id: HostId,
    pub expected_automation_revision: WireSequence,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub label: String,
    pub expires_at_unix_ms: i64,
    pub state: LoginAutomationSecretStageState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAutomationSecretStageBegin {
    pub record: LoginAutomationSecretStageRecord,
    pub created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCreatePasswordStageState {
    PendingVault,
    Staged,
    CleanupPending,
    Cancelled,
    Consumed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostCreatePasswordStageRecord {
    pub staged_password_id: HostCreatePasswordStageId,
    pub secret_ref_id: SecretRefId,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub identity_label: String,
    pub credential_label: String,
    pub expires_at_unix_ms: i64,
    pub state: HostCreatePasswordStageState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCreatePasswordStageBegin {
    pub record: HostCreatePasswordStageRecord,
    pub created: bool,
}

impl std::fmt::Debug for HostCreatePasswordStageRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostCreatePasswordStageRecord")
            .field("staged_password_id", &self.staged_password_id)
            .field("secret_ref_id", &"[REDACTED]")
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("identity_label", &self.identity_label)
            .field("credential_label", &self.credential_label)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .field("state", &self.state)
            .finish()
    }
}

impl std::fmt::Debug for LoginAutomationSecretStageRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginAutomationSecretStageRecord")
            .field("staged_secret_id", &self.staged_secret_id)
            .field("secret_ref_id", &"[REDACTED]")
            .field("host_id", &self.host_id)
            .field(
                "expected_automation_revision",
                &self.expected_automation_revision,
            )
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("label", &self.label)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBatchCreateInput {
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub ingress: RouteIngress,
    pub jump_hosts: Vec<HostBatchJumpHostInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBatchJumpHostInput {
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestoreSagaInput {
    pub attempt_id: OperationId,
    pub plugin_id: PluginId,
    pub profile_id: String,
    pub bundle_sha256: String,
    pub plan_sha256: String,
    pub secret_ref_ids: Vec<SecretRefId>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncRestoreSagaRecord {
    pub attempt_id: OperationId,
    pub plugin_id: PluginId,
    pub profile_id: String,
    pub bundle_sha256: String,
    pub plan_sha256: String,
    pub secret_ref_ids: Vec<SecretRefId>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl std::fmt::Debug for SshSyncRestoreSagaRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncRestoreSagaRecord")
            .field("attempt_id", &self.attempt_id)
            .field("plugin_id", &self.plugin_id)
            .field("profile_id", &self.profile_id)
            .field("bundle_sha256", &self.bundle_sha256)
            .field("plan_sha256", &self.plan_sha256)
            .field("secret_ref_ids", &"[OPAQUE]")
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .field("updated_at_unix_ms", &self.updated_at_unix_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestoreSagaBegin {
    pub record: SshSyncRestoreSagaRecord,
    pub created: bool,
}

/// Connection-local and database-wide SQLite change counters captured around
/// a staged SSH sync restore. The pair is meaningful only for the repository
/// instance that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSyncChangeFence {
    pub connection_total_changes: u64,
    pub database_data_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncFencedRestoreSagaBegin {
    pub begin: SshSyncRestoreSagaBegin,
    /// Fence after the durable saga write, suitable for the final metadata
    /// commit after the corresponding Vault inserts complete.
    pub change_fence: SshSyncChangeFence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestoreIdentityInput {
    pub identity_id: IdentityId,
    pub label: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestoreCredentialInput {
    pub credential_ref_id: CredentialRefId,
    pub identity_id: IdentityId,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub priority: u32,
    pub label: String,
    pub material: SshSyncRestoreCredentialMaterial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshSyncRestoreCredentialMaterial {
    Password {
        secret_ref_id: SecretRefId,
    },
    PrivateKey {
        secret_ref_id: SecretRefId,
        passphrase_secret_ref_id: Option<SecretRefId>,
        public_key_algorithm: String,
        public_key_fingerprint: String,
    },
    KeyboardInteractive {
        max_rounds: u8,
    },
}

#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SshSyncLoginAutomationStep {
    Expect {
        literal_text: String,
        timeout_seconds: u8,
    },
    SendText {
        text: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
    SendSecret {
        secret_ref_id: SecretRefId,
        secret_label: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
}

impl std::fmt::Debug for SshSyncLoginAutomationStep {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expect {
                literal_text,
                timeout_seconds,
            } => formatter
                .debug_struct("Expect")
                .field(
                    "literal_text",
                    &format_args!("[REDACTED; {} bytes]", literal_text.len()),
                )
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::SendText {
                text,
                append_enter,
                timeout_seconds,
            } => formatter
                .debug_struct("SendText")
                .field("text", &format_args!("[REDACTED; {} bytes]", text.len()))
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::SendSecret {
                secret_label,
                append_enter,
                timeout_seconds,
                ..
            } => formatter
                .debug_struct("SendSecret")
                .field("secret_ref_id", &"[OPAQUE]")
                .field("secret_label", secret_label)
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SshSyncLoginAutomation {
    pub enabled: bool,
    pub confirmed: bool,
    pub steps: Vec<SshSyncLoginAutomationStep>,
}

impl SshSyncLoginAutomation {
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            confirmed: false,
            steps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestoreHostInput {
    pub host_id: HostId,
    pub request: HostConfiguredCreateRequest,
    pub tag_labels: Vec<String>,
    pub login_automation: SshSyncLoginAutomation,
}

/// A portable desktop profile restored after its Host and Password credential
/// dependencies are available. `profile.revision` must be zero on input;
/// persistence assigns the initial local revision in the commit transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestoreDesktopProfileInput {
    pub portable_object_id: String,
    pub profile: DesktopProfile,
}

/// Stable IDs and operation IDs are derived by trusted Core from
/// plugin+profile+portable-object identity. Bundle hashes fence an attempt but
/// never participate in those stable object IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestorePlan {
    pub attempt_id: OperationId,
    pub plugin_id: PluginId,
    pub profile_id: String,
    pub bundle_sha256: String,
    pub plan_sha256: String,
    pub identities: Vec<SshSyncRestoreIdentityInput>,
    pub credentials: Vec<SshSyncRestoreCredentialInput>,
    pub hosts: Vec<SshSyncRestoreHostInput>,
    pub desktop_profiles: Vec<SshSyncRestoreDesktopProfileInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncRestorePreview {
    pub create_count: u32,
    pub already_applied_count: u32,
    pub conflict_count: u32,
    /// Vault reconciliation may delete only these refs after an aborted
    /// attempt. Refs owned by exact, already-published credentials are omitted.
    pub new_secret_ref_ids: Vec<SecretRefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSyncRestoreCommitResult {
    pub created_count: u32,
    pub already_applied_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncProfileStateKey {
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
    pub profile_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSyncProfileScopeMode {
    AllEligible,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncProfileScope {
    pub mode: SshSyncProfileScopeMode,
    pub custom_host_ids: Vec<HostId>,
    pub custom_credential_ref_ids: Vec<CredentialRefId>,
    pub custom_desktop_profile_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncProfileStateCreate {
    pub key: SshSyncProfileStateKey,
    pub scope: SshSyncProfileScope,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncProfileKeyBinding {
    pub sync_key_secret_ref_id: SecretRefId,
    pub password_wrapped_sync_key_envelope: Option<Vec<u8>>,
}

impl std::fmt::Debug for SshSyncProfileKeyBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncProfileKeyBinding")
            .field("sync_key_secret_ref_id", &"[OPAQUE]")
            .field(
                "password_wrapped_sync_key_envelope",
                &self
                    .password_wrapped_sync_key_envelope
                    .as_ref()
                    .map(|value| format!("[REDACTED; {} bytes]", value.len())),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncProfileRemoteBaseline {
    pub remote_revision: u64,
    pub remote_etag: String,
    pub baseline_content_sha256: String,
    /// SHA-256 of the exact encrypted exchange bytes retained by Core.
    pub baseline_exchange_sha256: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncProfileStateRecord {
    pub key: SshSyncProfileStateKey,
    pub scope: SshSyncProfileScope,
    pub key_binding: Option<SshSyncProfileKeyBinding>,
    pub remote_baseline: Option<SshSyncProfileRemoteBaseline>,
    pub state_version: WireSequence,
    pub updated_at_unix_ms: i64,
    pub last_successful_sync_at_unix_ms: Option<i64>,
}

impl std::fmt::Debug for SshSyncProfileStateRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncProfileStateRecord")
            .field("key", &self.key)
            .field("scope", &self.scope)
            .field(
                "key_binding",
                &self.key_binding.as_ref().map(|_| "[REDACTED]"),
            )
            .field("remote_baseline", &self.remote_baseline)
            .field("state_version", &self.state_version)
            .field("updated_at_unix_ms", &self.updated_at_unix_ms)
            .field(
                "last_successful_sync_at_unix_ms",
                &self.last_successful_sync_at_unix_ms,
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncProfileStateDeleteResult {
    pub deleted_count: u32,
    /// Stable Vault refs that Core must clean after this SQLite delete commits.
    pub sync_key_secret_ref_ids: Vec<SecretRefId>,
}

impl std::fmt::Debug for SshSyncProfileStateDeleteResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncProfileStateDeleteResult")
            .field("deleted_count", &self.deleted_count)
            .field("sync_key_secret_ref_ids", &"[OPAQUE]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SshSyncObjectKind {
    Host,
    Identity,
    Credential,
    Secret,
    DesktopProfile,
}

#[derive(Clone, PartialEq, Eq)]
pub enum SshSyncLocalObjectId {
    Host(HostId),
    Identity(IdentityId),
    Credential(CredentialRefId),
    Secret(SecretRefId),
    /// Canonical UUID of a local `DesktopProfile`.
    DesktopProfile(String),
}

impl std::fmt::Debug for SshSyncLocalObjectId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple(match self {
                Self::Host(_) => "Host",
                Self::Identity(_) => "Identity",
                Self::Credential(_) => "Credential",
                Self::Secret(_) => "Secret",
                Self::DesktopProfile(_) => "DesktopProfile",
            })
            .field(&"[OPAQUE]")
            .finish()
    }
}

impl SshSyncLocalObjectId {
    fn object_kind(&self) -> SshSyncObjectKind {
        match self {
            Self::Host(_) => SshSyncObjectKind::Host,
            Self::Identity(_) => SshSyncObjectKind::Identity,
            Self::Credential(_) => SshSyncObjectKind::Credential,
            Self::Secret(_) => SshSyncObjectKind::Secret,
            Self::DesktopProfile(_) => SshSyncObjectKind::DesktopProfile,
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Host(value) => value.as_str(),
            Self::Identity(value) => value.as_str(),
            Self::Credential(value) => value.as_str(),
            Self::Secret(value) => value.as_str(),
            Self::DesktopProfile(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncObjectMappingInput {
    pub portable_object_id: String,
    pub local_object_id: SshSyncLocalObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncObjectMappingRecord {
    pub owner: SshSyncProfileStateKey,
    pub object_kind: SshSyncObjectKind,
    pub portable_object_id: String,
    pub local_object_id: SshSyncLocalObjectId,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncObjectMappingResolution {
    pub mapping: SshSyncObjectMappingRecord,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedIdentityUpdate {
    pub portable_object_id: String,
    pub identity_id: IdentityId,
    pub expected_state_version: WireSequence,
    pub label: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedIdentityDelete {
    pub portable_object_id: String,
    pub identity_id: IdentityId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedCredentialUpdate {
    pub portable_object_id: String,
    pub credential_ref_id: CredentialRefId,
    pub expected_state_version: WireSequence,
    pub identity_id: IdentityId,
    pub priority: u32,
    pub label: String,
    pub material: SshSyncRestoreCredentialMaterial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedCredentialDelete {
    pub portable_object_id: String,
    pub credential_ref_id: CredentialRefId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedHostBaseVersions {
    pub host: WireSequence,
    pub route: WireSequence,
    pub authentication: WireSequence,
    pub algorithm: WireSequence,
    pub heartbeat: WireSequence,
    pub monitoring: WireSequence,
    pub login_automation: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedHostUpdate {
    pub portable_object_id: String,
    pub host_id: HostId,
    pub expected: SshSyncOwnedHostBaseVersions,
    pub desired: HostConfiguredCreateRequest,
    pub tag_labels: Vec<String>,
    pub login_automation: SshSyncLoginAutomation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedHostDelete {
    pub portable_object_id: String,
    pub host_id: HostId,
    pub expected: SshSyncOwnedHostBaseVersions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedDesktopProfileUpdate {
    pub portable_object_id: String,
    pub profile: DesktopProfile,
    pub expected_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedDesktopProfileDelete {
    pub portable_object_id: String,
    /// Canonical UUID of the local profile to delete.
    pub profile_id: String,
    pub expected_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedSecretDelete {
    pub portable_object_id: String,
    pub secret_ref_id: SecretRefId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedSecretReplacement {
    pub portable_object_id: String,
    pub expected_secret_ref_id: SecretRefId,
    pub new_secret_ref_id: SecretRefId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedCreateBatch {
    pub plan: SshSyncRestorePlan,
    pub mappings: Vec<SshSyncObjectMappingInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncOwnedMetadataDelta {
    pub owner: SshSyncProfileStateKey,
    pub attempt_id: OperationId,
    pub delta_sha256: String,
    pub creates: Option<SshSyncOwnedCreateBatch>,
    pub identity_updates: Vec<SshSyncOwnedIdentityUpdate>,
    pub identity_deletes: Vec<SshSyncOwnedIdentityDelete>,
    pub credential_updates: Vec<SshSyncOwnedCredentialUpdate>,
    pub credential_deletes: Vec<SshSyncOwnedCredentialDelete>,
    pub host_updates: Vec<SshSyncOwnedHostUpdate>,
    pub host_deletes: Vec<SshSyncOwnedHostDelete>,
    pub desktop_profile_updates: Vec<SshSyncOwnedDesktopProfileUpdate>,
    pub desktop_profile_deletes: Vec<SshSyncOwnedDesktopProfileDelete>,
    pub secret_replacements: Vec<SshSyncOwnedSecretReplacement>,
    pub secret_deletes: Vec<SshSyncOwnedSecretDelete>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncOwnedDeltaResult {
    pub created_count: u32,
    pub updated_count: u32,
    pub deleted_count: u32,
    pub pending_vault_gc_secret_ref_ids: Vec<SecretRefId>,
    pub replayed: bool,
}

impl std::fmt::Debug for SshSyncOwnedDeltaResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncOwnedDeltaResult")
            .field("created_count", &self.created_count)
            .field("updated_count", &self.updated_count)
            .field("deleted_count", &self.deleted_count)
            .field("pending_vault_gc_secret_ref_ids", &"[OPAQUE]")
            .field("replayed", &self.replayed)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncVaultGcRecord {
    pub owner: SshSyncProfileStateKey,
    pub secret_ref_id: SecretRefId,
    pub reason: String,
    pub attempt_id: OperationId,
    pub created_at_unix_ms: i64,
}

impl std::fmt::Debug for SshSyncVaultGcRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncVaultGcRecord")
            .field("owner", &self.owner)
            .field("secret_ref_id", &"[OPAQUE]")
            .field("reason", &self.reason)
            .field("attempt_id", &self.attempt_id)
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncPluginDeleteTask {
    pub operation_id: OperationId,
    pub owner: SshSyncProfileStateKey,
    pub secret_ref_ids: Vec<SecretRefId>,
    pub created_at_unix_ms: i64,
}

impl std::fmt::Debug for SshSyncPluginDeleteTask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncPluginDeleteTask")
            .field("operation_id", &self.operation_id)
            .field("owner", &self.owner)
            .field(
                "secret_ref_ids",
                &format_args!("[OPAQUE; {} refs]", self.secret_ref_ids.len()),
            )
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncPluginDeleteBegin {
    pub operation_id: OperationId,
    pub tasks: Vec<SshSyncPluginDeleteTask>,
    pub completed: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSyncPluginDeleteAck {
    pub acknowledged_count: u32,
    pub profile_finalized: bool,
    pub operation_completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SshSyncHttpUploadAttemptState {
    Prepared,
    Sent,
    Verifying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSyncHttpMethod {
    Put,
    Post,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncHttpUploadAttemptInput {
    pub owner: SshSyncProfileStateKey,
    pub canonical_url: String,
    pub http_method: SshSyncHttpMethod,
    pub use_oauth: bool,
    pub authorization_revision: WireSequence,
    pub configuration_revision: WireSequence,
    pub base_revision: u64,
    pub base_etag: Option<String>,
    pub target_revision: u64,
    /// Keyed digest of the portable plaintext content.
    pub keyed_content_sha256: String,
    /// SHA-256 of the exact encrypted HTTP request body.
    pub body_sha256: String,
    pub idempotency_key: String,
}

impl std::fmt::Debug for SshSyncHttpUploadAttemptInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncHttpUploadAttemptInput")
            .field("owner", &self.owner)
            .field("canonical_url", &"[REDACTED]")
            .field("http_method", &self.http_method)
            .field("use_oauth", &self.use_oauth)
            .field("authorization_revision", &self.authorization_revision)
            .field("configuration_revision", &self.configuration_revision)
            .field("base_revision", &self.base_revision)
            .field("base_etag", &self.base_etag)
            .field("target_revision", &self.target_revision)
            .field("keyed_content_sha256", &"[REDACTED]")
            .field("body_sha256", &"[REDACTED]")
            .field("idempotency_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncHttpUploadAttemptRecord {
    pub input: SshSyncHttpUploadAttemptInput,
    pub state: SshSyncHttpUploadAttemptState,
    pub state_version: WireSequence,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncHttpUploadCompletionFence {
    pub canonical_url: String,
    pub http_method: SshSyncHttpMethod,
    pub use_oauth: bool,
    pub authorization_revision: WireSequence,
    pub configuration_revision: WireSequence,
}

impl std::fmt::Debug for SshSyncHttpUploadCompletionFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncHttpUploadCompletionFence")
            .field("canonical_url", &"[REDACTED]")
            .field("http_method", &self.http_method)
            .field("use_oauth", &self.use_oauth)
            .field("authorization_revision", &self.authorization_revision)
            .field("configuration_revision", &self.configuration_revision)
            .finish()
    }
}

impl std::fmt::Debug for SshSyncHttpUploadAttemptRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncHttpUploadAttemptRecord")
            .field("input", &self.input)
            .field("state", &self.state)
            .field("state_version", &self.state_version)
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .field("updated_at_unix_ms", &self.updated_at_unix_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshSyncHttpUploadCompletionProof {
    ServerAcknowledged {
        acknowledged_revision: u64,
        acknowledged_body_sha256: String,
    },
    ExactBodyObserved {
        observed_revision: u64,
        observed_body_sha256: String,
    },
    SupersededByNewerRemote {
        authenticated_remote_revision: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSyncScopeMembershipState {
    Included,
    Excluded,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncScopeMembershipInput {
    pub object_kind: SshSyncObjectKind,
    pub portable_object_id: String,
    pub state: SshSyncScopeMembershipState,
}

impl std::fmt::Debug for SshSyncScopeMembershipInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncScopeMembershipInput")
            .field("object_kind", &self.object_kind)
            .field("portable_object_id", &"[OPAQUE]")
            .field("state", &self.state)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshSyncScopeMembershipRecord {
    pub owner: SshSyncProfileStateKey,
    pub membership: SshSyncScopeMembershipInput,
    pub updated_at_unix_ms: i64,
}

impl std::fmt::Debug for SshSyncScopeMembershipRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshSyncScopeMembershipRecord")
            .field("owner", &self.owner)
            .field("membership", &self.membership)
            .field("updated_at_unix_ms", &self.updated_at_unix_ms)
            .finish()
    }
}

struct NormalizedHostBatchCreateInput {
    label: String,
    address: String,
    normalized_address: String,
    port: u16,
    username: Option<String>,
    ingress: RouteIngress,
    jump_hosts: Vec<NormalizedHostBatchJumpHostInput>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NormalizedConfiguredHostCreate {
    label: String,
    address: String,
    normalized_address: String,
    port: u16,
    username: Option<String>,
    identity_id: Option<IdentityId>,
    favorite: bool,
    group_id: Option<HostGroupId>,
    tag_ids: Vec<HostTagId>,
    ingress: RouteIngress,
    jump_host_ids: Vec<HostId>,
    authentication_mode: AuthenticationPlanMode,
    credential_ref_ids: Vec<CredentialRefId>,
    algorithm_policy_id: String,
    compatibility_exceptions: Vec<AlgorithmCompatibilityException>,
    heartbeat_policy: HeartbeatPolicy,
    monitoring_policy: MonitoringPolicy,
    login_automation_enabled: bool,
    login_automation_confirmed: bool,
    login_automation_steps: Vec<LoginAutomationStepInput>,
    staged_password_id: Option<HostCreatePasswordStageId>,
}

struct NormalizedRestoreSaga {
    attempt_id: OperationId,
    plugin_id: PluginId,
    profile_id: String,
    bundle_sha256: String,
    plan_sha256: String,
    secret_ref_ids: Vec<SecretRefId>,
}

struct NormalizedRestorePlan {
    attempt_id: OperationId,
    plugin_id: PluginId,
    profile_id: String,
    bundle_sha256: String,
    plan_sha256: String,
    identities: Vec<SshSyncRestoreIdentityInput>,
    credentials: Vec<SshSyncRestoreCredentialInput>,
    hosts: Vec<NormalizedRestoreHost>,
    desktop_profiles: Vec<NormalizedSshSyncRestoreDesktopProfileInput>,
}

type NormalizedRestoreHost = (
    HostId,
    HostConfiguredCreateRequest,
    NormalizedConfiguredHostCreate,
    Vec<String>,
    SshSyncLoginAutomation,
);

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedSshSyncRestoreDesktopProfileInput {
    portable_object_id: String,
    profile: DesktopProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreObjectDisposition {
    Create,
    AlreadyApplied,
    Conflict,
}

#[derive(Clone, PartialEq, Eq)]
struct NormalizedSshSyncProfileStateKey {
    plugin_id: PluginId,
    signer_fingerprint_sha256: String,
    profile_id: String,
}

#[derive(Clone, PartialEq, Eq)]
struct NormalizedSshSyncHttpUploadAttemptInput {
    owner: NormalizedSshSyncProfileStateKey,
    canonical_url: String,
    http_method: SshSyncHttpMethod,
    use_oauth: bool,
    authorization_revision: WireSequence,
    configuration_revision: WireSequence,
    base_revision: u64,
    base_etag: Option<String>,
    target_revision: u64,
    keyed_content_sha256: String,
    body_sha256: String,
    idempotency_key: String,
}

struct NormalizedSshSyncObjectMappingInput {
    object_kind: SshSyncObjectKind,
    portable_object_id: String,
    local_object_id: SshSyncLocalObjectId,
}

#[derive(Clone)]
struct NormalizedHostBatchJumpHostInput {
    address: String,
    normalized_address: String,
    port: u16,
    username: Option<String>,
}

impl AppRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        ensure_private_parent(&path)?;
        ensure_private_file(&path)?;
        let connection = Connection::open(&path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        migrations::migrate(&connection)?;
        Ok(Self {
            path,
            connection,
            login_automation_secret_stages_poisoned: false,
            host_create_password_stages_poisoned: false,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Captures the repository-wide change fence used by staged SSH sync
    /// restore approval. Callers must compare fences from this same repository
    /// instance before retaining the staged plan.
    pub fn ssh_sync_change_fence(&self) -> Result<SshSyncChangeFence> {
        current_ssh_sync_change_fence(&self.connection)
    }

    /// Begins the non-secret durable restore marker before any Vault write.
    /// Exact replay returns the original row; reuse with drift fails closed.
    pub fn begin_ssh_sync_restore_saga(
        &mut self,
        input: &SshSyncRestoreSagaInput,
    ) -> Result<SshSyncRestoreSagaBegin> {
        Ok(self
            .begin_ssh_sync_restore_saga_internal(input, None)?
            .begin)
    }

    /// Begins a restore saga only if the repository still matches the staged
    /// snapshot and returns the post-saga fence for the final metadata commit.
    pub fn begin_ssh_sync_restore_saga_with_fence(
        &mut self,
        input: &SshSyncRestoreSagaInput,
        expected_fence: &SshSyncChangeFence,
    ) -> Result<SshSyncFencedRestoreSagaBegin> {
        self.begin_ssh_sync_restore_saga_internal(input, Some(expected_fence))
    }

    fn begin_ssh_sync_restore_saga_internal(
        &mut self,
        input: &SshSyncRestoreSagaInput,
        expected_fence: Option<&SshSyncChangeFence>,
    ) -> Result<SshSyncFencedRestoreSagaBegin> {
        let normalized = normalized_restore_saga_input(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(expected_fence) = expected_fence {
            require_ssh_sync_change_fence(&transaction, expected_fence)?;
        }
        if let Some(existing) = get_restore_saga(&transaction, &normalized.attempt_id)? {
            if !same_restore_saga(&existing, &normalized) {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            let change_fence = current_ssh_sync_change_fence(&transaction)?;
            transaction.commit()?;
            return Ok(SshSyncFencedRestoreSagaBegin {
                begin: SshSyncRestoreSagaBegin {
                    record: existing,
                    created: false,
                },
                change_fence,
            });
        }
        let now = unix_time_ms();
        let secret_ref_ids_json = encode_secret_ref_ids(&normalized.secret_ref_ids)?;
        transaction.execute(
            "INSERT INTO ssh_sync_restore_sagas
             (attempt_id, plugin_id, profile_id, bundle_sha256, plan_sha256,
              secret_ref_ids_json, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                normalized.attempt_id.as_str(),
                normalized.plugin_id.as_str(),
                normalized.profile_id,
                normalized.bundle_sha256,
                normalized.plan_sha256,
                secret_ref_ids_json,
                now,
            ],
        )?;
        let change_fence = current_ssh_sync_change_fence(&transaction)?;
        transaction.commit()?;
        Ok(SshSyncFencedRestoreSagaBegin {
            begin: SshSyncRestoreSagaBegin {
                record: SshSyncRestoreSagaRecord {
                    attempt_id: normalized.attempt_id,
                    plugin_id: normalized.plugin_id,
                    profile_id: normalized.profile_id,
                    bundle_sha256: normalized.bundle_sha256,
                    plan_sha256: normalized.plan_sha256,
                    secret_ref_ids: normalized.secret_ref_ids,
                    created_at_unix_ms: now,
                    updated_at_unix_ms: now,
                },
                created: true,
            },
            change_fence,
        })
    }

    pub fn get_pending_ssh_sync_restore_saga(
        &self,
        attempt_id: &OperationId,
    ) -> Result<Option<SshSyncRestoreSagaRecord>> {
        get_restore_saga(&self.connection, attempt_id)
    }

    pub fn list_pending_ssh_sync_restore_sagas(&self) -> Result<Vec<SshSyncRestoreSagaRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT attempt_id, plugin_id, profile_id, bundle_sha256, plan_sha256,
                    secret_ref_ids_json, created_at_ms, updated_at_ms
             FROM ssh_sync_restore_sagas ORDER BY created_at_ms, attempt_id",
        )?;
        statement
            .query_map([], read_restore_saga)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    /// Removes only the durable marker. The Vault reconciler must delete the
    /// listed stable SecretRefs before invoking this method.
    pub fn abort_ssh_sync_restore_saga(
        &mut self,
        attempt_id: &OperationId,
    ) -> Result<Option<SshSyncRestoreSagaRecord>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_restore_saga(&transaction, attempt_id)?;
        if existing.is_some() {
            transaction.execute(
                "DELETE FROM ssh_sync_restore_sagas WHERE attempt_id = ?1",
                [attempt_id.as_str()],
            )?;
        }
        transaction.commit()?;
        Ok(existing)
    }

    pub fn preview_ssh_sync_restore_plan(
        &mut self,
        plan: &SshSyncRestorePlan,
    ) -> Result<SshSyncRestorePreview> {
        let normalized = normalized_restore_plan(plan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let preview = preview_restore_plan(&transaction, &normalized)?;
        transaction.commit()?;
        Ok(preview)
    }

    /// Publishes all restored metadata and removes the durable saga in one
    /// IMMEDIATE transaction. A present saga therefore always means metadata
    /// publication did not commit.
    pub fn commit_ssh_sync_restore_plan(
        &mut self,
        plan: &SshSyncRestorePlan,
    ) -> Result<SshSyncRestoreCommitResult> {
        let normalized = normalized_restore_plan(plan)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saga = get_restore_saga(&transaction, &normalized.attempt_id)?;
        let preview = preview_restore_plan(&transaction, &normalized)?;
        if preview.conflict_count > 0 {
            return Err(AppPersistenceError::Conflict);
        }
        if let Some(saga) = &saga {
            if !same_restore_plan_saga(saga, &normalized, &preview.new_secret_ref_ids) {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
        } else if preview.create_count > 0 {
            return Err(AppPersistenceError::NotFound);
        }

        let now = unix_time_ms();
        publish_restore_plan(&transaction, &normalized, now)?;
        if saga.is_some() {
            let deleted = transaction.execute(
                "DELETE FROM ssh_sync_restore_sagas WHERE attempt_id = ?1",
                [normalized.attempt_id.as_str()],
            )?;
            if deleted != 1 {
                return Err(AppPersistenceError::RestoreCommitUnknown);
            }
        }
        let result = SshSyncRestoreCommitResult {
            created_count: preview.create_count,
            already_applied_count: preview.already_applied_count,
        };
        if transaction.commit().is_err() {
            if matches!(
                restore_plan_commit_confirmed(&self.connection, &normalized),
                Ok(true)
            ) {
                return Ok(SshSyncRestoreCommitResult {
                    created_count: 0,
                    already_applied_count: preview
                        .already_applied_count
                        .saturating_add(preview.create_count),
                });
            }
            return Err(AppPersistenceError::RestoreCommitUnknown);
        }
        Ok(result)
    }

    /// Creates the signer-bound local control state for one profile. Exact
    /// replay returns the current record; initial-scope drift fails closed.
    pub fn ensure_ssh_sync_profile_state(
        &mut self,
        input: &SshSyncProfileStateCreate,
    ) -> Result<SshSyncProfileStateRecord> {
        let key = normalized_ssh_sync_profile_state_key(&input.key)?;
        let scope = normalized_ssh_sync_profile_scope(&input.scope)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = get_ssh_sync_profile_state_connection(&transaction, &key)? {
            if existing.scope != scope {
                return Err(AppPersistenceError::Conflict);
            }
            transaction.commit()?;
            return Ok(existing);
        }
        if ssh_sync_profile_exists_connection(&transaction, &key)? {
            return Err(AppPersistenceError::Conflict);
        }
        let profile_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM ssh_sync_profile_states WHERE plugin_id = ?1",
            [key.plugin_id.as_str()],
            |row| row.get(0),
        )?;
        if profile_count >= MAX_SSH_SYNC_PROFILES_PER_PLUGIN as i64 {
            return Err(AppPersistenceError::InvalidInput(
                "too many SSH sync profiles for one plugin",
            ));
        }
        let host_ids_json = encode_id_list(
            scope.custom_host_ids.iter().map(HostId::as_str),
            "SSH sync Host scope cannot be serialized",
        )?;
        let credential_ids_json = encode_id_list(
            scope
                .custom_credential_ref_ids
                .iter()
                .map(CredentialRefId::as_str),
            "SSH sync credential scope cannot be serialized",
        )?;
        let desktop_profile_ids_json = encode_id_list(
            scope.custom_desktop_profile_ids.iter().map(String::as_str),
            "SSH sync desktop profile scope cannot be serialized",
        )?;
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO ssh_sync_profile_states
             (plugin_id, signer_fingerprint_sha256, profile_id, scope_mode,
              custom_host_ids_json, custom_credential_ref_ids_json,
              custom_desktop_profile_ids_json,
              sync_key_secret_ref_id, password_wrapped_sync_key_envelope,
              remote_revision, remote_etag, baseline_content_sha256,
              baseline_exchange_sha256, state_version, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, NULL, NULL, NULL, 1, ?8)",
            params![
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                ssh_sync_scope_mode_to_db(scope.mode),
                host_ids_json,
                credential_ids_json,
                desktop_profile_ids_json,
                now,
            ],
        )?;
        transaction.commit()?;
        Ok(SshSyncProfileStateRecord {
            key: SshSyncProfileStateKey {
                plugin_id: key.plugin_id,
                signer_fingerprint_sha256: key.signer_fingerprint_sha256,
                profile_id: key.profile_id,
            },
            scope,
            key_binding: None,
            remote_baseline: None,
            state_version: WireSequence::new(1),
            updated_at_unix_ms: now,
            last_successful_sync_at_unix_ms: None,
        })
    }

    pub fn get_ssh_sync_profile_state(
        &self,
        key: &SshSyncProfileStateKey,
    ) -> Result<Option<SshSyncProfileStateRecord>> {
        let key = normalized_ssh_sync_profile_state_key(key)?;
        get_ssh_sync_profile_state_connection(&self.connection, &key)
    }

    pub fn list_ssh_sync_profile_states_for_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Vec<SshSyncProfileStateRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, signer_fingerprint_sha256, profile_id, scope_mode,
                    custom_host_ids_json, custom_credential_ref_ids_json,
                    custom_desktop_profile_ids_json,
                    sync_key_secret_ref_id, password_wrapped_sync_key_envelope,
                    remote_revision, remote_etag, baseline_content_sha256,
                    baseline_exchange_sha256, state_version, updated_at_ms,
                    last_successful_sync_at_ms
             FROM ssh_sync_profile_states AS profile WHERE plugin_id = ?1
               AND NOT EXISTS(
                 SELECT 1 FROM ssh_sync_plugin_delete_profiles AS deletion
                 WHERE deletion.plugin_id = profile.plugin_id
                   AND deletion.signer_fingerprint_sha256 = profile.signer_fingerprint_sha256
                   AND deletion.profile_id = profile.profile_id
               )
             ORDER BY signer_fingerprint_sha256, profile_id",
        )?;
        let records = statement
            .query_map([plugin_id.as_str()], read_ssh_sync_profile_state)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if records.len() > MAX_SSH_SYNC_PROFILES_PER_PLUGIN {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        Ok(records)
    }

    /// Binds the first Vault-backed sync key. Exact replay is idempotent even
    /// after a lost response; any different ref or envelope fails closed.
    pub fn bind_ssh_sync_profile_key(
        &mut self,
        key: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        binding: &SshSyncProfileKeyBinding,
    ) -> Result<SshSyncProfileStateRecord> {
        let key = normalized_ssh_sync_profile_state_key(key)?;
        let binding = normalized_ssh_sync_profile_key_binding(binding)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_ssh_sync_profile_state_connection(&transaction, &key)?
            .ok_or(AppPersistenceError::NotFound)?;
        if let Some(existing_binding) = &existing.key_binding {
            return if existing_binding == &binding {
                transaction.commit()?;
                Ok(existing)
            } else {
                Err(AppPersistenceError::Conflict)
            };
        }
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let ref_used: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM ssh_sync_profile_states
             WHERE sync_key_secret_ref_id = ?1)
               OR EXISTS(SELECT 1 FROM credential_secret_slots WHERE secret_ref_id = ?1)
               OR EXISTS(SELECT 1 FROM host_login_automation_steps WHERE secret_ref_id = ?1)
               OR EXISTS(SELECT 1 FROM ssh_sync_object_mappings
                         WHERE object_kind = 'secret' AND local_object_id = ?1)",
            [binding.sync_key_secret_ref_id.as_str()],
            |row| row.get(0),
        )?;
        if ref_used {
            return Err(AppPersistenceError::Conflict);
        }
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE ssh_sync_profile_states
             SET sync_key_secret_ref_id = ?1,
                 password_wrapped_sync_key_envelope = ?2,
                 state_version = ?3, updated_at_ms = ?4
             WHERE plugin_id = ?5 AND signer_fingerprint_sha256 = ?6
               AND profile_id = ?7 AND state_version = ?8
               AND sync_key_secret_ref_id IS NULL",
            params![
                binding.sync_key_secret_ref_id.as_str(),
                binding.password_wrapped_sync_key_envelope,
                u64_to_i64(next)?,
                now,
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(SshSyncProfileStateRecord {
            key: existing.key,
            scope: existing.scope,
            key_binding: Some(binding),
            remote_baseline: existing.remote_baseline,
            state_version: WireSequence::new(next),
            updated_at_unix_ms: now,
            last_successful_sync_at_unix_ms: existing.last_successful_sync_at_unix_ms,
        })
    }

    pub fn replace_ssh_sync_profile_scope(
        &mut self,
        key: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        scope: &SshSyncProfileScope,
    ) -> Result<SshSyncProfileStateRecord> {
        let key = normalized_ssh_sync_profile_state_key(key)?;
        let scope = normalized_ssh_sync_profile_scope(scope)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_ssh_sync_profile_state_connection(&transaction, &key)?
            .ok_or(AppPersistenceError::NotFound)?;
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        if existing.scope == scope {
            transaction.commit()?;
            return Ok(existing);
        }
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let host_ids_json = encode_id_list(
            scope.custom_host_ids.iter().map(HostId::as_str),
            "SSH sync Host scope cannot be serialized",
        )?;
        let credential_ids_json = encode_id_list(
            scope
                .custom_credential_ref_ids
                .iter()
                .map(CredentialRefId::as_str),
            "SSH sync credential scope cannot be serialized",
        )?;
        let desktop_profile_ids_json = encode_id_list(
            scope.custom_desktop_profile_ids.iter().map(String::as_str),
            "SSH sync desktop profile scope cannot be serialized",
        )?;
        let changed = transaction.execute(
            "UPDATE ssh_sync_profile_states
             SET scope_mode = ?1, custom_host_ids_json = ?2,
                 custom_credential_ref_ids_json = ?3,
                 custom_desktop_profile_ids_json = ?4,
                 state_version = ?5, updated_at_ms = ?6
             WHERE plugin_id = ?7 AND signer_fingerprint_sha256 = ?8
               AND profile_id = ?9 AND state_version = ?10",
            params![
                ssh_sync_scope_mode_to_db(scope.mode),
                host_ids_json,
                credential_ids_json,
                desktop_profile_ids_json,
                u64_to_i64(next)?,
                now,
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(SshSyncProfileStateRecord {
            key: existing.key,
            scope,
            key_binding: existing.key_binding,
            remote_baseline: existing.remote_baseline,
            state_version: WireSequence::new(next),
            updated_at_unix_ms: now,
            last_successful_sync_at_unix_ms: existing.last_successful_sync_at_unix_ms,
        })
    }

    pub fn replace_ssh_sync_profile_remote_baseline(
        &mut self,
        key: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        baseline: Option<&SshSyncProfileRemoteBaseline>,
    ) -> Result<SshSyncProfileStateRecord> {
        let key = normalized_ssh_sync_profile_state_key(key)?;
        let baseline = baseline
            .map(normalized_ssh_sync_remote_baseline)
            .transpose()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_ssh_sync_profile_state_connection(&transaction, &key)?
            .ok_or(AppPersistenceError::NotFound)?;
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        if existing.remote_baseline == baseline {
            transaction.commit()?;
            return Ok(existing);
        }
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE ssh_sync_profile_states
             SET remote_revision = ?1, remote_etag = ?2,
                 baseline_content_sha256 = ?3, baseline_exchange_sha256 = ?4,
                 state_version = ?5, updated_at_ms = ?6
             WHERE plugin_id = ?7 AND signer_fingerprint_sha256 = ?8
               AND profile_id = ?9 AND state_version = ?10",
            params![
                baseline.as_ref().map(|value| value.remote_revision as i64),
                baseline.as_ref().map(|value| value.remote_etag.as_str()),
                baseline
                    .as_ref()
                    .map(|value| value.baseline_content_sha256.as_str()),
                baseline
                    .as_ref()
                    .map(|value| value.baseline_exchange_sha256.as_str()),
                u64_to_i64(next)?,
                now,
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(SshSyncProfileStateRecord {
            key: existing.key,
            scope: existing.scope,
            key_binding: existing.key_binding,
            remote_baseline: baseline,
            state_version: WireSequence::new(next),
            updated_at_unix_ms: now,
            last_successful_sync_at_unix_ms: existing.last_successful_sync_at_unix_ms,
        })
    }

    /// Forgets only the remote synchronization projection for one profile.
    /// The local scope and Vault-backed sync key remain available so the next
    /// explicit sync can create a fresh remote exchange. Any uncertain PUT is
    /// removed in the same transaction because a confirmed remote reset
    /// supersedes it.
    pub fn reset_ssh_sync_profile_remote_state(
        &mut self,
        key: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        expected_upload_fence: &SshSyncHttpUploadCompletionFence,
    ) -> Result<SshSyncProfileStateRecord> {
        let key = normalized_ssh_sync_profile_state_key(key)?;
        let expected_upload_fence =
            normalized_ssh_sync_http_upload_completion_fence(expected_upload_fence)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_ssh_sync_profile_state_connection(&transaction, &key)?
            .ok_or(AppPersistenceError::NotFound)?;
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let pending_upload = get_ssh_sync_http_upload_attempt_connection(&transaction, &key)?;
        if pending_upload.as_ref().is_some_and(|attempt| {
            attempt.input.canonical_url != expected_upload_fence.canonical_url
                || attempt.input.http_method != expected_upload_fence.http_method
                || attempt.input.use_oauth != expected_upload_fence.use_oauth
                || attempt.input.authorization_revision
                    != expected_upload_fence.authorization_revision
                || attempt.input.configuration_revision
                    != expected_upload_fence.configuration_revision
        }) {
            return Err(AppPersistenceError::Conflict);
        }
        if existing.remote_baseline.is_none() && pending_upload.is_none() {
            transaction.commit()?;
            return Ok(existing);
        }
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE ssh_sync_profile_states
             SET remote_revision = NULL, remote_etag = NULL,
                 baseline_content_sha256 = NULL, baseline_exchange_sha256 = NULL,
                 state_version = ?1, updated_at_ms = ?2
             WHERE plugin_id = ?3 AND signer_fingerprint_sha256 = ?4
               AND profile_id = ?5 AND state_version = ?6",
            params![
                u64_to_i64(next)?,
                now,
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let deleted_uploads = transaction.execute(
            "DELETE FROM ssh_sync_http_upload_attempts
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
               AND canonical_url = ?4 AND http_method = ?5 AND use_oauth = ?6
               AND authorization_revision = ?7 AND configuration_revision = ?8",
            params![
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                expected_upload_fence.canonical_url,
                ssh_sync_http_method_to_db(expected_upload_fence.http_method),
                expected_upload_fence.use_oauth,
                u64_to_i64(expected_upload_fence.authorization_revision.get())?,
                u64_to_i64(expected_upload_fence.configuration_revision.get())?,
            ],
        )?;
        if pending_upload.is_some() && deleted_uploads != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(SshSyncProfileStateRecord {
            key: existing.key,
            scope: existing.scope,
            key_binding: existing.key_binding,
            remote_baseline: None,
            state_version: WireSequence::new(next),
            updated_at_unix_ms: now,
            last_successful_sync_at_unix_ms: existing.last_successful_sync_at_unix_ms,
        })
    }

    /// Records wall-clock success only after an upload or remote apply has
    /// completed. Refresh and scope configuration must not call this method.
    pub fn mark_ssh_sync_profile_sync_succeeded(
        &mut self,
        key: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
    ) -> Result<SshSyncProfileStateRecord> {
        let key = normalized_ssh_sync_profile_state_key(key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut existing = get_ssh_sync_profile_state_connection(&transaction, &key)?
            .ok_or(AppPersistenceError::NotFound)?;
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE ssh_sync_profile_states SET last_successful_sync_at_ms = ?1,
                    state_version = ?2, updated_at_ms = ?1
             WHERE plugin_id = ?3 AND signer_fingerprint_sha256 = ?4
               AND profile_id = ?5 AND state_version = ?6",
            params![
                now,
                u64_to_i64(next)?,
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id,
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        existing.state_version = WireSequence::new(next);
        existing.updated_at_unix_ms = now;
        existing.last_successful_sync_at_unix_ms = Some(now);
        Ok(existing)
    }

    /// Creates the single durable upload attempt for an owner. Reopening the
    /// database and repeating the exact logical upload returns the same
    /// idempotency key; any body or baseline drift fails closed.
    pub fn ensure_ssh_sync_http_upload_attempt(
        &mut self,
        input: &SshSyncHttpUploadAttemptInput,
    ) -> Result<SshSyncHttpUploadAttemptRecord> {
        let input = normalized_ssh_sync_http_upload_attempt(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if get_ssh_sync_profile_state_connection(&transaction, &input.owner)?.is_none() {
            return Err(AppPersistenceError::NotFound);
        }
        if let Some(existing) =
            get_ssh_sync_http_upload_attempt_connection(&transaction, &input.owner)?
        {
            if existing.input.owner != denormalized_ssh_sync_http_upload_input(&input).owner
                || existing.input.canonical_url != input.canonical_url
                || existing.input.http_method != input.http_method
                || existing.input.use_oauth != input.use_oauth
                || existing.input.authorization_revision != input.authorization_revision
                || existing.input.configuration_revision != input.configuration_revision
                || existing.input.base_revision != input.base_revision
                || existing.input.base_etag != input.base_etag
                || existing.input.target_revision != input.target_revision
                || existing.input.keyed_content_sha256 != input.keyed_content_sha256
                || existing.input.body_sha256 != input.body_sha256
            {
                return Err(AppPersistenceError::Conflict);
            }
            transaction.commit()?;
            return Ok(existing);
        }
        let idempotency_in_use: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_http_upload_attempts WHERE idempotency_key = ?1
             )",
            [input.idempotency_key.as_str()],
            |row| row.get(0),
        )?;
        if idempotency_in_use {
            return Err(AppPersistenceError::IdempotencyConflict);
        }
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO ssh_sync_http_upload_attempts
             (plugin_id, signer_fingerprint_sha256, profile_id,
              canonical_url, http_method, use_oauth,
              authorization_revision, configuration_revision,
              base_revision, base_etag, target_revision,
              keyed_content_sha256, body_sha256,
              idempotency_key, state, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, 'prepared', 1, ?15, ?15)",
            params![
                input.owner.plugin_id.as_str(),
                input.owner.signer_fingerprint_sha256,
                input.owner.profile_id,
                input.canonical_url,
                ssh_sync_http_method_to_db(input.http_method),
                input.use_oauth,
                u64_to_i64(input.authorization_revision.get())?,
                u64_to_i64(input.configuration_revision.get())?,
                u64_to_i64(input.base_revision)?,
                input.base_etag,
                u64_to_i64(input.target_revision)?,
                input.keyed_content_sha256,
                input.body_sha256,
                input.idempotency_key,
                now
            ],
        )?;
        transaction.commit()?;
        Ok(SshSyncHttpUploadAttemptRecord {
            input: denormalized_ssh_sync_http_upload_input(&input),
            state: SshSyncHttpUploadAttemptState::Prepared,
            state_version: WireSequence::new(1),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        })
    }

    pub fn get_ssh_sync_http_upload_attempt(
        &self,
        owner: &SshSyncProfileStateKey,
    ) -> Result<Option<SshSyncHttpUploadAttemptRecord>> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        get_ssh_sync_http_upload_attempt_connection(&self.connection, &owner)
    }

    pub fn list_pending_ssh_sync_http_upload_attempts(
        &self,
    ) -> Result<Vec<SshSyncHttpUploadAttemptRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, signer_fingerprint_sha256, profile_id,
                    canonical_url, http_method, use_oauth,
                    authorization_revision, configuration_revision,
                    base_revision, base_etag, target_revision,
                    keyed_content_sha256, body_sha256,
                    idempotency_key, state, state_version, created_at_ms, updated_at_ms
             FROM ssh_sync_http_upload_attempts
             ORDER BY created_at_ms, plugin_id, signer_fingerprint_sha256, profile_id",
        )?;
        statement
            .query_map([], read_ssh_sync_http_upload_attempt)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn advance_ssh_sync_http_upload_attempt(
        &mut self,
        owner: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        state: SshSyncHttpUploadAttemptState,
    ) -> Result<SshSyncHttpUploadAttemptRecord> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_ssh_sync_http_upload_attempt_connection(&transaction, &owner)?
            .ok_or(AppPersistenceError::NotFound)?;
        if existing.state_version != expected_state_version || state < existing.state {
            return Err(AppPersistenceError::Conflict);
        }
        if state == existing.state {
            transaction.commit()?;
            return Ok(existing);
        }
        let next = next_revision(expected_state_version)?;
        let now = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE ssh_sync_http_upload_attempts
             SET state = ?1, state_version = ?2, updated_at_ms = ?3
             WHERE plugin_id = ?4 AND signer_fingerprint_sha256 = ?5
               AND profile_id = ?6 AND state_version = ?7",
            params![
                ssh_sync_http_upload_state_to_db(state),
                u64_to_i64(next)?,
                now,
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                u64_to_i64(expected_state_version.get())?
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(SshSyncHttpUploadAttemptRecord {
            input: existing.input,
            state,
            state_version: WireSequence::new(next),
            created_at_unix_ms: existing.created_at_unix_ms,
            updated_at_unix_ms: now,
        })
    }

    /// Clears only the exact upload after Core supplies either a successful
    /// server ACK or a GET that observed the exact encrypted body.
    pub fn complete_ssh_sync_http_upload_attempt(
        &mut self,
        owner: &SshSyncProfileStateKey,
        idempotency_key: &str,
        fence: &SshSyncHttpUploadCompletionFence,
        proof: &SshSyncHttpUploadCompletionProof,
    ) -> Result<u32> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let idempotency_key = normalized_upload_idempotency_key(idempotency_key)?;
        let fence = normalized_ssh_sync_http_upload_completion_fence(fence)?;
        validate_ssh_sync_http_upload_completion_proof(proof)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = get_ssh_sync_http_upload_attempt_connection(&transaction, &owner)?;
        let Some(existing) = existing else {
            transaction.commit()?;
            return Ok(0);
        };
        if existing.input.idempotency_key != idempotency_key {
            return Err(AppPersistenceError::Conflict);
        }
        if existing.input.canonical_url != fence.canonical_url
            || existing.input.http_method != fence.http_method
            || existing.input.use_oauth != fence.use_oauth
            || existing.input.authorization_revision != fence.authorization_revision
            || existing.input.configuration_revision != fence.configuration_revision
        {
            return Err(AppPersistenceError::Conflict);
        }
        match proof {
            SshSyncHttpUploadCompletionProof::ServerAcknowledged {
                acknowledged_revision,
                acknowledged_body_sha256,
            } => {
                validate_lower_sha256(
                    acknowledged_body_sha256,
                    "invalid acknowledged SSH sync upload body digest",
                )?;
                if *acknowledged_revision != existing.input.target_revision
                    || acknowledged_body_sha256 != &existing.input.body_sha256
                {
                    return Err(AppPersistenceError::Conflict);
                }
            }
            SshSyncHttpUploadCompletionProof::ExactBodyObserved {
                observed_revision,
                observed_body_sha256,
            } => {
                validate_lower_sha256(
                    observed_body_sha256,
                    "invalid observed SSH sync upload body digest",
                )?;
                if *observed_revision != existing.input.target_revision
                    || observed_body_sha256 != &existing.input.body_sha256
                {
                    return Err(AppPersistenceError::Conflict);
                }
            }
            SshSyncHttpUploadCompletionProof::SupersededByNewerRemote {
                authenticated_remote_revision,
            } if *authenticated_remote_revision > existing.input.target_revision => {}
            SshSyncHttpUploadCompletionProof::SupersededByNewerRemote { .. } => {
                return Err(AppPersistenceError::Conflict);
            }
        }
        let deleted = transaction.execute(
            "DELETE FROM ssh_sync_http_upload_attempts
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND profile_id = ?3 AND idempotency_key = ?4 AND body_sha256 = ?5
               AND target_revision = ?6 AND canonical_url = ?7 AND http_method = ?8
               AND use_oauth = ?9 AND authorization_revision = ?10
               AND configuration_revision = ?11",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                idempotency_key,
                existing.input.body_sha256,
                u64_to_i64(existing.input.target_revision)?,
                fence.canonical_url,
                ssh_sync_http_method_to_db(fence.http_method),
                fence.use_oauth,
                u64_to_i64(fence.authorization_revision.get())?,
                u64_to_i64(fence.configuration_revision.get())?
            ],
        )?;
        if deleted != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        usize_to_u32(deleted)
    }

    pub fn list_ssh_sync_scope_memberships(
        &self,
        owner: &SshSyncProfileStateKey,
    ) -> Result<Vec<SshSyncScopeMembershipRecord>> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        if get_ssh_sync_profile_state_connection(&self.connection, &owner)?.is_none() {
            return Err(AppPersistenceError::NotFound);
        }
        list_ssh_sync_scope_memberships_connection(&self.connection, &owner)
    }

    /// Replaces the profile's membership/exclusion projection without touching
    /// any local Host, identity, credential, SecretRef, or object mapping.
    pub fn replace_ssh_sync_scope_memberships(
        &mut self,
        owner: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        memberships: &[SshSyncScopeMembershipInput],
    ) -> Result<SshSyncProfileStateRecord> {
        self.replace_ssh_sync_scope_memberships_internal(
            owner,
            expected_state_version,
            memberships,
            None,
        )
    }

    /// Replaces scope membership only while the full local repository still
    /// matches the snapshot approved for this staged restore.
    pub fn replace_ssh_sync_scope_memberships_with_fence(
        &mut self,
        owner: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        memberships: &[SshSyncScopeMembershipInput],
        expected_fence: &SshSyncChangeFence,
    ) -> Result<SshSyncProfileStateRecord> {
        self.replace_ssh_sync_scope_memberships_internal(
            owner,
            expected_state_version,
            memberships,
            Some(expected_fence),
        )
    }

    fn replace_ssh_sync_scope_memberships_internal(
        &mut self,
        owner: &SshSyncProfileStateKey,
        expected_state_version: WireSequence,
        memberships: &[SshSyncScopeMembershipInput],
        expected_fence: Option<&SshSyncChangeFence>,
    ) -> Result<SshSyncProfileStateRecord> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let memberships = normalized_ssh_sync_scope_memberships(memberships)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(expected_fence) = expected_fence {
            require_ssh_sync_change_fence(&transaction, expected_fence)?;
        }
        let mut existing = get_ssh_sync_profile_state_connection(&transaction, &owner)?
            .ok_or(AppPersistenceError::NotFound)?;
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let current = list_ssh_sync_scope_memberships_connection(&transaction, &owner)?;
        if current
            .iter()
            .map(|record| &record.membership)
            .eq(memberships.iter())
        {
            transaction.commit()?;
            return Ok(existing);
        }
        let now = unix_time_ms();
        replace_ssh_sync_scope_memberships_connection(&transaction, &owner, &memberships, now)?;
        let next = next_revision(expected_state_version)?;
        let changed = transaction.execute(
            "UPDATE ssh_sync_profile_states SET state_version = ?1, updated_at_ms = ?2
             WHERE plugin_id = ?3 AND signer_fingerprint_sha256 = ?4
               AND profile_id = ?5 AND state_version = ?6",
            params![
                u64_to_i64(next)?,
                now,
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                u64_to_i64(expected_state_version.get())?
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        existing.state_version = WireSequence::new(next);
        existing.updated_at_unix_ms = now;
        Ok(existing)
    }

    /// Resolves or creates a bounded mapping batch in one IMMEDIATE
    /// transaction. Either side changing its partner is a closed conflict.
    pub fn resolve_or_create_ssh_sync_object_mappings(
        &mut self,
        owner: &SshSyncProfileStateKey,
        inputs: &[SshSyncObjectMappingInput],
    ) -> Result<Vec<SshSyncObjectMappingResolution>> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let inputs = normalized_ssh_sync_object_mapping_inputs(inputs)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if get_ssh_sync_profile_state_connection(&transaction, &owner)?.is_none() {
            return Err(AppPersistenceError::NotFound);
        }
        let now = unix_time_ms();
        let owner_key = SshSyncProfileStateKey {
            plugin_id: owner.plugin_id.clone(),
            signer_fingerprint_sha256: owner.signer_fingerprint_sha256.clone(),
            profile_id: owner.profile_id.clone(),
        };
        let mut resolutions = Vec::with_capacity(inputs.len());
        let mut create_count = 0_usize;
        for input in &inputs {
            let existing = lookup_ssh_sync_object_mapping(&transaction, &owner, input)?;
            match existing.as_slice() {
                [] => {
                    create_count =
                        create_count
                            .checked_add(1)
                            .ok_or(AppPersistenceError::InvalidInput(
                                "too many SSH sync object mappings",
                            ))?;
                    resolutions.push(SshSyncObjectMappingResolution {
                        mapping: SshSyncObjectMappingRecord {
                            owner: owner_key.clone(),
                            object_kind: input.object_kind,
                            portable_object_id: input.portable_object_id.clone(),
                            local_object_id: input.local_object_id.clone(),
                            created_at_unix_ms: now,
                            updated_at_unix_ms: now,
                        },
                        created: true,
                    });
                }
                [mapping]
                    if mapping.portable_object_id == input.portable_object_id
                        && mapping.local_object_id == input.local_object_id =>
                {
                    resolutions.push(SshSyncObjectMappingResolution {
                        mapping: mapping.clone(),
                        created: false,
                    });
                }
                _ => return Err(AppPersistenceError::Conflict),
            }
        }
        let existing_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM ssh_sync_object_mappings
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id
            ],
            |row| row.get(0),
        )?;
        let existing_count =
            usize::try_from(existing_count).map_err(|_| AppPersistenceError::InvalidStoredData)?;
        let resulting_count = existing_count
            .checked_add(create_count)
            .ok_or(AppPersistenceError::InvalidStoredData)?;
        if resulting_count > MAX_SSH_SYNC_OBJECT_MAPPINGS_PER_OWNER {
            return Err(AppPersistenceError::InvalidInput(
                "too many SSH sync object mappings for one profile",
            ));
        }
        for resolution in &resolutions {
            if !resolution.created {
                continue;
            }
            transaction.execute(
                "INSERT INTO ssh_sync_object_mappings
                 (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                  portable_object_id, local_object_id, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id,
                    ssh_sync_object_kind_to_db(resolution.mapping.object_kind),
                    resolution.mapping.portable_object_id,
                    resolution.mapping.local_object_id.as_str(),
                    now,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(resolutions)
    }

    pub fn list_ssh_sync_object_mappings(
        &self,
        owner: &SshSyncProfileStateKey,
    ) -> Result<Vec<SshSyncObjectMappingRecord>> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                    portable_object_id, local_object_id, created_at_ms, updated_at_ms
             FROM ssh_sync_object_mappings
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
             ORDER BY object_kind, portable_object_id",
        )?;
        let records = statement
            .query_map(
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id
                ],
                read_ssh_sync_object_mapping,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if records.len() > MAX_SSH_SYNC_OBJECT_MAPPINGS_PER_OWNER {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        Ok(records)
    }

    pub fn delete_ssh_sync_object_mappings_for_plugin(
        &mut self,
        plugin_id: &PluginId,
    ) -> Result<u32> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = transaction.execute(
            "DELETE FROM ssh_sync_object_mappings WHERE plugin_id = ?1",
            [plugin_id.as_str()],
        )?;
        let deleted = usize_to_u32(deleted)?;
        transaction.commit()?;
        Ok(deleted)
    }

    /// Applies only an explicit, Core-approved three-way-merge delta. Scope
    /// changes never call this API and therefore cannot implicitly delete data.
    pub fn apply_ssh_sync_owned_metadata_delta(
        &mut self,
        delta: &SshSyncOwnedMetadataDelta,
    ) -> Result<SshSyncOwnedDeltaResult> {
        self.apply_ssh_sync_owned_metadata_delta_internal(delta, None, None)
    }

    /// Applies an owned metadata delta and, when supplied, a full replacement
    /// of portable scope membership in the same SQLite transaction. An empty
    /// replacement means remote omission and never deletes local metadata.
    pub fn apply_ssh_sync_owned_metadata_delta_with_scope_memberships(
        &mut self,
        delta: &SshSyncOwnedMetadataDelta,
        memberships: Option<&[SshSyncScopeMembershipInput]>,
    ) -> Result<SshSyncOwnedDeltaResult> {
        self.apply_ssh_sync_owned_metadata_delta_internal(delta, memberships, None)
    }

    /// Applies the approved owned delta and scope replacement only if no local
    /// repository write has occurred since the staged snapshot (or the fenced
    /// restore-saga write that deliberately followed it).
    pub fn apply_ssh_sync_owned_metadata_delta_with_scope_memberships_and_fence(
        &mut self,
        delta: &SshSyncOwnedMetadataDelta,
        memberships: Option<&[SshSyncScopeMembershipInput]>,
        expected_fence: &SshSyncChangeFence,
    ) -> Result<SshSyncOwnedDeltaResult> {
        self.apply_ssh_sync_owned_metadata_delta_internal(delta, memberships, Some(expected_fence))
    }

    fn apply_ssh_sync_owned_metadata_delta_internal(
        &mut self,
        delta: &SshSyncOwnedMetadataDelta,
        memberships: Option<&[SshSyncScopeMembershipInput]>,
        expected_fence: Option<&SshSyncChangeFence>,
    ) -> Result<SshSyncOwnedDeltaResult> {
        validate_ssh_sync_owned_delta(delta)?;
        let owner = normalized_ssh_sync_profile_state_key(&delta.owner)?;
        let memberships = memberships
            .map(normalized_ssh_sync_scope_memberships)
            .transpose()?;
        let memberships_json = memberships
            .as_deref()
            .map(encode_ssh_sync_scope_memberships)
            .transpose()?;
        let normalized_creates = delta
            .creates
            .as_ref()
            .map(|batch| -> Result<_> {
                Ok((
                    normalized_restore_plan(&batch.plan)?,
                    normalized_ssh_sync_object_mapping_inputs(&batch.mappings)?,
                ))
            })
            .transpose()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(expected_fence) = expected_fence {
            require_ssh_sync_change_fence(&transaction, expected_fence)?;
        }
        if let Some(result) = read_owned_delta_replay(
            &transaction,
            &owner,
            &delta.attempt_id,
            &delta.delta_sha256,
            memberships_json.as_deref(),
        )? {
            transaction.commit()?;
            return Ok(result);
        }
        if get_ssh_sync_profile_state_connection(&transaction, &owner)?.is_none() {
            return Err(AppPersistenceError::NotFound);
        }
        let mut created_count = 0_u32;
        let mut create_saga_present = false;
        if let Some((plan, mappings)) = &normalized_creates {
            if plan.plugin_id != owner.plugin_id
                || plan.profile_id != owner.profile_id
                || plan.attempt_id != delta.attempt_id
            {
                return Err(AppPersistenceError::Conflict);
            }
            let preview = preview_restore_plan(&transaction, plan)?;
            if preview.conflict_count != 0 {
                return Err(AppPersistenceError::Conflict);
            }
            let mut expected_new_refs = preview.new_secret_ref_ids.clone();
            expected_new_refs.extend(
                delta
                    .secret_replacements
                    .iter()
                    .map(|value| value.new_secret_ref_id.clone()),
            );
            let expected_new_refs = normalized_secret_ref_ids(&expected_new_refs)?;
            let saga = get_restore_saga(&transaction, &plan.attempt_id)?
                .ok_or(AppPersistenceError::NotFound)?;
            if saga.plugin_id != plan.plugin_id
                || saga.profile_id != plan.profile_id
                || saga.bundle_sha256 != plan.bundle_sha256
                || saga.plan_sha256 != plan.plan_sha256
                || saga.secret_ref_ids != expected_new_refs
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            preflight_owned_create_mappings(&transaction, &owner, plan, mappings)?;
            created_count = preview.create_count;
            create_saga_present = true;
        } else if !delta.secret_replacements.is_empty() {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync Secret replacement requires a restore create fence",
            ));
        }
        preflight_owned_delta(&transaction, &owner, delta)?;
        let now = unix_time_ms();
        let mut gc_candidates = BTreeMap::<String, &'static str>::new();

        if let Some((plan, mappings)) = &normalized_creates {
            publish_restore_plan(&transaction, plan, now)?;
            apply_owned_create_mappings(&transaction, &owner, mappings, now)?;
        }

        for update in &delta.desktop_profile_updates {
            apply_owned_desktop_profile_update(&transaction, update)?;
        }

        for update in &delta.identity_updates {
            apply_owned_identity_update(&transaction, update, now)?;
        }
        for update in &delta.credential_updates {
            for secret_ref in apply_owned_credential_update(&transaction, update, now)? {
                gc_candidates.insert(secret_ref.as_str().to_owned(), "credential_replaced");
            }
        }
        for update in &delta.host_updates {
            for secret_ref in apply_owned_host_update(&transaction, update, now)? {
                gc_candidates.insert(secret_ref.as_str().to_owned(), "host_updated");
            }
        }
        for replacement in &delta.secret_replacements {
            apply_owned_secret_replacement(&transaction, &owner, replacement, now)?;
            gc_candidates.insert(
                replacement.expected_secret_ref_id.as_str().to_owned(),
                "secret_replaced",
            );
        }

        for delete in &delta.desktop_profile_deletes {
            delete_owned_desktop_profile(&transaction, &owner, delete)?;
        }
        delete_owned_hosts(&transaction, &owner, &delta.host_deletes)?;
        for delete in &delta.credential_deletes {
            for secret_ref in delete_owned_credential(&transaction, &owner, delete)? {
                gc_candidates.insert(secret_ref.as_str().to_owned(), "credential_deleted");
            }
        }
        for delete in &delta.identity_deletes {
            delete_owned_identity(&transaction, &owner, delete)?;
        }
        for delete in &delta.secret_deletes {
            delete_owned_secret_mapping(&transaction, &owner, delete)?;
            gc_candidates.insert(delete.secret_ref_id.as_str().to_owned(), "secret_deleted");
        }

        if let Some(memberships) = &memberships {
            replace_ssh_sync_scope_memberships_connection(&transaction, &owner, memberships, now)?;
        }

        let mut pending_gc = Vec::new();
        for (secret_ref, reason) in gc_candidates {
            let secret_ref = SecretRefId::parse(secret_ref)
                .map_err(|_| AppPersistenceError::InvalidStoredData)?;
            if secret_ref_is_referenced(&transaction, &secret_ref)? {
                if reason == "secret_deleted" {
                    return Err(AppPersistenceError::Conflict);
                }
                continue;
            }
            transaction.execute(
                "INSERT INTO ssh_sync_vault_gc_queue
                 (plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id,
                  reason, attempt_id, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id)
                 DO NOTHING",
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id,
                    secret_ref.as_str(),
                    reason,
                    delta.attempt_id.as_str(),
                    now,
                ],
            )?;
            pending_gc.push(secret_ref);
        }
        pending_gc.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        let updated_count = usize_to_u32(
            delta.identity_updates.len()
                + delta.credential_updates.len()
                + delta.host_updates.len()
                + delta.desktop_profile_updates.len()
                + delta.secret_replacements.len(),
        )?;
        let deleted_count = usize_to_u32(
            delta.identity_deletes.len()
                + delta.credential_deletes.len()
                + delta.host_deletes.len()
                + delta.desktop_profile_deletes.len()
                + delta.secret_deletes.len(),
        )?;
        let gc_json = encode_secret_ref_ids(&pending_gc)?;
        if create_saga_present {
            let deleted = transaction.execute(
                "DELETE FROM ssh_sync_restore_sagas WHERE attempt_id = ?1",
                [delta.attempt_id.as_str()],
            )?;
            if deleted != 1 {
                return Err(AppPersistenceError::RestoreCommitUnknown);
            }
        }
        transaction.execute(
            "INSERT INTO ssh_sync_owned_delta_operations
             (plugin_id, signer_fingerprint_sha256, profile_id, attempt_id,
              delta_sha256, created_count, updated_count, deleted_count,
              gc_ref_ids_json, committed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                delta.attempt_id.as_str(),
                delta.delta_sha256,
                i64::from(created_count),
                i64::from(updated_count),
                i64::from(deleted_count),
                gc_json,
                now,
            ],
        )?;
        if let Some(memberships_json) = memberships_json {
            transaction.execute(
                "INSERT INTO ssh_sync_owned_delta_scope_replacements
                 (plugin_id, signer_fingerprint_sha256, profile_id,
                  attempt_id, memberships_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id,
                    delta.attempt_id.as_str(),
                    memberships_json
                ],
            )?;
        }
        transaction.commit()?;
        Ok(SshSyncOwnedDeltaResult {
            created_count,
            updated_count,
            deleted_count,
            pending_vault_gc_secret_ref_ids: pending_gc,
            replayed: false,
        })
    }

    pub fn list_ssh_sync_vault_gc(
        &self,
        owner: &SshSyncProfileStateKey,
    ) -> Result<Vec<SshSyncVaultGcRecord>> {
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id,
                    reason, attempt_id, created_at_ms
             FROM ssh_sync_vault_gc_queue
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
             ORDER BY created_at_ms, secret_ref_id",
        )?;
        statement
            .query_map(
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id
                ],
                read_ssh_sync_vault_gc,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    /// Acknowledges only refs that Core has already deleted successfully from
    /// Vault. Missing refs are an idempotent no-op.
    pub fn ack_ssh_sync_vault_gc(
        &mut self,
        owner: &SshSyncProfileStateKey,
        secret_ref_ids: &[SecretRefId],
    ) -> Result<u32> {
        if secret_ref_ids.len() > MAX_SSH_SYNC_VAULT_GC_ACK {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync Vault GC acknowledgement is too large",
            ));
        }
        let refs = normalized_secret_ref_ids(secret_ref_ids)?;
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut deleted = 0_u32;
        for secret_ref in refs {
            deleted = deleted
                .checked_add(usize_to_u32(transaction.execute(
                    "DELETE FROM ssh_sync_vault_gc_queue
                     WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                       AND profile_id = ?3 AND secret_ref_id = ?4",
                    params![
                        owner.plugin_id.as_str(),
                        owner.signer_fingerprint_sha256,
                        owner.profile_id,
                        secret_ref.as_str(),
                    ],
                )?)?)
                .ok_or(AppPersistenceError::InvalidStoredData)?;
        }
        transaction.commit()?;
        Ok(deleted)
    }

    /// Atomically fences every profile currently owned by `plugin_id` before
    /// any Vault deletion starts. Exact operation replay returns the durable
    /// outstanding tasks; another concurrent delete operation fails closed.
    pub fn begin_ssh_sync_plugin_delete(
        &mut self,
        plugin_id: &PluginId,
        operation_id: &OperationId,
    ) -> Result<SshSyncPluginDeleteBegin> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT operation_id, state FROM ssh_sync_plugin_delete_operations
                 WHERE plugin_id = ?1 AND (state = 'pending' OR operation_id = ?2)
                 ORDER BY CASE WHEN operation_id = ?2 THEN 0 ELSE 1 END LIMIT 1",
                params![plugin_id.as_str(), operation_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((stored_operation_id, state)) = existing {
            if stored_operation_id != operation_id.as_str() {
                return Err(AppPersistenceError::Conflict);
            }
            let tasks = list_ssh_sync_plugin_delete_tasks_connection(
                &transaction,
                Some(plugin_id),
                Some(operation_id),
            )?;
            transaction.commit()?;
            return Ok(SshSyncPluginDeleteBegin {
                operation_id: operation_id.clone(),
                tasks,
                completed: state == "completed",
                replayed: true,
            });
        }

        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO ssh_sync_plugin_delete_operations
             (plugin_id, operation_id, state, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 'pending', ?3, ?3)",
            params![plugin_id.as_str(), operation_id.as_str(), now],
        )?;
        let owners = {
            let mut statement = transaction.prepare(
                "SELECT signer_fingerprint_sha256, profile_id
                 FROM ssh_sync_profile_states WHERE plugin_id = ?1
                 ORDER BY signer_fingerprint_sha256, profile_id",
            )?;
            statement
                .query_map([plugin_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if owners.len() > MAX_SSH_SYNC_PROFILES_PER_PLUGIN {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        for (signer, profile_id) in &owners {
            transaction.execute(
                "INSERT INTO ssh_sync_plugin_delete_profiles
                 (plugin_id, operation_id, signer_fingerprint_sha256, profile_id, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    plugin_id.as_str(),
                    operation_id.as_str(),
                    signer,
                    profile_id,
                    now
                ],
            )?;
            transaction.execute(
                "INSERT INTO ssh_sync_plugin_delete_refs
                 (plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id, created_at_ms)
                 SELECT plugin_id, signer_fingerprint_sha256, profile_id,
                        sync_key_secret_ref_id, ?4
                 FROM ssh_sync_profile_states
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND profile_id = ?3 AND sync_key_secret_ref_id IS NOT NULL
                 ON CONFLICT DO NOTHING",
                params![plugin_id.as_str(), signer, profile_id, now],
            )?;
            transaction.execute(
                "INSERT INTO ssh_sync_plugin_delete_refs
                 (plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id, created_at_ms)
                 SELECT plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id, ?4
                 FROM ssh_sync_vault_gc_queue
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
                 ON CONFLICT DO NOTHING",
                params![plugin_id.as_str(), signer, profile_id, now],
            )?;
            transaction.execute(
                "INSERT INTO ssh_sync_plugin_delete_refs
                 (plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id, created_at_ms)
                 SELECT ?1, ?2, ?3, value, ?4
                 FROM ssh_sync_restore_sagas, json_each(secret_ref_ids_json)
                 WHERE plugin_id = ?1 AND profile_id = ?3
                 ON CONFLICT DO NOTHING",
                params![plugin_id.as_str(), signer, profile_id, now],
            )?;
            transaction.execute(
                "DELETE FROM ssh_sync_http_upload_attempts
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3",
                params![plugin_id.as_str(), signer, profile_id],
            )?;
        }
        finalize_empty_ssh_sync_plugin_delete_profiles(&transaction, plugin_id, operation_id, now)?;
        let tasks = list_ssh_sync_plugin_delete_tasks_connection(
            &transaction,
            Some(plugin_id),
            Some(operation_id),
        )?;
        let completed = tasks.is_empty();
        if completed {
            transaction.execute(
                "UPDATE ssh_sync_plugin_delete_operations
                 SET state = 'completed', updated_at_ms = ?3
                 WHERE plugin_id = ?1 AND operation_id = ?2 AND state = 'pending'",
                params![plugin_id.as_str(), operation_id.as_str(), now],
            )?;
        }
        transaction.commit()?;
        Ok(SshSyncPluginDeleteBegin {
            operation_id: operation_id.clone(),
            tasks,
            completed,
            replayed: false,
        })
    }

    pub fn list_pending_ssh_sync_plugin_deletes(&self) -> Result<Vec<SshSyncPluginDeleteTask>> {
        list_ssh_sync_plugin_delete_tasks_connection(&self.connection, None, None)
    }

    /// ACKs only refs already removed idempotently from Vault. The last ACK
    /// finalizes the fenced profile and, once all profiles finish, the plugin
    /// delete operation. Exact ACK replay remains a successful no-op.
    pub fn ack_ssh_sync_plugin_delete_refs(
        &mut self,
        owner: &SshSyncProfileStateKey,
        operation_id: &OperationId,
        secret_ref_ids: &[SecretRefId],
    ) -> Result<SshSyncPluginDeleteAck> {
        if secret_ref_ids.len() > MAX_SSH_SYNC_VAULT_GC_ACK {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync plugin delete acknowledgement is too large",
            ));
        }
        let owner = normalized_ssh_sync_profile_state_key(owner)?;
        let refs = normalized_secret_ref_ids(secret_ref_ids)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation_state = transaction
            .query_row(
                "SELECT state FROM ssh_sync_plugin_delete_operations
                 WHERE plugin_id = ?1 AND operation_id = ?2",
                params![owner.plugin_id.as_str(), operation_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(operation_state) = operation_state else {
            return Err(AppPersistenceError::NotFound);
        };
        if operation_state == "completed" {
            transaction.commit()?;
            return Ok(SshSyncPluginDeleteAck {
                acknowledged_count: 0,
                profile_finalized: true,
                operation_completed: true,
            });
        }
        let profile_pending: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_plugin_delete_profiles
               WHERE plugin_id = ?1 AND operation_id = ?2
                 AND signer_fingerprint_sha256 = ?3 AND profile_id = ?4
             )",
            params![
                owner.plugin_id.as_str(),
                operation_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id
            ],
            |row| row.get(0),
        )?;
        if !profile_pending {
            return Err(AppPersistenceError::NotFound);
        }
        let mut acknowledged_count = 0_u32;
        for secret_ref in refs {
            acknowledged_count = acknowledged_count
                .checked_add(usize_to_u32(transaction.execute(
                    "DELETE FROM ssh_sync_plugin_delete_refs
                     WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                       AND profile_id = ?3 AND secret_ref_id = ?4",
                    params![
                        owner.plugin_id.as_str(),
                        owner.signer_fingerprint_sha256,
                        owner.profile_id,
                        secret_ref.as_str()
                    ],
                )?)?)
                .ok_or(AppPersistenceError::InvalidStoredData)?;
            transaction.execute(
                "DELETE FROM ssh_sync_vault_gc_queue
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND profile_id = ?3 AND secret_ref_id = ?4",
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id,
                    secret_ref.as_str()
                ],
            )?;
        }
        let now = unix_time_ms();
        finalize_empty_ssh_sync_plugin_delete_profiles(
            &transaction,
            &owner.plugin_id,
            operation_id,
            now,
        )?;
        let profile_finalized: bool = !transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_plugin_delete_profiles
               WHERE plugin_id = ?1 AND operation_id = ?2
                 AND signer_fingerprint_sha256 = ?3 AND profile_id = ?4
             )",
            params![
                owner.plugin_id.as_str(),
                operation_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id
            ],
            |row| row.get(0),
        )?;
        let pending_profiles: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_plugin_delete_profiles
               WHERE plugin_id = ?1 AND operation_id = ?2
             )",
            params![owner.plugin_id.as_str(), operation_id.as_str()],
            |row| row.get(0),
        )?;
        let operation_completed = !pending_profiles;
        if operation_completed {
            transaction.execute(
                "UPDATE ssh_sync_plugin_delete_operations
                 SET state = 'completed', updated_at_ms = ?3
                 WHERE plugin_id = ?1 AND operation_id = ?2 AND state = 'pending'",
                params![owner.plugin_id.as_str(), operation_id.as_str(), now],
            )?;
        }
        transaction.commit()?;
        Ok(SshSyncPluginDeleteAck {
            acknowledged_count,
            profile_finalized,
            operation_completed,
        })
    }

    /// Compatibility entrypoint. This now starts a durable fenced delete and
    /// intentionally does not erase profile pointers before Vault ACK.
    pub fn delete_ssh_sync_profile_states_for_plugin(
        &mut self,
        plugin_id: &PluginId,
    ) -> Result<SshSyncProfileStateDeleteResult> {
        let begun = self.begin_ssh_sync_plugin_delete(plugin_id, &OperationId::new())?;
        let mut sync_key_secret_ref_ids = begun
            .tasks
            .iter()
            .flat_map(|task| task.secret_ref_ids.iter().cloned())
            .collect::<Vec<_>>();
        sync_key_secret_ref_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        sync_key_secret_ref_ids.dedup();
        Ok(SshSyncProfileStateDeleteResult {
            deleted_count: usize_to_u32(begun.tasks.len())?,
            sync_key_secret_ref_ids,
        })
    }

    pub fn plugin_catalog_trust(&self) -> Result<Option<PluginCatalogTrustRecord>> {
        self.connection
            .query_row(
                "SELECT root_key_id, sequence, payload_sha256, catalog_signature_base64,
                        catalog_revision, expires_at_ms, verified_at_ms
                 FROM plugin_catalog_trust WHERE singleton = 1",
                [],
                read_plugin_catalog_trust,
            )
            .optional()
            .map_err(AppPersistenceError::from)
    }

    pub fn replace_plugin_catalog(
        &mut self,
        expected: Option<&PluginCatalogTrustRecord>,
        trust: &PluginCatalogTrustRecord,
        entries: &[PluginCatalogEntryRecord],
    ) -> Result<()> {
        validate_plugin_catalog(trust, entries)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction
            .query_row(
                "SELECT root_key_id, sequence, payload_sha256, catalog_signature_base64,
                        catalog_revision, expires_at_ms, verified_at_ms
                 FROM plugin_catalog_trust WHERE singleton = 1",
                [],
                read_plugin_catalog_trust,
            )
            .optional()?;
        if current.as_ref() != expected {
            return Err(AppPersistenceError::Conflict);
        }
        if let Some(current) = &current
            && (trust.sequence < current.sequence
                || (trust.sequence == current.sequence && trust != current))
        {
            return Err(AppPersistenceError::Conflict);
        }
        if current.as_ref() == Some(trust) {
            transaction.commit()?;
            return Ok(());
        }
        transaction.execute("DELETE FROM plugin_catalog_entries", [])?;
        for entry in entries {
            transaction.execute(
                "INSERT INTO plugin_catalog_entries
                 (plugin_id, version, name, publisher, protocol_major, protocol_minor,
                  platform, architectures_json, package_url, package_size, package_sha256,
                  publisher_key_base64, publisher_signature_base64, capabilities_json,
                  minimum_app_version, published_at_ms, catalog_sequence, release_details_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?16, ?17, ?18)",
                params![
                    entry.plugin_id.as_str(),
                    entry.version,
                    entry.name,
                    entry.publisher,
                    i64::from(entry.protocol_major),
                    i64::from(entry.protocol_minor),
                    entry.platform,
                    serde_json::to_string(&entry.architectures).map_err(|_| {
                        AppPersistenceError::InvalidInput("invalid plugin architectures")
                    })?,
                    entry.package_url,
                    u64_to_i64(entry.package_size)?,
                    entry.package_sha256,
                    entry.publisher_key_base64,
                    entry.publisher_signature_base64,
                    serde_json::to_string(&if entry.unsupported_manifest {
                        serde_json::json!({"formatVersion": 1, "capabilities": entry.raw_capabilities, "unsupportedManifest": true})
                    } else { serde_json::json!(entry.raw_capabilities) }).map_err(|_| {
                        AppPersistenceError::InvalidInput("invalid plugin capabilities")
                    })?,
                    entry.minimum_app_version,
                    entry.published_at_unix_ms,
                    u64_to_i64(trust.sequence)?,
                    entry
                        .details
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|_| AppPersistenceError::InvalidInput(
                            "invalid catalog release details"
                        ))?,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO plugin_catalog_trust
             (singleton, root_key_id, sequence, payload_sha256, catalog_signature_base64,
              catalog_revision, expires_at_ms, verified_at_ms)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(singleton) DO UPDATE SET
               root_key_id = excluded.root_key_id,
               sequence = excluded.sequence,
               payload_sha256 = excluded.payload_sha256,
               catalog_signature_base64 = excluded.catalog_signature_base64,
               catalog_revision = excluded.catalog_revision,
               expires_at_ms = excluded.expires_at_ms,
               verified_at_ms = excluded.verified_at_ms",
            params![
                trust.root_key_id,
                u64_to_i64(trust.sequence)?,
                trust.payload_sha256,
                trust.catalog_signature_base64,
                trust.catalog_revision,
                trust.expires_at_unix_ms,
                trust.verified_at_unix_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_plugin_catalog_entries(&self) -> Result<Vec<PluginCatalogEntryRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, version, name, publisher, protocol_major, protocol_minor,
                    platform, architectures_json, package_url, package_size, package_sha256,
                    publisher_key_base64, publisher_signature_base64, capabilities_json,
                    minimum_app_version, published_at_ms, release_details_json
             FROM plugin_catalog_entries ORDER BY plugin_id, version",
        )?;
        statement
            .query_map([], read_plugin_catalog_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn begin_plugin_operation(
        &mut self,
        operation_id: &PluginOperationId,
        plugin_id: Option<&PluginId>,
        kind: PluginOperationKind,
        idempotency_key: &str,
        request_fingerprint_sha256: &str,
        candidate_version: Option<&str>,
        expected_active_version: Option<&str>,
    ) -> Result<PluginOperationRecord> {
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        validate_digest(request_fingerprint_sha256)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT operation_id, plugin_id, kind, state, phase, idempotency_key,
                        request_fingerprint_sha256, candidate_version, expected_active_version,
                        error_code, state_version, created_at_ms, updated_at_ms
                 FROM plugin_operations
                 WHERE operation_id = ?1 OR idempotency_key = ?2",
                params![operation_id.as_str(), idempotency_key],
                read_plugin_operation,
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.operation_id != *operation_id
                || existing.plugin_id.as_ref() != plugin_id
                || existing.kind != kind
                || existing.idempotency_key != idempotency_key
                || existing.request_fingerprint_sha256 != request_fingerprint_sha256
                || existing.candidate_version.as_deref() != candidate_version
                || existing.expected_active_version.as_deref() != expected_active_version
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(existing);
        }
        if plugin_id.is_some()
            && matches!(
                kind,
                PluginOperationKind::Install | PluginOperationKind::Update
            )
        {
            let concurrent_install: bool = transaction.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM plugin_operations
                   WHERE plugin_id = ?1 AND kind IN ('install', 'update')
                     AND state NOT IN ('succeeded', 'failed', 'cancelled')
                 )",
                [plugin_id.map(PluginId::as_str)],
                |row| row.get(0),
            )?;
            if concurrent_install {
                return Err(AppPersistenceError::Conflict);
            }
        }
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO plugin_operations
             (operation_id, plugin_id, kind, state, phase, idempotency_key,
              request_fingerprint_sha256, candidate_version, expected_active_version,
              error_code, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 'pending', 'resolve', ?4, ?5, ?6, ?7, NULL, 1, ?8, ?8)",
            params![
                operation_id.as_str(),
                plugin_id.map(PluginId::as_str),
                plugin_operation_kind_to_db(kind),
                idempotency_key,
                request_fingerprint_sha256,
                candidate_version,
                expected_active_version,
                now,
            ],
        )?;
        transaction.commit()?;
        self.get_plugin_operation(operation_id)
    }

    pub fn get_plugin_operation(
        &self,
        operation_id: &PluginOperationId,
    ) -> Result<PluginOperationRecord> {
        self.connection
            .query_row(
                "SELECT operation_id, plugin_id, kind, state, phase, idempotency_key,
                        request_fingerprint_sha256, candidate_version, expected_active_version,
                        error_code, state_version, created_at_ms, updated_at_ms
                 FROM plugin_operations WHERE operation_id = ?1",
                [operation_id.as_str()],
                read_plugin_operation,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    pub fn advance_plugin_operation(
        &mut self,
        operation_id: &PluginOperationId,
        expected_state_version: WireSequence,
        state: PluginOperationState,
        phase: PluginOperationPhase,
        error_code: Option<&str>,
    ) -> Result<PluginOperationRecord> {
        let next = next_revision(expected_state_version)?;
        let changed = self.connection.execute(
            "UPDATE plugin_operations
             SET state = ?1, phase = ?2, error_code = ?3, state_version = ?4,
                 updated_at_ms = ?5
             WHERE operation_id = ?6 AND state_version = ?7",
            params![
                plugin_operation_state_to_db(state),
                plugin_operation_phase_to_db(phase),
                error_code,
                u64_to_i64(next)?,
                unix_time_ms(),
                operation_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        self.get_plugin_operation(operation_id)
    }

    pub fn list_reconcilable_plugin_operations(&self) -> Result<Vec<PluginOperationRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT operation_id, plugin_id, kind, state, phase, idempotency_key,
                    request_fingerprint_sha256, candidate_version, expected_active_version,
                    error_code, state_version, created_at_ms, updated_at_ms
             FROM plugin_operations
             WHERE state NOT IN ('succeeded', 'failed', 'cancelled')
                OR phase = 'reconcile_required'
             ORDER BY created_at_ms, operation_id",
        )?;
        statement
            .query_map([], read_plugin_operation)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn activate_plugin_installation(
        &mut self,
        expected_state_version: Option<WireSequence>,
        candidate: &PluginInstalledRecord,
        package_size: u64,
        publisher_key_base64: &str,
        publisher_signature_base64: &str,
        approval_granted: bool,
        permissions: Option<PluginActivationPermissions<'_>>,
    ) -> Result<PluginInstalledRecord> {
        self.activate_plugin_installation_with_settings(
            expected_state_version,
            candidate,
            package_size,
            publisher_key_base64,
            publisher_signature_base64,
            approval_granted,
            permissions,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn activate_plugin_installation_with_settings(
        &mut self,
        expected_state_version: Option<WireSequence>,
        candidate: &PluginInstalledRecord,
        package_size: u64,
        publisher_key_base64: &str,
        publisher_signature_base64: &str,
        approval_granted: bool,
        permissions: Option<PluginActivationPermissions<'_>>,
        settings: Option<PluginSettingsInstall<'_>>,
    ) -> Result<PluginInstalledRecord> {
        validate_plugin_installed(candidate)?;
        if let Some(permissions) = &permissions {
            validate_plugin_permission_binding(permissions.binding)?;
            if permissions.binding.artifact_sha256 != candidate.package_sha256
                || !complete_plugin_capability_decision(&candidate.capabilities, permissions.grants)
                || permissions
                    .carried_host_scope_capabilities
                    .iter()
                    .any(|capability| {
                        !host_scoped_plugin_capability(*capability)
                            || !permissions
                                .grants
                                .iter()
                                .any(|(candidate, granted)| candidate == capability && *granted)
                    })
            {
                return Err(AppPersistenceError::InvalidInput("invalid plugin grants"));
            }
            if let Some(previous_binding) = permissions.previous_binding {
                validate_plugin_permission_binding(previous_binding)?;
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT plugin_id, name, publisher, signer_fingerprint_sha256,
                        active_version, package_sha256, capabilities_json, state,
                        state_version, installed_at_ms, updated_at_ms
                 FROM plugin_installations WHERE plugin_id = ?1",
                [candidate.plugin_id.as_str()],
                read_plugin_installed,
            )
            .optional()?;
        if existing.as_ref().map(|value| value.state_version) != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let required_state_version = expected_state_version
            .map(next_revision)
            .transpose()?
            .unwrap_or(1);
        if candidate.state_version != WireSequence::new(required_state_version) {
            return Err(AppPersistenceError::Conflict);
        }
        let mut carried_host_scopes = Vec::new();
        if let Some(permissions) = &permissions {
            let previous_signer = existing
                .as_ref()
                .map_or(candidate.signer_fingerprint_sha256.as_str(), |record| {
                    record.signer_fingerprint_sha256.as_str()
                });
            let current_grant_version: Option<i64> = transaction.query_row(
                "SELECT MAX(state_version) FROM plugin_capability_grants
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    candidate.plugin_id.as_str(),
                    previous_signer,
                    u64_to_i64(permissions.protocol_major)?
                ],
                |row| row.get(0),
            )?;
            if current_grant_version.map(|value| WireSequence::new(value as u64))
                != permissions.expected_previous_grant_state_version
            {
                return Err(AppPersistenceError::Conflict);
            }
            let current_scope: Option<PluginPermissionScopeBindingRow> = transaction
                .query_row(
                    "SELECT state_version, artifact_sha256, app_version_major,
                                app_version_minor, secure_surface_contract_revision
                         FROM plugin_host_scope_sets
                         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                           AND major_version = ?3",
                    params![
                        candidate.plugin_id.as_str(),
                        previous_signer,
                        u64_to_i64(permissions.protocol_major)?
                    ],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()?;
            if current_scope
                .as_ref()
                .map(|value| WireSequence::new(value.0 as u64))
                != permissions.expected_previous_scope_state_version
            {
                return Err(AppPersistenceError::Conflict);
            }
            if permissions.approved_host_scopes.is_none()
                && !permissions.carried_host_scope_capabilities.is_empty()
            {
                let previous_binding = permissions
                    .previous_binding
                    .ok_or(AppPersistenceError::Conflict)?;
                if current_scope
                    .as_ref()
                    .and_then(permission_binding_from_scope_row)
                    != Some(previous_binding.clone())
                {
                    return Err(AppPersistenceError::Conflict);
                }
                let mut statement = transaction.prepare(
                    "SELECT host_id, capability FROM plugin_host_scope_grants
                     WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                       AND major_version = ?3 ORDER BY host_id, capability",
                )?;
                carried_host_scopes = statement
                    .query_map(
                        params![
                            candidate.plugin_id.as_str(),
                            previous_signer,
                            u64_to_i64(permissions.protocol_major)?
                        ],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                plugin_capability_from_db(&row.get::<_, String>(1)?)?,
                            ))
                        },
                    )?
                    .filter_map(|row| match row {
                        Ok((host_id, capability))
                            if permissions
                                .carried_host_scope_capabilities
                                .contains(&capability) =>
                        {
                            Some(Ok((host_id, capability)))
                        }
                        Ok(_) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
            }
        }
        if let Some(scopes) = permissions
            .as_ref()
            .and_then(|permissions| permissions.approved_host_scopes)
        {
            let permissions = permissions
                .as_ref()
                .expect("explicit scopes have permission context");
            if scopes.len() > 768 {
                return Err(AppPersistenceError::InvalidInput(
                    "too many plugin Host scope grants",
                ));
            }
            let mut unique = std::collections::BTreeSet::new();
            for (host_id, capability) in scopes {
                if !host_scoped_plugin_capability(*capability)
                    || !unique.insert((host_id.as_str().to_owned(), *capability))
                    || !permissions
                        .grants
                        .iter()
                        .any(|(candidate, granted)| candidate == capability && *granted)
                    || (matches!(
                        capability,
                        PluginCapability::HostMutationPropose
                            | PluginCapability::HostSessionRequest
                    ) && !scopes
                        .contains(&(host_id.clone(), PluginCapability::HostMetadataRead)))
                {
                    return Err(AppPersistenceError::InvalidInput(
                        "invalid plugin Host scope grant",
                    ));
                }
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1)",
                    [host_id.as_str()],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(AppPersistenceError::InvalidInput(
                        "Host scope references an unknown Host",
                    ));
                }
            }
            carried_host_scopes = scopes
                .iter()
                .map(|(host_id, capability)| (host_id.as_str().to_owned(), *capability))
                .collect();
        }
        if let Some(existing) = &existing {
            let expanded = candidate
                .capabilities
                .iter()
                .any(|capability| !existing.capabilities.contains(capability));
            let artifact_identity_changed =
                existing.signer_fingerprint_sha256 != candidate.signer_fingerprint_sha256;
            if !approval_granted && (artifact_identity_changed || expanded) {
                return Err(AppPersistenceError::Conflict);
            }
        }
        let version_existing: Option<(i64, String, String, String, String)> = transaction
            .query_row(
                "SELECT package_size, package_sha256, publisher_key_base64,
                        publisher_signature_base64, capabilities_json
                 FROM plugin_installed_versions WHERE plugin_id = ?1 AND version = ?2",
                params![candidate.plugin_id.as_str(), candidate.active_version],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        if let Some(existing) = version_existing {
            if existing
                != (
                    u64_to_i64(package_size)?,
                    candidate.package_sha256.clone(),
                    publisher_key_base64.to_owned(),
                    publisher_signature_base64.to_owned(),
                    serde_json::to_string(&candidate.capabilities).map_err(|_| {
                        AppPersistenceError::InvalidInput("invalid plugin capabilities")
                    })?,
                )
            {
                return Err(AppPersistenceError::Conflict);
            }
        } else {
            transaction.execute(
                "INSERT INTO plugin_installed_versions
                 (plugin_id, version, package_size, package_sha256, publisher_key_base64,
                  publisher_signature_base64, capabilities_json, installed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    candidate.plugin_id.as_str(),
                    candidate.active_version,
                    u64_to_i64(package_size)?,
                    candidate.package_sha256,
                    publisher_key_base64,
                    publisher_signature_base64,
                    serde_json::to_string(&candidate.capabilities).map_err(|_| {
                        AppPersistenceError::InvalidInput("invalid plugin capabilities")
                    })?,
                    candidate.updated_at_unix_ms,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO plugin_installations
             (plugin_id, name, publisher, signer_fingerprint_sha256, active_version,
              package_sha256, capabilities_json, state, state_version,
              installed_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(plugin_id) DO UPDATE SET
               name = excluded.name, publisher = excluded.publisher,
               signer_fingerprint_sha256 = excluded.signer_fingerprint_sha256,
               active_version = excluded.active_version,
               package_sha256 = excluded.package_sha256,
               capabilities_json = excluded.capabilities_json,
               state = excluded.state, state_version = excluded.state_version,
               updated_at_ms = excluded.updated_at_ms",
            params![
                candidate.plugin_id.as_str(),
                candidate.name,
                candidate.publisher,
                candidate.signer_fingerprint_sha256,
                candidate.active_version,
                candidate.package_sha256,
                serde_json::to_string(&candidate.capabilities).map_err(|_| {
                    AppPersistenceError::InvalidInput("invalid plugin capabilities")
                })?,
                plugin_install_state_to_db(candidate.state),
                u64_to_i64(candidate.state_version.get())?,
                candidate.installed_at_unix_ms,
                candidate.updated_at_unix_ms,
            ],
        )?;
        if let Some(settings) = settings {
            install_plugin_settings(
                &transaction,
                &candidate.plugin_id,
                &candidate.package_sha256,
                settings,
                candidate.updated_at_unix_ms,
            )?;
        }
        if let Some(permissions) = &permissions {
            let next_grant_version = permissions
                .expected_previous_grant_state_version
                .map(next_revision)
                .transpose()?
                .unwrap_or(1);
            transaction.execute(
                "DELETE FROM plugin_capability_grants
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    candidate.plugin_id.as_str(),
                    candidate.signer_fingerprint_sha256,
                    u64_to_i64(permissions.protocol_major)?
                ],
            )?;
            for (capability, granted) in permissions.grants {
                transaction.execute(
                    "INSERT INTO plugin_capability_grants
                     (plugin_id, signer_fingerprint_sha256, major_version, capability,
                      granted, state_version, updated_at_ms, artifact_sha256,
                      app_version_major, app_version_minor, secure_surface_contract_revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        candidate.plugin_id.as_str(),
                        candidate.signer_fingerprint_sha256,
                        u64_to_i64(permissions.protocol_major)?,
                        plugin_capability_to_db(*capability),
                        i64::from(*granted),
                        u64_to_i64(next_grant_version)?,
                        candidate.updated_at_unix_ms,
                        permissions.binding.artifact_sha256,
                        u64_to_i64(permissions.binding.app_version_major)?,
                        u64_to_i64(permissions.binding.app_version_minor)?,
                        u64_to_i64(permissions.binding.secure_surface_contract_revision)?,
                    ],
                )?;
            }
            transaction.execute(
                "DELETE FROM plugin_host_scope_sets
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    candidate.plugin_id.as_str(),
                    candidate.signer_fingerprint_sha256,
                    u64_to_i64(permissions.protocol_major)?
                ],
            )?;
            if !carried_host_scopes.is_empty() {
                let next_scope_version = permissions
                    .expected_previous_scope_state_version
                    .map(next_revision)
                    .transpose()?
                    .unwrap_or(1);
                transaction.execute(
                    "INSERT INTO plugin_host_scope_sets
                     (plugin_id, signer_fingerprint_sha256, major_version, state_version,
                      updated_at_ms, artifact_sha256, app_version_major, app_version_minor,
                      secure_surface_contract_revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        candidate.plugin_id.as_str(),
                        candidate.signer_fingerprint_sha256,
                        u64_to_i64(permissions.protocol_major)?,
                        u64_to_i64(next_scope_version)?,
                        candidate.updated_at_unix_ms,
                        permissions.binding.artifact_sha256,
                        u64_to_i64(permissions.binding.app_version_major)?,
                        u64_to_i64(permissions.binding.app_version_minor)?,
                        u64_to_i64(permissions.binding.secure_surface_contract_revision)?,
                    ],
                )?;
                for (host_id, capability) in &carried_host_scopes {
                    transaction.execute(
                        "INSERT INTO plugin_host_scope_grants
                         (plugin_id, signer_fingerprint_sha256, major_version, host_id,
                          capability, state_version, updated_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            candidate.plugin_id.as_str(),
                            candidate.signer_fingerprint_sha256,
                            u64_to_i64(permissions.protocol_major)?,
                            host_id,
                            plugin_capability_to_db(*capability),
                            u64_to_i64(next_scope_version)?,
                            candidate.updated_at_unix_ms,
                        ],
                    )?;
                }
            }
        }
        transaction.execute(
            "INSERT INTO plugin_audit
             (plugin_id, operation_id, action, outcome, detail_code, created_at_ms)
             VALUES (?1, NULL, ?2, 'committed', ?3, ?4)",
            params![
                candidate.plugin_id.as_str(),
                if existing.is_some() {
                    "lifecycle.update"
                } else {
                    "lifecycle.install"
                },
                if permissions.is_some() {
                    "package_and_grants"
                } else {
                    "package"
                },
                candidate.updated_at_unix_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(candidate.clone())
    }

    pub fn get_plugin_installation(&self, plugin_id: &PluginId) -> Result<PluginInstalledRecord> {
        self.connection
            .query_row(
                "SELECT plugin_id, name, publisher, signer_fingerprint_sha256,
                        active_version, package_sha256, capabilities_json, state,
                        state_version, installed_at_ms, updated_at_ms
                 FROM plugin_installations WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                read_plugin_installed,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    pub fn plugin_installed_version_publisher(
        &self,
        plugin_id: &PluginId,
        version: &str,
    ) -> Result<Option<PluginInstalledVersionPublisher>> {
        self.connection
            .query_row(
                "SELECT publisher_key_base64, publisher_signature_base64
                 FROM plugin_installed_versions WHERE plugin_id = ?1 AND version = ?2",
                params![plugin_id.as_str(), version],
                |row| {
                    Ok(PluginInstalledVersionPublisher {
                        publisher_key_base64: row.get(0)?,
                        publisher_signature_base64: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(AppPersistenceError::from)
    }

    pub fn list_plugin_installations(&self) -> Result<Vec<PluginInstalledRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, name, publisher, signer_fingerprint_sha256,
                    active_version, package_sha256, capabilities_json, state,
                    state_version, installed_at_ms, updated_at_ms
             FROM plugin_installations ORDER BY name, plugin_id",
        )?;
        statement
            .query_map([], read_plugin_installed)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn set_plugin_install_state(
        &mut self,
        plugin_id: &PluginId,
        expected_state_version: WireSequence,
        state: PluginInstallState,
    ) -> Result<PluginInstalledRecord> {
        if !matches!(
            state,
            PluginInstallState::Enabled
                | PluginInstallState::Disabled
                | PluginInstallState::Crashed
                | PluginInstallState::Quarantined
        ) {
            return Err(AppPersistenceError::InvalidInput(
                "invalid direct plugin state transition",
            ));
        }
        let next = next_revision(expected_state_version)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut updated = transaction
            .query_row(
                "SELECT plugin_id, name, publisher, signer_fingerprint_sha256,
                        active_version, package_sha256, capabilities_json, state,
                        state_version, installed_at_ms, updated_at_ms
                 FROM plugin_installations WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                read_plugin_installed,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        if updated.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let updated_at_unix_ms = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE plugin_installations
             SET state = ?1, state_version = ?2, updated_at_ms = ?3
             WHERE plugin_id = ?4 AND state_version = ?5",
            params![
                plugin_install_state_to_db(state),
                u64_to_i64(next)?,
                updated_at_unix_ms,
                plugin_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.execute(
            "INSERT INTO plugin_audit
             (plugin_id, operation_id, action, outcome, detail_code, created_at_ms)
             VALUES (?1, NULL, ?2, 'committed', NULL, ?3)",
            params![
                plugin_id.as_str(),
                match state {
                    PluginInstallState::Enabled => "lifecycle.enable",
                    PluginInstallState::Disabled => "lifecycle.disable",
                    PluginInstallState::Crashed => "lifecycle.crash",
                    PluginInstallState::Quarantined => "lifecycle.quarantine",
                    PluginInstallState::UpdateAvailable | PluginInstallState::Incompatible => {
                        unreachable!("state transition validated above")
                    }
                },
                updated_at_unix_ms,
            ],
        )?;
        transaction.commit()?;
        updated.state = state;
        updated.state_version = WireSequence::new(next);
        updated.updated_at_unix_ms = updated_at_unix_ms;
        Ok(updated)
    }

    /// Commits the database half of a durable uninstall saga. The operation
    /// remains non-terminal so the caller can reconcile filesystem cleanup,
    /// then advance it to `Completed`/`Succeeded` in a separate CAS step.
    pub fn uninstall_plugin(
        &mut self,
        plugin_id: &PluginId,
        expected_plugin_state_version: WireSequence,
        operation_id: &PluginOperationId,
        expected_operation_state_version: WireSequence,
        retain_version_metadata: bool,
    ) -> Result<PluginOperationRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plugin_version: Option<i64> = transaction
            .query_row(
                "SELECT state_version FROM plugin_installations WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        if plugin_version != Some(u64_to_i64(expected_plugin_state_version.get())?) {
            return Err(AppPersistenceError::Conflict);
        }
        let operation = transaction
            .query_row(
                "SELECT operation_id, plugin_id, kind, state, phase, idempotency_key,
                        request_fingerprint_sha256, candidate_version, expected_active_version,
                        error_code, state_version, created_at_ms, updated_at_ms
                 FROM plugin_operations WHERE operation_id = ?1",
                [operation_id.as_str()],
                read_plugin_operation,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        if operation.plugin_id.as_ref() != Some(plugin_id)
            || operation.kind != PluginOperationKind::Uninstall
            || operation.state_version != expected_operation_state_version
            || matches!(
                operation.state,
                PluginOperationState::Succeeded
                    | PluginOperationState::Failed
                    | PluginOperationState::Cancelled
            )
        {
            return Err(AppPersistenceError::Conflict);
        }
        let committed_at_unix_ms = unix_time_ms();
        transaction.execute(
            "DELETE FROM plugin_installations WHERE plugin_id = ?1 AND state_version = ?2",
            params![
                plugin_id.as_str(),
                u64_to_i64(expected_plugin_state_version.get())?
            ],
        )?;
        if !retain_version_metadata {
            transaction.execute(
                "DELETE FROM plugin_installed_versions WHERE plugin_id = ?1",
                [plugin_id.as_str()],
            )?;
            transaction.execute(
                "DELETE FROM plugin_settings WHERE plugin_id = ?1",
                [plugin_id.as_str()],
            )?;
        }
        let next_operation_version = next_revision(expected_operation_state_version)?;
        transaction.execute(
            "UPDATE plugin_operations
             SET state = 'running', phase = 'database_committed', state_version = ?1,
                 updated_at_ms = ?2
             WHERE operation_id = ?3 AND state_version = ?4",
            params![
                u64_to_i64(next_operation_version)?,
                committed_at_unix_ms,
                operation_id.as_str(),
                u64_to_i64(expected_operation_state_version.get())?,
            ],
        )?;
        transaction.execute(
            "INSERT INTO plugin_audit
             (plugin_id, operation_id, action, outcome, detail_code, created_at_ms)
             VALUES (?1, ?2, 'uninstall.database', 'committed', ?3, ?4)",
            params![
                plugin_id.as_str(),
                operation_id.as_str(),
                if retain_version_metadata {
                    "versions_retained"
                } else {
                    "versions_removed"
                },
                committed_at_unix_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(PluginOperationRecord {
            state: PluginOperationState::Running,
            phase: PluginOperationPhase::DatabaseCommitted,
            error_code: None,
            state_version: WireSequence::new(next_operation_version),
            updated_at_unix_ms: committed_at_unix_ms,
            ..operation
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_plugin_capability_grants(
        &mut self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
        expected_plugin_state_version: WireSequence,
        expected_state_version: Option<WireSequence>,
        binding: &PluginPermissionBinding,
        grants: &[(PluginCapability, bool)],
    ) -> Result<Vec<PluginCapabilityGrantRecord>> {
        validate_digest(signer_fingerprint_sha256)?;
        validate_plugin_permission_binding(binding)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (installed_signer, capabilities_json, plugin_state_version): (String, String, i64) =
            transaction
                .query_row(
                    "SELECT signer_fingerprint_sha256, capabilities_json, state_version
                 FROM plugin_installations WHERE plugin_id = ?1",
                    [plugin_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
                .ok_or(AppPersistenceError::NotFound)?;
        let capabilities: Vec<PluginCapability> = serde_json::from_str(&capabilities_json)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?;
        if plugin_state_version != u64_to_i64(expected_plugin_state_version.get())? {
            return Err(AppPersistenceError::Conflict);
        }
        if installed_signer != signer_fingerprint_sha256
            || !complete_plugin_capability_decision(&capabilities, grants)
        {
            return Err(AppPersistenceError::InvalidInput("invalid plugin grants"));
        }
        let current_version: Option<i64> = transaction.query_row(
            "SELECT MAX(state_version) FROM plugin_capability_grants
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?
            ],
            |row| row.get(0),
        )?;
        if current_version.map(|value| WireSequence::new(value as u64)) != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let next_version = expected_state_version.map_or(1, |value| value.get() + 1);
        let previous = transaction
            .prepare(
                "SELECT capability, granted, artifact_sha256, app_version_major,
                        app_version_minor, secure_surface_contract_revision
                 FROM plugin_capability_grants
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND major_version = ?3",
            )?
            .query_map(
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| {
                    Ok((
                        plugin_capability_from_db(&row.get::<_, String>(0)?)?,
                        (
                            row.get::<_, bool>(1)?,
                            permission_binding_from_columns(row, 2)?,
                        ),
                    ))
                },
            )?
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
        transaction.execute(
            "DELETE FROM plugin_capability_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?
            ],
        )?;
        for (capability, granted) in grants {
            let row_binding = if special_plugin_capability(*capability)
                && previous
                    .get(capability)
                    .is_some_and(|(previous_granted, _)| previous_granted == granted)
            {
                previous
                    .get(capability)
                    .and_then(|(_, binding)| binding.as_ref())
            } else {
                Some(binding)
            };
            transaction.execute(
                "INSERT INTO plugin_capability_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, capability,
                  granted, state_version, updated_at_ms, artifact_sha256,
                  app_version_major, app_version_minor, secure_surface_contract_revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?,
                    plugin_capability_to_db(*capability),
                    i64::from(*granted),
                    u64_to_i64(next_version)?,
                    unix_time_ms(),
                    row_binding.map(|binding| binding.artifact_sha256.as_str()),
                    row_binding
                        .map(|binding| u64_to_i64(binding.app_version_major))
                        .transpose()?,
                    row_binding
                        .map(|binding| u64_to_i64(binding.app_version_minor))
                        .transpose()?,
                    row_binding
                        .map(|binding| u64_to_i64(binding.secure_surface_contract_revision))
                        .transpose()?,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO plugin_audit
             (plugin_id, operation_id, action, outcome, detail_code, created_at_ms)
             VALUES (?1, NULL, 'permission.replace', 'committed',
                     'grant_set_replaced', ?2)",
            params![plugin_id.as_str(), unix_time_ms()],
        )?;
        transaction.commit()?;
        Ok(grants
            .iter()
            .map(|(capability, granted)| PluginCapabilityGrantRecord {
                plugin_id: plugin_id.clone(),
                signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
                major_version,
                capability: *capability,
                granted: *granted,
                state_version: WireSequence::new(next_version),
                binding: if special_plugin_capability(*capability)
                    && previous
                        .get(capability)
                        .is_some_and(|(previous_granted, _)| previous_granted == granted)
                {
                    previous
                        .get(capability)
                        .and_then(|(_, binding)| binding.clone())
                } else {
                    Some(binding.clone())
                },
            })
            .collect())
    }

    pub fn list_plugin_capability_grants(
        &self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
    ) -> Result<Vec<PluginCapabilityGrantRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                    granted, state_version, artifact_sha256, app_version_major,
                    app_version_minor, secure_surface_contract_revision
             FROM plugin_capability_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3
             ORDER BY capability",
        )?;
        statement
            .query_map(
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                read_plugin_capability_grant,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn append_plugin_audit(
        &mut self,
        plugin_id: Option<&PluginId>,
        operation_id: Option<&PluginOperationId>,
        action: &str,
        outcome: &str,
        detail_code: Option<&str>,
    ) -> Result<()> {
        if action.is_empty() || action.len() > 80 || outcome.is_empty() || outcome.len() > 40 {
            return Err(AppPersistenceError::InvalidInput(
                "invalid plugin audit metadata",
            ));
        }
        self.connection.execute(
            "INSERT INTO plugin_audit
             (plugin_id, operation_id, action, outcome, detail_code, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                plugin_id.map(PluginId::as_str),
                operation_id.map(PluginOperationId::as_str),
                action,
                outcome,
                detail_code,
                unix_time_ms(),
            ],
        )?;
        Ok(())
    }

    pub fn list_plugin_audit(&self, limit: u16) -> Result<Vec<PluginAuditRecord>> {
        if limit == 0 || limit > 500 {
            return Err(AppPersistenceError::InvalidInput(
                "invalid plugin audit limit",
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT audit_id, plugin_id, operation_id, action, outcome, detail_code, created_at_ms
             FROM plugin_audit ORDER BY audit_id DESC LIMIT ?1",
        )?;
        statement
            .query_map(params![i64::from(limit)], |row| {
                let audit_id = row.get::<_, i64>(0)?;
                Ok(PluginAuditRecord {
                    audit_id: u64::try_from(audit_id)
                        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, audit_id))?,
                    plugin_id: row.get(1)?,
                    operation_id: row.get(2)?,
                    action: row.get(3)?,
                    outcome: row.get(4)?,
                    detail_code: row.get(5)?,
                    created_at_unix_ms: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn get_terminal_workspace_layout(&self) -> Result<TerminalWorkspaceLayoutSnapshot> {
        let (schema_version, revision, layout_json, updated_at_unix_ms) =
            self.connection.query_row(
                "SELECT schema_version, revision, layout_json, updated_at_ms
             FROM terminal_workspace_layout WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )?;
        let layout: TerminalWorkspaceLayout = serde_json::from_str(&layout_json)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?;
        if i64::from(layout.schema_version) != schema_version || layout.validate().is_err() {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        Ok(TerminalWorkspaceLayoutSnapshot {
            revision: WireSequence::new(
                u64::try_from(revision).map_err(|_| AppPersistenceError::InvalidStoredData)?,
            ),
            layout,
            updated_at_unix_ms,
        })
    }

    pub fn replace_terminal_workspace_layout(
        &mut self,
        expected_revision: WireSequence,
        layout: &TerminalWorkspaceLayout,
    ) -> Result<TerminalWorkspaceLayoutSnapshot> {
        layout
            .validate()
            .map_err(AppPersistenceError::InvalidInput)?;
        let layout_json = serde_json::to_string(layout).map_err(|_| {
            AppPersistenceError::InvalidInput("terminal workspace cannot serialize")
        })?;
        if layout_json.len() > MAX_TERMINAL_WORKSPACE_LAYOUT_BYTES {
            return Err(AppPersistenceError::InvalidInput(
                "terminal workspace layout is too large",
            ));
        }
        let next_revision =
            expected_revision
                .get()
                .checked_add(1)
                .ok_or(AppPersistenceError::InvalidInput(
                    "terminal workspace revision is exhausted",
                ))?;
        let expected_revision = i64::try_from(expected_revision.get()).map_err(|_| {
            AppPersistenceError::InvalidInput("terminal workspace revision is invalid")
        })?;
        let next_revision = i64::try_from(next_revision).map_err(|_| {
            AppPersistenceError::InvalidInput("terminal workspace revision is invalid")
        })?;
        let updated_at_unix_ms = unix_time_ms();
        let changed = self.connection.execute(
            "UPDATE terminal_workspace_layout
             SET schema_version = ?1, revision = ?2, layout_json = ?3, updated_at_ms = ?4
             WHERE singleton = 1 AND revision = ?5",
            params![
                i64::from(layout.schema_version),
                next_revision,
                layout_json,
                updated_at_unix_ms,
                expected_revision,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        Ok(TerminalWorkspaceLayoutSnapshot {
            revision: WireSequence::new(next_revision as u64),
            layout: layout.clone(),
            updated_at_unix_ms,
        })
    }

    pub fn create_identity(
        &mut self,
        label: &str,
        username: Option<&str>,
    ) -> Result<IdentitySummary> {
        let label = normalized_label(label)?;
        let username = normalized_optional(username, 128, "username is too long")?;
        let identity_id = IdentityId::new();
        let now = unix_time_ms();
        self.connection.execute(
            "INSERT INTO identities (id, label, username, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4, ?4)",
            params![identity_id.as_str(), label, username, now],
        )?;
        Ok(IdentitySummary {
            identity_id,
            label,
            username,
            state_version: WireSequence::new(1),
        })
    }

    pub fn list_identities(&self) -> Result<Vec<IdentitySummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, label, username, state_version FROM identities ORDER BY label, id",
        )?;
        statement
            .query_map([], read_identity)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    /// Creates the deterministic identity assigned to a resumable SSH sync restore. Replaying
    /// the same ID and normalized fields returns the existing row; any field drift fails closed.
    pub fn create_ssh_sync_identity(
        &mut self,
        identity_id: &IdentityId,
        label: &str,
        username: Option<&str>,
    ) -> Result<IdentitySummary> {
        let label = normalized_label(label)?;
        let username = normalized_optional(username, 128, "username is too long")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT id, label, username, state_version FROM identities WHERE id = ?1",
                [identity_id.as_str()],
                read_identity,
            )
            .optional()?;
        if let Some(existing) = existing {
            return if existing.label == label && existing.username == username {
                Ok(existing)
            } else {
                Err(AppPersistenceError::IdempotencyConflict)
            };
        }
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO identities (id, label, username, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4, ?4)",
            params![identity_id.as_str(), label, username, now],
        )?;
        transaction.commit()?;
        Ok(IdentitySummary {
            identity_id: identity_id.clone(),
            label,
            username,
            state_version: WireSequence::new(1),
        })
    }

    pub fn get_identity(&self, identity_id: &IdentityId) -> Result<IdentitySummary> {
        self.connection
            .query_row(
                "SELECT id, label, username, state_version FROM identities WHERE id = ?1",
                [identity_id.as_str()],
                read_identity,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    pub fn update_identity(
        &mut self,
        identity_id: &IdentityId,
        expected_state_version: WireSequence,
        label: &str,
        username: Option<&str>,
    ) -> Result<IdentitySummary> {
        let label = normalized_label(label)?;
        let username = normalized_optional(username, 128, "username is too long")?;
        let next_version = next_revision(expected_state_version)?;
        let changed = self.connection.execute(
            "UPDATE identities
             SET label = ?1, username = ?2, state_version = ?3, updated_at_ms = ?4
             WHERE id = ?5 AND state_version = ?6",
            params![
                label,
                username,
                u64_to_i64(next_version)?,
                unix_time_ms(),
                identity_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(if self.identity_exists(identity_id)? {
                AppPersistenceError::Conflict
            } else {
                AppPersistenceError::NotFound
            });
        }
        self.get_identity(identity_id)
    }

    pub fn identity_delete_impact(&self, identity_id: &IdentityId) -> Result<IdentityDeleteImpact> {
        let transaction = self.connection.unchecked_transaction()?;
        let identity = transaction
            .query_row(
                "SELECT id, label, username, state_version FROM identities WHERE id = ?1",
                [identity_id.as_str()],
                read_identity,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let mut host_statement = transaction.prepare(
            "SELECT id, label, address, normalized_address, port, username, identity_id,
                    favorite, state_version
             FROM hosts WHERE identity_id = ?1 ORDER BY label, id",
        )?;
        let referencing_hosts = host_statement
            .query_map([identity_id.as_str()], read_host)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(host_statement);
        let referencing_credential_refs =
            list_credential_ref_summaries(&transaction, identity_id, false)?;
        transaction.commit()?;
        Ok(IdentityDeleteImpact {
            identity,
            referencing_host_count: usize_to_u32(referencing_hosts.len())?,
            referencing_hosts,
            referencing_credential_ref_count: usize_to_u32(referencing_credential_refs.len())?,
            referencing_credential_refs,
        })
    }

    pub fn delete_identity(
        &mut self,
        identity_id: &IdentityId,
        expected_state_version: WireSequence,
    ) -> Result<IdentityDeleteResponse> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_version = transaction
            .query_row(
                "SELECT state_version FROM identities WHERE id = ?1",
                [identity_id.as_str()],
                |row| read_wire_sequence(row, 0),
            )
            .optional()?;
        let Some(current_version) = current_version else {
            transaction.commit()?;
            return Ok(IdentityDeleteResponse {
                identity_id: identity_id.clone(),
                deleted: false,
            });
        };
        if current_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let reference_count: i64 = transaction.query_row(
            "SELECT
               (SELECT COUNT(*) FROM hosts WHERE identity_id = ?1) +
               (SELECT COUNT(*) FROM credential_refs WHERE identity_id = ?1)",
            [identity_id.as_str()],
            |row| row.get(0),
        )?;
        if reference_count != 0 {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = transaction.execute(
            "DELETE FROM identities WHERE id = ?1 AND state_version = ?2",
            params![
                identity_id.as_str(),
                u64_to_i64(expected_state_version.get())?
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(IdentityDeleteResponse {
            identity_id: identity_id.clone(),
            deleted: true,
        })
    }

    /// Persists stable, Core-minted references before the Vault is changed.
    ///
    /// A replay with the same operation identity and non-secret request fields returns the
    /// original references. Secret bytes are deliberately not hashed into SQLite; the Vault
    /// write must enforce that an existing reference cannot be rebound to different material.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_credential_import(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
        identity_id: &IdentityId,
        kind: CredentialKind,
        priority: u32,
        label: &str,
        has_passphrase: bool,
    ) -> Result<CredentialRecord> {
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let label = normalized_label(label)?;
        if matches!(kind, CredentialKind::Password) && has_passphrase {
            return Err(AppPersistenceError::InvalidInput(
                "password credentials cannot contain a passphrase",
            ));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = lookup_credential_import(&transaction, operation_id, &idempotency_key)?;
        if !existing.is_empty() {
            if existing.len() != 1
                || !same_credential_import(
                    &existing[0],
                    operation_id,
                    &idempotency_key,
                    identity_id,
                    kind,
                    priority,
                    &label,
                    has_passphrase,
                )
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            return Ok(existing.into_iter().next().expect("length checked"));
        }

        let identity_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM identities WHERE id = ?1)",
            [identity_id.as_str()],
            |row| row.get(0),
        )?;
        if !identity_exists {
            return Err(AppPersistenceError::NotFound);
        }
        let priority_in_use: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM credential_refs WHERE identity_id = ?1 AND priority = ?2
             )",
            params![identity_id.as_str(), i64::from(priority)],
            |row| row.get(0),
        )?;
        if priority_in_use {
            return Err(AppPersistenceError::Conflict);
        }

        let credential_ref_id = CredentialRefId::new();
        let secret_ref_id = SecretRefId::new();
        let passphrase_secret_ref_id = has_passphrase.then(SecretRefId::new);
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO credential_refs
             (id, identity_id, kind, priority, label, state_version, created_at_ms,
              updated_at_ms, import_operation_id, import_idempotency_key, import_state)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6, ?7, ?8, 'pending')",
            params![
                credential_ref_id.as_str(),
                identity_id.as_str(),
                credential_kind_to_db(kind),
                i64::from(priority),
                label,
                now,
                operation_id.as_str(),
                idempotency_key,
            ],
        )?;
        match kind {
            CredentialKind::Password => {
                transaction.execute(
                    "INSERT INTO credential_password_details (credential_ref_id) VALUES (?1)",
                    [credential_ref_id.as_str()],
                )?;
                transaction.execute(
                    "INSERT INTO credential_secret_slots
                     (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
                     VALUES (?1, 0, 'password', ?2, 'Password')",
                    params![credential_ref_id.as_str(), secret_ref_id.as_str()],
                )?;
            }
            CredentialKind::PrivateKey => {
                transaction.execute(
                    "INSERT INTO credential_private_key_details
                     (credential_ref_id, public_key_algorithm, public_key_fingerprint)
                     VALUES (?1, NULL, NULL)",
                    [credential_ref_id.as_str()],
                )?;
                transaction.execute(
                    "INSERT INTO credential_secret_slots
                     (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
                     VALUES (?1, 0, 'private_key', ?2, 'Private key')",
                    params![credential_ref_id.as_str(), secret_ref_id.as_str()],
                )?;
                if let Some(passphrase_secret_ref_id) = &passphrase_secret_ref_id {
                    transaction.execute(
                        "INSERT INTO credential_secret_slots
                         (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
                         VALUES (?1, 1, 'passphrase', ?2, 'Passphrase')",
                        params![
                            credential_ref_id.as_str(),
                            passphrase_secret_ref_id.as_str()
                        ],
                    )?;
                }
            }
        }
        transaction.commit()?;
        let details = match kind {
            CredentialKind::Password => CredentialRecordDetails::Password { secret_ref_id },
            CredentialKind::PrivateKey => CredentialRecordDetails::PrivateKey {
                secret_ref_id,
                passphrase_secret_ref_id,
                public_key_algorithm: None,
                public_key_fingerprint: None,
            },
        };
        Ok(CredentialRecord {
            credential_ref_id,
            identity_id: identity_id.clone(),
            method: authentication_method_from_import_kind(kind),
            details,
            priority,
            label,
            state_version: WireSequence::new(1),
            import_operation_id: Some(operation_id.clone()),
            import_idempotency_key: Some(idempotency_key),
            import_state: CredentialImportState::Pending,
        })
    }

    /// Marks a Vault-backed import usable. Private-key metadata must be derived by trusted Core
    /// code from the imported key, never copied from a UI request. A matching ready row is returned
    /// on replay so callers can reconcile a commit whose SQLite outcome was previously uncertain.
    pub fn mark_credential_import_ready(
        &mut self,
        credential_ref_id: &CredentialRefId,
        operation_id: &OperationId,
        expected_state_version: WireSequence,
        public_key_algorithm: Option<&str>,
        public_key_fingerprint: Option<&str>,
    ) -> Result<CredentialRecord> {
        let (public_key_algorithm, public_key_fingerprint) =
            normalized_public_key_metadata(public_key_algorithm, public_key_fingerprint)?;
        let existing = self.get_credential_import_record(credential_ref_id)?;
        if existing.import_operation_id.as_ref() != Some(operation_id) {
            return Err(AppPersistenceError::Conflict);
        }
        validate_public_key_metadata(
            credential_import_kind(&existing)?,
            &public_key_algorithm,
            &public_key_fingerprint,
        )?;

        if existing.import_state == CredentialImportState::Ready {
            let (existing_algorithm, existing_fingerprint) = private_key_public_metadata(&existing);
            return if existing_algorithm == public_key_algorithm.as_deref()
                && existing_fingerprint == public_key_fingerprint.as_deref()
            {
                Ok(existing)
            } else {
                Err(AppPersistenceError::Conflict)
            };
        }
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }

        let next_version = expected_state_version
            .get()
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if existing.method == AuthenticationMethodKind::PrivateKey {
            transaction.execute(
                "UPDATE credential_private_key_details
                 SET public_key_algorithm = ?1, public_key_fingerprint = ?2
                 WHERE credential_ref_id = ?3",
                params![
                    public_key_algorithm,
                    public_key_fingerprint,
                    credential_ref_id.as_str(),
                ],
            )?;
        }
        let changed = transaction.execute(
            "UPDATE credential_refs
             SET import_state = 'ready', state_version = ?1, updated_at_ms = ?2
             WHERE id = ?3 AND import_operation_id = ?4 AND import_state = 'pending'
                   AND state_version = ?5",
            params![
                u64_to_i64(next_version)?,
                unix_time_ms(),
                credential_ref_id.as_str(),
                operation_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            drop(transaction);
            let reconciled = self.get_credential_import_record(credential_ref_id)?;
            let (reconciled_algorithm, reconciled_fingerprint) =
                private_key_public_metadata(&reconciled);
            return if reconciled.import_operation_id.as_ref() == Some(operation_id)
                && reconciled.import_state == CredentialImportState::Ready
                && reconciled_algorithm == public_key_algorithm.as_deref()
                && reconciled_fingerprint == public_key_fingerprint.as_deref()
            {
                Ok(reconciled)
            } else {
                Err(AppPersistenceError::Conflict)
            };
        }
        transaction.commit()?;
        self.get_credential_import_record(credential_ref_id)
    }

    /// Creates a ready, non-secret reference to one explicitly selected Agent public key.
    /// The Agent endpoint and key comment are intentionally absent from persistence.
    #[allow(clippy::too_many_arguments)]
    pub fn create_ssh_agent_credential(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
        identity_id: &IdentityId,
        priority: u32,
        label: &str,
        public_key_blob: &[u8],
        public_key_algorithm: &str,
        public_key_fingerprint: &str,
    ) -> Result<CredentialRecord> {
        self.create_agent_identity_credential(
            operation_id,
            idempotency_key,
            identity_id,
            priority,
            label,
            AgentIdentityKind::Ordinary,
            public_key_blob,
            public_key_algorithm,
            public_key_fingerprint,
            None,
            None,
        )
    }

    /// Creates a ready, non-secret reference to one exact identity selected from the system SSH
    /// Agent. Certificate metadata is parsed by trusted Core code before this boundary. Unknown
    /// critical options are deliberately retained so the authentication layer can fail closed.
    #[allow(clippy::too_many_arguments)]
    pub fn create_agent_identity_credential(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
        identity_id: &IdentityId,
        priority: u32,
        label: &str,
        identity_kind: AgentIdentityKind,
        public_key_blob: &[u8],
        public_key_algorithm: &str,
        public_key_fingerprint: &str,
        hardware_application: Option<&str>,
        certificate: Option<&SshCertificateMetadata>,
    ) -> Result<CredentialRecord> {
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let label = normalized_label(label)?;
        let expected_details = validated_agent_identity_details(
            identity_kind,
            public_key_blob,
            public_key_algorithm,
            public_key_fingerprint,
            hardware_application,
            certificate,
        )?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                &format!(
                    "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
                     {CREDENTIAL_RECORD_JOINS}
                     WHERE credential_refs.import_operation_id = ?1
                        OR credential_refs.import_idempotency_key = ?2"
                ),
                params![operation_id.as_str(), idempotency_key],
                read_credential_record,
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.import_operation_id.as_ref() == Some(operation_id)
                && existing.import_idempotency_key.as_deref() == Some(&idempotency_key)
                && existing.identity_id == *identity_id
                && existing.priority == priority
                && existing.label == label
                && existing.details == expected_details
                && existing.import_state == CredentialImportState::Ready
            {
                return Ok(existing);
            }
            return Err(AppPersistenceError::IdempotencyConflict);
        }

        let identity_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM identities WHERE id = ?1)",
            [identity_id.as_str()],
            |row| row.get(0),
        )?;
        if !identity_exists {
            return Err(AppPersistenceError::NotFound);
        }
        let priority_in_use: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM credential_refs WHERE identity_id = ?1 AND priority = ?2
             )",
            params![identity_id.as_str(), i64::from(priority)],
            |row| row.get(0),
        )?;
        if priority_in_use {
            return Err(AppPersistenceError::Conflict);
        }

        let credential_ref_id = CredentialRefId::new();
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO credential_refs
             (id, identity_id, kind, priority, label, state_version, created_at_ms,
              updated_at_ms, import_operation_id, import_idempotency_key, import_state)
             VALUES (?1, ?2, 'ssh_agent', ?3, ?4, 1, ?5, ?5, ?6, ?7, 'ready')",
            params![
                credential_ref_id.as_str(),
                identity_id.as_str(),
                i64::from(priority),
                label,
                now,
                operation_id.as_str(),
                idempotency_key,
            ],
        )?;
        transaction.execute(
            "INSERT INTO credential_ssh_agent_details
             (credential_ref_id, agent_scope, public_key_blob, public_key_algorithm,
              public_key_fingerprint, identity_kind, certificate_blob, certificate_algorithm,
              hardware_application, certificate_fingerprint,
              certificate_ca_public_key_fingerprint, certificate_serial, certificate_key_id,
              certificate_valid_principals_json, certificate_type,
              certificate_valid_after_unix_seconds, certificate_valid_before_unix_seconds,
              certificate_critical_options_json, certificate_extensions_json)
             VALUES (?1, 'default_environment', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                credential_ref_id.as_str(),
                public_key_blob,
                public_key_algorithm,
                public_key_fingerprint,
                agent_identity_kind_to_db(identity_kind),
                certificate.map(|metadata| metadata.certificate_blob.as_slice()),
                certificate.map(|metadata| metadata.certificate_algorithm.as_str()),
                hardware_application,
                certificate.map(|metadata| metadata.certificate_fingerprint.as_str()),
                certificate.map(|metadata| metadata.ca_public_key_fingerprint.as_str()),
                certificate.map(|metadata| metadata.serial.as_str()),
                certificate.map(|metadata| metadata.key_id.as_str()),
                certificate
                    .map(|metadata| serde_json::to_string(&metadata.valid_principals))
                    .transpose()
                    .map_err(|_| {
                        AppPersistenceError::InvalidInput(
                            "SSH certificate principals cannot be serialized",
                        )
                    })?,
                certificate.map(|_| "user"),
                certificate.map(|metadata| metadata.valid_after_unix_seconds),
                certificate.and_then(|metadata| metadata.valid_before_unix_seconds),
                certificate
                    .map(|metadata| serde_json::to_string(&metadata.critical_options))
                    .transpose()
                    .map_err(|_| {
                        AppPersistenceError::InvalidInput(
                            "SSH certificate critical options cannot be serialized",
                        )
                    })?,
                certificate
                    .map(|metadata| serde_json::to_string(&metadata.extensions))
                    .transpose()
                    .map_err(|_| {
                        AppPersistenceError::InvalidInput(
                            "SSH certificate extensions cannot be serialized",
                        )
                    })?,
            ],
        )?;
        transaction.commit()?;
        self.get_ready_credential_record(&credential_ref_id)
    }

    /// Creates a ready, non-secret keyboard-interactive policy. Prompt answers
    /// are intentionally absent from SQLite and are supplied through the
    /// session actor's one-time answer references.
    pub fn create_keyboard_interactive_credential(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
        identity_id: &IdentityId,
        priority: u32,
        label: &str,
        max_rounds: u8,
    ) -> Result<CredentialRecord> {
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let label = normalized_label(label)?;
        if !(1..=32).contains(&max_rounds) {
            return Err(AppPersistenceError::InvalidInput(
                "keyboard-interactive max rounds must be between 1 and 32",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                &format!(
                    "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
                     {CREDENTIAL_RECORD_JOINS}
                     WHERE credential_refs.import_operation_id = ?1
                        OR credential_refs.import_idempotency_key = ?2"
                ),
                params![operation_id.as_str(), idempotency_key],
                read_credential_record,
            )
            .optional()?;
        if let Some(existing) = existing {
            let same_policy = matches!(
                existing.details,
                CredentialRecordDetails::KeyboardInteractive {
                    max_rounds: existing_max_rounds,
                } if existing_max_rounds == max_rounds
            );
            if existing.import_operation_id.as_ref() == Some(operation_id)
                && existing.import_idempotency_key.as_deref() == Some(&idempotency_key)
                && existing.identity_id == *identity_id
                && existing.priority == priority
                && existing.label == label
                && same_policy
                && existing.import_state == CredentialImportState::Ready
            {
                return Ok(existing);
            }
            return Err(AppPersistenceError::IdempotencyConflict);
        }
        let identity_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM identities WHERE id = ?1)",
            [identity_id.as_str()],
            |row| row.get(0),
        )?;
        if !identity_exists {
            return Err(AppPersistenceError::NotFound);
        }
        let priority_in_use: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM credential_refs WHERE identity_id = ?1 AND priority = ?2
             )",
            params![identity_id.as_str(), i64::from(priority)],
            |row| row.get(0),
        )?;
        if priority_in_use {
            return Err(AppPersistenceError::Conflict);
        }
        let credential_ref_id = CredentialRefId::new();
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO credential_refs
             (id, identity_id, kind, priority, label, state_version, created_at_ms,
              updated_at_ms, import_operation_id, import_idempotency_key, import_state)
             VALUES (?1, ?2, 'keyboard_interactive', ?3, ?4, 1, ?5, ?5, ?6, ?7, 'ready')",
            params![
                credential_ref_id.as_str(),
                identity_id.as_str(),
                i64::from(priority),
                label,
                now,
                operation_id.as_str(),
                idempotency_key,
            ],
        )?;
        transaction.execute(
            "INSERT INTO credential_keyboard_interactive_details
             (credential_ref_id, max_rounds) VALUES (?1, ?2)",
            params![credential_ref_id.as_str(), i64::from(max_rounds)],
        )?;
        transaction.commit()?;
        self.get_ready_credential_record(&credential_ref_id)
    }

    pub fn list_credential_refs(
        &self,
        identity_id: &IdentityId,
    ) -> Result<Vec<CredentialRefSummary>> {
        list_credential_ref_summaries(&self.connection, identity_id, true)
    }

    /// Reads any saga state for reconciliation; pending records must not be used to authenticate.
    pub fn get_credential_import_record(
        &self,
        credential_ref_id: &CredentialRefId,
    ) -> Result<CredentialRecord> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
                     {CREDENTIAL_RECORD_JOINS}
                     WHERE credential_refs.id = ?1
                       AND credential_refs.kind IN
                         ('password', 'private_key', 'keyboard_interactive', 'ssh_agent')"
                ),
                [credential_ref_id.as_str()],
                read_credential_record,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    /// Reads a credential only after both Vault and SQLite stages completed.
    pub fn get_ready_credential_record(
        &self,
        credential_ref_id: &CredentialRefId,
    ) -> Result<CredentialRecord> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
                     {CREDENTIAL_RECORD_JOINS}
                     WHERE credential_refs.id = ?1 AND credential_refs.import_state = 'ready'
                       AND credential_refs.kind IN
                         ('password', 'private_key', 'keyboard_interactive', 'ssh_agent')"
                ),
                [credential_ref_id.as_str()],
                read_credential_record,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    /// Resolves the ready credentials available to one Host through its Identity.
    pub fn list_ready_credential_records_for_host(
        &self,
        host_id: &HostId,
    ) -> Result<Vec<CredentialRecord>> {
        let host = self.get_host(host_id)?;
        let Some(identity_id) = host.identity_id else {
            return Ok(Vec::new());
        };
        let mut statement = self.connection.prepare(&format!(
            "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
             {CREDENTIAL_RECORD_JOINS}
             WHERE credential_refs.identity_id = ?1 AND credential_refs.import_state = 'ready'
               AND credential_refs.kind IN
                 ('password', 'private_key', 'keyboard_interactive', 'ssh_agent')
             ORDER BY credential_refs.priority, credential_refs.id"
        ))?;
        statement
            .query_map([identity_id.as_str()], read_credential_record)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_host(
        &mut self,
        label: &str,
        address: &str,
        port: u16,
        username: Option<&str>,
        identity_id: Option<&IdentityId>,
        favorite: bool,
    ) -> Result<HostSummary> {
        let endpoint = Endpoint::parse(address, port)?;
        let label = if label.trim().is_empty() {
            endpoint.normalized_address().to_owned()
        } else {
            normalized_label(label)?
        };
        let username = normalized_optional(username, 128, "username is too long")?;
        let host_id = HostId::new();
        let now = unix_time_ms();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO hosts
             (id, label, address, normalized_address, port, username, identity_id, favorite,
              state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
            params![
                host_id.as_str(),
                label,
                address.trim(),
                endpoint.normalized_address(),
                i64::from(port),
                username,
                identity_id.map(IdentityId::as_str),
                favorite,
                now,
            ],
        )?;
        insert_host_config_defaults(&transaction, &host_id, now)?;
        transaction.commit()?;
        Ok(HostSummary {
            host_id,
            label,
            address: address.trim().to_owned(),
            normalized_address: endpoint.normalized_address().to_owned(),
            port,
            username,
            identity_id: identity_id.cloned(),
            favorite,
            has_ready_credential: false,
            state_version: WireSequence::new(1),
        })
    }

    /// Creates a Host and every non-secret configuration projection in one SQLite transaction.
    /// The durable operation row is committed with the Host, so a lost response can be replayed
    /// exactly while operation/idempotency reuse with changed normalized input fails closed.
    pub fn create_host_configured(
        &mut self,
        request: &HostConfiguredCreateRequest,
    ) -> Result<HostConfiguredCreateResponse> {
        let idempotency_key = validated_idempotency_key(&request.idempotency_key)?;
        let normalized = normalized_configured_host_create(request)?;
        let request_json = serde_json::to_string(&normalized).map_err(|_| {
            AppPersistenceError::InvalidInput("configured host request cannot be serialized")
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing =
            lookup_configured_host_create(&transaction, &request.operation_id, &idempotency_key)?;
        if !existing.is_empty() {
            if existing.len() != 1
                || existing[0].0 != request.operation_id
                || existing[0].1 != idempotency_key
                || existing[0].2 != request_json
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            let host_id = existing[0].3.clone();
            transaction.commit()?;
            return self.configured_host_create_response(&host_id);
        }

        if let Some(identity_id) = &normalized.identity_id {
            require_metadata_exists(&transaction, "identities", identity_id.as_str())?;
        }
        if let Some(group_id) = &normalized.group_id {
            require_metadata_exists(&transaction, "host_groups", group_id.as_str())?;
        }
        for tag_id in &normalized.tag_ids {
            require_metadata_exists(&transaction, "host_tags", tag_id.as_str())?;
        }

        let host_id = HostId::new();
        let now = unix_time_ms();
        let staged_password = normalized
            .staged_password_id
            .as_ref()
            .map(|staged_password_id| {
                require_staged_host_create_password(
                    &transaction,
                    staged_password_id,
                    &request.operation_id,
                    &idempotency_key,
                    now,
                )
            })
            .transpose()?;
        let dedicated_identity_id = staged_password.as_ref().map(|_| IdentityId::new());
        let effective_identity_id = dedicated_identity_id
            .as_ref()
            .or(normalized.identity_id.as_ref());
        if let (Some(staged), Some(identity_id)) =
            (staged_password.as_ref(), dedicated_identity_id.as_ref())
        {
            transaction.execute(
                "INSERT INTO identities
                 (id, label, username, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                params![
                    identity_id.as_str(),
                    staged.identity_label,
                    normalized.username,
                    now,
                ],
            )?;
            insert_ready_password_credential(
                &transaction,
                identity_id,
                &staged.secret_ref_id,
                &staged.credential_label,
                now,
            )?;
        }
        transaction.execute(
            "INSERT INTO hosts
             (id, label, address, normalized_address, port, username, identity_id, favorite,
              state_version, created_at_ms, updated_at_ms, group_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?10)",
            params![
                host_id.as_str(),
                normalized.label,
                normalized.address,
                normalized.normalized_address,
                i64::from(normalized.port),
                normalized.username,
                effective_identity_id.map(IdentityId::as_str),
                normalized.favorite,
                now,
                normalized.group_id.as_ref().map(HostGroupId::as_str),
            ],
        )?;
        for tag_id in &normalized.tag_ids {
            transaction.execute(
                "INSERT INTO host_tag_assignments (host_id, tag_id) VALUES (?1, ?2)",
                params![host_id.as_str(), tag_id.as_str()],
            )?;
        }
        insert_host_config_defaults(&transaction, &host_id, now)?;
        initialize_configured_route_plan(
            &transaction,
            &host_id,
            &normalized.ingress,
            &normalized.jump_host_ids,
            now,
        )?;
        initialize_configured_authentication_plan(
            &transaction,
            &host_id,
            effective_identity_id,
            normalized.authentication_mode,
            &normalized.credential_ref_ids,
            now,
        )?;
        initialize_configured_algorithm_policy(
            &transaction,
            &host_id,
            &normalized.algorithm_policy_id,
            &normalized.compatibility_exceptions,
            now,
        )?;
        initialize_configured_heartbeat_policy(
            &transaction,
            &host_id,
            &normalized.heartbeat_policy,
            now,
        )?;
        initialize_configured_monitoring_policy(
            &transaction,
            &host_id,
            &normalized.monitoring_policy,
            now,
        )?;
        initialize_configured_login_automation(
            &transaction,
            &host_id,
            normalized.login_automation_enabled,
            normalized.login_automation_confirmed,
            &normalized.login_automation_steps,
            now,
        )?;
        if let Some(staged) = &staged_password {
            let changed = transaction.execute(
                "UPDATE host_create_password_stages
                 SET state = 'consumed', updated_at_ms = ?1
                 WHERE stage_id = ?2 AND state = 'staged'",
                params![now, staged.staged_password_id.as_str()],
            )?;
            if changed != 1 {
                return Err(AppPersistenceError::RequiresReload);
            }
        }
        transaction.execute(
            "INSERT INTO host_configured_create_operations
             (operation_id, idempotency_key, request_json, host_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                request.operation_id.as_str(),
                idempotency_key,
                request_json,
                host_id.as_str(),
                now,
            ],
        )?;
        transaction.commit()?;
        self.configured_host_create_response(&host_id)
    }

    fn configured_host_create_response(
        &self,
        host_id: &HostId,
    ) -> Result<HostConfiguredCreateResponse> {
        Ok(HostConfiguredCreateResponse {
            host: self.get_host(host_id)?,
            organization: self.get_host_organization(host_id)?,
            connection_config: self.get_host_connection_config(host_id)?,
        })
    }

    /// Creates a bounded set of non-secret Hosts in one transaction. Callers must preview and
    /// explicitly select every entry before invoking this method.
    pub fn create_hosts_atomically(
        &mut self,
        inputs: &[HostBatchCreateInput],
    ) -> Result<Vec<HostSummary>> {
        if inputs.is_empty() || inputs.len() > 128 {
            return Err(AppPersistenceError::InvalidInput(
                "host import batch is empty or too large",
            ));
        }
        let normalized = inputs
            .iter()
            .map(normalized_host_batch_create_input)
            .collect::<Result<Vec<_>>>()?;
        validate_batch_jump_hosts(&normalized)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_time_ms();
        let mut target_hosts = Vec::with_capacity(normalized.len());
        for input in &normalized {
            target_hosts.push(insert_import_host(
                &transaction,
                &input.label,
                &input.address,
                &input.normalized_address,
                input.port,
                input.username.as_deref(),
                now,
            )?);
        }

        let mut selected_targets =
            BTreeMap::<(String, u16, Option<String>), Option<HostSummary>>::new();
        let mut selected_endpoint_usernames =
            BTreeMap::<(String, u16), BTreeSet<Option<String>>>::new();
        for (input, target) in normalized.iter().zip(&target_hosts) {
            let key = (
                input.normalized_address.clone(),
                input.port,
                input.username.clone(),
            );
            selected_targets
                .entry(key)
                .and_modify(|candidate| *candidate = None)
                .or_insert_with(|| Some(target.clone()));
            selected_endpoint_usernames
                .entry((input.normalized_address.clone(), input.port))
                .or_default()
                .insert(input.username.clone());
        }

        let mut jump_hosts = BTreeMap::<(String, u16), HostSummary>::new();
        let mut generated_jump_hosts = Vec::new();
        for input in &normalized {
            for jump in &input.jump_hosts {
                let key = (jump.normalized_address.clone(), jump.port);
                if jump_hosts.contains_key(&key) {
                    continue;
                }
                let target_key = (
                    jump.normalized_address.clone(),
                    jump.port,
                    jump.username.clone(),
                );
                if selected_endpoint_usernames
                    .get(&key)
                    .is_some_and(|usernames| {
                        usernames.len() != 1 || !usernames.contains(&jump.username)
                    })
                {
                    return Err(AppPersistenceError::InvalidInput(
                        "selected target conflicts with jump endpoint username",
                    ));
                }
                let host = match selected_targets.get(&target_key) {
                    Some(Some(target)) => target.clone(),
                    Some(None) => {
                        return Err(AppPersistenceError::InvalidInput(
                            "jump endpoint matches multiple selected targets",
                        ));
                    }
                    None => {
                        let generated = insert_import_host(
                            &transaction,
                            "",
                            &jump.address,
                            &jump.normalized_address,
                            jump.port,
                            jump.username.as_deref(),
                            now,
                        )?;
                        generated_jump_hosts.push(generated.clone());
                        generated
                    }
                };
                jump_hosts.insert(key, host);
            }
        }

        for (input, target) in normalized.iter().zip(&target_hosts) {
            let jump_host_ids = input
                .jump_hosts
                .iter()
                .map(|jump| {
                    jump_hosts
                        .get(&(jump.normalized_address.clone(), jump.port))
                        .map(|host| host.host_id.clone())
                        .ok_or(AppPersistenceError::InvalidStoredData)
                })
                .collect::<Result<Vec<_>>>()?;
            initialize_import_route_plan(
                &transaction,
                &target.host_id,
                &input.ingress,
                &jump_host_ids,
                now,
            )?;
        }
        transaction.commit()?;

        let mut hosts = target_hosts;
        hosts.extend(generated_jump_hosts);
        Ok(hosts)
    }

    pub fn list_hosts(&self) -> Result<Vec<HostSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, label, address, normalized_address, port, username, identity_id,
                    favorite, state_version
             FROM hosts ORDER BY favorite DESC, label, id",
        )?;
        statement
            .query_map([], read_host)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn get_host(&self, host_id: &HostId) -> Result<HostSummary> {
        self.connection
            .query_row(
                "SELECT id, label, address, normalized_address, port, username, identity_id,
                        favorite, state_version
                 FROM hosts WHERE id = ?1",
                [host_id.as_str()],
                read_host,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_host(
        &mut self,
        host_id: &HostId,
        expected_state_version: WireSequence,
        label: &str,
        address: &str,
        port: u16,
        username: Option<&str>,
        identity_id: Option<&IdentityId>,
        favorite: bool,
    ) -> Result<HostSummary> {
        let endpoint = Endpoint::parse(address, port)?;
        let label = normalized_label(label)?;
        let username = normalized_optional(username, 128, "username is too long")?;
        let next_version = expected_state_version
            .get()
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = transaction
            .query_row(
                "SELECT state_version, identity_id FROM hosts WHERE id = ?1",
                [host_id.as_str()],
                |row| {
                    Ok((
                        read_wire_sequence(row, 0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        let Some((current_state_version, current_identity_id)) = current else {
            return Err(AppPersistenceError::NotFound);
        };
        if current_state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let next_identity_id = identity_id.map(IdentityId::as_str);
        if current_identity_id.as_deref() != next_identity_id {
            let authentication_mode: String = transaction.query_row(
                "SELECT mode FROM host_authentication_plans WHERE host_id = ?1",
                [host_id.as_str()],
                |row| row.get(0),
            )?;
            if authentication_mode == "host_override" {
                return Err(AppPersistenceError::InvalidInput(
                    "switch the authentication plan to identity before changing the host identity",
                ));
            }
        }
        let changed = transaction.execute(
            "UPDATE hosts SET label = ?1, address = ?2, normalized_address = ?3, port = ?4,
                    username = ?5, identity_id = ?6, favorite = ?7, state_version = ?8,
                    updated_at_ms = ?9
             WHERE id = ?10 AND state_version = ?11",
            params![
                label,
                address.trim(),
                endpoint.normalized_address(),
                i64::from(port),
                username,
                identity_id.map(IdentityId::as_str),
                favorite,
                u64_to_i64(next_version)?,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(HostSummary {
            host_id: host_id.clone(),
            label,
            address: address.trim().to_owned(),
            normalized_address: endpoint.normalized_address().to_owned(),
            port,
            username,
            identity_id: identity_id.cloned(),
            favorite,
            has_ready_credential: false,
            state_version: WireSequence::new(next_version),
        })
    }

    pub fn delete_host(
        &mut self,
        host_id: &HostId,
        expected_state_version: WireSequence,
    ) -> Result<()> {
        let changed = self.connection.execute(
            "DELETE FROM hosts WHERE id = ?1 AND state_version = ?2",
            params![host_id.as_str(), u64_to_i64(expected_state_version.get())?],
        )?;
        if changed == 1 {
            return Ok(());
        }
        Err(if self.host_exists(host_id)? {
            AppPersistenceError::Conflict
        } else {
            AppPersistenceError::NotFound
        })
    }

    pub fn create_host_group(&mut self, label: &str) -> Result<HostGroupSummary> {
        let label = normalized_label(label)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if metadata_label_exists(&transaction, "host_groups", &label, None)? {
            return Err(AppPersistenceError::Conflict);
        }
        let group_id = HostGroupId::new();
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO host_groups (id, label, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 1, ?3, ?3)",
            params![group_id.as_str(), label, now],
        )?;
        transaction.commit()?;
        Ok(HostGroupSummary {
            group_id,
            label,
            state_version: WireSequence::new(1),
        })
    }

    pub fn list_host_groups(&self) -> Result<Vec<HostGroupSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, label, state_version FROM host_groups ORDER BY label COLLATE NOCASE, id",
        )?;
        statement
            .query_map([], read_host_group)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn update_host_group(
        &mut self,
        group_id: &HostGroupId,
        expected_state_version: WireSequence,
        label: &str,
    ) -> Result<HostGroupSummary> {
        let label = normalized_label(label)?;
        let next_version = next_revision(expected_state_version)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_metadata_version(
            &transaction,
            "host_groups",
            group_id.as_str(),
            expected_state_version,
        )?;
        if metadata_label_exists(&transaction, "host_groups", &label, Some(group_id.as_str()))? {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = transaction.execute(
            "UPDATE host_groups SET label = ?1, state_version = ?2, updated_at_ms = ?3
             WHERE id = ?4 AND state_version = ?5",
            params![
                label,
                u64_to_i64(next_version)?,
                unix_time_ms(),
                group_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(HostGroupSummary {
            group_id: group_id.clone(),
            label,
            state_version: WireSequence::new(next_version),
        })
    }

    pub fn delete_host_group(
        &mut self,
        group_id: &HostGroupId,
        expected_state_version: WireSequence,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_metadata_version(
            &transaction,
            "host_groups",
            group_id.as_str(),
            expected_state_version,
        )?;
        let referenced: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE group_id = ?1)",
            [group_id.as_str()],
            |row| row.get(0),
        )?;
        if referenced {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = transaction.execute(
            "DELETE FROM host_groups WHERE id = ?1 AND state_version = ?2",
            params![group_id.as_str(), u64_to_i64(expected_state_version.get())?],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn create_host_tag(&mut self, label: &str) -> Result<HostTagSummary> {
        let label = normalized_label(label)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if metadata_label_exists(&transaction, "host_tags", &label, None)? {
            return Err(AppPersistenceError::Conflict);
        }
        let tag_id = HostTagId::new();
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO host_tags (id, label, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 1, ?3, ?3)",
            params![tag_id.as_str(), label, now],
        )?;
        transaction.commit()?;
        Ok(HostTagSummary {
            tag_id,
            label,
            state_version: WireSequence::new(1),
        })
    }

    pub fn list_host_tags(&self) -> Result<Vec<HostTagSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, label, state_version FROM host_tags ORDER BY label COLLATE NOCASE, id",
        )?;
        statement
            .query_map([], read_host_tag)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn update_host_tag(
        &mut self,
        tag_id: &HostTagId,
        expected_state_version: WireSequence,
        label: &str,
    ) -> Result<HostTagSummary> {
        let label = normalized_label(label)?;
        let next_version = next_revision(expected_state_version)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_metadata_version(
            &transaction,
            "host_tags",
            tag_id.as_str(),
            expected_state_version,
        )?;
        if metadata_label_exists(&transaction, "host_tags", &label, Some(tag_id.as_str()))? {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = transaction.execute(
            "UPDATE host_tags SET label = ?1, state_version = ?2, updated_at_ms = ?3
             WHERE id = ?4 AND state_version = ?5",
            params![
                label,
                u64_to_i64(next_version)?,
                unix_time_ms(),
                tag_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(HostTagSummary {
            tag_id: tag_id.clone(),
            label,
            state_version: WireSequence::new(next_version),
        })
    }

    pub fn delete_host_tag(
        &mut self,
        tag_id: &HostTagId,
        expected_state_version: WireSequence,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_metadata_version(
            &transaction,
            "host_tags",
            tag_id.as_str(),
            expected_state_version,
        )?;
        let referenced: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM host_tag_assignments WHERE tag_id = ?1)",
            [tag_id.as_str()],
            |row| row.get(0),
        )?;
        if referenced {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = transaction.execute(
            "DELETE FROM host_tags WHERE id = ?1 AND state_version = ?2",
            params![tag_id.as_str(), u64_to_i64(expected_state_version.get())?],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn get_host_organization(&self, host_id: &HostId) -> Result<HostOrganizationSummary> {
        let (group_id, host_state_version) = self
            .connection
            .query_row(
                "SELECT group_id, state_version FROM hosts WHERE id = ?1",
                [host_id.as_str()],
                |row| {
                    let group_id = row
                        .get::<_, Option<String>>(0)?
                        .map(|value| parse_id(value, HostGroupId::parse))
                        .transpose()?;
                    Ok((group_id, read_wire_sequence(row, 1)?))
                },
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        Ok(HostOrganizationSummary {
            host_id: host_id.clone(),
            group_id,
            tag_ids: read_host_tag_ids(&self.connection, host_id)?,
            host_state_version,
        })
    }

    pub fn replace_host_organization(
        &mut self,
        host_id: &HostId,
        expected_host_state_version: WireSequence,
        group_id: Option<&HostGroupId>,
        tag_ids: &[HostTagId],
    ) -> Result<HostOrganizationSummary> {
        if tag_ids.len() > MAX_HOST_TAGS
            || tag_ids
                .iter()
                .enumerate()
                .any(|(index, tag_id)| tag_ids[..index].contains(tag_id))
        {
            return Err(AppPersistenceError::InvalidInput(
                "host tags are duplicated or exceed the limit",
            ));
        }
        let next_version = next_revision(expected_host_state_version)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_metadata_version(
            &transaction,
            "hosts",
            host_id.as_str(),
            expected_host_state_version,
        )?;
        if let Some(group_id) = group_id {
            require_metadata_exists(&transaction, "host_groups", group_id.as_str())?;
        }
        for tag_id in tag_ids {
            require_metadata_exists(&transaction, "host_tags", tag_id.as_str())?;
        }
        let changed = transaction.execute(
            "UPDATE hosts SET group_id = ?1, state_version = ?2, updated_at_ms = ?3
             WHERE id = ?4 AND state_version = ?5",
            params![
                group_id.map(HostGroupId::as_str),
                u64_to_i64(next_version)?,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_host_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.execute(
            "DELETE FROM host_tag_assignments WHERE host_id = ?1",
            [host_id.as_str()],
        )?;
        for tag_id in tag_ids {
            transaction.execute(
                "INSERT INTO host_tag_assignments (host_id, tag_id) VALUES (?1, ?2)",
                params![host_id.as_str(), tag_id.as_str()],
            )?;
        }
        transaction.commit()?;
        self.get_host_organization(host_id)
    }

    pub fn update_host_favorite(
        &mut self,
        host_id: &HostId,
        expected_state_version: WireSequence,
        favorite: bool,
    ) -> Result<HostSummary> {
        let next_version = next_revision(expected_state_version)?;
        let changed = self.connection.execute(
            "UPDATE hosts SET favorite = ?1, state_version = ?2, updated_at_ms = ?3
             WHERE id = ?4 AND state_version = ?5",
            params![
                favorite,
                u64_to_i64(next_version)?,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(if self.host_exists(host_id)? {
                AppPersistenceError::Conflict
            } else {
                AppPersistenceError::NotFound
            });
        }
        self.get_host(host_id)
    }

    /// Records only an authenticated successful connection fact for a saved Host.
    pub fn record_successful_connection(
        &mut self,
        host_id: &HostId,
    ) -> Result<RecentConnectionSummary> {
        self.record_successful_connection_at(host_id, unix_time_ms())
    }

    pub fn list_recent_connections(&self, limit: u16) -> Result<Vec<RecentConnectionSummary>> {
        if !(1..=MAX_RECENT_CONNECTIONS).contains(&limit) {
            return Err(AppPersistenceError::InvalidInput(
                "recent connection limit is outside its bounds",
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT host_id, connected_at_ms, recency_sequence, successful_connection_count
             FROM host_recent_connections
             ORDER BY recency_sequence DESC, host_id LIMIT ?1",
        )?;
        statement
            .query_map([i64::from(limit)], read_recent_connection)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn list_host_catalog(&self, sort: HostCatalogSort) -> Result<Vec<HostCatalogEntry>> {
        let order = match sort {
            HostCatalogSort::Label => "hosts.label COLLATE NOCASE, hosts.id",
            HostCatalogSort::FavoriteThenLabel => {
                "hosts.favorite DESC, hosts.label COLLATE NOCASE, hosts.id"
            }
            HostCatalogSort::RecentlyConnected => {
                "host_recent_connections.recency_sequence IS NULL, \
                 host_recent_connections.recency_sequence DESC, hosts.favorite DESC, \
                 hosts.label COLLATE NOCASE, hosts.id"
            }
        };
        let mut statement = self.connection.prepare(&format!(
            "SELECT hosts.id, hosts.label, hosts.address, hosts.normalized_address, hosts.port,
                    hosts.username, hosts.identity_id, hosts.favorite, hosts.state_version,
                    host_groups.id, host_groups.label, host_groups.state_version,
                    host_recent_connections.host_id, host_recent_connections.connected_at_ms,
                    host_recent_connections.recency_sequence,
                    host_recent_connections.successful_connection_count
             FROM hosts
             LEFT JOIN host_groups ON host_groups.id = hosts.group_id
             LEFT JOIN host_recent_connections ON host_recent_connections.host_id = hosts.id
             ORDER BY {order}"
        ))?;
        let rows = statement
            .query_map([], read_host_catalog_base)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        rows.into_iter()
            .map(|(host, group, recent_connection)| {
                let tags = read_host_tags(&self.connection, &host.host_id)?;
                Ok(HostCatalogEntry {
                    host,
                    group,
                    tags,
                    recent_connection,
                })
            })
            .collect()
    }

    fn record_successful_connection_at(
        &mut self,
        host_id: &HostId,
        connected_at_unix_ms: i64,
    ) -> Result<RecentConnectionSummary> {
        if connected_at_unix_ms < 0 {
            return Err(AppPersistenceError::InvalidInput(
                "connection timestamp is invalid",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_metadata_exists(&transaction, "hosts", host_id.as_str())?;
        let last_sequence: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(recency_sequence), 0) FROM host_recent_connections",
            [],
            |row| row.get(0),
        )?;
        let next_sequence = last_sequence
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?;
        let current_count: Option<i64> = transaction
            .query_row(
                "SELECT successful_connection_count FROM host_recent_connections
                 WHERE host_id = ?1",
                [host_id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let next_count = current_count
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?;
        transaction.execute(
            "INSERT INTO host_recent_connections
             (host_id, connected_at_ms, recency_sequence, successful_connection_count)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(host_id) DO UPDATE SET
               connected_at_ms = excluded.connected_at_ms,
               recency_sequence = excluded.recency_sequence,
               successful_connection_count = excluded.successful_connection_count",
            params![
                host_id.as_str(),
                connected_at_unix_ms,
                next_sequence,
                next_count,
            ],
        )?;
        transaction.commit()?;
        Ok(RecentConnectionSummary {
            host_id: host_id.clone(),
            connected_at_unix_ms,
            recency_sequence: WireSequence::new(
                u64::try_from(next_sequence).map_err(|_| AppPersistenceError::InvalidStoredData)?,
            ),
            successful_connection_count: WireSequence::new(
                u64::try_from(next_count).map_err(|_| AppPersistenceError::InvalidStoredData)?,
            ),
        })
    }

    pub fn get_host_connection_config(
        &self,
        host_id: &HostId,
    ) -> Result<HostConnectionConfigSummary> {
        if !self.host_exists(host_id)? {
            return Err(AppPersistenceError::NotFound);
        }
        Ok(HostConnectionConfigSummary {
            route_plan: self.get_route_plan(host_id)?,
            authentication_plan: self.get_authentication_plan(host_id)?,
            algorithm_policy: self.get_algorithm_policy(host_id)?,
            heartbeat_policy: self.get_heartbeat_policy(host_id)?,
            monitoring_policy: self.get_monitoring_policy(host_id)?,
            login_automation: self.get_login_automation(host_id)?,
        })
    }

    pub fn get_host_connection_snapshot(&self, host_id: &HostId) -> Result<HostConnectionSnapshot> {
        let transaction = self.connection.unchecked_transaction()?;
        let host = self.get_host(host_id)?;
        let identity = host
            .identity_id
            .as_ref()
            .map(|identity_id| self.get_identity(identity_id))
            .transpose()?;
        let config = self.get_host_connection_config(host_id)?;
        let login_automation = self.get_login_automation_execution_plan(host_id)?;
        let credentials = match config.authentication_plan.mode {
            AuthenticationPlanMode::Identity => {
                self.list_ready_credential_records_for_host(host_id)?
            }
            AuthenticationPlanMode::HostOverride => config
                .authentication_plan
                .credential_ref_ids
                .iter()
                .map(|credential_ref_id| {
                    let credential = self.get_ready_credential_record(credential_ref_id)?;
                    if host.identity_id.as_ref() != Some(&credential.identity_id) {
                        return Err(AppPersistenceError::InvalidStoredData);
                    }
                    Ok(credential)
                })
                .collect::<Result<Vec<_>>>()?,
        };
        transaction.commit()?;
        Ok(HostConnectionSnapshot {
            host,
            identity,
            config,
            credentials,
            login_automation,
        })
    }

    pub fn get_route_plan(&self, host_id: &HostId) -> Result<RoutePlanSummary> {
        let (revision, kind, proxy_address, proxy_normalized_address, proxy_port, dns_mode, auth) =
            self.connection
                .query_row(
                    "SELECT revision, ingress_kind, proxy_address, proxy_normalized_address,
                            proxy_port, proxy_dns_mode, proxy_auth_credential_ref_id
                     FROM host_route_plans WHERE host_id = ?1",
                    [host_id.as_str()],
                    |row| {
                        Ok((
                            read_wire_sequence(row, 0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(AppPersistenceError::NotFound)?;
        let ingress = route_ingress_from_db(
            &kind,
            proxy_address,
            proxy_normalized_address,
            proxy_port,
            dns_mode,
            auth,
        )?;
        let mut statement = self.connection.prepare(
            "SELECT jump_host_id FROM host_route_jump_hops
             WHERE host_id = ?1 ORDER BY ordinal",
        )?;
        let jump_host_ids = statement
            .query_map([host_id.as_str()], |row| {
                parse_id(row.get::<_, String>(0)?, HostId::parse)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(RoutePlanSummary {
            host_id: host_id.clone(),
            revision,
            ingress,
            jump_host_ids,
        })
    }

    pub fn replace_route_plan(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
        ingress: &RouteIngress,
        jump_host_ids: &[HostId],
    ) -> Result<RoutePlanSummary> {
        let ingress = validated_route_ingress(ingress)?;
        validate_unique_bounded_ids(jump_host_ids, MAX_JUMP_HOPS, "jump host chain is invalid")?;
        if jump_host_ids.iter().any(|candidate| candidate == host_id) {
            return Err(AppPersistenceError::InvalidInput(
                "jump host chain cannot reference its target host",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_route_graph(&transaction, host_id, jump_host_ids)?;
        let next_revision = next_revision(expected_revision)?;
        let encoded = route_ingress_to_db(&ingress);
        if let Some(proxy_auth_credential_ref_id) = &encoded.proxy_auth_credential_ref_id {
            let is_ready_password: bool = transaction.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM credential_refs
                   WHERE id = ?1 AND kind = 'password' AND import_state = 'ready'
                 )",
                [proxy_auth_credential_ref_id],
                |row| row.get(0),
            )?;
            if !is_ready_password {
                return Err(AppPersistenceError::InvalidInput(
                    "proxy authentication requires a ready password credential",
                ));
            }
        }
        let changed = transaction.execute(
            "UPDATE host_route_plans
             SET revision = ?1, ingress_kind = ?2, proxy_address = ?3,
                 proxy_normalized_address = ?4, proxy_port = ?5, proxy_dns_mode = ?6,
                 proxy_auth_credential_ref_id = ?7, updated_at_ms = ?8
             WHERE host_id = ?9 AND revision = ?10",
            params![
                u64_to_i64(next_revision)?,
                encoded.kind,
                encoded.address,
                encoded.normalized_address,
                encoded.port,
                encoded.dns_mode,
                encoded.proxy_auth_credential_ref_id,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(config_update_error(
                &transaction,
                "host_route_plans",
                host_id,
            )?);
        }
        transaction.execute(
            "DELETE FROM host_route_jump_hops WHERE host_id = ?1",
            [host_id.as_str()],
        )?;
        for (ordinal, jump_host_id) in jump_host_ids.iter().enumerate() {
            transaction.execute(
                "INSERT INTO host_route_jump_hops (host_id, ordinal, jump_host_id)
                 VALUES (?1, ?2, ?3)",
                params![
                    host_id.as_str(),
                    usize_to_i64(ordinal)?,
                    jump_host_id.as_str()
                ],
            )?;
        }
        transaction.commit()?;
        self.get_route_plan(host_id)
    }

    pub fn get_authentication_plan(&self, host_id: &HostId) -> Result<AuthenticationPlanSummary> {
        let (revision, mode) = self
            .connection
            .query_row(
                "SELECT revision, mode FROM host_authentication_plans WHERE host_id = ?1",
                [host_id.as_str()],
                |row| Ok((read_wire_sequence(row, 0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let mode = authentication_mode_from_db(&mode)?;
        let mut statement = self.connection.prepare(
            "SELECT credential_ref_id FROM host_authentication_credentials
             WHERE host_id = ?1 ORDER BY ordinal",
        )?;
        let credential_ref_ids = statement
            .query_map([host_id.as_str()], |row| {
                parse_id(row.get::<_, String>(0)?, CredentialRefId::parse)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(AuthenticationPlanSummary {
            host_id: host_id.clone(),
            revision,
            mode,
            credential_ref_ids,
        })
    }

    pub fn replace_authentication_plan(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
        mode: AuthenticationPlanMode,
        credential_ref_ids: &[CredentialRefId],
    ) -> Result<AuthenticationPlanSummary> {
        validate_unique_bounded_ids(
            credential_ref_ids,
            MAX_AUTHENTICATION_CREDENTIALS,
            "authentication plan is invalid",
        )?;
        match mode {
            AuthenticationPlanMode::Identity if !credential_ref_ids.is_empty() => {
                return Err(AppPersistenceError::InvalidInput(
                    "identity authentication cannot contain host override credentials",
                ));
            }
            AuthenticationPlanMode::HostOverride if credential_ref_ids.is_empty() => {
                return Err(AppPersistenceError::InvalidInput(
                    "host authentication override must contain a credential",
                ));
            }
            _ => {}
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for credential_ref_id in credential_ref_ids {
            let ready_for_host_identity: bool = transaction.query_row(
                "SELECT EXISTS(
                   SELECT 1
                   FROM credential_refs AS credential
                   JOIN hosts AS host ON host.id = ?2
                   WHERE credential.id = ?1
                     AND credential.import_state = 'ready'
                     AND credential.identity_id = host.identity_id
                 )",
                params![credential_ref_id.as_str(), host_id.as_str()],
                |row| row.get(0),
            )?;
            if !ready_for_host_identity {
                return Err(AppPersistenceError::InvalidInput(
                    "host authentication override credential must belong to the host identity",
                ));
            }
        }
        let next_revision = next_revision(expected_revision)?;
        let changed = transaction.execute(
            "UPDATE host_authentication_plans SET revision = ?1, mode = ?2, updated_at_ms = ?3
             WHERE host_id = ?4 AND revision = ?5",
            params![
                u64_to_i64(next_revision)?,
                authentication_mode_to_db(mode),
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(config_update_error(
                &transaction,
                "host_authentication_plans",
                host_id,
            )?);
        }
        transaction.execute(
            "DELETE FROM host_authentication_credentials WHERE host_id = ?1",
            [host_id.as_str()],
        )?;
        for (ordinal, credential_ref_id) in credential_ref_ids.iter().enumerate() {
            transaction.execute(
                "INSERT INTO host_authentication_credentials (host_id, ordinal, credential_ref_id)
                 VALUES (?1, ?2, ?3)",
                params![
                    host_id.as_str(),
                    usize_to_i64(ordinal)?,
                    credential_ref_id.as_str(),
                ],
            )?;
        }
        transaction.commit()?;
        self.get_authentication_plan(host_id)
    }

    pub fn get_algorithm_policy(&self, host_id: &HostId) -> Result<AlgorithmPolicySummary> {
        let (revision, policy_id) = self
            .connection
            .query_row(
                "SELECT revision, policy_id FROM host_algorithm_policies WHERE host_id = ?1",
                [host_id.as_str()],
                |row| Ok((read_wire_sequence(row, 0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let mut statement = self.connection.prepare(
            "SELECT category, exception_id, reason FROM host_algorithm_exceptions
             WHERE host_id = ?1 ORDER BY category, exception_id",
        )?;
        let compatibility_exceptions = statement
            .query_map([host_id.as_str()], |row| {
                Ok(AlgorithmCompatibilityException {
                    category: algorithm_category_from_db(&row.get::<_, String>(0)?)?,
                    exception_id: row.get(1)?,
                    reason: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(AlgorithmPolicySummary {
            host_id: host_id.clone(),
            revision,
            policy_id,
            compatibility_exceptions,
        })
    }

    pub fn replace_algorithm_policy(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
        policy_id: &str,
        compatibility_exceptions: &[AlgorithmCompatibilityException],
    ) -> Result<AlgorithmPolicySummary> {
        let policy_id = validated_catalog_id(policy_id, "algorithm policy id is invalid")?;
        if compatibility_exceptions.len() > 64 {
            return Err(AppPersistenceError::InvalidInput(
                "too many algorithm compatibility exceptions",
            ));
        }
        let mut keys = std::collections::HashSet::new();
        let exceptions = compatibility_exceptions
            .iter()
            .map(|exception| {
                let exception_id = validated_catalog_id(
                    &exception.exception_id,
                    "algorithm exception id is invalid",
                )?;
                let reason = normalized_optional(
                    exception.reason.as_deref(),
                    240,
                    "algorithm exception reason is too long",
                )?;
                if !keys.insert((exception.category, exception_id.clone())) {
                    return Err(AppPersistenceError::InvalidInput(
                        "algorithm exception is duplicated",
                    ));
                }
                Ok(AlgorithmCompatibilityException {
                    category: exception.category,
                    exception_id,
                    reason,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next_revision = next_revision(expected_revision)?;
        let changed = transaction.execute(
            "UPDATE host_algorithm_policies SET revision = ?1, policy_id = ?2, updated_at_ms = ?3
             WHERE host_id = ?4 AND revision = ?5",
            params![
                u64_to_i64(next_revision)?,
                policy_id,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(config_update_error(
                &transaction,
                "host_algorithm_policies",
                host_id,
            )?);
        }
        transaction.execute(
            "DELETE FROM host_algorithm_exceptions WHERE host_id = ?1",
            [host_id.as_str()],
        )?;
        for exception in exceptions {
            transaction.execute(
                "INSERT INTO host_algorithm_exceptions (host_id, category, exception_id, reason)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    host_id.as_str(),
                    algorithm_category_to_db(exception.category),
                    exception.exception_id,
                    exception.reason,
                ],
            )?;
        }
        transaction.commit()?;
        self.get_algorithm_policy(host_id)
    }

    pub fn get_heartbeat_policy(&self, host_id: &HostId) -> Result<HeartbeatPolicySummary> {
        let (
            revision,
            mode,
            interval,
            reply_timeout,
            failure_threshold,
            payload,
            line_ending,
            idle,
        ) = self
            .connection
            .query_row(
                "SELECT revision, mode, interval_seconds, reply_timeout_seconds,
                            failure_threshold, shell_payload_text, shell_line_ending,
                            shell_user_idle_seconds
                     FROM host_heartbeat_policies WHERE host_id = ?1",
                [host_id.as_str()],
                |row| {
                    Ok((
                        read_wire_sequence(row, 0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let policy = heartbeat_from_db(
            &mode,
            interval,
            reply_timeout,
            failure_threshold,
            payload,
            line_ending,
            idle,
        )?;
        Ok(HeartbeatPolicySummary {
            host_id: host_id.clone(),
            revision,
            policy,
        })
    }

    pub fn replace_heartbeat_policy(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
        policy: &HeartbeatPolicy,
    ) -> Result<HeartbeatPolicySummary> {
        let policy = validated_heartbeat_policy(policy)?;
        let encoded = heartbeat_to_db(&policy);
        let next_revision = next_revision(expected_revision)?;
        let changed = self.connection.execute(
            "UPDATE host_heartbeat_policies
             SET revision = ?1, mode = ?2, interval_seconds = ?3,
                 reply_timeout_seconds = ?4, failure_threshold = ?5,
                 shell_payload_text = ?6, shell_line_ending = ?7,
                 shell_user_idle_seconds = ?8, updated_at_ms = ?9
             WHERE host_id = ?10 AND revision = ?11",
            params![
                u64_to_i64(next_revision)?,
                encoded.mode,
                encoded.interval,
                encoded.reply_timeout,
                encoded.failure_threshold,
                encoded.payload,
                encoded.line_ending,
                encoded.user_idle,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(if self.host_exists(host_id)? {
                AppPersistenceError::Conflict
            } else {
                AppPersistenceError::NotFound
            });
        }
        self.get_heartbeat_policy(host_id)
    }

    pub fn get_monitoring_policy(&self, host_id: &HostId) -> Result<MonitoringPolicySummary> {
        let (revision, enabled, interval, timeout) = self
            .connection
            .query_row(
                "SELECT revision, enabled, sample_interval_seconds, sample_timeout_seconds
                 FROM host_monitoring_policies WHERE host_id = ?1",
                [host_id.as_str()],
                |row| {
                    Ok((
                        read_wire_sequence(row, 0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let disk_mount_ids = read_disk_monitoring_selections(&self.connection, host_id)?;
        let network_interface_ids = read_network_monitoring_selections(&self.connection, host_id)?;
        if disk_mount_ids.as_slice() != [DiskResourceId::Root]
            || network_interface_ids.as_slice() != [NetworkResourceId::AggregateNonLoopback]
        {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        Ok(MonitoringPolicySummary {
            host_id: host_id.clone(),
            revision,
            policy: MonitoringPolicy {
                enabled,
                sample_interval_seconds: u32::try_from(interval)
                    .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                sample_timeout_seconds: u32::try_from(timeout)
                    .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                disk_mount_ids,
                network_interface_ids,
            },
        })
    }

    pub fn replace_monitoring_policy(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
        policy: &MonitoringPolicy,
    ) -> Result<MonitoringPolicySummary> {
        let policy = validated_monitoring_policy(policy)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next_revision = next_revision(expected_revision)?;
        let changed = transaction.execute(
            "UPDATE host_monitoring_policies
             SET revision = ?1, enabled = ?2, sample_interval_seconds = ?3,
                 sample_timeout_seconds = ?4, updated_at_ms = ?5
             WHERE host_id = ?6 AND revision = ?7",
            params![
                u64_to_i64(next_revision)?,
                policy.enabled,
                i64::from(policy.sample_interval_seconds),
                i64::from(policy.sample_timeout_seconds),
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(config_update_error(
                &transaction,
                "host_monitoring_policies",
                host_id,
            )?);
        }
        transaction.execute(
            "DELETE FROM host_monitoring_selections WHERE host_id = ?1",
            [host_id.as_str()],
        )?;
        for value in &policy.disk_mount_ids {
            transaction.execute(
                "INSERT INTO host_monitoring_selections (host_id, kind, stable_id)
                 VALUES (?1, 'disk_mount', ?2)",
                params![host_id.as_str(), value.as_str()],
            )?;
        }
        for value in &policy.network_interface_ids {
            transaction.execute(
                "INSERT INTO host_monitoring_selections (host_id, kind, stable_id)
                 VALUES (?1, 'network_interface', ?2)",
                params![host_id.as_str(), value.as_str()],
            )?;
        }
        transaction.commit()?;
        self.get_monitoring_policy(host_id)
    }

    pub fn begin_host_create_password_stage_with_outcome(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
        identity_label: &str,
        credential_label: &str,
    ) -> Result<HostCreatePasswordStageBegin> {
        self.ensure_host_create_password_stages_usable()?;
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let identity_label = normalized_label(identity_label)?;
        let credential_label = normalized_label(credential_label)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let by_operation =
            lookup_host_create_password_stage(&transaction, "operation_id", operation_id.as_str())?;
        let by_idempotency =
            lookup_host_create_password_stage(&transaction, "idempotency_key", &idempotency_key)?;
        let existing = match (by_operation, by_idempotency) {
            (None, None) => None,
            (Some(left), Some(right)) if left.staged_password_id == right.staged_password_id => {
                Some(left)
            }
            _ => return Err(AppPersistenceError::IdempotencyConflict),
        };
        if let Some(existing) = existing {
            if existing.operation_id != *operation_id
                || existing.idempotency_key != idempotency_key
                || existing.identity_label != identity_label
                || existing.credential_label != credential_label
                || matches!(
                    existing.state,
                    HostCreatePasswordStageState::CleanupPending
                        | HostCreatePasswordStageState::Cancelled
                        | HostCreatePasswordStageState::Consumed
                )
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(HostCreatePasswordStageBegin {
                record: existing,
                created: false,
            });
        }
        let now = unix_time_ms();
        let record = HostCreatePasswordStageRecord {
            staged_password_id: HostCreatePasswordStageId::new(),
            secret_ref_id: SecretRefId::new(),
            operation_id: operation_id.clone(),
            idempotency_key,
            identity_label,
            credential_label,
            expires_at_unix_ms: now.saturating_add(HOST_CREATE_PASSWORD_STAGE_TTL_MS),
            state: HostCreatePasswordStageState::PendingVault,
        };
        if transaction
            .execute(
                "INSERT INTO host_create_password_stages
                 (stage_id, secret_ref_id, operation_id, idempotency_key, identity_label,
                  credential_label, expires_at_ms, state, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending_vault', ?8, ?8)",
                params![
                    record.staged_password_id.as_str(),
                    record.secret_ref_id.as_str(),
                    record.operation_id.as_str(),
                    record.idempotency_key,
                    record.identity_label,
                    record.credential_label,
                    record.expires_at_unix_ms,
                    now,
                ],
            )
            .is_err()
        {
            self.host_create_password_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        transaction.commit()?;
        Ok(HostCreatePasswordStageBegin {
            record,
            created: true,
        })
    }

    pub fn mark_host_create_password_staged(
        &mut self,
        staged_password_id: &HostCreatePasswordStageId,
    ) -> Result<HostCreatePasswordStageRecord> {
        self.ensure_host_create_password_stages_usable()?;
        let existing = lookup_host_create_password_stage(
            &self.connection,
            "stage_id",
            staged_password_id.as_str(),
        )?
        .ok_or(AppPersistenceError::NotFound)?;
        if existing.state == HostCreatePasswordStageState::Staged {
            return Ok(existing);
        }
        if existing.state != HostCreatePasswordStageState::PendingVault {
            return Err(AppPersistenceError::RequiresReload);
        }
        if self
            .connection
            .execute(
                "UPDATE host_create_password_stages
                 SET state = 'staged', updated_at_ms = ?1
                 WHERE stage_id = ?2 AND state = 'pending_vault'",
                params![unix_time_ms(), staged_password_id.as_str()],
            )
            .is_err()
        {
            self.host_create_password_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(HostCreatePasswordStageRecord {
            state: HostCreatePasswordStageState::Staged,
            ..existing
        })
    }

    pub fn abort_pending_host_create_password_stage(
        &mut self,
        staged_password_id: &HostCreatePasswordStageId,
    ) -> Result<()> {
        self.ensure_host_create_password_stages_usable()?;
        let changed = self.connection.execute(
            "UPDATE host_create_password_stages
             SET state = 'cancelled', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state = 'pending_vault'",
            params![unix_time_ms(), staged_password_id.as_str()],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(())
    }

    pub fn list_host_create_password_cleanup_candidates(
        &self,
        maximum: usize,
    ) -> Result<Vec<HostCreatePasswordStageRecord>> {
        self.ensure_host_create_password_stages_usable()?;
        let maximum = maximum.clamp(1, 256);
        let mut statement = self.connection.prepare(
            "SELECT stage_id, secret_ref_id, operation_id, idempotency_key, identity_label,
                    credential_label, expires_at_ms, state
             FROM host_create_password_stages
             WHERE state IN ('pending_vault', 'cleanup_pending')
                OR (state = 'staged' AND expires_at_ms <= ?1)
             ORDER BY updated_at_ms, stage_id LIMIT ?2",
        )?;
        statement
            .query_map(
                params![unix_time_ms(), usize_to_i64(maximum)?],
                read_host_create_password_stage,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn begin_host_create_password_cleanup(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
    ) -> Result<Option<HostCreatePasswordStageRecord>> {
        self.ensure_host_create_password_stages_usable()?;
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let by_operation =
            lookup_host_create_password_stage(&transaction, "operation_id", operation_id.as_str())?;
        let by_idempotency =
            lookup_host_create_password_stage(&transaction, "idempotency_key", &idempotency_key)?;
        let Some(existing) = (match (by_operation, by_idempotency) {
            (None, None) => None,
            (Some(left), Some(right)) if left.staged_password_id == right.staged_password_id => {
                Some(left)
            }
            _ => return Err(AppPersistenceError::IdempotencyConflict),
        }) else {
            transaction.commit()?;
            return Ok(None);
        };
        if existing.operation_id != *operation_id || existing.idempotency_key != idempotency_key {
            return Err(AppPersistenceError::IdempotencyConflict);
        }
        if matches!(
            existing.state,
            HostCreatePasswordStageState::Cancelled | HostCreatePasswordStageState::Consumed
        ) {
            transaction.commit()?;
            return Ok(Some(existing));
        }
        if existing.state != HostCreatePasswordStageState::CleanupPending {
            transaction.execute(
                "UPDATE host_create_password_stages
                 SET state = 'cleanup_pending', updated_at_ms = ?1
                 WHERE stage_id = ?2 AND state IN ('pending_vault', 'staged')",
                params![unix_time_ms(), existing.staged_password_id.as_str()],
            )?;
        }
        transaction.commit()?;
        Ok(Some(HostCreatePasswordStageRecord {
            state: HostCreatePasswordStageState::CleanupPending,
            ..existing
        }))
    }

    pub fn finalize_host_create_password_cleanup(
        &mut self,
        staged_password_id: &HostCreatePasswordStageId,
    ) -> Result<HostCreatePasswordCancelResponse> {
        self.ensure_host_create_password_stages_usable()?;
        let changed = self.connection.execute(
            "UPDATE host_create_password_stages
             SET state = 'cancelled', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state = 'cleanup_pending'",
            params![unix_time_ms(), staged_password_id.as_str()],
        )?;
        if changed == 0 {
            let existing = lookup_host_create_password_stage(
                &self.connection,
                "stage_id",
                staged_password_id.as_str(),
            )?;
            if existing
                .is_some_and(|record| record.state == HostCreatePasswordStageState::Cancelled)
            {
                return Ok(HostCreatePasswordCancelResponse { cancelled: false });
            }
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(HostCreatePasswordCancelResponse { cancelled: true })
    }

    pub fn begin_login_automation_secret_stage(
        &mut self,
        host_id: &HostId,
        expected_automation_revision: WireSequence,
        operation_id: &OperationId,
        idempotency_key: &str,
        label: &str,
    ) -> Result<LoginAutomationSecretStageRecord> {
        self.begin_login_automation_secret_stage_with_outcome(
            host_id,
            expected_automation_revision,
            operation_id,
            idempotency_key,
            label,
        )
        .map(|outcome| outcome.record)
    }

    pub fn begin_login_automation_secret_stage_with_outcome(
        &mut self,
        host_id: &HostId,
        expected_automation_revision: WireSequence,
        operation_id: &OperationId,
        idempotency_key: &str,
        label: &str,
    ) -> Result<LoginAutomationSecretStageBegin> {
        self.ensure_login_automation_secret_stages_usable()?;
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let label = normalized_label(label)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let by_operation = lookup_login_automation_secret_stage(
            &transaction,
            "operation_id",
            operation_id.as_str(),
        )?;
        let by_idempotency = lookup_login_automation_secret_stage(
            &transaction,
            "idempotency_key",
            &idempotency_key,
        )?;
        if let Some(existing) = by_operation.or(by_idempotency) {
            let both_match = lookup_login_automation_secret_stage(
                &transaction,
                "operation_id",
                operation_id.as_str(),
            )?
            .is_some_and(|record| record.staged_secret_id == existing.staged_secret_id)
                && lookup_login_automation_secret_stage(
                    &transaction,
                    "idempotency_key",
                    &idempotency_key,
                )?
                .is_some_and(|record| record.staged_secret_id == existing.staged_secret_id);
            if !both_match
                || existing.host_id != *host_id
                || existing.expected_automation_revision != expected_automation_revision
                || existing.label != label
                || matches!(
                    existing.state,
                    LoginAutomationSecretStageState::CleanupPending
                        | LoginAutomationSecretStageState::Cancelled
                        | LoginAutomationSecretStageState::Consumed
                )
                || existing.expires_at_unix_ms <= unix_time_ms()
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(LoginAutomationSecretStageBegin {
                record: existing,
                created: false,
            });
        }
        let current_revision = transaction
            .query_row(
                "SELECT revision FROM host_login_automations WHERE host_id = ?1",
                [host_id.as_str()],
                |row| read_wire_sequence(row, 0),
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        if current_revision != expected_automation_revision {
            return Err(AppPersistenceError::Conflict);
        }
        let now = unix_time_ms();
        let record = LoginAutomationSecretStageRecord {
            staged_secret_id: LoginAutomationSecretStageId::new(),
            secret_ref_id: SecretRefId::new(),
            host_id: host_id.clone(),
            expected_automation_revision,
            operation_id: operation_id.clone(),
            idempotency_key,
            label,
            expires_at_unix_ms: now.saturating_add(LOGIN_AUTOMATION_SECRET_STAGE_TTL_MS),
            state: LoginAutomationSecretStageState::PendingVault,
        };
        transaction.execute(
            "INSERT INTO login_automation_secret_stages
             (stage_id, secret_ref_id, host_id, expected_automation_revision,
              operation_id, idempotency_key, label, expires_at_ms, state,
              created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending_vault', ?9, ?9)",
            params![
                record.staged_secret_id.as_str(),
                record.secret_ref_id.as_str(),
                record.host_id.as_str(),
                u64_to_i64(record.expected_automation_revision.get())?,
                record.operation_id.as_str(),
                record.idempotency_key,
                record.label,
                record.expires_at_unix_ms,
                now,
            ],
        )?;
        if transaction.commit().is_err() {
            self.login_automation_secret_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(LoginAutomationSecretStageBegin {
            record,
            created: true,
        })
    }

    pub fn mark_login_automation_secret_staged(
        &mut self,
        staged_secret_id: &LoginAutomationSecretStageId,
    ) -> Result<LoginAutomationSecretStageRecord> {
        self.ensure_login_automation_secret_stages_usable()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = lookup_login_automation_secret_stage(
            &transaction,
            "stage_id",
            staged_secret_id.as_str(),
        )?
        .ok_or(AppPersistenceError::NotFound)?;
        if existing.state == LoginAutomationSecretStageState::Staged {
            transaction.commit()?;
            return Ok(existing);
        }
        if existing.state != LoginAutomationSecretStageState::PendingVault
            || existing.expires_at_unix_ms <= unix_time_ms()
        {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.execute(
            "UPDATE login_automation_secret_stages
             SET state = 'staged', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state = 'pending_vault'",
            params![unix_time_ms(), staged_secret_id.as_str()],
        )?;
        if transaction.commit().is_err() {
            self.login_automation_secret_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(LoginAutomationSecretStageRecord {
            state: LoginAutomationSecretStageState::Staged,
            ..existing
        })
    }

    /// Finalizes an intent only when Vault insertion definitely did not write
    /// the caller-minted reference. Unknown Vault outcomes must remain pending
    /// for delete-first reconciliation after reload.
    pub fn abort_pending_login_automation_secret_stage(
        &mut self,
        staged_secret_id: &LoginAutomationSecretStageId,
    ) -> Result<()> {
        self.ensure_login_automation_secret_stages_usable()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = lookup_login_automation_secret_stage(
            &transaction,
            "stage_id",
            staged_secret_id.as_str(),
        )?
        .ok_or(AppPersistenceError::NotFound)?;
        if existing.state == LoginAutomationSecretStageState::Cancelled {
            transaction.commit()?;
            return Ok(());
        }
        if existing.state != LoginAutomationSecretStageState::PendingVault {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.execute(
            "UPDATE login_automation_secret_stages
             SET state = 'cancelled', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state = 'pending_vault'",
            params![unix_time_ms(), staged_secret_id.as_str()],
        )?;
        if transaction.commit().is_err() {
            self.login_automation_secret_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(())
    }

    pub fn list_login_automation_secret_cleanup_candidates(
        &self,
        maximum: usize,
    ) -> Result<Vec<LoginAutomationSecretStageRecord>> {
        self.ensure_login_automation_secret_stages_usable()?;
        if maximum == 0 || maximum > 100 {
            return Err(AppPersistenceError::InvalidInput(
                "cleanup candidate limit must be between 1 and 100",
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT stage_id, secret_ref_id, host_id, expected_automation_revision,
                    operation_id, idempotency_key, label, expires_at_ms, state
             FROM login_automation_secret_stages
             WHERE state IN ('pending_vault', 'cleanup_pending')
                OR (state = 'staged' AND expires_at_ms <= ?1)
             ORDER BY updated_at_ms, stage_id
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![unix_time_ms(), usize_to_i64(maximum)?],
                read_login_automation_secret_stage,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn begin_login_automation_secret_cleanup(
        &mut self,
        operation_id: &OperationId,
        idempotency_key: &str,
    ) -> Result<Option<LoginAutomationSecretStageRecord>> {
        self.ensure_login_automation_secret_stages_usable()?;
        let idempotency_key = validated_idempotency_key(idempotency_key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let by_operation = lookup_login_automation_secret_stage(
            &transaction,
            "operation_id",
            operation_id.as_str(),
        )?;
        let by_idempotency = lookup_login_automation_secret_stage(
            &transaction,
            "idempotency_key",
            &idempotency_key,
        )?;
        let existing = match (by_operation, by_idempotency) {
            (None, None) => {
                transaction.commit()?;
                return Ok(None);
            }
            (Some(by_operation), Some(by_idempotency))
                if by_operation.staged_secret_id == by_idempotency.staged_secret_id =>
            {
                by_operation
            }
            _ => return Err(AppPersistenceError::IdempotencyConflict),
        };
        match existing.state {
            LoginAutomationSecretStageState::Cancelled => {
                transaction.commit()?;
                return Ok(Some(existing));
            }
            LoginAutomationSecretStageState::Consumed => {
                transaction.commit()?;
                return Ok(Some(existing));
            }
            LoginAutomationSecretStageState::CleanupPending => {
                transaction.commit()?;
                return Ok(Some(existing));
            }
            LoginAutomationSecretStageState::PendingVault
            | LoginAutomationSecretStageState::Staged => {}
        }
        transaction.execute(
            "UPDATE login_automation_secret_stages
             SET state = 'cleanup_pending', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state IN ('pending_vault', 'staged')",
            params![unix_time_ms(), existing.staged_secret_id.as_str()],
        )?;
        if transaction.commit().is_err() {
            self.login_automation_secret_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(Some(LoginAutomationSecretStageRecord {
            state: LoginAutomationSecretStageState::CleanupPending,
            ..existing
        }))
    }

    pub fn finalize_login_automation_secret_cleanup(
        &mut self,
        staged_secret_id: &LoginAutomationSecretStageId,
    ) -> Result<LoginAutomationSecretCancelResponse> {
        self.ensure_login_automation_secret_stages_usable()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE login_automation_secret_stages
             SET state = 'cancelled', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state = 'cleanup_pending'",
            params![unix_time_ms(), staged_secret_id.as_str()],
        )?;
        if changed == 0 {
            let existing = lookup_login_automation_secret_stage(
                &transaction,
                "stage_id",
                staged_secret_id.as_str(),
            )?;
            if existing
                .is_some_and(|record| record.state == LoginAutomationSecretStageState::Cancelled)
            {
                transaction.commit()?;
                return Ok(LoginAutomationSecretCancelResponse { cancelled: false });
            }
            return Err(AppPersistenceError::Conflict);
        }
        if transaction.commit().is_err() {
            self.login_automation_secret_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        Ok(LoginAutomationSecretCancelResponse { cancelled: true })
    }

    pub fn get_login_automation(&self, host_id: &HostId) -> Result<LoginAutomationSummary> {
        let (revision, enabled, confirmed_revision) = self
            .connection
            .query_row(
                "SELECT revision, enabled, confirmed_revision FROM host_login_automations WHERE host_id = ?1",
                [host_id.as_str()],
                |row| {
                    Ok((
                        read_wire_sequence(row, 0)?,
                        row.get::<_, bool>(1)?,
                        read_optional_wire_sequence(row, 2)?,
                    ))
                },
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let mut statement = self.connection.prepare(
            "SELECT kind, literal_text, append_enter, timeout_seconds, secret_label
             FROM host_login_automation_steps WHERE host_id = ?1 ORDER BY ordinal",
        )?;
        let steps = statement
            .query_map([host_id.as_str()], read_login_automation_step_summary)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(LoginAutomationSummary {
            host_id: host_id.clone(),
            revision,
            confirmed_revision,
            enabled,
            steps,
        })
    }

    fn get_login_automation_execution_plan(
        &self,
        host_id: &HostId,
    ) -> Result<LoginAutomationExecutionPlan> {
        let (revision, enabled, confirmed_revision) = self
            .connection
            .query_row(
                "SELECT revision, enabled, confirmed_revision FROM host_login_automations WHERE host_id = ?1",
                [host_id.as_str()],
                |row| {
                    Ok((
                        read_wire_sequence(row, 0)?,
                        row.get::<_, bool>(1)?,
                        read_optional_wire_sequence(row, 2)?,
                    ))
                },
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        let mut statement = self.connection.prepare(
            "SELECT kind, literal_text, append_enter, timeout_seconds, secret_ref_id, secret_label
             FROM host_login_automation_steps WHERE host_id = ?1 ORDER BY ordinal",
        )?;
        let steps = statement
            .query_map([host_id.as_str()], read_login_automation_step_input)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(LoginAutomationExecutionPlan {
            revision,
            confirmed_revision,
            enabled,
            steps,
        })
    }

    pub fn replace_login_automation(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
        enabled: bool,
        steps: &[LoginAutomationStepInput],
    ) -> Result<LoginAutomationSummary> {
        self.ensure_login_automation_secret_stages_usable()?;
        let steps = validated_login_automation(enabled, steps)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next_revision = next_revision(expected_revision)?;
        let changed = transaction.execute(
            "UPDATE host_login_automations SET revision = ?1, enabled = ?2,
                    confirmed_revision = NULL, updated_at_ms = ?3
             WHERE host_id = ?4 AND revision = ?5",
            params![
                u64_to_i64(next_revision)?,
                enabled,
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(config_update_error(
                &transaction,
                "host_login_automations",
                host_id,
            )?);
        }
        let mut resolved_steps = Vec::with_capacity(steps.len());
        for step in steps {
            match step {
                LoginAutomationStepInput::SendSecret {
                    secret_ref_id: staged_secret_id,
                    append_enter,
                    timeout_seconds,
                    ..
                } => {
                    let staged = lookup_login_automation_secret_stage(
                        &transaction,
                        "stage_id",
                        staged_secret_id.as_str(),
                    )?
                    .ok_or(AppPersistenceError::Conflict)?;
                    if staged.host_id != *host_id
                        || staged.expected_automation_revision != expected_revision
                        || staged.state != LoginAutomationSecretStageState::Staged
                        || staged.expires_at_unix_ms <= unix_time_ms()
                    {
                        return Err(AppPersistenceError::Conflict);
                    }
                    let consumed = transaction.execute(
                        "UPDATE login_automation_secret_stages
                         SET state = 'consumed', updated_at_ms = ?1
                         WHERE stage_id = ?2 AND state = 'staged'",
                        params![unix_time_ms(), staged_secret_id.as_str()],
                    )?;
                    if consumed != 1 {
                        return Err(AppPersistenceError::Conflict);
                    }
                    resolved_steps.push(LoginAutomationStepInput::SendSecret {
                        secret_ref_id: LoginAutomationSecretStageId::parse(
                            staged.secret_ref_id.as_str(),
                        )
                        .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                        secret_label: staged.label,
                        append_enter,
                        timeout_seconds,
                    });
                }
                LoginAutomationStepInput::PreserveExistingSecret {
                    existing_ordinal,
                    append_enter,
                    timeout_seconds,
                } => {
                    let (secret_ref_id, secret_label) = transaction
                        .query_row(
                            "SELECT secret_ref_id, secret_label
                             FROM host_login_automation_steps
                             WHERE host_id = ?1 AND ordinal = ?2 AND kind = 'send_secret'",
                            params![host_id.as_str(), i64::from(existing_ordinal)],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .optional()?
                        .ok_or(AppPersistenceError::Conflict)?;
                    resolved_steps.push(LoginAutomationStepInput::SendSecret {
                        secret_ref_id: LoginAutomationSecretStageId::parse(secret_ref_id)
                            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                        secret_label,
                        append_enter,
                        timeout_seconds,
                    });
                }
                step => resolved_steps.push(step.clone()),
            }
        }
        transaction.execute(
            "DELETE FROM host_login_automation_steps WHERE host_id = ?1",
            [host_id.as_str()],
        )?;
        for (ordinal, step) in resolved_steps.iter().enumerate() {
            let encoded = login_automation_step_to_db(step);
            transaction.execute(
                "INSERT INTO host_login_automation_steps
                 (host_id, ordinal, kind, literal_text, append_enter, timeout_seconds,
                  secret_ref_id, secret_label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    host_id.as_str(),
                    usize_to_i64(ordinal)?,
                    encoded.kind,
                    encoded.literal_text,
                    encoded.append_enter,
                    i64::from(encoded.timeout_seconds),
                    encoded.secret_ref_id,
                    encoded.secret_label,
                ],
            )?;
        }
        if transaction.commit().is_err() {
            self.login_automation_secret_stages_poisoned = true;
            return Err(AppPersistenceError::RequiresReload);
        }
        self.get_login_automation(host_id)
    }

    pub fn confirm_login_automation(
        &mut self,
        host_id: &HostId,
        expected_revision: WireSequence,
    ) -> Result<LoginAutomationSummary> {
        let changed = self.connection.execute(
            "UPDATE host_login_automations SET confirmed_revision = revision, updated_at_ms = ?1
             WHERE host_id = ?2 AND revision = ?3 AND enabled = 1",
            params![
                unix_time_ms(),
                host_id.as_str(),
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        self.get_login_automation(host_id)
    }

    pub fn list_known_hosts(&self) -> Result<Vec<KnownHostSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, normalized_address, port, key_algorithm, public_key_base64,
                    fingerprint_sha256, first_trusted_at_ms, last_verified_at_ms, state_version
             FROM known_hosts ORDER BY normalized_address, port, key_algorithm",
        )?;
        statement
            .query_map([], read_known_host)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    pub fn observe_known_host(
        &self,
        address: &str,
        port: u16,
        key_algorithm: &str,
        public_key_blob: &[u8],
    ) -> Result<KnownHostObservation> {
        let observed = observed_host_key(address, port, key_algorithm, public_key_blob)?;
        let existing = lookup_known_host(
            &self.connection,
            &observed.normalized_address,
            observed.port,
            &observed.key_algorithm,
        )?;
        Ok(match existing {
            None => match lookup_any_known_host(
                &self.connection,
                &observed.normalized_address,
                observed.port,
            )? {
                Some(trusted) => KnownHostObservation::AlgorithmChanged { trusted, observed },
                None => KnownHostObservation::Unknown(observed),
            },
            Some(existing) if existing.public_key_base64 == observed.public_key_base64 => {
                KnownHostObservation::Trusted(existing)
            }
            Some(trusted) => KnownHostObservation::Mismatch { trusted, observed },
        })
    }

    pub fn trust_known_host(
        &mut self,
        address: &str,
        port: u16,
        key_algorithm: &str,
        public_key_blob: &[u8],
    ) -> Result<KnownHostSummary> {
        let observed = observed_host_key(address, port, key_algorithm, public_key_blob)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = lookup_known_host(
            &transaction,
            &observed.normalized_address,
            observed.port,
            &observed.key_algorithm,
        )?;
        let summary = match existing {
            Some(existing) if existing.public_key_base64 == observed.public_key_base64 => existing,
            Some(existing) => {
                return Err(AppPersistenceError::KnownHostMismatch {
                    trusted_fingerprint: existing.fingerprint_sha256,
                    observed_fingerprint: observed.fingerprint_sha256,
                });
            }
            None => {
                if let Some(trusted) = lookup_any_known_host(
                    &transaction,
                    &observed.normalized_address,
                    observed.port,
                )? {
                    return Err(AppPersistenceError::KnownHostAlgorithmChanged {
                        trusted_algorithm: trusted.key_algorithm,
                        observed_algorithm: observed.key_algorithm,
                    });
                }
                let now = unix_time_ms();
                let summary = KnownHostSummary {
                    known_host_id: KnownHostId::new(),
                    normalized_address: observed.normalized_address,
                    port: observed.port,
                    key_algorithm: observed.key_algorithm,
                    public_key_base64: observed.public_key_base64,
                    fingerprint_sha256: observed.fingerprint_sha256,
                    first_trusted_at_unix_ms: now,
                    last_verified_at_unix_ms: now,
                    state_version: WireSequence::new(1),
                };
                transaction.execute(
                    "INSERT INTO known_hosts
                     (id, normalized_address, port, key_algorithm, public_key_base64,
                      fingerprint_sha256, first_trusted_at_ms, last_verified_at_ms, state_version)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)",
                    params![
                        summary.known_host_id.as_str(),
                        summary.normalized_address,
                        i64::from(summary.port),
                        summary.key_algorithm,
                        summary.public_key_base64,
                        summary.fingerprint_sha256,
                        now,
                    ],
                )?;
                summary
            }
        };
        transaction.commit()?;
        Ok(summary)
    }

    pub fn record_known_host_verified(
        &mut self,
        address: &str,
        port: u16,
        key_algorithm: &str,
        public_key_blob: &[u8],
    ) -> Result<KnownHostSummary> {
        let observed = observed_host_key(address, port, key_algorithm, public_key_blob)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = lookup_known_host(
            &transaction,
            &observed.normalized_address,
            observed.port,
            &observed.key_algorithm,
        )?;
        let existing = match existing {
            Some(existing) => existing,
            None => {
                return match lookup_any_known_host(
                    &transaction,
                    &observed.normalized_address,
                    observed.port,
                )? {
                    Some(trusted) => Err(AppPersistenceError::KnownHostAlgorithmChanged {
                        trusted_algorithm: trusted.key_algorithm,
                        observed_algorithm: observed.key_algorithm,
                    }),
                    None => Err(AppPersistenceError::NotFound),
                };
            }
        };
        if existing.public_key_base64 != observed.public_key_base64 {
            return Err(AppPersistenceError::KnownHostMismatch {
                trusted_fingerprint: existing.fingerprint_sha256,
                observed_fingerprint: observed.fingerprint_sha256,
            });
        }
        let next_version = existing
            .state_version
            .get()
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?;
        let verified_at = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE known_hosts SET last_verified_at_ms = ?1, state_version = ?2
             WHERE id = ?3 AND state_version = ?4",
            params![
                verified_at,
                u64_to_i64(next_version)?,
                existing.known_host_id.as_str(),
                u64_to_i64(existing.state_version.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(KnownHostSummary {
            last_verified_at_unix_ms: verified_at,
            state_version: WireSequence::new(next_version),
            ..existing
        })
    }

    pub fn delete_known_host(
        &mut self,
        known_host_id: &KnownHostId,
        expected_state_version: WireSequence,
    ) -> Result<KnownHostDeleteResponse> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT id, normalized_address, port, key_algorithm, public_key_base64,
                        fingerprint_sha256, first_trusted_at_ms, last_verified_at_ms, state_version
                 FROM known_hosts WHERE id = ?1",
                [known_host_id.as_str()],
                read_known_host,
            )
            .optional()?;
        let Some(existing) = existing else {
            transaction.commit()?;
            return Ok(KnownHostDeleteResponse {
                known_host_id: known_host_id.clone(),
                deleted: false,
                deleted_known_host: None,
            });
        };
        if existing.state_version != expected_state_version {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = transaction.execute(
            "DELETE FROM known_hosts WHERE id = ?1 AND state_version = ?2",
            params![
                known_host_id.as_str(),
                u64_to_i64(expected_state_version.get())?
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(KnownHostDeleteResponse {
            known_host_id: known_host_id.clone(),
            deleted: true,
            deleted_known_host: Some(existing),
        })
    }

    fn host_exists(&self, host_id: &HostId) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1)",
            [host_id.as_str()],
            |row| row.get(0),
        )?)
    }

    fn identity_exists(&self, identity_id: &IdentityId) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM identities WHERE id = ?1)",
            [identity_id.as_str()],
            |row| row.get(0),
        )?)
    }

    fn ensure_login_automation_secret_stages_usable(&self) -> Result<()> {
        if self.login_automation_secret_stages_poisoned {
            Err(AppPersistenceError::RequiresReload)
        } else {
            Ok(())
        }
    }

    fn ensure_host_create_password_stages_usable(&self) -> Result<()> {
        if self.host_create_password_stages_poisoned {
            Err(AppPersistenceError::RequiresReload)
        } else {
            Ok(())
        }
    }
}

fn validate_ssh_sync_http_upload_completion_proof(
    proof: &SshSyncHttpUploadCompletionProof,
) -> Result<()> {
    match proof {
        SshSyncHttpUploadCompletionProof::ServerAcknowledged {
            acknowledged_revision,
            acknowledged_body_sha256,
        } => {
            if *acknowledged_revision > i64::MAX as u64 {
                return Err(AppPersistenceError::InvalidInput(
                    "acknowledged SSH sync revision exceeds the supported bound",
                ));
            }
            validate_lower_sha256(
                acknowledged_body_sha256,
                "invalid acknowledged SSH sync upload body digest",
            )
        }
        SshSyncHttpUploadCompletionProof::ExactBodyObserved {
            observed_revision,
            observed_body_sha256,
        } => {
            if *observed_revision > i64::MAX as u64 {
                return Err(AppPersistenceError::InvalidInput(
                    "observed SSH sync revision exceeds the supported bound",
                ));
            }
            validate_lower_sha256(
                observed_body_sha256,
                "invalid observed SSH sync upload body digest",
            )
        }
        SshSyncHttpUploadCompletionProof::SupersededByNewerRemote {
            authenticated_remote_revision,
        } if *authenticated_remote_revision <= i64::MAX as u64 => Ok(()),
        SshSyncHttpUploadCompletionProof::SupersededByNewerRemote { .. } => {
            Err(AppPersistenceError::InvalidInput(
                "authenticated SSH sync revision exceeds the supported bound",
            ))
        }
    }
}

fn metadata_table_queries(table: &str) -> Result<(&'static str, &'static str, &'static str)> {
    match table {
        "hosts" => Ok((
            "SELECT state_version FROM hosts WHERE id = ?1",
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1)",
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE label = ?1 COLLATE NOCASE AND id <> ?2)",
        )),
        "host_groups" => Ok((
            "SELECT state_version FROM host_groups WHERE id = ?1",
            "SELECT EXISTS(SELECT 1 FROM host_groups WHERE id = ?1)",
            "SELECT EXISTS(
               SELECT 1 FROM host_groups WHERE label = ?1 COLLATE NOCASE AND id <> ?2
             )",
        )),
        "host_tags" => Ok((
            "SELECT state_version FROM host_tags WHERE id = ?1",
            "SELECT EXISTS(SELECT 1 FROM host_tags WHERE id = ?1)",
            "SELECT EXISTS(
               SELECT 1 FROM host_tags WHERE label = ?1 COLLATE NOCASE AND id <> ?2
             )",
        )),
        _ => Err(AppPersistenceError::InvalidStoredData),
    }
}

fn require_metadata_exists(transaction: &Transaction<'_>, table: &str, id: &str) -> Result<()> {
    if table == "identities" {
        return if metadata_exists(transaction, table, id)? {
            Ok(())
        } else {
            Err(AppPersistenceError::NotFound)
        };
    }
    let (_, exists_query, _) = metadata_table_queries(table)?;
    let exists: bool = transaction.query_row(exists_query, [id], |row| row.get(0))?;
    if exists {
        Ok(())
    } else {
        Err(AppPersistenceError::NotFound)
    }
}

fn require_metadata_version(
    transaction: &Transaction<'_>,
    table: &str,
    id: &str,
    expected_state_version: WireSequence,
) -> Result<()> {
    let (version_query, _, _) = metadata_table_queries(table)?;
    let current = transaction
        .query_row(version_query, [id], |row| read_wire_sequence(row, 0))
        .optional()?;
    match current {
        None => Err(AppPersistenceError::NotFound),
        Some(current) if current != expected_state_version => Err(AppPersistenceError::Conflict),
        Some(_) => Ok(()),
    }
}

fn metadata_label_exists(
    transaction: &Transaction<'_>,
    table: &str,
    label: &str,
    excluded_id: Option<&str>,
) -> Result<bool> {
    let (_, _, duplicate_query) = metadata_table_queries(table)?;
    Ok(transaction.query_row(
        duplicate_query,
        params![label, excluded_id.unwrap_or("")],
        |row| row.get(0),
    )?)
}

fn normalized_host_batch_create_input(
    input: &HostBatchCreateInput,
) -> Result<NormalizedHostBatchCreateInput> {
    let endpoint = Endpoint::parse(&input.address, input.port)?;
    let label = if input.label.trim().is_empty() {
        endpoint.normalized_address().to_owned()
    } else {
        normalized_label(&input.label)?
    };
    let username = normalized_optional(input.username.as_deref(), 128, "username is too long")?;
    let ingress = validated_route_ingress(&input.ingress)?;
    if input.jump_hosts.len() > MAX_JUMP_HOPS {
        return Err(AppPersistenceError::InvalidInput(
            "jump host chain is invalid",
        ));
    }
    let jump_hosts = input
        .jump_hosts
        .iter()
        .map(|jump| {
            let endpoint = Endpoint::parse(&jump.address, jump.port)?;
            Ok(NormalizedHostBatchJumpHostInput {
                address: jump.address.trim().to_owned(),
                normalized_address: endpoint.normalized_address().to_owned(),
                port: jump.port,
                username: normalized_optional(
                    jump.username.as_deref(),
                    128,
                    "username is too long",
                )?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(NormalizedHostBatchCreateInput {
        label,
        address: input.address.trim().to_owned(),
        normalized_address: endpoint.normalized_address().to_owned(),
        port: input.port,
        username,
        ingress,
        jump_hosts,
    })
}

fn validate_batch_jump_hosts(inputs: &[NormalizedHostBatchCreateInput]) -> Result<()> {
    let mut shared_hops = BTreeMap::<(String, u16), Option<String>>::new();
    for input in inputs {
        let mut route_hops = BTreeSet::new();
        for jump in &input.jump_hosts {
            let key = (jump.normalized_address.clone(), jump.port);
            if key == (input.normalized_address.clone(), input.port) {
                return Err(AppPersistenceError::InvalidInput(
                    "jump host chain cannot route back to its target endpoint",
                ));
            }
            if !route_hops.insert(key.clone()) {
                return Err(AppPersistenceError::InvalidInput(
                    "jump host chain contains a repeated endpoint",
                ));
            }
            if let Some(existing_username) = shared_hops.get(&key) {
                if existing_username != &jump.username {
                    return Err(AppPersistenceError::InvalidInput(
                        "shared jump endpoint has conflicting usernames",
                    ));
                }
            } else {
                shared_hops.insert(key, jump.username.clone());
            }
        }
    }
    Ok(())
}

fn validate_portable_object_id(value: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
        AppPersistenceError::InvalidInput("SSH sync portable object id must be a canonical UUID")
    })?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync portable object id must be a canonical UUID",
        ));
    }
    Ok(())
}

fn normalized_desktop_profile_id(value: &str) -> Result<String> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
        AppPersistenceError::InvalidInput("SSH sync desktop profile id must be a canonical UUID")
    })?;
    let normalized = parsed.to_string();
    if parsed.is_nil() || normalized != value {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync desktop profile id must be a canonical UUID",
        ));
    }
    Ok(normalized)
}

fn validate_ssh_sync_owned_delta(delta: &SshSyncOwnedMetadataDelta) -> Result<()> {
    validate_lower_sha256(&delta.delta_sha256, "invalid SSH sync delta digest")?;
    let create_total = delta.creates.as_ref().map_or(0, |batch| {
        batch.plan.identities.len()
            + batch.plan.credentials.len()
            + batch.plan.hosts.len()
            + batch.plan.desktop_profiles.len()
    });
    let total = create_total
        + delta.identity_updates.len()
        + delta.identity_deletes.len()
        + delta.credential_updates.len()
        + delta.credential_deletes.len()
        + delta.host_updates.len()
        + delta.host_deletes.len()
        + delta.desktop_profile_updates.len()
        + delta.desktop_profile_deletes.len()
        + delta.secret_replacements.len()
        + delta.secret_deletes.len();
    if total == 0 || total > MAX_SSH_SYNC_OWNED_DELTA_OBJECTS {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync owned delta is empty or too large",
        ));
    }
    let mut portable = BTreeSet::new();
    let mut local = BTreeSet::new();
    let mut add = |kind: SshSyncObjectKind, portable_id: &str, local_id: &str| -> Result<()> {
        validate_portable_object_id(portable_id)?;
        if !portable.insert((kind, portable_id.to_owned()))
            || !local.insert((kind, local_id.to_owned()))
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync owned delta contains duplicate objects",
            ));
        }
        Ok(())
    };
    for value in &delta.identity_updates {
        normalized_label(&value.label)?;
        normalized_optional(value.username.as_deref(), 256, "username is too long")?;
        add(
            SshSyncObjectKind::Identity,
            &value.portable_object_id,
            value.identity_id.as_str(),
        )?;
    }
    for value in &delta.identity_deletes {
        add(
            SshSyncObjectKind::Identity,
            &value.portable_object_id,
            value.identity_id.as_str(),
        )?;
    }
    for value in &delta.credential_updates {
        normalized_label(&value.label)?;
        normalized_owned_credential_material(&value.material)?;
        add(
            SshSyncObjectKind::Credential,
            &value.portable_object_id,
            value.credential_ref_id.as_str(),
        )?;
    }
    for value in &delta.credential_deletes {
        add(
            SshSyncObjectKind::Credential,
            &value.portable_object_id,
            value.credential_ref_id.as_str(),
        )?;
    }
    for value in &delta.host_updates {
        if value.desired.staged_password_id.is_some() || !value.desired.tag_ids.is_empty() {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync Host update must use safe sync-owned metadata",
            ));
        }
        normalized_sync_tag_labels(&value.tag_labels)?;
        normalized_configured_host_create(&value.desired)?;
        normalized_ssh_sync_login_automation(&value.login_automation)?;
        add(
            SshSyncObjectKind::Host,
            &value.portable_object_id,
            value.host_id.as_str(),
        )?;
    }
    for value in &delta.host_deletes {
        add(
            SshSyncObjectKind::Host,
            &value.portable_object_id,
            value.host_id.as_str(),
        )?;
    }
    for value in &delta.desktop_profile_updates {
        validate_desktop_profile(&value.profile)?;
        normalized_desktop_profile_id(&value.profile.id)?;
        add(
            SshSyncObjectKind::DesktopProfile,
            &value.portable_object_id,
            value.profile.id.as_str(),
        )?;
    }
    for value in &delta.desktop_profile_deletes {
        let profile_id = normalized_desktop_profile_id(&value.profile_id)?;
        add(
            SshSyncObjectKind::DesktopProfile,
            &value.portable_object_id,
            &profile_id,
        )?;
    }
    for value in &delta.secret_deletes {
        add(
            SshSyncObjectKind::Secret,
            &value.portable_object_id,
            value.secret_ref_id.as_str(),
        )?;
    }
    for value in &delta.secret_replacements {
        if value.expected_secret_ref_id == value.new_secret_ref_id {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync Secret replacement must use a new Vault ref",
            ));
        }
        add(
            SshSyncObjectKind::Secret,
            &value.portable_object_id,
            value.expected_secret_ref_id.as_str(),
        )?;
        let desired_uses = delta
            .credential_updates
            .iter()
            .flat_map(|update| owned_material_secret_refs(&update.material))
            .chain(delta.host_updates.iter().flat_map(|update| {
                ssh_sync_login_automation_secret_refs(&update.login_automation)
                    .into_iter()
                    .cloned()
            }))
            .filter(|candidate| candidate == &value.new_secret_ref_id)
            .count();
        if desired_uses == 0 {
            return Err(AppPersistenceError::InvalidInput(
                "new SSH sync Secret ref must be used by the delta",
            ));
        }
    }
    Ok(())
}

fn normalized_owned_credential_material(
    material: &SshSyncRestoreCredentialMaterial,
) -> Result<SshSyncRestoreCredentialMaterial> {
    match material {
        SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => {
            Ok(SshSyncRestoreCredentialMaterial::Password {
                secret_ref_id: secret_ref_id.clone(),
            })
        }
        SshSyncRestoreCredentialMaterial::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            public_key_algorithm,
            public_key_fingerprint,
        } => {
            let (algorithm, fingerprint) = normalized_public_key_metadata(
                Some(public_key_algorithm),
                Some(public_key_fingerprint),
            )?;
            Ok(SshSyncRestoreCredentialMaterial::PrivateKey {
                secret_ref_id: secret_ref_id.clone(),
                passphrase_secret_ref_id: passphrase_secret_ref_id.clone(),
                public_key_algorithm: algorithm.ok_or(AppPersistenceError::InvalidInput(
                    "private key credentials require public key metadata",
                ))?,
                public_key_fingerprint: fingerprint.ok_or(AppPersistenceError::InvalidInput(
                    "private key credentials require public key metadata",
                ))?,
            })
        }
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds }
            if (1..=32).contains(max_rounds) =>
        {
            Ok(SshSyncRestoreCredentialMaterial::KeyboardInteractive {
                max_rounds: *max_rounds,
            })
        }
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => Err(
            AppPersistenceError::InvalidInput("keyboard-interactive max rounds are invalid"),
        ),
    }
}

fn current_ssh_sync_change_fence(connection: &Connection) -> Result<SshSyncChangeFence> {
    let database_data_version =
        connection.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))?;
    Ok(SshSyncChangeFence {
        connection_total_changes: connection.total_changes(),
        database_data_version: u64::try_from(database_data_version)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
    })
}

fn require_ssh_sync_change_fence(
    connection: &Connection,
    expected: &SshSyncChangeFence,
) -> Result<()> {
    if current_ssh_sync_change_fence(connection)? == *expected {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn require_owned_mapping(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    kind: SshSyncObjectKind,
    portable_object_id: &str,
    local_object_id: &str,
) -> Result<()> {
    validate_portable_object_id(portable_object_id)?;
    let exact: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM ssh_sync_object_mappings
         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
           AND object_kind = ?4 AND portable_object_id = ?5 AND local_object_id = ?6)",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            owner.profile_id,
            ssh_sync_object_kind_to_db(kind),
            portable_object_id,
            local_object_id,
        ],
        |row| row.get(0),
    )?;
    if exact {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn require_exclusive_owned_mapping(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    kind: SshSyncObjectKind,
    portable_object_id: &str,
    local_object_id: &str,
) -> Result<()> {
    require_owned_mapping(connection, owner, kind, portable_object_id, local_object_id)?;
    let mapped_by_another_owner: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM ssh_sync_object_mappings
         WHERE object_kind = ?1 AND local_object_id = ?2
           AND (plugin_id != ?3 OR signer_fingerprint_sha256 != ?4 OR profile_id != ?5))",
        params![
            ssh_sync_object_kind_to_db(kind),
            local_object_id,
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            owner.profile_id,
        ],
        |row| row.get(0),
    )?;
    if mapped_by_another_owner {
        Err(AppPersistenceError::Conflict)
    } else {
        Ok(())
    }
}

fn preflight_owned_delta(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    delta: &SshSyncOwnedMetadataDelta,
) -> Result<()> {
    for value in &delta.identity_updates {
        require_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Identity,
            &value.portable_object_id,
            value.identity_id.as_str(),
        )?;
        require_row_version(
            connection,
            "identities",
            value.identity_id.as_str(),
            value.expected_state_version,
        )?;
    }
    for value in &delta.identity_deletes {
        require_exclusive_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Identity,
            &value.portable_object_id,
            value.identity_id.as_str(),
        )?;
        require_row_version(
            connection,
            "identities",
            value.identity_id.as_str(),
            value.expected_state_version,
        )?;
    }
    for value in &delta.credential_updates {
        require_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Credential,
            &value.portable_object_id,
            value.credential_ref_id.as_str(),
        )?;
        require_row_version(
            connection,
            "credential_refs",
            value.credential_ref_id.as_str(),
            value.expected_state_version,
        )?;
    }
    for value in &delta.credential_deletes {
        require_exclusive_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Credential,
            &value.portable_object_id,
            value.credential_ref_id.as_str(),
        )?;
        require_row_version(
            connection,
            "credential_refs",
            value.credential_ref_id.as_str(),
            value.expected_state_version,
        )?;
    }
    for value in &delta.host_updates {
        require_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Host,
            &value.portable_object_id,
            value.host_id.as_str(),
        )?;
        require_host_versions(connection, &value.host_id, &value.expected)?;
        let mut statement = connection.prepare(
            "SELECT secret_ref_id FROM host_login_automation_steps
             WHERE host_id = ?1 AND kind = 'send_secret' ORDER BY ordinal",
        )?;
        let existing_refs = statement
            .query_map([value.host_id.as_str()], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<BTreeSet<_>, _>>()?;
        for secret_ref in ssh_sync_login_automation_secret_refs(&value.login_automation) {
            if !existing_refs.contains(secret_ref.as_str())
                && !delta
                    .secret_replacements
                    .iter()
                    .any(|replacement| replacement.new_secret_ref_id == *secret_ref)
            {
                return Err(AppPersistenceError::InvalidInput(
                    "new SSH sync automation Secret ref requires an owned replacement",
                ));
            }
        }
    }
    for value in &delta.host_deletes {
        require_exclusive_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Host,
            &value.portable_object_id,
            value.host_id.as_str(),
        )?;
        require_host_versions(connection, &value.host_id, &value.expected)?;
    }
    for value in &delta.desktop_profile_updates {
        require_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::DesktopProfile,
            &value.portable_object_id,
            value.profile.id.as_str(),
        )?;
        require_desktop_profile_revision(connection, &value.profile.id, value.expected_revision)?;
    }
    for value in &delta.desktop_profile_deletes {
        require_exclusive_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::DesktopProfile,
            &value.portable_object_id,
            &value.profile_id,
        )?;
        require_desktop_profile_revision(connection, &value.profile_id, value.expected_revision)?;
    }
    for value in &delta.secret_deletes {
        require_exclusive_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Secret,
            &value.portable_object_id,
            value.secret_ref_id.as_str(),
        )?;
    }
    for value in &delta.secret_replacements {
        require_owned_mapping(
            connection,
            owner,
            SshSyncObjectKind::Secret,
            &value.portable_object_id,
            value.expected_secret_ref_id.as_str(),
        )?;
        let new_is_used: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM credential_secret_slots WHERE secret_ref_id = ?1)
              OR EXISTS(SELECT 1 FROM host_login_automation_steps WHERE secret_ref_id = ?1)
              OR EXISTS(SELECT 1 FROM ssh_sync_object_mappings WHERE local_object_id = ?1)",
            [value.new_secret_ref_id.as_str()],
            |row| row.get(0),
        )?;
        if new_is_used {
            return Err(AppPersistenceError::Conflict);
        }
        let owned_credentials = {
            let mut statement = connection.prepare(
                "SELECT DISTINCT mapping.local_object_id
                 FROM ssh_sync_object_mappings AS mapping
                 JOIN credential_secret_slots AS slot
                   ON slot.credential_ref_id = mapping.local_object_id
                 WHERE mapping.plugin_id = ?1
                   AND mapping.signer_fingerprint_sha256 = ?2
                   AND mapping.profile_id = ?3
                   AND mapping.object_kind = 'credential'
                   AND slot.secret_ref_id = ?4
                 ORDER BY mapping.local_object_id",
            )?;
            statement
                .query_map(
                    params![
                        owner.plugin_id.as_str(),
                        owner.signer_fingerprint_sha256,
                        owner.profile_id,
                        value.expected_secret_ref_id.as_str()
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for credential_id in owned_credentials {
            let migrated = delta.credential_updates.iter().any(|update| {
                update.credential_ref_id.as_str() == credential_id
                    && owned_material_secret_refs(&update.material)
                        .contains(&value.new_secret_ref_id)
            });
            if !migrated {
                return Err(AppPersistenceError::Conflict);
            }
        }
        let owned_hosts = {
            let mut statement = connection.prepare(
                "SELECT DISTINCT mapping.local_object_id
                 FROM ssh_sync_object_mappings AS mapping
                 JOIN host_login_automation_steps AS step
                   ON step.host_id = mapping.local_object_id
                 WHERE mapping.plugin_id = ?1
                   AND mapping.signer_fingerprint_sha256 = ?2
                   AND mapping.profile_id = ?3
                   AND mapping.object_kind = 'host'
                   AND step.secret_ref_id = ?4
                 ORDER BY mapping.local_object_id",
            )?;
            statement
                .query_map(
                    params![
                        owner.plugin_id.as_str(),
                        owner.signer_fingerprint_sha256,
                        owner.profile_id,
                        value.expected_secret_ref_id.as_str()
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for host_id in owned_hosts {
            let migrated = delta.host_updates.iter().any(|update| {
                update.host_id.as_str() == host_id
                    && ssh_sync_login_automation_secret_refs(&update.login_automation)
                        .contains(&&value.new_secret_ref_id)
            });
            if !migrated {
                return Err(AppPersistenceError::Conflict);
            }
        }
    }
    Ok(())
}

fn preflight_owned_create_mappings(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    plan: &NormalizedRestorePlan,
    mappings: &[NormalizedSshSyncObjectMappingInput],
) -> Result<()> {
    let mut required = BTreeSet::new();
    for identity in &plan.identities {
        required.insert((
            SshSyncObjectKind::Identity,
            identity.identity_id.as_str().to_owned(),
        ));
    }
    for credential in &plan.credentials {
        required.insert((
            SshSyncObjectKind::Credential,
            credential.credential_ref_id.as_str().to_owned(),
        ));
        for secret_ref in restore_credential_secret_refs(credential) {
            required.insert((SshSyncObjectKind::Secret, secret_ref.as_str().to_owned()));
        }
    }
    for (host_id, _, _, _, automation) in &plan.hosts {
        required.insert((SshSyncObjectKind::Host, host_id.as_str().to_owned()));
        for secret_ref in ssh_sync_login_automation_secret_refs(automation) {
            required.insert((SshSyncObjectKind::Secret, secret_ref.as_str().to_owned()));
        }
    }
    for desktop in &plan.desktop_profiles {
        required.insert((
            SshSyncObjectKind::DesktopProfile,
            desktop.profile.id.clone(),
        ));
    }
    let provided = mappings
        .iter()
        .map(|mapping| {
            (
                mapping.object_kind,
                mapping.local_object_id.as_str().to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    if required != provided {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync create mappings do not exactly cover the create plan",
        ));
    }
    for desktop in &plan.desktop_profiles {
        let exact = mappings.iter().any(|mapping| {
            mapping.object_kind == SshSyncObjectKind::DesktopProfile
                && mapping.portable_object_id == desktop.portable_object_id
                && mapping.local_object_id.as_str() == desktop.profile.id
        });
        if !exact {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync desktop create mapping does not match the restore plan",
            ));
        }
    }
    for mapping in mappings {
        let existing = lookup_ssh_sync_object_mapping(connection, owner, mapping)?;
        if !existing.is_empty()
            && !(existing.len() == 1
                && existing[0].portable_object_id == mapping.portable_object_id
                && existing[0].local_object_id == mapping.local_object_id)
        {
            return Err(AppPersistenceError::Conflict);
        }
    }
    Ok(())
}

fn apply_owned_create_mappings(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    mappings: &[NormalizedSshSyncObjectMappingInput],
    now: i64,
) -> Result<()> {
    for mapping in mappings {
        if !lookup_ssh_sync_object_mapping(transaction, owner, mapping)?.is_empty() {
            continue;
        }
        transaction.execute(
            "INSERT INTO ssh_sync_object_mappings
             (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
              portable_object_id, local_object_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                ssh_sync_object_kind_to_db(mapping.object_kind),
                mapping.portable_object_id,
                mapping.local_object_id.as_str(),
                now,
            ],
        )?;
    }
    Ok(())
}

fn require_row_version(
    connection: &Connection,
    table: &str,
    id: &str,
    expected: WireSequence,
) -> Result<()> {
    let table = match table {
        "hosts" => "hosts",
        "identities" => "identities",
        "credential_refs" => "credential_refs",
        _ => {
            return Err(AppPersistenceError::InvalidInput(
                "invalid sync metadata table",
            ));
        }
    };
    let version = connection
        .query_row(
            &format!("SELECT state_version FROM {table} WHERE id = ?1"),
            [id],
            |row| read_wire_sequence(row, 0),
        )
        .optional()?;
    if version == Some(expected) {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn require_desktop_profile_revision(
    connection: &Connection,
    profile_id: &str,
    expected: WireSequence,
) -> Result<()> {
    let profile_id = normalized_desktop_profile_id(profile_id)?;
    let revision = connection
        .query_row(
            "SELECT revision FROM desktop_profiles WHERE id = ?1",
            [profile_id.as_str()],
            |row| read_wire_sequence(row, 0),
        )
        .optional()?;
    if revision == Some(expected) {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn require_host_versions(
    connection: &Connection,
    host_id: &HostId,
    expected: &SshSyncOwnedHostBaseVersions,
) -> Result<()> {
    let versions = connection
        .query_row(
            "SELECT hosts.state_version, host_route_plans.revision,
                    host_authentication_plans.revision, host_algorithm_policies.revision,
                    host_heartbeat_policies.revision, host_monitoring_policies.revision,
                    host_login_automations.revision
             FROM hosts
             JOIN host_route_plans ON host_route_plans.host_id = hosts.id
             JOIN host_authentication_plans ON host_authentication_plans.host_id = hosts.id
             JOIN host_algorithm_policies ON host_algorithm_policies.host_id = hosts.id
             JOIN host_heartbeat_policies ON host_heartbeat_policies.host_id = hosts.id
             JOIN host_monitoring_policies ON host_monitoring_policies.host_id = hosts.id
             JOIN host_login_automations ON host_login_automations.host_id = hosts.id
             WHERE hosts.id = ?1",
            [host_id.as_str()],
            |row| {
                Ok(SshSyncOwnedHostBaseVersions {
                    host: read_wire_sequence(row, 0)?,
                    route: read_wire_sequence(row, 1)?,
                    authentication: read_wire_sequence(row, 2)?,
                    algorithm: read_wire_sequence(row, 3)?,
                    heartbeat: read_wire_sequence(row, 4)?,
                    monitoring: read_wire_sequence(row, 5)?,
                    login_automation: read_wire_sequence(row, 6)?,
                })
            },
        )
        .optional()?;
    if versions.as_ref() == Some(expected) {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn apply_owned_identity_update(
    transaction: &Transaction<'_>,
    update: &SshSyncOwnedIdentityUpdate,
    now: i64,
) -> Result<()> {
    let label = normalized_label(&update.label)?;
    let username = normalized_optional(update.username.as_deref(), 256, "username is too long")?;
    let next = next_revision(update.expected_state_version)?;
    let changed = transaction.execute(
        "UPDATE identities SET label = ?1, username = ?2, state_version = ?3, updated_at_ms = ?4
         WHERE id = ?5 AND state_version = ?6",
        params![
            label,
            username,
            u64_to_i64(next)?,
            now,
            update.identity_id.as_str(),
            u64_to_i64(update.expected_state_version.get())?,
        ],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn get_credential_record_connection(
    connection: &Connection,
    credential_ref_id: &CredentialRefId,
) -> Result<CredentialRecord> {
    connection
        .query_row(
            &format!(
                "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
                 {CREDENTIAL_RECORD_JOINS} WHERE credential_refs.id = ?1"
            ),
            [credential_ref_id.as_str()],
            read_credential_record,
        )
        .optional()?
        .ok_or(AppPersistenceError::Conflict)
}

fn credential_record_secret_refs(record: &CredentialRecord) -> Vec<SecretRefId> {
    match &record.details {
        CredentialRecordDetails::Password { secret_ref_id } => vec![secret_ref_id.clone()],
        CredentialRecordDetails::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            ..
        } => std::iter::once(secret_ref_id.clone())
            .chain(passphrase_secret_ref_id.clone())
            .collect(),
        _ => Vec::new(),
    }
}

fn owned_material_secret_refs(material: &SshSyncRestoreCredentialMaterial) -> Vec<SecretRefId> {
    match material {
        SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => {
            vec![secret_ref_id.clone()]
        }
        SshSyncRestoreCredentialMaterial::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            ..
        } => std::iter::once(secret_ref_id.clone())
            .chain(passphrase_secret_ref_id.clone())
            .collect(),
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => Vec::new(),
    }
}

fn apply_owned_credential_update(
    transaction: &Transaction<'_>,
    update: &SshSyncOwnedCredentialUpdate,
    now: i64,
) -> Result<Vec<SecretRefId>> {
    let existing = get_credential_record_connection(transaction, &update.credential_ref_id)?;
    if existing.state_version != update.expected_state_version {
        return Err(AppPersistenceError::Conflict);
    }
    let material = normalized_owned_credential_material(&update.material)?;
    let label = normalized_label(&update.label)?;
    if !metadata_exists(transaction, "identities", update.identity_id.as_str())? {
        return Err(AppPersistenceError::Conflict);
    }
    let priority_conflict: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM credential_refs
         WHERE identity_id = ?1 AND priority = ?2 AND id <> ?3)",
        params![
            update.identity_id.as_str(),
            i64::from(update.priority),
            update.credential_ref_id.as_str(),
        ],
        |row| row.get(0),
    )?;
    if priority_conflict {
        return Err(AppPersistenceError::Conflict);
    }
    let invalid_host_identity: bool = transaction.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM host_authentication_credentials AS selected
           JOIN hosts ON hosts.id = selected.host_id
           WHERE selected.credential_ref_id = ?1 AND hosts.identity_id <> ?2
         )",
        params![
            update.credential_ref_id.as_str(),
            update.identity_id.as_str()
        ],
        |row| row.get(0),
    )?;
    if invalid_host_identity {
        return Err(AppPersistenceError::Conflict);
    }
    let proxy_references: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_route_plans WHERE proxy_auth_credential_ref_id = ?1)",
        [update.credential_ref_id.as_str()],
        |row| row.get(0),
    )?;
    if proxy_references && !matches!(material, SshSyncRestoreCredentialMaterial::Password { .. }) {
        return Err(AppPersistenceError::Conflict);
    }
    let desktop_references: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM desktop_profiles WHERE credential_ref_id = ?1)",
        [update.credential_ref_id.as_str()],
        |row| row.get(0),
    )?;
    if desktop_references && !matches!(material, SshSyncRestoreCredentialMaterial::Password { .. })
    {
        return Err(AppPersistenceError::Conflict);
    }
    let old_refs = credential_record_secret_refs(&existing);
    let new_refs = owned_material_secret_refs(&material);
    transaction.execute(
        "DELETE FROM credential_secret_slots WHERE credential_ref_id = ?1",
        [update.credential_ref_id.as_str()],
    )?;
    transaction.execute(
        "DELETE FROM credential_password_details WHERE credential_ref_id = ?1",
        [update.credential_ref_id.as_str()],
    )?;
    transaction.execute(
        "DELETE FROM credential_private_key_details WHERE credential_ref_id = ?1",
        [update.credential_ref_id.as_str()],
    )?;
    transaction.execute(
        "DELETE FROM credential_keyboard_interactive_details WHERE credential_ref_id = ?1",
        [update.credential_ref_id.as_str()],
    )?;
    let kind = match material {
        SshSyncRestoreCredentialMaterial::Password { .. } => "password",
        SshSyncRestoreCredentialMaterial::PrivateKey { .. } => "private_key",
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => "keyboard_interactive",
    };
    let next = next_revision(update.expected_state_version)?;
    let changed = transaction.execute(
        "UPDATE credential_refs SET identity_id = ?1, kind = ?2, priority = ?3, label = ?4,
                state_version = ?5, updated_at_ms = ?6, import_state = 'ready'
         WHERE id = ?7 AND state_version = ?8",
        params![
            update.identity_id.as_str(),
            kind,
            i64::from(update.priority),
            label,
            u64_to_i64(next)?,
            now,
            update.credential_ref_id.as_str(),
            u64_to_i64(update.expected_state_version.get())?,
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::Conflict);
    }
    insert_owned_credential_details(transaction, &update.credential_ref_id, &material)?;
    Ok(old_refs
        .into_iter()
        .filter(|old| !new_refs.contains(old))
        .collect())
}

fn insert_owned_credential_details(
    transaction: &Transaction<'_>,
    credential_ref_id: &CredentialRefId,
    material: &SshSyncRestoreCredentialMaterial,
) -> Result<()> {
    match material {
        SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => {
            transaction.execute(
                "INSERT INTO credential_password_details (credential_ref_id) VALUES (?1)",
                [credential_ref_id.as_str()],
            )?;
            insert_restore_secret_slot(
                transaction,
                credential_ref_id,
                0,
                "password",
                secret_ref_id,
                "Password",
            )?;
        }
        SshSyncRestoreCredentialMaterial::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            public_key_algorithm,
            public_key_fingerprint,
        } => {
            transaction.execute(
                "INSERT INTO credential_private_key_details
                 (credential_ref_id, public_key_algorithm, public_key_fingerprint)
                 VALUES (?1, ?2, ?3)",
                params![
                    credential_ref_id.as_str(),
                    public_key_algorithm,
                    public_key_fingerprint,
                ],
            )?;
            insert_restore_secret_slot(
                transaction,
                credential_ref_id,
                0,
                "private_key",
                secret_ref_id,
                "Private key",
            )?;
            if let Some(passphrase) = passphrase_secret_ref_id {
                insert_restore_secret_slot(
                    transaction,
                    credential_ref_id,
                    1,
                    "passphrase",
                    passphrase,
                    "Passphrase",
                )?;
            }
        }
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds } => {
            transaction.execute(
                "INSERT INTO credential_keyboard_interactive_details
                 (credential_ref_id, max_rounds) VALUES (?1, ?2)",
                params![credential_ref_id.as_str(), i64::from(*max_rounds)],
            )?;
        }
    }
    Ok(())
}

fn apply_owned_host_update(
    transaction: &Transaction<'_>,
    update: &SshSyncOwnedHostUpdate,
    now: i64,
) -> Result<Vec<SecretRefId>> {
    let mut normalized = normalized_configured_host_create(&update.desired)?;
    if normalized.staged_password_id.is_some() {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync Host update cannot consume a password stage",
        ));
    }
    if update.desired.login_automation_enabled
        || update.desired.login_automation_confirmed
        || !update.desired.login_automation_steps.is_empty()
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync Host update automation must use the internal typed input",
        ));
    }
    let login_automation = normalized_ssh_sync_login_automation(&update.login_automation)?;
    normalized.tag_ids = resolve_sync_tag_labels(
        transaction,
        &normalized_sync_tag_labels(&update.tag_labels)?,
        now,
    )?;
    if let Some(identity_id) = &normalized.identity_id
        && !metadata_exists(transaction, "identities", identity_id.as_str())?
    {
        return Err(AppPersistenceError::Conflict);
    }
    if let Some(group_id) = &normalized.group_id {
        require_metadata_exists(transaction, "host_groups", group_id.as_str())?;
    }
    for tag_id in &normalized.tag_ids {
        require_metadata_exists(transaction, "host_tags", tag_id.as_str())?;
    }
    let mut statement = transaction.prepare(
        "SELECT secret_ref_id FROM host_login_automation_steps
         WHERE host_id = ?1 AND kind = 'send_secret' ORDER BY ordinal",
    )?;
    let removed_secret_refs = statement
        .query_map([update.host_id.as_str()], |row| {
            parse_id(row.get::<_, String>(0)?, SecretRefId::parse)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let next_host = next_revision(update.expected.host)?;
    let changed = transaction.execute(
        "UPDATE hosts SET label = ?1, address = ?2, normalized_address = ?3, port = ?4,
                username = ?5, identity_id = ?6, favorite = ?7, group_id = ?8,
                state_version = ?9, updated_at_ms = ?10
         WHERE id = ?11 AND state_version = ?12",
        params![
            normalized.label,
            normalized.address,
            normalized.normalized_address,
            i64::from(normalized.port),
            normalized.username,
            normalized.identity_id.as_ref().map(IdentityId::as_str),
            normalized.favorite,
            normalized.group_id.as_ref().map(HostGroupId::as_str),
            u64_to_i64(next_host)?,
            now,
            update.host_id.as_str(),
            u64_to_i64(update.expected.host.get())?,
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::Conflict);
    }
    transaction.execute(
        "DELETE FROM host_tag_assignments WHERE host_id = ?1",
        [update.host_id.as_str()],
    )?;
    for tag_id in &normalized.tag_ids {
        transaction.execute(
            "INSERT INTO host_tag_assignments (host_id, tag_id) VALUES (?1, ?2)",
            params![update.host_id.as_str(), tag_id.as_str()],
        )?;
    }
    for table in [
        "host_route_jump_hops",
        "host_authentication_credentials",
        "host_algorithm_exceptions",
        "host_monitoring_selections",
        "host_login_automation_steps",
        "host_route_plans",
        "host_authentication_plans",
        "host_algorithm_policies",
        "host_heartbeat_policies",
        "host_monitoring_policies",
        "host_login_automations",
    ] {
        transaction.execute(
            &format!("DELETE FROM {table} WHERE host_id = ?1"),
            [update.host_id.as_str()],
        )?;
    }
    insert_host_config_defaults(transaction, &update.host_id, now)?;
    initialize_configured_route_plan(
        transaction,
        &update.host_id,
        &normalized.ingress,
        &normalized.jump_host_ids,
        now,
    )?;
    initialize_configured_authentication_plan(
        transaction,
        &update.host_id,
        normalized.identity_id.as_ref(),
        normalized.authentication_mode,
        &normalized.credential_ref_ids,
        now,
    )?;
    initialize_configured_algorithm_policy(
        transaction,
        &update.host_id,
        &normalized.algorithm_policy_id,
        &normalized.compatibility_exceptions,
        now,
    )?;
    initialize_configured_heartbeat_policy(
        transaction,
        &update.host_id,
        &normalized.heartbeat_policy,
        now,
    )?;
    initialize_configured_monitoring_policy(
        transaction,
        &update.host_id,
        &normalized.monitoring_policy,
        now,
    )?;
    initialize_ssh_sync_login_automation(transaction, &update.host_id, &login_automation, now)?;
    for (table, expected) in [
        ("host_route_plans", update.expected.route),
        ("host_authentication_plans", update.expected.authentication),
        ("host_algorithm_policies", update.expected.algorithm),
        ("host_heartbeat_policies", update.expected.heartbeat),
        ("host_monitoring_policies", update.expected.monitoring),
        ("host_login_automations", update.expected.login_automation),
    ] {
        let next = next_revision(expected)?;
        transaction.execute(
            &format!("UPDATE {table} SET revision = ?1, updated_at_ms = ?2 WHERE host_id = ?3"),
            params![u64_to_i64(next)?, now, update.host_id.as_str()],
        )?;
    }
    let login_next = next_revision(update.expected.login_automation)?;
    transaction.execute(
        "UPDATE host_login_automations SET confirmed_revision = ?1 WHERE host_id = ?2",
        params![
            if login_automation.confirmed {
                Some(u64_to_i64(login_next)?)
            } else {
                None
            },
            update.host_id.as_str(),
        ],
    )?;
    Ok(removed_secret_refs)
}

fn desktop_profile_references_valid(
    connection: &Connection,
    profile: &DesktopProfile,
) -> Result<bool> {
    for host_id in [&profile.host_id, &profile.gateway_host_id]
        .into_iter()
        .flatten()
    {
        if !metadata_exists(connection, "hosts", host_id.as_str())? {
            return Ok(false);
        }
    }
    let Some(credential_ref_id) = &profile.credential_ref_id else {
        return Ok(true);
    };
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM credential_refs
             WHERE id = ?1 AND kind = 'password' AND import_state = 'ready')",
            [credential_ref_id.as_str()],
            |row| row.get(0),
        )
        .map_err(AppPersistenceError::from)
}

fn apply_owned_desktop_profile_update(
    transaction: &Transaction<'_>,
    update: &SshSyncOwnedDesktopProfileUpdate,
) -> Result<()> {
    validate_desktop_profile(&update.profile)?;
    let mut profile = update.profile.clone();
    profile.id = normalized_desktop_profile_id(&profile.id)?;
    if !desktop_profile_references_valid(transaction, &profile)? {
        return Err(AppPersistenceError::Conflict);
    }
    let next = next_revision(update.expected_revision)?;
    profile.revision = WireSequence::new(next);
    let json = serde_json::to_string(&profile)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
    if json.len() > 8192 {
        return Err(AppPersistenceError::InvalidInput("invalid desktop profile"));
    }
    let changed = transaction.execute(
        "UPDATE desktop_profiles
         SET profile_json = ?1, revision = ?2, host_id = ?3,
             gateway_host_id = ?4, credential_ref_id = ?5
         WHERE id = ?6 AND revision = ?7",
        params![
            json,
            u64_to_i64(next)?,
            profile.host_id.as_ref().map(HostId::as_str),
            profile.gateway_host_id.as_ref().map(HostId::as_str),
            profile
                .credential_ref_id
                .as_ref()
                .map(CredentialRefId::as_str),
            profile.id,
            u64_to_i64(update.expected_revision.get())?,
        ],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn delete_owned_desktop_profile(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    delete: &SshSyncOwnedDesktopProfileDelete,
) -> Result<()> {
    let profile_id = normalized_desktop_profile_id(&delete.profile_id)?;
    let changed = transaction.execute(
        "DELETE FROM desktop_profiles WHERE id = ?1 AND revision = ?2",
        params![profile_id, u64_to_i64(delete.expected_revision.get())?,],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::Conflict);
    }
    delete_owned_mapping(
        transaction,
        owner,
        SshSyncObjectKind::DesktopProfile,
        &delete.portable_object_id,
        &delete.profile_id,
    )
}

fn delete_owned_hosts(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    deletes: &[SshSyncOwnedHostDelete],
) -> Result<()> {
    let mut remaining = deletes.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let mut position = None;
        for (index, delete) in remaining.iter().enumerate() {
            let desktop_references: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM desktop_profiles
                 WHERE host_id = ?1 OR gateway_host_id = ?1)",
                [delete.host_id.as_str()],
                |row| row.get(0),
            )?;
            if desktop_references {
                return Err(AppPersistenceError::Conflict);
            }
            let unreferenced: bool = transaction.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM host_route_jump_hops WHERE jump_host_id = ?1)",
                [delete.host_id.as_str()],
                |row| row.get(0),
            )?;
            if unreferenced {
                position = Some(index);
                break;
            }
        }
        let Some(position) = position else {
            return Err(AppPersistenceError::Conflict);
        };
        let delete = remaining.remove(position);
        let changed = transaction.execute(
            "DELETE FROM hosts WHERE id = ?1 AND state_version = ?2",
            params![
                delete.host_id.as_str(),
                u64_to_i64(delete.expected.host.get())?
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        delete_owned_mapping(
            transaction,
            owner,
            SshSyncObjectKind::Host,
            &delete.portable_object_id,
            delete.host_id.as_str(),
        )?;
    }
    Ok(())
}

fn delete_owned_credential(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    delete: &SshSyncOwnedCredentialDelete,
) -> Result<Vec<SecretRefId>> {
    let references: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_authentication_credentials WHERE credential_ref_id = ?1)
          OR EXISTS(SELECT 1 FROM host_route_plans WHERE proxy_auth_credential_ref_id = ?1)
          OR EXISTS(SELECT 1 FROM desktop_profiles WHERE credential_ref_id = ?1)",
        [delete.credential_ref_id.as_str()],
        |row| row.get(0),
    )?;
    if references {
        return Err(AppPersistenceError::Conflict);
    }
    let record = get_credential_record_connection(transaction, &delete.credential_ref_id)?;
    let refs = credential_record_secret_refs(&record);
    let changed = transaction.execute(
        "DELETE FROM credential_refs WHERE id = ?1 AND state_version = ?2",
        params![
            delete.credential_ref_id.as_str(),
            u64_to_i64(delete.expected_state_version.get())?
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::Conflict);
    }
    delete_owned_mapping(
        transaction,
        owner,
        SshSyncObjectKind::Credential,
        &delete.portable_object_id,
        delete.credential_ref_id.as_str(),
    )?;
    Ok(refs)
}

fn delete_owned_identity(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    delete: &SshSyncOwnedIdentityDelete,
) -> Result<()> {
    let references: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM hosts WHERE identity_id = ?1)
          OR EXISTS(SELECT 1 FROM credential_refs WHERE identity_id = ?1)",
        [delete.identity_id.as_str()],
        |row| row.get(0),
    )?;
    if references {
        return Err(AppPersistenceError::Conflict);
    }
    let changed = transaction.execute(
        "DELETE FROM identities WHERE id = ?1 AND state_version = ?2",
        params![
            delete.identity_id.as_str(),
            u64_to_i64(delete.expected_state_version.get())?
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::Conflict);
    }
    delete_owned_mapping(
        transaction,
        owner,
        SshSyncObjectKind::Identity,
        &delete.portable_object_id,
        delete.identity_id.as_str(),
    )
}

fn delete_owned_secret_mapping(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    delete: &SshSyncOwnedSecretDelete,
) -> Result<()> {
    if secret_ref_is_used_by_local_metadata(transaction, &delete.secret_ref_id)? {
        return Err(AppPersistenceError::Conflict);
    }
    delete_owned_mapping(
        transaction,
        owner,
        SshSyncObjectKind::Secret,
        &delete.portable_object_id,
        delete.secret_ref_id.as_str(),
    )
}

fn apply_owned_secret_replacement(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    replacement: &SshSyncOwnedSecretReplacement,
    now: i64,
) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE ssh_sync_object_mappings SET local_object_id = ?1, updated_at_ms = ?2
         WHERE plugin_id = ?3 AND signer_fingerprint_sha256 = ?4 AND profile_id = ?5
           AND object_kind = 'secret' AND portable_object_id = ?6 AND local_object_id = ?7",
        params![
            replacement.new_secret_ref_id.as_str(),
            now,
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            owner.profile_id,
            replacement.portable_object_id,
            replacement.expected_secret_ref_id.as_str(),
        ],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn delete_owned_mapping(
    transaction: &Transaction<'_>,
    owner: &NormalizedSshSyncProfileStateKey,
    kind: SshSyncObjectKind,
    portable: &str,
    local: &str,
) -> Result<()> {
    let changed = transaction.execute(
        "DELETE FROM ssh_sync_object_mappings WHERE plugin_id = ?1
         AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3 AND object_kind = ?4
         AND portable_object_id = ?5 AND local_object_id = ?6",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            owner.profile_id,
            ssh_sync_object_kind_to_db(kind),
            portable,
            local
        ],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn secret_ref_is_used_by_local_metadata(
    connection: &Connection,
    secret_ref_id: &SecretRefId,
) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM credential_secret_slots WHERE secret_ref_id = ?1)
          OR EXISTS(SELECT 1 FROM host_login_automation_steps WHERE secret_ref_id = ?1)",
            [secret_ref_id.as_str()],
            |row| row.get(0),
        )
        .map_err(AppPersistenceError::from)
}

fn secret_ref_is_referenced(connection: &Connection, secret_ref_id: &SecretRefId) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM credential_secret_slots WHERE secret_ref_id = ?1)
          OR EXISTS(SELECT 1 FROM host_login_automation_steps WHERE secret_ref_id = ?1)
          OR EXISTS(SELECT 1 FROM ssh_sync_object_mappings
                    WHERE object_kind = 'secret' AND local_object_id = ?1)",
            [secret_ref_id.as_str()],
            |row| row.get(0),
        )
        .map_err(AppPersistenceError::from)
}

fn read_owned_delta_replay(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    attempt_id: &OperationId,
    delta_sha256: &str,
    memberships_json: Option<&str>,
) -> Result<Option<SshSyncOwnedDeltaResult>> {
    let row = connection
        .query_row(
            "SELECT delta_sha256, created_count, updated_count, deleted_count, gc_ref_ids_json
         FROM ssh_sync_owned_delta_operations WHERE plugin_id = ?1
         AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3 AND attempt_id = ?4",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                attempt_id.as_str()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((stored_digest, created, updated, deleted, refs_json)) = row else {
        return Ok(None);
    };
    if stored_digest != delta_sha256 {
        return Err(AppPersistenceError::Conflict);
    }
    let stored_memberships = connection
        .query_row(
            "SELECT memberships_json FROM ssh_sync_owned_delta_scope_replacements
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND profile_id = ?3 AND attempt_id = ?4",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                attempt_id.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if stored_memberships.as_deref() != memberships_json {
        return Err(AppPersistenceError::Conflict);
    }
    let refs = serde_json::from_str::<Vec<String>>(&refs_json)
        .map_err(|_| AppPersistenceError::InvalidStoredData)?
        .into_iter()
        .map(|value| SecretRefId::parse(value).map_err(|_| AppPersistenceError::InvalidStoredData))
        .collect::<Result<Vec<_>>>()?;
    let mut pending_refs = Vec::new();
    for secret_ref in refs {
        let pending: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM ssh_sync_vault_gc_queue
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND profile_id = ?3 AND secret_ref_id = ?4)",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                secret_ref.as_str(),
            ],
            |row| row.get(0),
        )?;
        if pending {
            pending_refs.push(secret_ref);
        }
    }
    Ok(Some(SshSyncOwnedDeltaResult {
        created_count: u32::try_from(created)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        updated_count: u32::try_from(updated)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        deleted_count: u32::try_from(deleted)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        pending_vault_gc_secret_ref_ids: pending_refs,
        replayed: true,
    }))
}

fn read_ssh_sync_vault_gc(row: &rusqlite::Row<'_>) -> rusqlite::Result<SshSyncVaultGcRecord> {
    Ok(SshSyncVaultGcRecord {
        owner: SshSyncProfileStateKey {
            plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
            signer_fingerprint_sha256: row.get(1)?,
            profile_id: row.get(2)?,
        },
        secret_ref_id: parse_id(row.get::<_, String>(3)?, SecretRefId::parse)?,
        reason: row.get(4)?,
        attempt_id: parse_id(row.get::<_, String>(5)?, OperationId::parse)?,
        created_at_unix_ms: row.get(6)?,
    })
}

fn list_ssh_sync_plugin_delete_tasks_connection(
    connection: &Connection,
    plugin_id: Option<&PluginId>,
    operation_id: Option<&OperationId>,
) -> Result<Vec<SshSyncPluginDeleteTask>> {
    let mut statement = connection.prepare(
        "SELECT deletion.operation_id, deletion.plugin_id,
                deletion.signer_fingerprint_sha256, deletion.profile_id,
                deletion.created_at_ms
         FROM ssh_sync_plugin_delete_profiles AS deletion
         JOIN ssh_sync_plugin_delete_operations AS operation
           ON operation.plugin_id = deletion.plugin_id
          AND operation.operation_id = deletion.operation_id
         WHERE operation.state = 'pending'
           AND (?1 IS NULL OR deletion.plugin_id = ?1)
           AND (?2 IS NULL OR deletion.operation_id = ?2)
         ORDER BY deletion.created_at_ms, deletion.plugin_id,
                  deletion.signer_fingerprint_sha256, deletion.profile_id",
    )?;
    let owners = statement
        .query_map(
            params![
                plugin_id.map(PluginId::as_str),
                operation_id.map(OperationId::as_str)
            ],
            |row| {
                Ok((
                    parse_id(row.get::<_, String>(0)?, OperationId::parse)?,
                    parse_id(row.get::<_, String>(1)?, PluginId::parse)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if owners.len() > MAX_SSH_SYNC_PROFILES_PER_PLUGIN {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    let mut tasks = Vec::with_capacity(owners.len());
    for (operation_id, plugin_id, signer, profile_id, created_at_unix_ms) in owners {
        let owner = SshSyncProfileStateKey {
            plugin_id,
            signer_fingerprint_sha256: signer,
            profile_id,
        };
        if normalized_ssh_sync_profile_state_key(&owner).is_err() || created_at_unix_ms < 0 {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        let mut refs_statement = connection.prepare(
            "SELECT secret_ref_id FROM ssh_sync_plugin_delete_refs
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
             ORDER BY secret_ref_id",
        )?;
        let secret_ref_ids = refs_statement
            .query_map(
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    owner.profile_id
                ],
                |row| parse_id(row.get::<_, String>(0)?, SecretRefId::parse),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if secret_ref_ids.len() > MAX_SSH_SYNC_DELETE_REFS {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        tasks.push(SshSyncPluginDeleteTask {
            operation_id,
            owner,
            secret_ref_ids,
            created_at_unix_ms,
        });
    }
    Ok(tasks)
}

fn finalize_empty_ssh_sync_plugin_delete_profiles(
    connection: &Connection,
    plugin_id: &PluginId,
    operation_id: &OperationId,
    _now: i64,
) -> Result<()> {
    let owners = {
        let mut statement = connection.prepare(
            "SELECT signer_fingerprint_sha256, profile_id
             FROM ssh_sync_plugin_delete_profiles AS deletion
             WHERE plugin_id = ?1 AND operation_id = ?2
               AND NOT EXISTS(
                 SELECT 1 FROM ssh_sync_plugin_delete_refs AS reference
                 WHERE reference.plugin_id = deletion.plugin_id
                   AND reference.signer_fingerprint_sha256 = deletion.signer_fingerprint_sha256
                   AND reference.profile_id = deletion.profile_id
               )
             ORDER BY signer_fingerprint_sha256, profile_id",
        )?;
        statement
            .query_map(params![plugin_id.as_str(), operation_id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (signer, profile_id) in owners {
        connection.execute(
            "DELETE FROM ssh_sync_restore_sagas
             WHERE plugin_id = ?1 AND profile_id = ?2",
            params![plugin_id.as_str(), profile_id],
        )?;
        let deleted = connection.execute(
            "DELETE FROM ssh_sync_profile_states
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3",
            params![plugin_id.as_str(), signer, profile_id],
        )?;
        if deleted != 1 {
            return Err(AppPersistenceError::RestoreCommitUnknown);
        }
    }
    Ok(())
}

fn normalized_ssh_sync_object_mapping_inputs(
    inputs: &[SshSyncObjectMappingInput],
) -> Result<Vec<NormalizedSshSyncObjectMappingInput>> {
    if inputs.len() > MAX_SSH_SYNC_OBJECT_MAPPINGS_PER_BATCH {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync object mapping batch exceeds the supported bound",
        ));
    }
    let mut portable_keys = BTreeSet::new();
    let mut local_keys = BTreeSet::new();
    inputs
        .iter()
        .map(|input| {
            let parsed = uuid::Uuid::parse_str(&input.portable_object_id).map_err(|_| {
                AppPersistenceError::InvalidInput(
                    "SSH sync portable object id must be a canonical UUID",
                )
            })?;
            let portable_object_id = parsed.to_string();
            if parsed.is_nil() || portable_object_id != input.portable_object_id {
                return Err(AppPersistenceError::InvalidInput(
                    "SSH sync portable object id must be a canonical UUID",
                ));
            }
            let object_kind = input.local_object_id.object_kind();
            if let SshSyncLocalObjectId::DesktopProfile(profile_id) = &input.local_object_id {
                normalized_desktop_profile_id(profile_id)?;
            }
            if !portable_keys.insert((object_kind, portable_object_id.clone()))
                || !local_keys.insert((object_kind, input.local_object_id.as_str().to_owned()))
            {
                return Err(AppPersistenceError::InvalidInput(
                    "SSH sync object mapping batch contains duplicate identities",
                ));
            }
            Ok(NormalizedSshSyncObjectMappingInput {
                object_kind,
                portable_object_id,
                local_object_id: input.local_object_id.clone(),
            })
        })
        .collect()
}

fn ssh_sync_object_kind_to_db(kind: SshSyncObjectKind) -> &'static str {
    match kind {
        SshSyncObjectKind::Host => "host",
        SshSyncObjectKind::Identity => "identity",
        SshSyncObjectKind::Credential => "credential",
        SshSyncObjectKind::Secret => "secret",
        SshSyncObjectKind::DesktopProfile => "desktop_profile",
    }
}

fn ssh_sync_object_kind_from_db(value: &str) -> rusqlite::Result<SshSyncObjectKind> {
    match value {
        "host" => Ok(SshSyncObjectKind::Host),
        "identity" => Ok(SshSyncObjectKind::Identity),
        "credential" => Ok(SshSyncObjectKind::Credential),
        "secret" => Ok(SshSyncObjectKind::Secret),
        "desktop_profile" => Ok(SshSyncObjectKind::DesktopProfile),
        _ => Err(invalid_stored_column()),
    }
}

fn parse_ssh_sync_local_object_id(
    kind: SshSyncObjectKind,
    value: String,
) -> rusqlite::Result<SshSyncLocalObjectId> {
    match kind {
        SshSyncObjectKind::Host => parse_id(value, HostId::parse).map(SshSyncLocalObjectId::Host),
        SshSyncObjectKind::Identity => {
            parse_id(value, IdentityId::parse).map(SshSyncLocalObjectId::Identity)
        }
        SshSyncObjectKind::Credential => {
            parse_id(value, CredentialRefId::parse).map(SshSyncLocalObjectId::Credential)
        }
        SshSyncObjectKind::Secret => {
            parse_id(value, SecretRefId::parse).map(SshSyncLocalObjectId::Secret)
        }
        SshSyncObjectKind::DesktopProfile => {
            let parsed = uuid::Uuid::parse_str(&value).map_err(invalid_column)?;
            if parsed.is_nil() || parsed.to_string() != value {
                return Err(invalid_stored_column());
            }
            Ok(SshSyncLocalObjectId::DesktopProfile(value))
        }
    }
}

fn read_ssh_sync_object_mapping(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SshSyncObjectMappingRecord> {
    let owner = SshSyncProfileStateKey {
        plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
        signer_fingerprint_sha256: row.get(1)?,
        profile_id: row.get(2)?,
    };
    if normalized_ssh_sync_profile_state_key(&owner).is_err() {
        return Err(invalid_stored_column());
    }
    let object_kind = ssh_sync_object_kind_from_db(&row.get::<_, String>(3)?)?;
    let portable_object_id = row.get::<_, String>(4)?;
    let portable_uuid = uuid::Uuid::parse_str(&portable_object_id).map_err(invalid_column)?;
    if portable_uuid.is_nil() || portable_uuid.to_string() != portable_object_id {
        return Err(invalid_stored_column());
    }
    let local_object_id = parse_ssh_sync_local_object_id(object_kind, row.get::<_, String>(5)?)?;
    let created_at_unix_ms = row.get::<_, i64>(6)?;
    let updated_at_unix_ms = row.get::<_, i64>(7)?;
    if created_at_unix_ms < 0 || updated_at_unix_ms < created_at_unix_ms {
        return Err(invalid_stored_column());
    }
    Ok(SshSyncObjectMappingRecord {
        owner,
        object_kind,
        portable_object_id,
        local_object_id,
        created_at_unix_ms,
        updated_at_unix_ms,
    })
}

fn lookup_ssh_sync_object_mapping(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    input: &NormalizedSshSyncObjectMappingInput,
) -> Result<Vec<SshSyncObjectMappingRecord>> {
    let mut statement = connection.prepare(
        "SELECT plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
                portable_object_id, local_object_id, created_at_ms, updated_at_ms
         FROM ssh_sync_object_mappings
         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
           AND object_kind = ?4 AND (portable_object_id = ?5 OR local_object_id = ?6)
         ORDER BY portable_object_id",
    )?;
    statement
        .query_map(
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                ssh_sync_object_kind_to_db(input.object_kind),
                input.portable_object_id,
                input.local_object_id.as_str(),
            ],
            read_ssh_sync_object_mapping,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn normalized_ssh_sync_profile_state_key(
    key: &SshSyncProfileStateKey,
) -> Result<NormalizedSshSyncProfileStateKey> {
    validate_lower_sha256(
        &key.signer_fingerprint_sha256,
        "invalid SSH sync signer fingerprint",
    )?;
    let profile_id = normalized_required(
        &key.profile_id,
        160,
        "SSH sync profile id is empty or too long",
    )?;
    if profile_id != key.profile_id
        || !profile_id.is_ascii()
        || !profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync profile id contains unsupported characters",
        ));
    }
    Ok(NormalizedSshSyncProfileStateKey {
        plugin_id: key.plugin_id.clone(),
        signer_fingerprint_sha256: key.signer_fingerprint_sha256.clone(),
        profile_id,
    })
}

fn normalized_ssh_sync_profile_scope(scope: &SshSyncProfileScope) -> Result<SshSyncProfileScope> {
    if scope.custom_host_ids.len() > MAX_SSH_SYNC_SCOPE_HOSTS
        || scope.custom_credential_ref_ids.len() > MAX_SSH_SYNC_SCOPE_CREDENTIALS
        || scope.custom_desktop_profile_ids.len() > MAX_SSH_SYNC_SCOPE_DESKTOP_PROFILES
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync custom scope exceeds the supported bound",
        ));
    }
    let mut custom_host_ids = scope.custom_host_ids.clone();
    custom_host_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if custom_host_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync custom Host scope contains duplicates",
        ));
    }
    let mut custom_credential_ref_ids = scope.custom_credential_ref_ids.clone();
    custom_credential_ref_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if custom_credential_ref_ids
        .windows(2)
        .any(|pair| pair[0] == pair[1])
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync custom credential scope contains duplicates",
        ));
    }
    let mut custom_desktop_profile_ids = scope
        .custom_desktop_profile_ids
        .iter()
        .map(|id| normalized_desktop_profile_id(id))
        .collect::<Result<Vec<_>>>()?;
    custom_desktop_profile_ids.sort();
    if custom_desktop_profile_ids
        .windows(2)
        .any(|pair| pair[0] == pair[1])
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync custom desktop profile scope contains duplicates",
        ));
    }
    if scope.mode == SshSyncProfileScopeMode::AllEligible
        && (!custom_host_ids.is_empty()
            || !custom_credential_ref_ids.is_empty()
            || !custom_desktop_profile_ids.is_empty())
    {
        return Err(AppPersistenceError::InvalidInput(
            "all-eligible SSH sync scope cannot contain custom selections",
        ));
    }
    Ok(SshSyncProfileScope {
        mode: scope.mode,
        custom_host_ids,
        custom_credential_ref_ids,
        custom_desktop_profile_ids,
    })
}

fn normalized_ssh_sync_profile_key_binding(
    binding: &SshSyncProfileKeyBinding,
) -> Result<SshSyncProfileKeyBinding> {
    if binding
        .password_wrapped_sync_key_envelope
        .as_ref()
        .is_some_and(|value| !(16..=MAX_SSH_SYNC_KEY_ENVELOPE_BYTES).contains(&value.len()))
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync key envelope exceeds the supported bound",
        ));
    }
    Ok(binding.clone())
}

fn normalized_ssh_sync_remote_baseline(
    baseline: &SshSyncProfileRemoteBaseline,
) -> Result<SshSyncProfileRemoteBaseline> {
    if baseline.remote_revision > i64::MAX as u64 {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync remote revision exceeds the supported bound",
        ));
    }
    validate_strong_etag(&baseline.remote_etag)?;
    validate_lower_sha256(
        &baseline.baseline_content_sha256,
        "invalid SSH sync baseline content digest",
    )?;
    validate_lower_sha256(
        &baseline.baseline_exchange_sha256,
        "invalid SSH sync baseline exchange digest",
    )?;
    Ok(baseline.clone())
}

fn normalized_ssh_sync_http_upload_attempt(
    input: &SshSyncHttpUploadAttemptInput,
) -> Result<NormalizedSshSyncHttpUploadAttemptInput> {
    let owner = normalized_ssh_sync_profile_state_key(&input.owner)?;
    let canonical_url = normalized_ssh_sync_canonical_url(&input.canonical_url)?;
    if input.base_revision > i64::MAX as u64
        || input.target_revision > i64::MAX as u64
        || input.target_revision <= input.base_revision
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync upload revisions are invalid",
        ));
    }
    match (input.base_revision, input.base_etag.as_deref()) {
        (0, None) => {}
        (0, Some(_)) | (_, None) => {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync upload baseline is incomplete",
            ));
        }
        (_, Some(etag)) => validate_strong_etag(etag)?,
    }
    validate_lower_sha256(
        &input.keyed_content_sha256,
        "invalid SSH sync keyed content digest",
    )?;
    validate_lower_sha256(&input.body_sha256, "invalid SSH sync upload body digest")?;
    let idempotency_key = normalized_upload_idempotency_key(&input.idempotency_key)?;
    Ok(NormalizedSshSyncHttpUploadAttemptInput {
        owner,
        canonical_url,
        http_method: input.http_method,
        use_oauth: input.use_oauth,
        authorization_revision: input.authorization_revision,
        configuration_revision: input.configuration_revision,
        base_revision: input.base_revision,
        base_etag: input.base_etag.clone(),
        target_revision: input.target_revision,
        keyed_content_sha256: input.keyed_content_sha256.clone(),
        body_sha256: input.body_sha256.clone(),
        idempotency_key,
    })
}

fn normalized_ssh_sync_http_upload_completion_fence(
    fence: &SshSyncHttpUploadCompletionFence,
) -> Result<SshSyncHttpUploadCompletionFence> {
    let normalized_url = normalized_ssh_sync_canonical_url(&fence.canonical_url)?;
    Ok(SshSyncHttpUploadCompletionFence {
        canonical_url: normalized_url,
        http_method: fence.http_method,
        use_oauth: fence.use_oauth,
        authorization_revision: fence.authorization_revision,
        configuration_revision: fence.configuration_revision,
    })
}

fn normalized_ssh_sync_canonical_url(value: &str) -> Result<String> {
    if value.trim() != value
        || !(9..=2_048).contains(&value.len())
        || !value.is_ascii()
        || value.chars().any(char::is_control)
        || !value.starts_with("https://")
        || value.contains('#')
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync upload URL must be a bounded canonical HTTPS URL",
        ));
    }
    let remainder = &value[8..];
    let authority_end = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.is_empty()
        || authority.contains('@')
        || authority.ends_with('.')
        || authority.ends_with(":443")
        || authority.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync upload URL authority is not canonical",
        ));
    }
    let resource = &remainder[authority_end..];
    let lower_resource = resource.to_ascii_lowercase();
    if resource.contains("//")
        || resource.contains("/./")
        || resource.contains("/../")
        || resource.ends_with("/.")
        || resource.ends_with("/..")
        || lower_resource.contains("%2e")
        || !canonical_percent_encoding(resource)
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync upload URL path is not canonical",
        ));
    }
    Ok(value.to_owned())
}

fn canonical_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len()
            || !bytes[index + 1].is_ascii_hexdigit()
            || !bytes[index + 2].is_ascii_hexdigit()
            || bytes[index + 1].is_ascii_lowercase()
            || bytes[index + 2].is_ascii_lowercase()
        {
            return false;
        }
        index += 3;
    }
    true
}

fn normalized_upload_idempotency_key(value: &str) -> Result<String> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
        AppPersistenceError::InvalidInput("SSH sync upload idempotency key must be a UUID")
    })?;
    let normalized = parsed.to_string();
    if parsed.is_nil() || normalized != value {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync upload idempotency key must be a canonical UUID",
        ));
    }
    Ok(normalized)
}

fn denormalized_ssh_sync_http_upload_input(
    input: &NormalizedSshSyncHttpUploadAttemptInput,
) -> SshSyncHttpUploadAttemptInput {
    SshSyncHttpUploadAttemptInput {
        owner: SshSyncProfileStateKey {
            plugin_id: input.owner.plugin_id.clone(),
            signer_fingerprint_sha256: input.owner.signer_fingerprint_sha256.clone(),
            profile_id: input.owner.profile_id.clone(),
        },
        canonical_url: input.canonical_url.clone(),
        http_method: input.http_method,
        use_oauth: input.use_oauth,
        authorization_revision: input.authorization_revision,
        configuration_revision: input.configuration_revision,
        base_revision: input.base_revision,
        base_etag: input.base_etag.clone(),
        target_revision: input.target_revision,
        keyed_content_sha256: input.keyed_content_sha256.clone(),
        body_sha256: input.body_sha256.clone(),
        idempotency_key: input.idempotency_key.clone(),
    }
}

fn ssh_sync_http_method_to_db(method: SshSyncHttpMethod) -> &'static str {
    match method {
        SshSyncHttpMethod::Put => "PUT",
        SshSyncHttpMethod::Post => "POST",
    }
}

fn ssh_sync_http_method_from_db(value: &str) -> rusqlite::Result<SshSyncHttpMethod> {
    match value {
        "PUT" => Ok(SshSyncHttpMethod::Put),
        "POST" => Ok(SshSyncHttpMethod::Post),
        _ => Err(invalid_stored_column()),
    }
}

fn ssh_sync_http_upload_state_to_db(state: SshSyncHttpUploadAttemptState) -> &'static str {
    match state {
        SshSyncHttpUploadAttemptState::Prepared => "prepared",
        SshSyncHttpUploadAttemptState::Sent => "sent",
        SshSyncHttpUploadAttemptState::Verifying => "verifying",
    }
}

fn ssh_sync_http_upload_state_from_db(
    value: &str,
) -> rusqlite::Result<SshSyncHttpUploadAttemptState> {
    match value {
        "prepared" => Ok(SshSyncHttpUploadAttemptState::Prepared),
        "sent" => Ok(SshSyncHttpUploadAttemptState::Sent),
        "verifying" => Ok(SshSyncHttpUploadAttemptState::Verifying),
        _ => Err(invalid_stored_column()),
    }
}

fn get_ssh_sync_http_upload_attempt_connection(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
) -> Result<Option<SshSyncHttpUploadAttemptRecord>> {
    connection
        .query_row(
            "SELECT plugin_id, signer_fingerprint_sha256, profile_id,
                    canonical_url, http_method, use_oauth,
                    authorization_revision, configuration_revision,
                    base_revision, base_etag, target_revision,
                    keyed_content_sha256, body_sha256,
                    idempotency_key, state, state_version, created_at_ms, updated_at_ms
             FROM ssh_sync_http_upload_attempts
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id
            ],
            read_ssh_sync_http_upload_attempt,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn read_ssh_sync_http_upload_attempt(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SshSyncHttpUploadAttemptRecord> {
    let owner = SshSyncProfileStateKey {
        plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
        signer_fingerprint_sha256: row.get(1)?,
        profile_id: row.get(2)?,
    };
    if normalized_ssh_sync_profile_state_key(&owner).is_err() {
        return Err(invalid_stored_column());
    }
    let canonical_url = row.get::<_, String>(3)?;
    if normalized_ssh_sync_canonical_url(&canonical_url).is_err() {
        return Err(invalid_stored_column());
    }
    let http_method = ssh_sync_http_method_from_db(&row.get::<_, String>(4)?)?;
    let use_oauth = row.get::<_, bool>(5)?;
    let authorization_revision = read_wire_sequence(row, 6)?;
    let configuration_revision = read_wire_sequence(row, 7)?;
    let base_revision = row.get::<_, i64>(8)?;
    let base_etag = row.get::<_, Option<String>>(9)?;
    if base_revision < 0
        || match (base_revision, base_etag.as_deref()) {
            (0, None) => false,
            (0, Some(_)) | (_, None) => true,
            (_, Some(etag)) => validate_strong_etag(etag).is_err(),
        }
    {
        return Err(invalid_stored_column());
    }
    let target_revision = row.get::<_, i64>(10)?;
    if target_revision <= base_revision {
        return Err(invalid_stored_column());
    }
    let keyed_content_sha256 = row.get::<_, String>(11)?;
    let body_sha256 = row.get::<_, String>(12)?;
    let idempotency_key = row.get::<_, String>(13)?;
    if validate_lower_sha256(&keyed_content_sha256, "invalid stored digest").is_err()
        || validate_lower_sha256(&body_sha256, "invalid stored digest").is_err()
        || normalized_upload_idempotency_key(&idempotency_key).is_err()
    {
        return Err(invalid_stored_column());
    }
    let created_at_unix_ms = row.get::<_, i64>(16)?;
    let updated_at_unix_ms = row.get::<_, i64>(17)?;
    if created_at_unix_ms < 0 || updated_at_unix_ms < created_at_unix_ms {
        return Err(invalid_stored_column());
    }
    Ok(SshSyncHttpUploadAttemptRecord {
        input: SshSyncHttpUploadAttemptInput {
            owner,
            canonical_url,
            http_method,
            use_oauth,
            authorization_revision,
            configuration_revision,
            base_revision: base_revision as u64,
            base_etag,
            target_revision: target_revision as u64,
            keyed_content_sha256,
            body_sha256,
            idempotency_key,
        },
        state: ssh_sync_http_upload_state_from_db(&row.get::<_, String>(14)?)?,
        state_version: read_wire_sequence(row, 15)?,
        created_at_unix_ms,
        updated_at_unix_ms,
    })
}

fn normalized_ssh_sync_scope_memberships(
    memberships: &[SshSyncScopeMembershipInput],
) -> Result<Vec<SshSyncScopeMembershipInput>> {
    if memberships.len() > MAX_SSH_SYNC_SCOPE_MEMBERSHIPS {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync scope membership set exceeds the supported bound",
        ));
    }
    let mut normalized = memberships.to_vec();
    for membership in &mut normalized {
        let parsed = uuid::Uuid::parse_str(&membership.portable_object_id).map_err(|_| {
            AppPersistenceError::InvalidInput(
                "SSH sync portable scope object id must be a canonical UUID",
            )
        })?;
        let portable_object_id = parsed.to_string();
        if parsed.is_nil() || portable_object_id != membership.portable_object_id {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync portable scope object id must be a canonical UUID",
            ));
        }
        membership.portable_object_id = portable_object_id;
    }
    normalized.sort_by(|left, right| {
        ssh_sync_object_kind_to_db(left.object_kind)
            .cmp(ssh_sync_object_kind_to_db(right.object_kind))
            .then_with(|| left.portable_object_id.cmp(&right.portable_object_id))
    });
    if normalized.windows(2).any(|pair| {
        pair[0].object_kind == pair[1].object_kind
            && pair[0].portable_object_id == pair[1].portable_object_id
    }) {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync scope membership set contains duplicates",
        ));
    }
    Ok(normalized)
}

fn ssh_sync_scope_membership_to_db(state: SshSyncScopeMembershipState) -> &'static str {
    match state {
        SshSyncScopeMembershipState::Included => "included",
        SshSyncScopeMembershipState::Excluded => "excluded",
    }
}

fn ssh_sync_scope_membership_from_db(value: &str) -> rusqlite::Result<SshSyncScopeMembershipState> {
    match value {
        "included" => Ok(SshSyncScopeMembershipState::Included),
        "excluded" => Ok(SshSyncScopeMembershipState::Excluded),
        _ => Err(invalid_stored_column()),
    }
}

fn encode_ssh_sync_scope_memberships(
    memberships: &[SshSyncScopeMembershipInput],
) -> Result<String> {
    let values = memberships
        .iter()
        .map(|membership| {
            (
                ssh_sync_object_kind_to_db(membership.object_kind),
                membership.portable_object_id.as_str(),
                ssh_sync_scope_membership_to_db(membership.state),
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&values).map_err(|_| {
        AppPersistenceError::InvalidInput("SSH sync scope memberships cannot be serialized")
    })
}

fn list_ssh_sync_scope_memberships_connection(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
) -> Result<Vec<SshSyncScopeMembershipRecord>> {
    let mut statement = connection.prepare(
        "SELECT object_kind, portable_object_id, membership, updated_at_ms
         FROM ssh_sync_scope_memberships
         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
         ORDER BY object_kind, portable_object_id",
    )?;
    let owner_record = SshSyncProfileStateKey {
        plugin_id: owner.plugin_id.clone(),
        signer_fingerprint_sha256: owner.signer_fingerprint_sha256.clone(),
        profile_id: owner.profile_id.clone(),
    };
    let records = statement
        .query_map(
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id
            ],
            |row| {
                let portable_object_id = row.get::<_, String>(1)?;
                let parsed = uuid::Uuid::parse_str(&portable_object_id).map_err(invalid_column)?;
                if parsed.is_nil() || parsed.to_string() != portable_object_id {
                    return Err(invalid_stored_column());
                }
                let updated_at_unix_ms = row.get::<_, i64>(3)?;
                if updated_at_unix_ms < 0 {
                    return Err(invalid_stored_column());
                }
                Ok(SshSyncScopeMembershipRecord {
                    owner: owner_record.clone(),
                    membership: SshSyncScopeMembershipInput {
                        object_kind: ssh_sync_object_kind_from_db(&row.get::<_, String>(0)?)?,
                        portable_object_id,
                        state: ssh_sync_scope_membership_from_db(&row.get::<_, String>(2)?)?,
                    },
                    updated_at_unix_ms,
                })
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if records.len() > MAX_SSH_SYNC_SCOPE_MEMBERSHIPS {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(records)
}

fn replace_ssh_sync_scope_memberships_connection(
    connection: &Connection,
    owner: &NormalizedSshSyncProfileStateKey,
    memberships: &[SshSyncScopeMembershipInput],
    now: i64,
) -> Result<()> {
    connection.execute(
        "DELETE FROM ssh_sync_scope_memberships
         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            owner.profile_id
        ],
    )?;
    for membership in memberships {
        connection.execute(
            "INSERT INTO ssh_sync_scope_memberships
             (plugin_id, signer_fingerprint_sha256, profile_id,
              object_kind, portable_object_id, membership, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                owner.profile_id,
                ssh_sync_object_kind_to_db(membership.object_kind),
                membership.portable_object_id,
                ssh_sync_scope_membership_to_db(membership.state),
                now
            ],
        )?;
    }
    Ok(())
}

fn validate_lower_sha256(value: &str, error: &'static str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppPersistenceError::InvalidInput(error));
    }
    Ok(())
}

fn validate_strong_etag(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if !(2..=1_024).contains(&bytes.len())
        || bytes.first() != Some(&b'"')
        || bytes.last() != Some(&b'"')
        || !bytes[1..bytes.len() - 1]
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte))
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync remote ETag must be a bounded strong ETag",
        ));
    }
    Ok(())
}

fn ssh_sync_scope_mode_to_db(mode: SshSyncProfileScopeMode) -> &'static str {
    match mode {
        SshSyncProfileScopeMode::AllEligible => "all_eligible",
        SshSyncProfileScopeMode::Custom => "custom",
    }
}

fn encode_id_list<'a>(
    values: impl Iterator<Item = &'a str>,
    error: &'static str,
) -> Result<String> {
    serde_json::to_string(&values.collect::<Vec<_>>())
        .map_err(|_| AppPersistenceError::InvalidInput(error))
}

fn get_ssh_sync_profile_state_connection(
    connection: &Connection,
    key: &NormalizedSshSyncProfileStateKey,
) -> Result<Option<SshSyncProfileStateRecord>> {
    connection
        .query_row(
            "SELECT plugin_id, signer_fingerprint_sha256, profile_id, scope_mode,
                    custom_host_ids_json, custom_credential_ref_ids_json,
                    custom_desktop_profile_ids_json,
                    sync_key_secret_ref_id, password_wrapped_sync_key_envelope,
                    remote_revision, remote_etag, baseline_content_sha256,
                    baseline_exchange_sha256, state_version, updated_at_ms,
                    last_successful_sync_at_ms
             FROM ssh_sync_profile_states AS profile
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
               AND NOT EXISTS(
                 SELECT 1 FROM ssh_sync_plugin_delete_profiles AS deletion
                 WHERE deletion.plugin_id = profile.plugin_id
                   AND deletion.signer_fingerprint_sha256 = profile.signer_fingerprint_sha256
                   AND deletion.profile_id = profile.profile_id
               )",
            params![
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id
            ],
            read_ssh_sync_profile_state,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn ssh_sync_profile_exists_connection(
    connection: &Connection,
    key: &NormalizedSshSyncProfileStateKey,
) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_profile_states
               WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND profile_id = ?3
             )",
            params![
                key.plugin_id.as_str(),
                key.signer_fingerprint_sha256,
                key.profile_id
            ],
            |row| row.get(0),
        )
        .map_err(AppPersistenceError::from)
}

fn read_ssh_sync_profile_state(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SshSyncProfileStateRecord> {
    let plugin_id = parse_id(row.get::<_, String>(0)?, PluginId::parse)?;
    let signer_fingerprint_sha256 = row.get::<_, String>(1)?;
    let profile_id = row.get::<_, String>(2)?;
    if validate_lower_sha256(&signer_fingerprint_sha256, "invalid stored signer").is_err()
        || profile_id.is_empty()
        || profile_id.len() > 160
        || !profile_id.is_ascii()
        || !profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
    {
        return Err(invalid_stored_column());
    }
    let mode = match row.get::<_, String>(3)?.as_str() {
        "all_eligible" => SshSyncProfileScopeMode::AllEligible,
        "custom" => SshSyncProfileScopeMode::Custom,
        _ => return Err(invalid_stored_column()),
    };
    let custom_host_ids = decode_stored_id_list(
        &row.get::<_, String>(4)?,
        MAX_SSH_SYNC_SCOPE_HOSTS,
        HostId::parse,
    )?;
    let custom_credential_ref_ids = decode_stored_id_list(
        &row.get::<_, String>(5)?,
        MAX_SSH_SYNC_SCOPE_CREDENTIALS,
        CredentialRefId::parse,
    )?;
    let custom_desktop_profile_ids = decode_stored_uuid_list(
        &row.get::<_, String>(6)?,
        MAX_SSH_SYNC_SCOPE_DESKTOP_PROFILES,
    )?;
    if mode == SshSyncProfileScopeMode::AllEligible
        && (!custom_host_ids.is_empty()
            || !custom_credential_ref_ids.is_empty()
            || !custom_desktop_profile_ids.is_empty())
    {
        return Err(invalid_stored_column());
    }
    let sync_key_secret_ref_id = row
        .get::<_, Option<String>>(7)?
        .map(|value| parse_id(value, SecretRefId::parse))
        .transpose()?;
    let envelope = row.get::<_, Option<Vec<u8>>>(8)?;
    if envelope
        .as_ref()
        .is_some_and(|value| !(16..=MAX_SSH_SYNC_KEY_ENVELOPE_BYTES).contains(&value.len()))
        || (envelope.is_some() && sync_key_secret_ref_id.is_none())
    {
        return Err(invalid_stored_column());
    }
    let remote_revision = row.get::<_, Option<i64>>(9)?;
    let remote_etag = row.get::<_, Option<String>>(10)?;
    let baseline_content_sha256 = row.get::<_, Option<String>>(11)?;
    let baseline_exchange_sha256 = row.get::<_, Option<String>>(12)?;
    let remote_baseline = match (
        remote_revision,
        remote_etag,
        baseline_content_sha256,
        baseline_exchange_sha256,
    ) {
        (None, None, None, None) => None,
        (Some(revision), Some(etag), Some(content_digest), Some(exchange_digest))
            if revision >= 0
                && validate_strong_etag(&etag).is_ok()
                && validate_lower_sha256(&content_digest, "invalid stored baseline content")
                    .is_ok()
                && validate_lower_sha256(&exchange_digest, "invalid stored baseline exchange")
                    .is_ok() =>
        {
            Some(SshSyncProfileRemoteBaseline {
                remote_revision: revision as u64,
                remote_etag: etag,
                baseline_content_sha256: content_digest,
                baseline_exchange_sha256: exchange_digest,
            })
        }
        _ => return Err(invalid_stored_column()),
    };
    let updated_at_unix_ms = row.get::<_, i64>(14)?;
    let last_successful_sync_at_unix_ms = row.get::<_, Option<i64>>(15)?;
    if updated_at_unix_ms < 0 || last_successful_sync_at_unix_ms.is_some_and(|value| value < 0) {
        return Err(invalid_stored_column());
    }
    Ok(SshSyncProfileStateRecord {
        key: SshSyncProfileStateKey {
            plugin_id,
            signer_fingerprint_sha256,
            profile_id,
        },
        scope: SshSyncProfileScope {
            mode,
            custom_host_ids,
            custom_credential_ref_ids,
            custom_desktop_profile_ids,
        },
        key_binding: sync_key_secret_ref_id.map(|sync_key_secret_ref_id| {
            SshSyncProfileKeyBinding {
                sync_key_secret_ref_id,
                password_wrapped_sync_key_envelope: envelope,
            }
        }),
        remote_baseline,
        state_version: read_wire_sequence(row, 13)?,
        updated_at_unix_ms,
        last_successful_sync_at_unix_ms,
    })
}

fn decode_stored_id_list<T>(
    value: &str,
    maximum: usize,
    parse: impl Fn(String) -> std::result::Result<T, &'static str>,
) -> rusqlite::Result<Vec<T>> {
    let values = serde_json::from_str::<Vec<String>>(value).map_err(invalid_column)?;
    if values.len() > maximum
        || values
            .windows(2)
            .any(|pair| pair[0].as_str() >= pair[1].as_str())
    {
        return Err(invalid_stored_column());
    }
    values
        .into_iter()
        .map(|value| {
            parse(value).map_err(|message| {
                invalid_column(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    message,
                ))
            })
        })
        .collect()
}

fn decode_stored_uuid_list(value: &str, maximum: usize) -> rusqlite::Result<Vec<String>> {
    let values = serde_json::from_str::<Vec<String>>(value).map_err(invalid_column)?;
    if values.len() > maximum
        || values
            .windows(2)
            .any(|pair| pair[0].as_str() >= pair[1].as_str())
    {
        return Err(invalid_stored_column());
    }
    for value in &values {
        let parsed = uuid::Uuid::parse_str(value).map_err(invalid_column)?;
        if parsed.is_nil() || parsed.to_string() != *value {
            return Err(invalid_stored_column());
        }
    }
    Ok(values)
}

fn normalized_restore_saga_input(input: &SshSyncRestoreSagaInput) -> Result<NormalizedRestoreSaga> {
    let profile_id = normalized_required(
        &input.profile_id,
        160,
        "SSH sync profile id is empty or too long",
    )?;
    validate_restore_digest(&input.bundle_sha256)?;
    validate_restore_digest(&input.plan_sha256)?;
    let secret_ref_ids = normalized_secret_ref_ids(&input.secret_ref_ids)?;
    Ok(NormalizedRestoreSaga {
        attempt_id: input.attempt_id.clone(),
        plugin_id: input.plugin_id.clone(),
        profile_id,
        bundle_sha256: input.bundle_sha256.clone(),
        plan_sha256: input.plan_sha256.clone(),
        secret_ref_ids,
    })
}

fn normalized_restore_plan(plan: &SshSyncRestorePlan) -> Result<NormalizedRestorePlan> {
    if plan.identities.len() > 1_024
        || plan.credentials.len() > 4_096
        || plan.hosts.len() > 1_024
        || plan.desktop_profiles.len() > MAX_SSH_SYNC_SCOPE_DESKTOP_PROFILES
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync restore plan exceeds the supported bound",
        ));
    }
    let profile_id = normalized_required(
        &plan.profile_id,
        160,
        "SSH sync profile id is empty or too long",
    )?;
    validate_restore_digest(&plan.bundle_sha256)?;
    validate_restore_digest(&plan.plan_sha256)?;

    let mut identity_ids = BTreeSet::new();
    let mut identities = Vec::with_capacity(plan.identities.len());
    for input in &plan.identities {
        if !identity_ids.insert(input.identity_id.as_str().to_owned()) {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore identity is duplicated",
            ));
        }
        identities.push(SshSyncRestoreIdentityInput {
            identity_id: input.identity_id.clone(),
            label: normalized_label(&input.label)?,
            username: normalized_optional(input.username.as_deref(), 256, "username is too long")?,
        });
    }
    identities.sort_by(|left, right| left.identity_id.as_str().cmp(right.identity_id.as_str()));

    let mut credential_ids = BTreeSet::new();
    let mut operation_ids = BTreeSet::new();
    let mut idempotency_keys = BTreeSet::new();
    let mut identity_priorities = BTreeSet::new();
    let mut secret_ref_ids = Vec::new();
    let mut credentials = Vec::with_capacity(plan.credentials.len());
    for input in &plan.credentials {
        let idempotency_key = validated_idempotency_key(&input.idempotency_key)?;
        if !credential_ids.insert(input.credential_ref_id.as_str().to_owned())
            || !operation_ids.insert(input.operation_id.as_str().to_owned())
            || !idempotency_keys.insert(idempotency_key.clone())
            || !identity_priorities.insert((input.identity_id.as_str().to_owned(), input.priority))
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore credential identity is duplicated",
            ));
        }
        let material = match &input.material {
            SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => {
                secret_ref_ids.push(secret_ref_id.clone());
                SshSyncRestoreCredentialMaterial::Password {
                    secret_ref_id: secret_ref_id.clone(),
                }
            }
            SshSyncRestoreCredentialMaterial::PrivateKey {
                secret_ref_id,
                passphrase_secret_ref_id,
                public_key_algorithm,
                public_key_fingerprint,
            } => {
                let (algorithm, fingerprint) = normalized_public_key_metadata(
                    Some(public_key_algorithm),
                    Some(public_key_fingerprint),
                )?;
                let algorithm = algorithm.ok_or(AppPersistenceError::InvalidInput(
                    "private key credentials require derived public key metadata",
                ))?;
                let fingerprint = fingerprint.ok_or(AppPersistenceError::InvalidInput(
                    "private key credentials require derived public key metadata",
                ))?;
                secret_ref_ids.push(secret_ref_id.clone());
                if let Some(passphrase) = passphrase_secret_ref_id {
                    secret_ref_ids.push(passphrase.clone());
                }
                SshSyncRestoreCredentialMaterial::PrivateKey {
                    secret_ref_id: secret_ref_id.clone(),
                    passphrase_secret_ref_id: passphrase_secret_ref_id.clone(),
                    public_key_algorithm: algorithm,
                    public_key_fingerprint: fingerprint,
                }
            }
            SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds } => {
                if !(1..=32).contains(max_rounds) {
                    return Err(AppPersistenceError::InvalidInput(
                        "keyboard-interactive max rounds must be between 1 and 32",
                    ));
                }
                SshSyncRestoreCredentialMaterial::KeyboardInteractive {
                    max_rounds: *max_rounds,
                }
            }
        };
        credentials.push(SshSyncRestoreCredentialInput {
            credential_ref_id: input.credential_ref_id.clone(),
            identity_id: input.identity_id.clone(),
            operation_id: input.operation_id.clone(),
            idempotency_key,
            priority: input.priority,
            label: normalized_label(&input.label)?,
            material,
        });
    }
    credentials.sort_by(|left, right| {
        left.credential_ref_id
            .as_str()
            .cmp(right.credential_ref_id.as_str())
    });
    let mut host_ids = BTreeSet::new();
    let mut host_operation_ids = BTreeSet::new();
    let mut host_idempotency_keys = BTreeSet::new();
    let mut hosts = Vec::with_capacity(plan.hosts.len());
    for input in &plan.hosts {
        if input.request.staged_password_id.is_some() {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore cannot consume a Host password stage",
            ));
        }
        if !input.request.tag_ids.is_empty() {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore Host tags must use tag labels",
            ));
        }
        if input.request.login_automation_enabled
            || input.request.login_automation_confirmed
            || !input.request.login_automation_steps.is_empty()
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore automation must use the internal typed input",
            ));
        }
        let tag_labels = normalized_sync_tag_labels(&input.tag_labels)?;
        let login_automation = normalized_ssh_sync_login_automation(&input.login_automation)?;
        secret_ref_ids.extend(
            ssh_sync_login_automation_secret_refs(&login_automation)
                .into_iter()
                .cloned(),
        );
        let idempotency_key = validated_idempotency_key(&input.request.idempotency_key)?;
        if !host_ids.insert(input.host_id.as_str().to_owned())
            || !host_operation_ids.insert(input.request.operation_id.as_str().to_owned())
            || !host_idempotency_keys.insert(idempotency_key)
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore Host identity is duplicated",
            ));
        }
        hosts.push((
            input.host_id.clone(),
            input.request.clone(),
            normalized_configured_host_create(&input.request)?,
            tag_labels,
            login_automation,
        ));
    }
    normalized_secret_ref_ids(&secret_ref_ids)?;
    hosts = topologically_sorted_restore_hosts(hosts)?;

    let mut desktop_portable_ids = BTreeSet::new();
    let mut desktop_profile_ids = BTreeSet::new();
    let mut desktop_profiles = Vec::with_capacity(plan.desktop_profiles.len());
    for input in &plan.desktop_profiles {
        let normalized = normalized_restore_desktop_profile_input(input)?;
        if !desktop_portable_ids.insert(normalized.portable_object_id.clone())
            || !desktop_profile_ids.insert(normalized.profile.id.clone())
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore desktop profile is duplicated",
            ));
        }
        desktop_profiles.push(normalized);
    }
    desktop_profiles.sort_by(|left, right| {
        left.portable_object_id
            .cmp(&right.portable_object_id)
            .then_with(|| left.profile.id.cmp(&right.profile.id))
    });

    Ok(NormalizedRestorePlan {
        attempt_id: plan.attempt_id.clone(),
        plugin_id: plan.plugin_id.clone(),
        profile_id,
        bundle_sha256: plan.bundle_sha256.clone(),
        plan_sha256: plan.plan_sha256.clone(),
        identities,
        credentials,
        hosts,
        desktop_profiles,
    })
}

fn normalized_restore_desktop_profile_input(
    input: &SshSyncRestoreDesktopProfileInput,
) -> Result<NormalizedSshSyncRestoreDesktopProfileInput> {
    validate_portable_object_id(&input.portable_object_id)?;
    if input.profile.revision != WireSequence::new(0) {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync restored desktop profile must have revision zero",
        ));
    }
    validate_desktop_profile(&input.profile)?;
    let mut profile = input.profile.clone();
    profile.id = normalized_desktop_profile_id(&profile.id)?;
    profile.revision = WireSequence::new(1);
    Ok(NormalizedSshSyncRestoreDesktopProfileInput {
        portable_object_id: input.portable_object_id.clone(),
        profile,
    })
}

fn normalized_sync_tag_labels(labels: &[String]) -> Result<Vec<String>> {
    if labels.len() > MAX_HOST_TAGS {
        return Err(AppPersistenceError::InvalidInput(
            "too many SSH sync Host tags",
        ));
    }
    let mut labels = labels
        .iter()
        .map(|label| normalized_label(label))
        .collect::<Result<Vec<_>>>()?;
    labels.sort_by_key(|label| label.to_ascii_lowercase());
    if labels
        .windows(2)
        .any(|pair| pair[0].eq_ignore_ascii_case(&pair[1]))
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync Host tag labels are duplicated",
        ));
    }
    Ok(labels)
}

fn resolve_sync_tag_labels(
    transaction: &Transaction<'_>,
    labels: &[String],
    now: i64,
) -> Result<Vec<HostTagId>> {
    let mut ids = Vec::with_capacity(labels.len());
    for label in labels {
        let existing = transaction
            .query_row(
                "SELECT id FROM host_tags WHERE label = ?1 COLLATE NOCASE",
                [label],
                |row| parse_id(row.get::<_, String>(0)?, HostTagId::parse),
            )
            .optional()?;
        let id = if let Some(id) = existing {
            id
        } else {
            let id = HostTagId::new();
            transaction.execute(
                "INSERT INTO host_tags (id, label, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 1, ?3, ?3)",
                params![id.as_str(), label, now],
            )?;
            id
        };
        ids.push(id);
    }
    ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(ids)
}

fn validate_restore_host_tag_labels(
    connection: &Connection,
    host_id: &HostId,
    expected: &[String],
) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT tag.label FROM host_tag_assignments AS assignment
         JOIN host_tags AS tag ON tag.id = assignment.tag_id
         WHERE assignment.host_id = ?1 ORDER BY tag.label COLLATE NOCASE",
    )?;
    let actual = statement
        .query_map([host_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn topologically_sorted_restore_hosts(
    mut hosts: Vec<NormalizedRestoreHost>,
) -> Result<Vec<NormalizedRestoreHost>> {
    let planned = hosts
        .iter()
        .map(|(id, _, _, _, _)| id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let mut sorted = Vec::with_capacity(hosts.len());
    let mut resolved = BTreeSet::new();
    while !hosts.is_empty() {
        let position = hosts.iter().position(|(_, _, normalized, _, _)| {
            normalized
                .jump_host_ids
                .iter()
                .all(|jump| !planned.contains(jump.as_str()) || resolved.contains(jump.as_str()))
        });
        let Some(position) = position else {
            return Err(AppPersistenceError::InvalidInput(
                "SSH sync restore jump Host graph contains a cycle",
            ));
        };
        let host = hosts.remove(position);
        resolved.insert(host.0.as_str().to_owned());
        sorted.push(host);
    }
    Ok(sorted)
}

fn validate_restore_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid SSH sync restore sha256 digest",
        ));
    }
    Ok(())
}

fn normalized_secret_ref_ids(values: &[SecretRefId]) -> Result<Vec<SecretRefId>> {
    let mut values = values.to_vec();
    values.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    values.dedup();
    if values.len() > 4_096 {
        return Err(AppPersistenceError::InvalidInput(
            "too many SSH sync restore secret references",
        ));
    }
    Ok(values)
}

fn encode_secret_ref_ids(values: &[SecretRefId]) -> Result<String> {
    serde_json::to_string(&values.iter().map(SecretRefId::as_str).collect::<Vec<_>>())
        .map_err(|_| AppPersistenceError::InvalidInput("secret references cannot be serialized"))
}

fn read_restore_saga(row: &rusqlite::Row<'_>) -> rusqlite::Result<SshSyncRestoreSagaRecord> {
    let values =
        serde_json::from_str::<Vec<String>>(&row.get::<_, String>(5)?).map_err(invalid_column)?;
    let secret_ref_ids = values
        .into_iter()
        .map(|value| parse_id(value, SecretRefId::parse))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(SshSyncRestoreSagaRecord {
        attempt_id: parse_id(row.get::<_, String>(0)?, OperationId::parse)?,
        plugin_id: parse_id(row.get::<_, String>(1)?, PluginId::parse)?,
        profile_id: row.get(2)?,
        bundle_sha256: row.get(3)?,
        plan_sha256: row.get(4)?,
        secret_ref_ids,
        created_at_unix_ms: row.get(6)?,
        updated_at_unix_ms: row.get(7)?,
    })
}

fn get_restore_saga(
    connection: &Connection,
    attempt_id: &OperationId,
) -> Result<Option<SshSyncRestoreSagaRecord>> {
    connection
        .query_row(
            "SELECT attempt_id, plugin_id, profile_id, bundle_sha256, plan_sha256,
                    secret_ref_ids_json, created_at_ms, updated_at_ms
             FROM ssh_sync_restore_sagas WHERE attempt_id = ?1",
            [attempt_id.as_str()],
            read_restore_saga,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn same_restore_saga(existing: &SshSyncRestoreSagaRecord, input: &NormalizedRestoreSaga) -> bool {
    existing.attempt_id == input.attempt_id
        && existing.plugin_id == input.plugin_id
        && existing.profile_id == input.profile_id
        && existing.bundle_sha256 == input.bundle_sha256
        && existing.plan_sha256 == input.plan_sha256
        && existing.secret_ref_ids == input.secret_ref_ids
}

fn same_restore_plan_saga(
    existing: &SshSyncRestoreSagaRecord,
    plan: &NormalizedRestorePlan,
    new_secret_ref_ids: &[SecretRefId],
) -> bool {
    existing.attempt_id == plan.attempt_id
        && existing.plugin_id == plan.plugin_id
        && existing.profile_id == plan.profile_id
        && existing.bundle_sha256 == plan.bundle_sha256
        && existing.plan_sha256 == plan.plan_sha256
        && existing.secret_ref_ids == new_secret_ref_ids
}

fn preview_restore_plan(
    connection: &Connection,
    plan: &NormalizedRestorePlan,
) -> Result<SshSyncRestorePreview> {
    let planned_identities = plan
        .identities
        .iter()
        .map(|input| input.identity_id.as_str())
        .collect::<BTreeSet<_>>();
    let planned_credentials = plan
        .credentials
        .iter()
        .map(|input| (input.credential_ref_id.as_str(), input))
        .collect::<BTreeMap<_, _>>();
    let planned_hosts = plan
        .hosts
        .iter()
        .map(|(id, _, _, _, _)| id.as_str())
        .collect::<BTreeSet<_>>();
    let mut preview = SshSyncRestorePreview {
        create_count: 0,
        already_applied_count: 0,
        conflict_count: 0,
        new_secret_ref_ids: Vec::new(),
    };
    for identity in &plan.identities {
        add_restore_disposition(
            &mut preview,
            restore_identity_disposition(connection, identity)?,
        )?;
    }
    for credential in &plan.credentials {
        let mut disposition = restore_credential_disposition(connection, credential)?;
        if disposition == RestoreObjectDisposition::Create
            && !restore_credential_create_references_valid(
                connection,
                credential,
                &planned_identities,
            )?
        {
            disposition = RestoreObjectDisposition::Conflict;
        }
        if disposition == RestoreObjectDisposition::Create {
            for secret_ref in restore_credential_secret_refs(credential) {
                if !secret_ref_is_referenced(connection, secret_ref)? {
                    preview.new_secret_ref_ids.push(secret_ref.clone());
                }
            }
        }
        add_restore_disposition(&mut preview, disposition)?;
    }
    for (host_id, request, normalized, tag_labels, automation) in &plan.hosts {
        let mut disposition = restore_host_disposition(
            connection, host_id, request, normalized, tag_labels, automation,
        )?;
        if disposition == RestoreObjectDisposition::Create
            && !restore_host_create_references_valid(
                connection,
                normalized,
                automation,
                &planned_identities,
                &planned_credentials,
                &planned_hosts,
            )?
        {
            disposition = RestoreObjectDisposition::Conflict;
        }
        if disposition == RestoreObjectDisposition::Create {
            for secret_ref in ssh_sync_login_automation_secret_refs(automation) {
                if !secret_ref_is_referenced(connection, secret_ref)? {
                    preview.new_secret_ref_ids.push(secret_ref.clone());
                }
            }
        }
        add_restore_disposition(&mut preview, disposition)?;
        if disposition == RestoreObjectDisposition::AlreadyApplied {
            validate_restore_host_tag_labels(connection, host_id, tag_labels)?;
        }
    }
    for desktop in &plan.desktop_profiles {
        let mut disposition = restore_desktop_profile_disposition(connection, desktop)?;
        if disposition == RestoreObjectDisposition::Create
            && !restore_desktop_profile_create_references_valid(
                connection,
                &desktop.profile,
                &planned_credentials,
                &planned_hosts,
            )?
        {
            disposition = RestoreObjectDisposition::Conflict;
        }
        add_restore_disposition(&mut preview, disposition)?;
    }
    preview
        .new_secret_ref_ids
        .sort_by(|left, right| left.as_str().cmp(right.as_str()));
    preview.new_secret_ref_ids.dedup();
    Ok(preview)
}

fn add_restore_disposition(
    preview: &mut SshSyncRestorePreview,
    disposition: RestoreObjectDisposition,
) -> Result<()> {
    let target = match disposition {
        RestoreObjectDisposition::Create => &mut preview.create_count,
        RestoreObjectDisposition::AlreadyApplied => &mut preview.already_applied_count,
        RestoreObjectDisposition::Conflict => &mut preview.conflict_count,
    };
    *target = target
        .checked_add(1)
        .ok_or(AppPersistenceError::InvalidStoredData)?;
    Ok(())
}

fn restore_identity_disposition(
    connection: &Connection,
    input: &SshSyncRestoreIdentityInput,
) -> Result<RestoreObjectDisposition> {
    let existing = connection
        .query_row(
            "SELECT label, username, state_version FROM identities WHERE id = ?1",
            [input.identity_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(match existing {
        None => RestoreObjectDisposition::Create,
        Some((label, username, 1)) if label == input.label && username == input.username => {
            RestoreObjectDisposition::AlreadyApplied
        }
        Some(_) => RestoreObjectDisposition::Conflict,
    })
}

fn restore_credential_disposition(
    connection: &Connection,
    input: &SshSyncRestoreCredentialInput,
) -> Result<RestoreObjectDisposition> {
    let mut statement = connection.prepare(&format!(
        "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
         {CREDENTIAL_RECORD_JOINS}
         WHERE credential_refs.id = ?1
            OR credential_refs.import_operation_id = ?2
            OR credential_refs.import_idempotency_key = ?3
         ORDER BY credential_refs.id"
    ))?;
    let existing = statement
        .query_map(
            params![
                input.credential_ref_id.as_str(),
                input.operation_id.as_str(),
                input.idempotency_key,
            ],
            read_credential_record,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if existing.is_empty() {
        return Ok(RestoreObjectDisposition::Create);
    }
    if existing.len() == 1 && same_restore_credential(&existing[0], input) {
        Ok(RestoreObjectDisposition::AlreadyApplied)
    } else {
        Ok(RestoreObjectDisposition::Conflict)
    }
}

fn same_restore_credential(
    existing: &CredentialRecord,
    input: &SshSyncRestoreCredentialInput,
) -> bool {
    existing.credential_ref_id == input.credential_ref_id
        && existing.identity_id == input.identity_id
        && existing.priority == input.priority
        && existing.label == input.label
        && existing.state_version == WireSequence::new(1)
        && existing.import_operation_id.as_ref() == Some(&input.operation_id)
        && existing.import_idempotency_key.as_deref() == Some(&input.idempotency_key)
        && existing.import_state == CredentialImportState::Ready
        && match (&existing.details, &input.material) {
            (
                CredentialRecordDetails::Password {
                    secret_ref_id: left,
                },
                SshSyncRestoreCredentialMaterial::Password {
                    secret_ref_id: right,
                },
            ) => left == right,
            (
                CredentialRecordDetails::PrivateKey {
                    secret_ref_id: left,
                    passphrase_secret_ref_id: left_passphrase,
                    public_key_algorithm: left_algorithm,
                    public_key_fingerprint: left_fingerprint,
                },
                SshSyncRestoreCredentialMaterial::PrivateKey {
                    secret_ref_id: right,
                    passphrase_secret_ref_id: right_passphrase,
                    public_key_algorithm: right_algorithm,
                    public_key_fingerprint: right_fingerprint,
                },
            ) => {
                left == right
                    && left_passphrase == right_passphrase
                    && left_algorithm.as_deref() == Some(right_algorithm)
                    && left_fingerprint.as_deref() == Some(right_fingerprint)
            }
            (
                CredentialRecordDetails::KeyboardInteractive { max_rounds: left },
                SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds: right },
            ) => left == right,
            _ => false,
        }
}

fn restore_desktop_profile_disposition(
    connection: &Connection,
    input: &NormalizedSshSyncRestoreDesktopProfileInput,
) -> Result<RestoreObjectDisposition> {
    let existing = connection
        .query_row(
            "SELECT profile_json, revision FROM desktop_profiles WHERE id = ?1",
            [input.profile.id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((json, revision)) = existing else {
        return Ok(RestoreObjectDisposition::Create);
    };
    if revision != 1 || json.len() > 8192 {
        return Ok(RestoreObjectDisposition::Conflict);
    }
    let stored = serde_json::from_str::<DesktopProfile>(&json)
        .map_err(|_| AppPersistenceError::InvalidStoredData)?;
    if validate_desktop_profile(&stored).is_err()
        || stored.id != input.profile.id
        || stored.revision != WireSequence::new(1)
    {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(if stored == input.profile {
        RestoreObjectDisposition::AlreadyApplied
    } else {
        RestoreObjectDisposition::Conflict
    })
}

fn insert_restore_desktop_profile(
    transaction: &Transaction<'_>,
    profile: &DesktopProfile,
) -> Result<()> {
    let json = serde_json::to_string(profile)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
    if json.len() > 8192 {
        return Err(AppPersistenceError::InvalidInput("invalid desktop profile"));
    }
    let inserted = transaction.execute(
        "INSERT INTO desktop_profiles
         (id, profile_json, revision, host_id, gateway_host_id, credential_ref_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            profile.id,
            json,
            u64_to_i64(profile.revision.get())?,
            profile.host_id.as_ref().map(HostId::as_str),
            profile.gateway_host_id.as_ref().map(HostId::as_str),
            profile
                .credential_ref_id
                .as_ref()
                .map(CredentialRefId::as_str),
        ],
    )?;
    if inserted == 1 {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn restore_host_operation_json(
    normalized: &NormalizedConfiguredHostCreate,
    tag_labels: &[String],
    automation: &SshSyncLoginAutomation,
) -> Result<String> {
    let serialized = if automation == &SshSyncLoginAutomation::disabled() {
        if tag_labels.is_empty() {
            serde_json::to_string(normalized)
        } else {
            serde_json::to_string(&(normalized, tag_labels))
        }
    } else {
        serde_json::to_string(&(normalized, tag_labels, automation))
    };
    serialized.map_err(|_| {
        AppPersistenceError::InvalidInput("configured host request cannot be serialized")
    })
}

fn restore_host_disposition(
    connection: &Connection,
    host_id: &HostId,
    request: &HostConfiguredCreateRequest,
    normalized: &NormalizedConfiguredHostCreate,
    tag_labels: &[String],
    automation: &SshSyncLoginAutomation,
) -> Result<RestoreObjectDisposition> {
    let idempotency_key = validated_idempotency_key(&request.idempotency_key)?;
    let request_json = restore_host_operation_json(normalized, tag_labels, automation)?;
    let existing = lookup_configured_host_create_connection(
        connection,
        &request.operation_id,
        &idempotency_key,
    )?;
    if existing.is_empty() {
        let host_exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1)",
            [host_id.as_str()],
            |row| row.get(0),
        )?;
        return Ok(if host_exists {
            RestoreObjectDisposition::Conflict
        } else {
            RestoreObjectDisposition::Create
        });
    }
    Ok(
        if existing.len() == 1
            && existing[0].0 == request.operation_id
            && existing[0].1 == idempotency_key
            && existing[0].2 == request_json
            && existing[0].3 == *host_id
            && restore_host_revisions_are_initial(connection, host_id)?
        {
            RestoreObjectDisposition::AlreadyApplied
        } else {
            RestoreObjectDisposition::Conflict
        },
    )
}

fn restore_host_revisions_are_initial(connection: &Connection, host_id: &HostId) -> Result<bool> {
    connection
        .query_row(
            "SELECT hosts.state_version, host_route_plans.revision,
                    host_authentication_plans.revision, host_algorithm_policies.revision,
                    host_heartbeat_policies.revision, host_monitoring_policies.revision,
                    host_login_automations.revision
             FROM hosts
             JOIN host_route_plans ON host_route_plans.host_id = hosts.id
             JOIN host_authentication_plans ON host_authentication_plans.host_id = hosts.id
             JOIN host_algorithm_policies ON host_algorithm_policies.host_id = hosts.id
             JOIN host_heartbeat_policies ON host_heartbeat_policies.host_id = hosts.id
             JOIN host_monitoring_policies ON host_monitoring_policies.host_id = hosts.id
             JOIN host_login_automations ON host_login_automations.host_id = hosts.id
             WHERE hosts.id = ?1",
            [host_id.as_str()],
            |row| Ok((0..7).all(|index| row.get::<_, i64>(index).is_ok_and(|value| value == 1))),
        )
        .optional()
        .map(Option::unwrap_or_default)
        .map_err(AppPersistenceError::from)
}

fn lookup_configured_host_create_connection(
    connection: &Connection,
    operation_id: &OperationId,
    idempotency_key: &str,
) -> Result<Vec<(OperationId, String, String, HostId)>> {
    let mut statement = connection.prepare(
        "SELECT operation_id, idempotency_key, request_json, host_id
         FROM host_configured_create_operations
         WHERE operation_id = ?1 OR idempotency_key = ?2
         ORDER BY operation_id",
    )?;
    statement
        .query_map(params![operation_id.as_str(), idempotency_key], |row| {
            Ok((
                parse_id(row.get::<_, String>(0)?, OperationId::parse)?,
                row.get(1)?,
                row.get(2)?,
                parse_id(row.get::<_, String>(3)?, HostId::parse)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn restore_credential_create_references_valid(
    connection: &Connection,
    credential: &SshSyncRestoreCredentialInput,
    planned_identities: &BTreeSet<&str>,
) -> Result<bool> {
    if !planned_identities.contains(credential.identity_id.as_str())
        && !metadata_exists(connection, "identities", credential.identity_id.as_str())?
    {
        return Ok(false);
    }
    let priority_used: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM credential_refs
         WHERE identity_id = ?1 AND priority = ?2)",
        params![
            credential.identity_id.as_str(),
            i64::from(credential.priority)
        ],
        |row| row.get(0),
    )?;
    if priority_used {
        return Ok(false);
    }
    for secret_ref in restore_credential_secret_refs(credential) {
        let used: bool = connection.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_profile_states WHERE sync_key_secret_ref_id = ?1
             )",
            [secret_ref.as_str()],
            |row| row.get(0),
        )?;
        if used {
            return Ok(false);
        }
    }
    Ok(true)
}

fn restore_host_create_references_valid(
    connection: &Connection,
    host: &NormalizedConfiguredHostCreate,
    automation: &SshSyncLoginAutomation,
    planned_identities: &BTreeSet<&str>,
    planned_credentials: &BTreeMap<&str, &SshSyncRestoreCredentialInput>,
    planned_hosts: &BTreeSet<&str>,
) -> Result<bool> {
    for secret_ref in ssh_sync_login_automation_secret_refs(automation) {
        let is_sync_key: bool = connection.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM ssh_sync_profile_states WHERE sync_key_secret_ref_id = ?1
             )",
            [secret_ref.as_str()],
            |row| row.get(0),
        )?;
        if is_sync_key {
            return Ok(false);
        }
    }
    if let Some(identity_id) = &host.identity_id
        && !planned_identities.contains(identity_id.as_str())
        && !metadata_exists(connection, "identities", identity_id.as_str())?
    {
        return Ok(false);
    }
    if let Some(group_id) = &host.group_id
        && !metadata_exists(connection, "host_groups", group_id.as_str())?
    {
        return Ok(false);
    }
    for tag_id in &host.tag_ids {
        if !metadata_exists(connection, "host_tags", tag_id.as_str())? {
            return Ok(false);
        }
    }
    for jump_id in &host.jump_host_ids {
        if !planned_hosts.contains(jump_id.as_str())
            && !metadata_exists(connection, "hosts", jump_id.as_str())?
        {
            return Ok(false);
        }
    }
    for credential_id in &host.credential_ref_ids {
        match planned_credentials.get(credential_id.as_str()) {
            Some(planned) if Some(&planned.identity_id) == host.identity_id.as_ref() => {}
            Some(_) => return Ok(false),
            None => {
                let belongs_to_identity: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM credential_refs
                     WHERE id = ?1 AND identity_id = ?2 AND import_state = 'ready')",
                    params![
                        credential_id.as_str(),
                        host.identity_id.as_ref().map(IdentityId::as_str)
                    ],
                    |row| row.get(0),
                )?;
                if !belongs_to_identity {
                    return Ok(false);
                }
            }
        }
    }
    if let Some(proxy_credential) = route_proxy_credential(&host.ingress) {
        match planned_credentials.get(proxy_credential.as_str()) {
            Some(planned)
                if matches!(
                    planned.material,
                    SshSyncRestoreCredentialMaterial::Password { .. }
                ) => {}
            Some(_) => return Ok(false),
            None => {
                let is_ready_password: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM credential_refs
                     WHERE id = ?1 AND kind = 'password' AND import_state = 'ready')",
                    [proxy_credential.as_str()],
                    |row| row.get(0),
                )?;
                if !is_ready_password {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn restore_desktop_profile_create_references_valid(
    connection: &Connection,
    profile: &DesktopProfile,
    planned_credentials: &BTreeMap<&str, &SshSyncRestoreCredentialInput>,
    planned_hosts: &BTreeSet<&str>,
) -> Result<bool> {
    for host_id in [&profile.host_id, &profile.gateway_host_id]
        .into_iter()
        .flatten()
    {
        if !planned_hosts.contains(host_id.as_str())
            && !metadata_exists(connection, "hosts", host_id.as_str())?
        {
            return Ok(false);
        }
    }
    let Some(credential_ref_id) = &profile.credential_ref_id else {
        return Ok(true);
    };
    match planned_credentials.get(credential_ref_id.as_str()) {
        Some(credential) => Ok(matches!(
            credential.material,
            SshSyncRestoreCredentialMaterial::Password { .. }
        )),
        None => connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM credential_refs
                 WHERE id = ?1 AND kind = 'password' AND import_state = 'ready')",
                [credential_ref_id.as_str()],
                |row| row.get(0),
            )
            .map_err(AppPersistenceError::from),
    }
}

fn metadata_exists(connection: &Connection, table: &str, id: &str) -> Result<bool> {
    let table = match table {
        "identities" => "identities",
        "hosts" => "hosts",
        "host_groups" => "host_groups",
        "host_tags" => "host_tags",
        _ => return Err(AppPersistenceError::InvalidInput("invalid metadata table")),
    };
    connection
        .query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
            [id],
            |row| row.get(0),
        )
        .map_err(AppPersistenceError::from)
}

fn route_proxy_credential(ingress: &RouteIngress) -> Option<&CredentialRefId> {
    match ingress {
        RouteIngress::DirectTcp => None,
        RouteIngress::HttpConnectProxy {
            proxy_auth_credential_ref_id,
            ..
        }
        | RouteIngress::Socks5Proxy {
            proxy_auth_credential_ref_id,
            ..
        } => proxy_auth_credential_ref_id.as_ref(),
    }
}

fn restore_credential_secret_refs(input: &SshSyncRestoreCredentialInput) -> Vec<&SecretRefId> {
    match &input.material {
        SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => vec![secret_ref_id],
        SshSyncRestoreCredentialMaterial::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            ..
        } => std::iter::once(secret_ref_id)
            .chain(passphrase_secret_ref_id.iter())
            .collect(),
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => Vec::new(),
    }
}

fn publish_restore_plan(
    transaction: &Transaction<'_>,
    plan: &NormalizedRestorePlan,
    now: i64,
) -> Result<()> {
    for identity in &plan.identities {
        if restore_identity_disposition(transaction, identity)? == RestoreObjectDisposition::Create
        {
            transaction.execute(
                "INSERT INTO identities
                 (id, label, username, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                params![
                    identity.identity_id.as_str(),
                    identity.label,
                    identity.username,
                    now,
                ],
            )?;
        }
    }
    for credential in &plan.credentials {
        if restore_credential_disposition(transaction, credential)?
            == RestoreObjectDisposition::Create
        {
            insert_restore_credential(transaction, credential, now)?;
        }
    }
    for (host_id, request, normalized, tag_labels, automation) in &plan.hosts {
        if restore_host_disposition(
            transaction,
            host_id,
            request,
            normalized,
            tag_labels,
            automation,
        )? == RestoreObjectDisposition::Create
        {
            insert_restore_host(
                transaction,
                host_id,
                request,
                normalized,
                tag_labels,
                automation,
                now,
            )?;
        }
    }
    for desktop in &plan.desktop_profiles {
        if restore_desktop_profile_disposition(transaction, desktop)?
            == RestoreObjectDisposition::Create
        {
            insert_restore_desktop_profile(transaction, &desktop.profile)?;
        }
    }
    Ok(())
}

fn insert_restore_credential(
    transaction: &Transaction<'_>,
    input: &SshSyncRestoreCredentialInput,
    now: i64,
) -> Result<()> {
    let kind = match input.material {
        SshSyncRestoreCredentialMaterial::Password { .. } => "password",
        SshSyncRestoreCredentialMaterial::PrivateKey { .. } => "private_key",
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => "keyboard_interactive",
    };
    transaction.execute(
        "INSERT INTO credential_refs
         (id, identity_id, kind, priority, label, state_version, created_at_ms,
          updated_at_ms, import_operation_id, import_idempotency_key, import_state)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6, ?7, ?8, 'ready')",
        params![
            input.credential_ref_id.as_str(),
            input.identity_id.as_str(),
            kind,
            i64::from(input.priority),
            input.label,
            now,
            input.operation_id.as_str(),
            input.idempotency_key,
        ],
    )?;
    match &input.material {
        SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => {
            transaction.execute(
                "INSERT INTO credential_password_details (credential_ref_id) VALUES (?1)",
                [input.credential_ref_id.as_str()],
            )?;
            insert_restore_secret_slot(
                transaction,
                &input.credential_ref_id,
                0,
                "password",
                secret_ref_id,
                "Password",
            )?;
        }
        SshSyncRestoreCredentialMaterial::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            public_key_algorithm,
            public_key_fingerprint,
        } => {
            transaction.execute(
                "INSERT INTO credential_private_key_details
                 (credential_ref_id, public_key_algorithm, public_key_fingerprint)
                 VALUES (?1, ?2, ?3)",
                params![
                    input.credential_ref_id.as_str(),
                    public_key_algorithm,
                    public_key_fingerprint,
                ],
            )?;
            insert_restore_secret_slot(
                transaction,
                &input.credential_ref_id,
                0,
                "private_key",
                secret_ref_id,
                "Private key",
            )?;
            if let Some(passphrase) = passphrase_secret_ref_id {
                insert_restore_secret_slot(
                    transaction,
                    &input.credential_ref_id,
                    1,
                    "passphrase",
                    passphrase,
                    "Passphrase",
                )?;
            }
        }
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds } => {
            transaction.execute(
                "INSERT INTO credential_keyboard_interactive_details
                 (credential_ref_id, max_rounds) VALUES (?1, ?2)",
                params![input.credential_ref_id.as_str(), i64::from(*max_rounds)],
            )?;
        }
    }
    Ok(())
}

fn insert_restore_secret_slot(
    transaction: &Transaction<'_>,
    credential_ref_id: &CredentialRefId,
    slot_index: u8,
    slot_kind: &str,
    secret_ref_id: &SecretRefId,
    label: &str,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO credential_secret_slots
         (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            credential_ref_id.as_str(),
            i64::from(slot_index),
            slot_kind,
            secret_ref_id.as_str(),
            label,
        ],
    )?;
    Ok(())
}

fn insert_restore_host(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    request: &HostConfiguredCreateRequest,
    normalized: &NormalizedConfiguredHostCreate,
    tag_labels: &[String],
    automation: &SshSyncLoginAutomation,
    now: i64,
) -> Result<()> {
    let operation_request_json = restore_host_operation_json(normalized, tag_labels, automation)?;
    let mut resolved = normalized.clone();
    resolved.tag_ids = resolve_sync_tag_labels(transaction, tag_labels, now)?;
    let normalized = &resolved;
    if let Some(identity_id) = &normalized.identity_id
        && !metadata_exists(transaction, "identities", identity_id.as_str())?
    {
        return Err(AppPersistenceError::NotFound);
    }
    if let Some(group_id) = &normalized.group_id {
        require_metadata_exists(transaction, "host_groups", group_id.as_str())?;
    }
    for tag_id in &normalized.tag_ids {
        require_metadata_exists(transaction, "host_tags", tag_id.as_str())?;
    }
    transaction.execute(
        "INSERT INTO hosts
         (id, label, address, normalized_address, port, username, identity_id, favorite,
          state_version, created_at_ms, updated_at_ms, group_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?10)",
        params![
            host_id.as_str(),
            normalized.label,
            normalized.address,
            normalized.normalized_address,
            i64::from(normalized.port),
            normalized.username,
            normalized.identity_id.as_ref().map(IdentityId::as_str),
            normalized.favorite,
            now,
            normalized.group_id.as_ref().map(HostGroupId::as_str),
        ],
    )?;
    for tag_id in &normalized.tag_ids {
        transaction.execute(
            "INSERT INTO host_tag_assignments (host_id, tag_id) VALUES (?1, ?2)",
            params![host_id.as_str(), tag_id.as_str()],
        )?;
    }
    insert_host_config_defaults(transaction, host_id, now)?;
    initialize_configured_route_plan(
        transaction,
        host_id,
        &normalized.ingress,
        &normalized.jump_host_ids,
        now,
    )?;
    initialize_configured_authentication_plan(
        transaction,
        host_id,
        normalized.identity_id.as_ref(),
        normalized.authentication_mode,
        &normalized.credential_ref_ids,
        now,
    )?;
    initialize_configured_algorithm_policy(
        transaction,
        host_id,
        &normalized.algorithm_policy_id,
        &normalized.compatibility_exceptions,
        now,
    )?;
    initialize_configured_heartbeat_policy(
        transaction,
        host_id,
        &normalized.heartbeat_policy,
        now,
    )?;
    initialize_configured_monitoring_policy(
        transaction,
        host_id,
        &normalized.monitoring_policy,
        now,
    )?;
    initialize_ssh_sync_login_automation(transaction, host_id, automation, now)?;
    transaction.execute(
        "INSERT INTO host_configured_create_operations
         (operation_id, idempotency_key, request_json, host_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            request.operation_id.as_str(),
            validated_idempotency_key(&request.idempotency_key)?,
            operation_request_json,
            host_id.as_str(),
            now,
        ],
    )?;
    Ok(())
}

fn restore_plan_commit_confirmed(
    connection: &Connection,
    plan: &NormalizedRestorePlan,
) -> Result<bool> {
    let preview = preview_restore_plan(connection, plan)?;
    Ok(preview.create_count == 0
        && preview.conflict_count == 0
        && get_restore_saga(connection, &plan.attempt_id)?.is_none())
}

fn normalized_configured_host_create(
    request: &HostConfiguredCreateRequest,
) -> Result<NormalizedConfiguredHostCreate> {
    let endpoint = Endpoint::parse(&request.address, request.port)?;
    let label = if request.label.trim().is_empty() {
        endpoint.normalized_address().to_owned()
    } else {
        normalized_label(&request.label)?
    };
    let username = normalized_optional(request.username.as_deref(), 128, "username is too long")?;
    let ingress = validated_route_ingress(&request.ingress)?;
    validate_unique_bounded_ids(
        &request.jump_host_ids,
        MAX_JUMP_HOPS,
        "jump host chain is invalid",
    )?;
    validate_unique_bounded_ids(
        &request.credential_ref_ids,
        MAX_AUTHENTICATION_CREDENTIALS,
        "authentication plan is invalid",
    )?;
    match request.authentication_mode {
        AuthenticationPlanMode::Identity if !request.credential_ref_ids.is_empty() => {
            return Err(AppPersistenceError::InvalidInput(
                "identity authentication cannot contain host override credentials",
            ));
        }
        AuthenticationPlanMode::HostOverride if request.credential_ref_ids.is_empty() => {
            return Err(AppPersistenceError::InvalidInput(
                "host authentication override must contain a credential",
            ));
        }
        AuthenticationPlanMode::HostOverride if request.identity_id.is_none() => {
            return Err(AppPersistenceError::InvalidInput(
                "host authentication override requires an identity",
            ));
        }
        _ => {}
    }
    if request.login_automation_confirmed && !request.login_automation_enabled {
        return Err(AppPersistenceError::InvalidInput(
            "disabled login automation cannot be confirmed",
        ));
    }
    if request.staged_password_id.is_some()
        && (request.identity_id.is_some()
            || request.authentication_mode != AuthenticationPlanMode::Identity
            || !request.credential_ref_ids.is_empty())
    {
        return Err(AppPersistenceError::InvalidInput(
            "staged Host password requires a dedicated identity authentication plan",
        ));
    }
    if request.tag_ids.len() > MAX_HOST_TAGS
        || request
            .tag_ids
            .iter()
            .enumerate()
            .any(|(index, tag_id)| request.tag_ids[..index].contains(tag_id))
    {
        return Err(AppPersistenceError::InvalidInput(
            "host tags are duplicated or exceed the limit",
        ));
    }
    let algorithm_policy_id = validated_catalog_id(
        &request.algorithm_policy_id,
        "algorithm policy id is invalid",
    )?;
    if request.compatibility_exceptions.len() > 64 {
        return Err(AppPersistenceError::InvalidInput(
            "too many algorithm compatibility exceptions",
        ));
    }
    let mut exception_keys = BTreeSet::new();
    let mut compatibility_exceptions = request
        .compatibility_exceptions
        .iter()
        .map(|exception| {
            let exception_id =
                validated_catalog_id(&exception.exception_id, "algorithm exception id is invalid")?;
            let reason = normalized_optional(
                exception.reason.as_deref(),
                240,
                "algorithm exception reason is too long",
            )?;
            let key = (
                algorithm_category_to_db(exception.category),
                exception_id.clone(),
            );
            if !exception_keys.insert(key) {
                return Err(AppPersistenceError::InvalidInput(
                    "algorithm exception is duplicated",
                ));
            }
            Ok(AlgorithmCompatibilityException {
                category: exception.category,
                exception_id,
                reason,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    compatibility_exceptions.sort_by(|left, right| {
        algorithm_category_to_db(left.category)
            .cmp(algorithm_category_to_db(right.category))
            .then_with(|| left.exception_id.cmp(&right.exception_id))
    });
    let heartbeat_policy = validated_heartbeat_policy(&request.heartbeat_policy)?;
    let monitoring_policy = validated_monitoring_policy(&request.monitoring_policy)?;
    let login_automation_steps = request
        .login_automation_steps
        .iter()
        .map(|step| match step {
            HostCreateLoginAutomationStep::Expect {
                literal_text,
                timeout_seconds,
            } => LoginAutomationStepInput::Expect {
                literal_text: literal_text.clone(),
                timeout_seconds: *timeout_seconds,
            },
            HostCreateLoginAutomationStep::SendText {
                text,
                append_enter,
                timeout_seconds,
            } => LoginAutomationStepInput::SendText {
                text: text.clone(),
                append_enter: *append_enter,
                timeout_seconds: *timeout_seconds,
            },
        })
        .collect::<Vec<_>>();
    let login_automation_steps =
        validated_login_automation(request.login_automation_enabled, &login_automation_steps)?;
    let mut tag_ids = request.tag_ids.clone();
    tag_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(NormalizedConfiguredHostCreate {
        label,
        address: request.address.trim().to_owned(),
        normalized_address: endpoint.normalized_address().to_owned(),
        port: request.port,
        username,
        identity_id: request.identity_id.clone(),
        favorite: request.favorite,
        group_id: request.group_id.clone(),
        tag_ids,
        ingress,
        jump_host_ids: request.jump_host_ids.clone(),
        authentication_mode: request.authentication_mode,
        credential_ref_ids: request.credential_ref_ids.clone(),
        algorithm_policy_id,
        compatibility_exceptions,
        heartbeat_policy,
        monitoring_policy,
        login_automation_enabled: request.login_automation_enabled,
        login_automation_confirmed: request.login_automation_confirmed,
        login_automation_steps,
        staged_password_id: request.staged_password_id.clone(),
    })
}

fn lookup_configured_host_create(
    transaction: &Transaction<'_>,
    operation_id: &OperationId,
    idempotency_key: &str,
) -> Result<Vec<(OperationId, String, String, HostId)>> {
    let mut statement = transaction.prepare(
        "SELECT operation_id, idempotency_key, request_json, host_id
         FROM host_configured_create_operations
         WHERE operation_id = ?1 OR idempotency_key = ?2
         ORDER BY operation_id",
    )?;
    statement
        .query_map(params![operation_id.as_str(), idempotency_key], |row| {
            Ok((
                parse_id(row.get::<_, String>(0)?, OperationId::parse)?,
                row.get(1)?,
                row.get(2)?,
                parse_id(row.get::<_, String>(3)?, HostId::parse)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn require_staged_host_create_password(
    transaction: &Transaction<'_>,
    staged_password_id: &HostCreatePasswordStageId,
    operation_id: &OperationId,
    idempotency_key: &str,
    now: i64,
) -> Result<HostCreatePasswordStageRecord> {
    let staged =
        lookup_host_create_password_stage(transaction, "stage_id", staged_password_id.as_str())?
            .ok_or(AppPersistenceError::NotFound)?;
    if staged.operation_id != *operation_id
        || staged.idempotency_key != idempotency_key
        || staged.state != HostCreatePasswordStageState::Staged
        || staged.expires_at_unix_ms <= now
    {
        return Err(AppPersistenceError::IdempotencyConflict);
    }
    Ok(staged)
}

fn insert_ready_password_credential(
    transaction: &Transaction<'_>,
    identity_id: &IdentityId,
    secret_ref_id: &SecretRefId,
    label: &str,
    now: i64,
) -> Result<CredentialRefId> {
    let credential_ref_id = CredentialRefId::new();
    transaction.execute(
        "INSERT INTO credential_refs
         (id, identity_id, kind, priority, label, state_version, created_at_ms,
          updated_at_ms, import_operation_id, import_idempotency_key, import_state)
         VALUES (?1, ?2, 'password', 0, ?3, 1, ?4, ?4, NULL, NULL, 'ready')",
        params![credential_ref_id.as_str(), identity_id.as_str(), label, now],
    )?;
    transaction.execute(
        "INSERT INTO credential_password_details (credential_ref_id) VALUES (?1)",
        [credential_ref_id.as_str()],
    )?;
    transaction.execute(
        "INSERT INTO credential_secret_slots
         (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
         VALUES (?1, 0, 'password', ?2, ?3)",
        params![credential_ref_id.as_str(), secret_ref_id.as_str(), label],
    )?;
    Ok(credential_ref_id)
}

#[allow(clippy::too_many_arguments)]
fn insert_import_host(
    transaction: &Transaction<'_>,
    label: &str,
    address: &str,
    normalized_address: &str,
    port: u16,
    username: Option<&str>,
    now: i64,
) -> Result<HostSummary> {
    let host_id = HostId::new();
    let label = if label.is_empty() {
        normalized_address.to_owned()
    } else {
        label.to_owned()
    };
    let username = username.map(str::to_owned);
    transaction.execute(
        "INSERT INTO hosts
         (id, label, address, normalized_address, port, username, identity_id, favorite,
          state_version, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 0, 1, ?7, ?7)",
        params![
            host_id.as_str(),
            label,
            address,
            normalized_address,
            i64::from(port),
            username,
            now,
        ],
    )?;
    insert_host_config_defaults(transaction, &host_id, now)?;
    Ok(HostSummary {
        host_id,
        label,
        address: address.to_owned(),
        normalized_address: normalized_address.to_owned(),
        port,
        username,
        identity_id: None,
        favorite: false,
        has_ready_credential: false,
        state_version: WireSequence::new(1),
    })
}

fn initialize_import_route_plan(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    ingress: &RouteIngress,
    jump_host_ids: &[HostId],
    now: i64,
) -> Result<()> {
    validate_unique_bounded_ids(jump_host_ids, MAX_JUMP_HOPS, "jump host chain is invalid")?;
    validate_route_graph(transaction, host_id, jump_host_ids)?;
    let encoded = route_ingress_to_db(ingress);
    if let Some(proxy_auth_credential_ref_id) = &encoded.proxy_auth_credential_ref_id {
        let is_ready_password: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM credential_refs
               WHERE id = ?1 AND kind = 'password' AND import_state = 'ready'
             )",
            [proxy_auth_credential_ref_id],
            |row| row.get(0),
        )?;
        if !is_ready_password {
            return Err(AppPersistenceError::InvalidInput(
                "proxy authentication requires a ready password credential",
            ));
        }
    }
    let changed = transaction.execute(
        "UPDATE host_route_plans
         SET ingress_kind = ?1, proxy_address = ?2, proxy_normalized_address = ?3,
             proxy_port = ?4, proxy_dns_mode = ?5, proxy_auth_credential_ref_id = ?6,
             updated_at_ms = ?7
         WHERE host_id = ?8 AND revision = 1",
        params![
            encoded.kind,
            encoded.address,
            encoded.normalized_address,
            encoded.port,
            encoded.dns_mode,
            encoded.proxy_auth_credential_ref_id,
            now,
            host_id.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    for (ordinal, jump_host_id) in jump_host_ids.iter().enumerate() {
        transaction.execute(
            "INSERT INTO host_route_jump_hops (host_id, ordinal, jump_host_id)
             VALUES (?1, ?2, ?3)",
            params![
                host_id.as_str(),
                usize_to_i64(ordinal)?,
                jump_host_id.as_str()
            ],
        )?;
    }
    Ok(())
}

fn initialize_configured_route_plan(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    ingress: &RouteIngress,
    jump_host_ids: &[HostId],
    now: i64,
) -> Result<()> {
    initialize_import_route_plan(transaction, host_id, ingress, jump_host_ids, now)
}

fn initialize_configured_authentication_plan(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    identity_id: Option<&IdentityId>,
    mode: AuthenticationPlanMode,
    credential_ref_ids: &[CredentialRefId],
    now: i64,
) -> Result<()> {
    for credential_ref_id in credential_ref_ids {
        let ready_for_identity: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM credential_refs
               WHERE id = ?1 AND identity_id = ?2 AND import_state = 'ready'
             )",
            params![
                credential_ref_id.as_str(),
                identity_id.map(IdentityId::as_str)
            ],
            |row| row.get(0),
        )?;
        if !ready_for_identity {
            return Err(AppPersistenceError::InvalidInput(
                "host authentication override credential must belong to the host identity",
            ));
        }
    }
    let changed = transaction.execute(
        "UPDATE host_authentication_plans SET mode = ?1, updated_at_ms = ?2
         WHERE host_id = ?3 AND revision = 1",
        params![authentication_mode_to_db(mode), now, host_id.as_str()],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    for (ordinal, credential_ref_id) in credential_ref_ids.iter().enumerate() {
        transaction.execute(
            "INSERT INTO host_authentication_credentials (host_id, ordinal, credential_ref_id)
             VALUES (?1, ?2, ?3)",
            params![
                host_id.as_str(),
                usize_to_i64(ordinal)?,
                credential_ref_id.as_str(),
            ],
        )?;
    }
    Ok(())
}

fn initialize_configured_algorithm_policy(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    policy_id: &str,
    compatibility_exceptions: &[AlgorithmCompatibilityException],
    now: i64,
) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE host_algorithm_policies SET policy_id = ?1, updated_at_ms = ?2
         WHERE host_id = ?3 AND revision = 1",
        params![policy_id, now, host_id.as_str()],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    for exception in compatibility_exceptions {
        transaction.execute(
            "INSERT INTO host_algorithm_exceptions (host_id, category, exception_id, reason)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                host_id.as_str(),
                algorithm_category_to_db(exception.category),
                exception.exception_id,
                exception.reason,
            ],
        )?;
    }
    Ok(())
}

fn initialize_configured_heartbeat_policy(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    policy: &HeartbeatPolicy,
    now: i64,
) -> Result<()> {
    let encoded = heartbeat_to_db(policy);
    let changed = transaction.execute(
        "UPDATE host_heartbeat_policies
         SET mode = ?1, interval_seconds = ?2, reply_timeout_seconds = ?3,
             failure_threshold = ?4, shell_payload_text = ?5, shell_line_ending = ?6,
             shell_user_idle_seconds = ?7, updated_at_ms = ?8
         WHERE host_id = ?9 AND revision = 1",
        params![
            encoded.mode,
            encoded.interval,
            encoded.reply_timeout,
            encoded.failure_threshold,
            encoded.payload,
            encoded.line_ending,
            encoded.user_idle,
            now,
            host_id.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(())
}

fn initialize_configured_monitoring_policy(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    policy: &MonitoringPolicy,
    now: i64,
) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE host_monitoring_policies
         SET enabled = ?1, sample_interval_seconds = ?2, sample_timeout_seconds = ?3,
             updated_at_ms = ?4
         WHERE host_id = ?5 AND revision = 1",
        params![
            policy.enabled,
            i64::from(policy.sample_interval_seconds),
            i64::from(policy.sample_timeout_seconds),
            now,
            host_id.as_str(),
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(())
}

fn initialize_configured_login_automation(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    enabled: bool,
    confirmed: bool,
    steps: &[LoginAutomationStepInput],
    now: i64,
) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE host_login_automations
         SET enabled = ?1, confirmed_revision = ?2, updated_at_ms = ?3
         WHERE host_id = ?4 AND revision = 1",
        params![
            enabled,
            if confirmed { Some(1_i64) } else { None },
            now,
            host_id.as_str()
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    for (ordinal, step) in steps.iter().enumerate() {
        let encoded = login_automation_step_to_db(step);
        if encoded.secret_ref_id.is_some() {
            return Err(AppPersistenceError::InvalidInput(
                "configured host creation cannot include secret automation steps",
            ));
        }
        transaction.execute(
            "INSERT INTO host_login_automation_steps
             (host_id, ordinal, kind, literal_text, append_enter, timeout_seconds,
              secret_ref_id, secret_label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)",
            params![
                host_id.as_str(),
                usize_to_i64(ordinal)?,
                encoded.kind,
                encoded.literal_text,
                encoded.append_enter,
                i64::from(encoded.timeout_seconds),
            ],
        )?;
    }
    Ok(())
}

/// Sync-only persistence path. SecretRef values are opaque Vault references
/// already staged by trusted Core and never cross the public Host API.
fn initialize_ssh_sync_login_automation(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    automation: &SshSyncLoginAutomation,
    now: i64,
) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE host_login_automations
         SET enabled = ?1, confirmed_revision = ?2, updated_at_ms = ?3
         WHERE host_id = ?4 AND revision = 1",
        params![
            automation.enabled,
            if automation.confirmed {
                Some(1_i64)
            } else {
                None
            },
            now,
            host_id.as_str()
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    for (ordinal, step) in automation.steps.iter().enumerate() {
        let (kind, literal_text, append_enter, timeout_seconds, secret_ref_id, secret_label) =
            match step {
                SshSyncLoginAutomationStep::Expect {
                    literal_text,
                    timeout_seconds,
                } => (
                    "expect",
                    Some(literal_text.as_str()),
                    None,
                    *timeout_seconds,
                    None,
                    None,
                ),
                SshSyncLoginAutomationStep::SendText {
                    text,
                    append_enter,
                    timeout_seconds,
                } => (
                    "send_text",
                    Some(text.as_str()),
                    Some(*append_enter),
                    *timeout_seconds,
                    None,
                    None,
                ),
                SshSyncLoginAutomationStep::SendSecret {
                    secret_ref_id,
                    secret_label,
                    append_enter,
                    timeout_seconds,
                } => (
                    "send_secret",
                    None,
                    Some(*append_enter),
                    *timeout_seconds,
                    Some(secret_ref_id.as_str()),
                    Some(secret_label.as_str()),
                ),
            };
        transaction.execute(
            "INSERT INTO host_login_automation_steps
             (host_id, ordinal, kind, literal_text, append_enter, timeout_seconds,
              secret_ref_id, secret_label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                host_id.as_str(),
                usize_to_i64(ordinal)?,
                kind,
                literal_text,
                append_enter,
                i64::from(timeout_seconds),
                secret_ref_id,
                secret_label
            ],
        )?;
    }
    Ok(())
}

fn insert_host_config_defaults(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    now: i64,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO host_route_plans
         (host_id, revision, ingress_kind, created_at_ms, updated_at_ms)
         VALUES (?1, 1, 'direct_tcp', ?2, ?2)",
        params![host_id.as_str(), now],
    )?;
    transaction.execute(
        "INSERT INTO host_authentication_plans
         (host_id, revision, mode, created_at_ms, updated_at_ms)
         VALUES (?1, 1, 'identity', ?2, ?2)",
        params![host_id.as_str(), now],
    )?;
    transaction.execute(
        "INSERT INTO host_algorithm_policies
         (host_id, revision, policy_id, created_at_ms, updated_at_ms)
         VALUES (?1, 1, ?2, ?3, ?3)",
        params![host_id.as_str(), DEFAULT_ALGORITHM_POLICY_ID, now],
    )?;
    transaction.execute(
        "INSERT INTO host_heartbeat_policies
         (host_id, revision, mode, created_at_ms, updated_at_ms)
         VALUES (?1, 1, 'disabled', ?2, ?2)",
        params![host_id.as_str(), now],
    )?;
    transaction.execute(
        "INSERT INTO host_monitoring_policies
         (host_id, revision, enabled, sample_interval_seconds, sample_timeout_seconds,
          created_at_ms, updated_at_ms)
         VALUES (?1, 1, 0, 15, 5, ?2, ?2)",
        params![host_id.as_str(), now],
    )?;
    transaction.execute(
        "INSERT INTO host_monitoring_selections (host_id, kind, stable_id)
         VALUES (?1, 'disk_mount', ?2), (?1, 'network_interface', ?3)",
        params![
            host_id.as_str(),
            ROOT_DISK_RESOURCE_ID,
            AGGREGATE_NON_LOOPBACK_NETWORK_RESOURCE_ID
        ],
    )?;
    transaction.execute(
        "INSERT INTO host_login_automations
         (host_id, revision, enabled, created_at_ms, updated_at_ms)
         VALUES (?1, 1, 0, ?2, ?2)",
        params![host_id.as_str(), now],
    )?;
    Ok(())
}

struct EncodedRouteIngress {
    kind: &'static str,
    address: Option<String>,
    normalized_address: Option<String>,
    port: Option<i64>,
    dns_mode: Option<&'static str>,
    proxy_auth_credential_ref_id: Option<String>,
}

fn validated_route_ingress(ingress: &RouteIngress) -> Result<RouteIngress> {
    match ingress {
        RouteIngress::DirectTcp => Ok(RouteIngress::DirectTcp),
        RouteIngress::HttpConnectProxy {
            endpoint,
            proxy_auth_credential_ref_id,
        } => Ok(RouteIngress::HttpConnectProxy {
            endpoint: validated_proxy_endpoint(endpoint)?,
            proxy_auth_credential_ref_id: proxy_auth_credential_ref_id.clone(),
        }),
        RouteIngress::Socks5Proxy {
            endpoint,
            dns_mode,
            proxy_auth_credential_ref_id,
        } => Ok(RouteIngress::Socks5Proxy {
            endpoint: validated_proxy_endpoint(endpoint)?,
            dns_mode: *dns_mode,
            proxy_auth_credential_ref_id: proxy_auth_credential_ref_id.clone(),
        }),
    }
}

fn validated_proxy_endpoint(endpoint: &ProxyEndpoint) -> Result<ProxyEndpoint> {
    let parsed = Endpoint::parse(&endpoint.address, endpoint.port)?;
    Ok(ProxyEndpoint {
        address: endpoint.address.trim().to_owned(),
        normalized_address: parsed.normalized_address().to_owned(),
        port: endpoint.port,
    })
}

fn route_ingress_to_db(ingress: &RouteIngress) -> EncodedRouteIngress {
    match ingress {
        RouteIngress::DirectTcp => EncodedRouteIngress {
            kind: "direct_tcp",
            address: None,
            normalized_address: None,
            port: None,
            dns_mode: None,
            proxy_auth_credential_ref_id: None,
        },
        RouteIngress::HttpConnectProxy {
            endpoint,
            proxy_auth_credential_ref_id,
        } => EncodedRouteIngress {
            kind: "http_connect_proxy",
            address: Some(endpoint.address.clone()),
            normalized_address: Some(endpoint.normalized_address.clone()),
            port: Some(i64::from(endpoint.port)),
            dns_mode: None,
            proxy_auth_credential_ref_id: proxy_auth_credential_ref_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        },
        RouteIngress::Socks5Proxy {
            endpoint,
            dns_mode,
            proxy_auth_credential_ref_id,
        } => EncodedRouteIngress {
            kind: "socks5_proxy",
            address: Some(endpoint.address.clone()),
            normalized_address: Some(endpoint.normalized_address.clone()),
            port: Some(i64::from(endpoint.port)),
            dns_mode: Some(match dns_mode {
                ProxyDnsMode::Local => "local",
                ProxyDnsMode::Proxy => "proxy",
            }),
            proxy_auth_credential_ref_id: proxy_auth_credential_ref_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn route_ingress_from_db(
    kind: &str,
    address: Option<String>,
    normalized_address: Option<String>,
    port: Option<i64>,
    dns_mode: Option<String>,
    proxy_auth_credential_ref_id: Option<String>,
) -> Result<RouteIngress> {
    if kind == "direct_tcp" {
        return if address.is_none()
            && normalized_address.is_none()
            && port.is_none()
            && dns_mode.is_none()
            && proxy_auth_credential_ref_id.is_none()
        {
            Ok(RouteIngress::DirectTcp)
        } else {
            Err(AppPersistenceError::InvalidStoredData)
        };
    }
    let endpoint = ProxyEndpoint {
        address: address.ok_or(AppPersistenceError::InvalidStoredData)?,
        normalized_address: normalized_address.ok_or(AppPersistenceError::InvalidStoredData)?,
        port: u16::try_from(port.ok_or(AppPersistenceError::InvalidStoredData)?)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
    };
    let auth = proxy_auth_credential_ref_id
        .map(|value| {
            CredentialRefId::parse(value).map_err(|_| AppPersistenceError::InvalidStoredData)
        })
        .transpose()?;
    match kind {
        "http_connect_proxy" if dns_mode.is_none() => Ok(RouteIngress::HttpConnectProxy {
            endpoint,
            proxy_auth_credential_ref_id: auth,
        }),
        "socks5_proxy" => Ok(RouteIngress::Socks5Proxy {
            endpoint,
            dns_mode: match dns_mode.as_deref() {
                Some("local") => ProxyDnsMode::Local,
                Some("proxy") => ProxyDnsMode::Proxy,
                _ => return Err(AppPersistenceError::InvalidStoredData),
            },
            proxy_auth_credential_ref_id: auth,
        }),
        _ => Err(AppPersistenceError::InvalidStoredData),
    }
}

fn validate_route_graph(
    transaction: &Transaction<'_>,
    host_id: &HostId,
    jump_host_ids: &[HostId],
) -> Result<()> {
    for jump_host_id in jump_host_ids {
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1)",
            [jump_host_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AppPersistenceError::NotFound);
        }
        let cycles: bool = transaction.query_row(
            "WITH RECURSIVE reachable(id) AS (
               SELECT ?1
               UNION
               SELECT hop.jump_host_id
               FROM host_route_jump_hops AS hop
               JOIN reachable ON hop.host_id = reachable.id
             )
             SELECT EXISTS(SELECT 1 FROM reachable WHERE id = ?2)",
            params![jump_host_id.as_str(), host_id.as_str()],
            |row| row.get(0),
        )?;
        if cycles {
            return Err(AppPersistenceError::InvalidInput(
                "jump host chain contains an indirect cycle",
            ));
        }
    }
    Ok(())
}

fn authentication_mode_to_db(mode: AuthenticationPlanMode) -> &'static str {
    match mode {
        AuthenticationPlanMode::Identity => "identity",
        AuthenticationPlanMode::HostOverride => "host_override",
    }
}

fn authentication_mode_from_db(value: &str) -> rusqlite::Result<AuthenticationPlanMode> {
    match value {
        "identity" => Ok(AuthenticationPlanMode::Identity),
        "host_override" => Ok(AuthenticationPlanMode::HostOverride),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid authentication plan mode",
        ))),
    }
}

fn algorithm_category_to_db(category: AlgorithmCategory) -> &'static str {
    match category {
        AlgorithmCategory::KeyExchange => "key_exchange",
        AlgorithmCategory::HostKey => "host_key",
        AlgorithmCategory::Cipher => "cipher",
        AlgorithmCategory::Mac => "mac",
    }
}

fn algorithm_category_from_db(value: &str) -> rusqlite::Result<AlgorithmCategory> {
    match value {
        "key_exchange" => Ok(AlgorithmCategory::KeyExchange),
        "host_key" => Ok(AlgorithmCategory::HostKey),
        "cipher" => Ok(AlgorithmCategory::Cipher),
        "mac" => Ok(AlgorithmCategory::Mac),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid algorithm category",
        ))),
    }
}

fn validated_catalog_id(value: &str, error: &'static str) -> Result<String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(AppPersistenceError::InvalidInput(error));
    }
    Ok(value.to_owned())
}

struct EncodedHeartbeat<'a> {
    mode: &'static str,
    interval: Option<i64>,
    reply_timeout: Option<i64>,
    failure_threshold: Option<i64>,
    payload: Option<&'a str>,
    line_ending: Option<&'static str>,
    user_idle: Option<i64>,
}

fn validated_heartbeat_policy(policy: &HeartbeatPolicy) -> Result<HeartbeatPolicy> {
    match policy {
        HeartbeatPolicy::Disabled => Ok(HeartbeatPolicy::Disabled),
        HeartbeatPolicy::TransportKeepalive {
            interval_seconds,
            reply_timeout_seconds,
            failure_threshold,
        } if (10..=3600).contains(interval_seconds)
            && (5..=60).contains(reply_timeout_seconds)
            && reply_timeout_seconds < interval_seconds
            && (1..=10).contains(failure_threshold) =>
        {
            Ok(policy.clone())
        }
        HeartbeatPolicy::ShellHeartbeat {
            payload_text,
            line_ending,
            interval_seconds,
            user_idle_seconds,
        } if payload_text.len() <= 1024
            && !payload_text.chars().any(char::is_control)
            && !contains_known_high_risk_secret_pattern(payload_text)
            && !(payload_text.is_empty() && *line_ending == ShellHeartbeatLineEnding::None)
            && (30..=3600).contains(interval_seconds)
            && (5..=3600).contains(user_idle_seconds) =>
        {
            Ok(policy.clone())
        }
        _ => Err(AppPersistenceError::InvalidInput(
            "heartbeat policy is outside its safe bounds",
        )),
    }
}

fn heartbeat_to_db(policy: &HeartbeatPolicy) -> EncodedHeartbeat<'_> {
    match policy {
        HeartbeatPolicy::Disabled => EncodedHeartbeat {
            mode: "disabled",
            interval: None,
            reply_timeout: None,
            failure_threshold: None,
            payload: None,
            line_ending: None,
            user_idle: None,
        },
        HeartbeatPolicy::TransportKeepalive {
            interval_seconds,
            reply_timeout_seconds,
            failure_threshold,
        } => EncodedHeartbeat {
            mode: "transport_keepalive",
            interval: Some(i64::from(*interval_seconds)),
            reply_timeout: Some(i64::from(*reply_timeout_seconds)),
            failure_threshold: Some(i64::from(*failure_threshold)),
            payload: None,
            line_ending: None,
            user_idle: None,
        },
        HeartbeatPolicy::ShellHeartbeat {
            payload_text,
            line_ending,
            interval_seconds,
            user_idle_seconds,
        } => EncodedHeartbeat {
            mode: "shell_heartbeat",
            interval: Some(i64::from(*interval_seconds)),
            reply_timeout: None,
            failure_threshold: None,
            payload: Some(payload_text),
            line_ending: Some(line_ending_to_db(*line_ending)),
            user_idle: Some(i64::from(*user_idle_seconds)),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn heartbeat_from_db(
    mode: &str,
    interval: Option<i64>,
    reply_timeout: Option<i64>,
    failure_threshold: Option<i64>,
    payload: Option<String>,
    line_ending: Option<String>,
    user_idle: Option<i64>,
) -> Result<HeartbeatPolicy> {
    let policy = match mode {
        "disabled" => HeartbeatPolicy::Disabled,
        "transport_keepalive" => HeartbeatPolicy::TransportKeepalive {
            interval_seconds: optional_u32(interval)?,
            reply_timeout_seconds: optional_u32(reply_timeout)?,
            failure_threshold: u8::try_from(
                failure_threshold.ok_or(AppPersistenceError::InvalidStoredData)?,
            )
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        },
        "shell_heartbeat" => HeartbeatPolicy::ShellHeartbeat {
            payload_text: payload.ok_or(AppPersistenceError::InvalidStoredData)?,
            line_ending: line_ending_from_db(
                line_ending
                    .as_deref()
                    .ok_or(AppPersistenceError::InvalidStoredData)?,
            )?,
            interval_seconds: optional_u32(interval)?,
            user_idle_seconds: optional_u32(user_idle)?,
        },
        _ => return Err(AppPersistenceError::InvalidStoredData),
    };
    validated_heartbeat_policy(&policy)
}

fn line_ending_to_db(value: ShellHeartbeatLineEnding) -> &'static str {
    match value {
        ShellHeartbeatLineEnding::None => "none",
        ShellHeartbeatLineEnding::Cr => "cr",
        ShellHeartbeatLineEnding::Lf => "lf",
        ShellHeartbeatLineEnding::Crlf => "crlf",
    }
}

fn line_ending_from_db(value: &str) -> Result<ShellHeartbeatLineEnding> {
    match value {
        "none" => Ok(ShellHeartbeatLineEnding::None),
        "cr" => Ok(ShellHeartbeatLineEnding::Cr),
        "lf" => Ok(ShellHeartbeatLineEnding::Lf),
        "crlf" => Ok(ShellHeartbeatLineEnding::Crlf),
        _ => Err(AppPersistenceError::InvalidStoredData),
    }
}

fn validated_monitoring_policy(policy: &MonitoringPolicy) -> Result<MonitoringPolicy> {
    if !(5..=300).contains(&policy.sample_interval_seconds)
        || !(2..=30).contains(&policy.sample_timeout_seconds)
        || policy.sample_timeout_seconds >= policy.sample_interval_seconds
    {
        return Err(AppPersistenceError::InvalidInput(
            "monitoring policy is outside its safe bounds",
        ));
    }
    if policy.disk_mount_ids.as_slice() != [DiskResourceId::Root]
        || policy.network_interface_ids.as_slice() != [NetworkResourceId::AggregateNonLoopback]
    {
        return Err(AppPersistenceError::InvalidInput(
            "monitoring policy must select each canonical resource exactly once",
        ));
    }
    Ok(policy.clone())
}

fn read_monitoring_selection_strings(
    connection: &Connection,
    host_id: &HostId,
    kind: &str,
) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT stable_id FROM host_monitoring_selections
         WHERE host_id = ?1 AND kind = ?2 ORDER BY stable_id",
    )?;
    statement
        .query_map(params![host_id.as_str(), kind], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn read_disk_monitoring_selections(
    connection: &Connection,
    host_id: &HostId,
) -> Result<Vec<DiskResourceId>> {
    read_monitoring_selection_strings(connection, host_id, "disk_mount")?
        .into_iter()
        .map(|value| match value.as_str() {
            ROOT_DISK_RESOURCE_ID => Ok(DiskResourceId::Root),
            _ => Err(AppPersistenceError::InvalidStoredData),
        })
        .collect()
}

fn read_network_monitoring_selections(
    connection: &Connection,
    host_id: &HostId,
) -> Result<Vec<NetworkResourceId>> {
    read_monitoring_selection_strings(connection, host_id, "network_interface")?
        .into_iter()
        .map(|value| match value.as_str() {
            AGGREGATE_NON_LOOPBACK_NETWORK_RESOURCE_ID => {
                Ok(NetworkResourceId::AggregateNonLoopback)
            }
            _ => Err(AppPersistenceError::InvalidStoredData),
        })
        .collect()
}

fn validated_login_automation(
    enabled: bool,
    steps: &[LoginAutomationStepInput],
) -> Result<Vec<LoginAutomationStepInput>> {
    if steps.len() > MAX_AUTOMATION_STEPS || (enabled && steps.is_empty()) {
        return Err(AppPersistenceError::InvalidInput(
            "login automation step count is invalid",
        ));
    }
    let mut total_timeout = 0_u32;
    let mut validated = Vec::with_capacity(steps.len());
    for step in steps {
        let step = match step {
            LoginAutomationStepInput::Expect {
                literal_text,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                LoginAutomationStepInput::Expect {
                    literal_text: validated_automation_text(literal_text, false)?,
                    timeout_seconds: *timeout_seconds,
                }
            }
            LoginAutomationStepInput::SendText {
                text,
                append_enter,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                if contains_known_high_risk_secret_pattern(text) {
                    return Err(AppPersistenceError::InvalidInput(
                        "login automation text matches a high-risk secret pattern",
                    ));
                }
                LoginAutomationStepInput::SendText {
                    text: validated_automation_text(text, true)?,
                    append_enter: *append_enter,
                    timeout_seconds: *timeout_seconds,
                }
            }
            LoginAutomationStepInput::SendSecret {
                secret_ref_id,
                secret_label,
                append_enter,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                LoginAutomationStepInput::SendSecret {
                    secret_ref_id: secret_ref_id.clone(),
                    secret_label: normalized_label(secret_label)?,
                    append_enter: *append_enter,
                    timeout_seconds: *timeout_seconds,
                }
            }
            LoginAutomationStepInput::PreserveExistingSecret {
                existing_ordinal,
                append_enter,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                if usize::from(*existing_ordinal) >= MAX_AUTOMATION_STEPS {
                    return Err(AppPersistenceError::InvalidInput(
                        "login automation preserved step is invalid",
                    ));
                }
                LoginAutomationStepInput::PreserveExistingSecret {
                    existing_ordinal: *existing_ordinal,
                    append_enter: *append_enter,
                    timeout_seconds: *timeout_seconds,
                }
            }
        };
        validated.push(step);
    }
    Ok(validated)
}

fn normalized_ssh_sync_login_automation(
    automation: &SshSyncLoginAutomation,
) -> Result<SshSyncLoginAutomation> {
    if automation.confirmed && !automation.enabled {
        return Err(AppPersistenceError::InvalidInput(
            "disabled SSH sync login automation cannot be confirmed",
        ));
    }
    if automation.steps.len() > MAX_AUTOMATION_STEPS
        || (automation.enabled && automation.steps.is_empty())
        || (!automation.enabled && !automation.steps.is_empty())
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH sync login automation step count is invalid",
        ));
    }
    let mut total_timeout = 0_u32;
    let mut steps = Vec::with_capacity(automation.steps.len());
    for step in &automation.steps {
        let step = match step {
            SshSyncLoginAutomationStep::Expect {
                literal_text,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                SshSyncLoginAutomationStep::Expect {
                    literal_text: validated_automation_text(literal_text, false)?,
                    timeout_seconds: *timeout_seconds,
                }
            }
            SshSyncLoginAutomationStep::SendText {
                text,
                append_enter,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                if contains_known_high_risk_secret_pattern(text) {
                    return Err(AppPersistenceError::InvalidInput(
                        "SSH sync login automation text matches a high-risk secret pattern",
                    ));
                }
                SshSyncLoginAutomationStep::SendText {
                    text: validated_automation_text(text, true)?,
                    append_enter: *append_enter,
                    timeout_seconds: *timeout_seconds,
                }
            }
            SshSyncLoginAutomationStep::SendSecret {
                secret_ref_id,
                secret_label,
                append_enter,
                timeout_seconds,
            } => {
                validate_automation_timeout(*timeout_seconds, &mut total_timeout)?;
                SshSyncLoginAutomationStep::SendSecret {
                    secret_ref_id: secret_ref_id.clone(),
                    secret_label: normalized_label(secret_label)?,
                    append_enter: *append_enter,
                    timeout_seconds: *timeout_seconds,
                }
            }
        };
        steps.push(step);
    }
    Ok(SshSyncLoginAutomation {
        enabled: automation.enabled,
        confirmed: automation.confirmed,
        steps,
    })
}

fn ssh_sync_login_automation_secret_refs(automation: &SshSyncLoginAutomation) -> Vec<&SecretRefId> {
    automation
        .steps
        .iter()
        .filter_map(|step| match step {
            SshSyncLoginAutomationStep::SendSecret { secret_ref_id, .. } => Some(secret_ref_id),
            _ => None,
        })
        .collect()
}

fn validate_automation_timeout(timeout: u8, total: &mut u32) -> Result<()> {
    if !(1..=60).contains(&timeout) {
        return Err(AppPersistenceError::InvalidInput(
            "login automation timeout is invalid",
        ));
    }
    *total += u32::from(timeout);
    if *total > 300 {
        return Err(AppPersistenceError::InvalidInput(
            "login automation total timeout is too long",
        ));
    }
    Ok(())
}

fn validated_automation_text(value: &str, allow_empty: bool) -> Result<String> {
    if value.len() > 4096 || (!allow_empty && value.is_empty()) || value.contains('\0') {
        return Err(AppPersistenceError::InvalidInput(
            "login automation text is invalid",
        ));
    }
    Ok(value.to_owned())
}

fn contains_known_high_risk_secret_pattern(value: &str) -> bool {
    let uppercase = value.to_ascii_uppercase();
    if uppercase.contains("-----BEGIN") && uppercase.contains("PRIVATE KEY-----") {
        return true;
    }
    value
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        })
        .any(|token| {
            let lower = token.to_ascii_lowercase();
            (token.len() >= 24
                && ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix)))
                || (token.len() >= 23 && lower.starts_with("sk-"))
                || (token.len() == 20
                    && token.starts_with("AKIA")
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
        })
}

struct EncodedLoginAutomationStep<'a> {
    kind: &'static str,
    literal_text: Option<&'a str>,
    append_enter: Option<bool>,
    timeout_seconds: u8,
    secret_ref_id: Option<&'a str>,
    secret_label: Option<&'a str>,
}

fn login_automation_step_to_db(step: &LoginAutomationStepInput) -> EncodedLoginAutomationStep<'_> {
    match step {
        LoginAutomationStepInput::Expect {
            literal_text,
            timeout_seconds,
        } => EncodedLoginAutomationStep {
            kind: "expect",
            literal_text: Some(literal_text),
            append_enter: None,
            timeout_seconds: *timeout_seconds,
            secret_ref_id: None,
            secret_label: None,
        },
        LoginAutomationStepInput::SendText {
            text,
            append_enter,
            timeout_seconds,
        } => EncodedLoginAutomationStep {
            kind: "send_text",
            literal_text: Some(text),
            append_enter: Some(*append_enter),
            timeout_seconds: *timeout_seconds,
            secret_ref_id: None,
            secret_label: None,
        },
        LoginAutomationStepInput::SendSecret {
            secret_ref_id,
            secret_label,
            append_enter,
            timeout_seconds,
        } => EncodedLoginAutomationStep {
            kind: "send_secret",
            literal_text: None,
            append_enter: Some(*append_enter),
            timeout_seconds: *timeout_seconds,
            secret_ref_id: Some(secret_ref_id.as_str()),
            secret_label: Some(secret_label),
        },
        LoginAutomationStepInput::PreserveExistingSecret { .. } => {
            unreachable!("preserved secret steps are resolved before persistence")
        }
    }
}

fn read_login_automation_step_summary(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LoginAutomationStepSummary> {
    let kind = row.get::<_, String>(0)?;
    let text = row.get::<_, Option<String>>(1)?;
    let append_enter = row.get::<_, Option<bool>>(2)?;
    let timeout = u8::try_from(row.get::<_, i64>(3)?).map_err(invalid_column)?;
    let label = row.get::<_, Option<String>>(4)?;
    match kind.as_str() {
        "expect" => Ok(LoginAutomationStepSummary::Expect {
            literal_text: text.ok_or_else(invalid_stored_column)?,
            timeout_seconds: timeout,
        }),
        "send_text" => Ok(LoginAutomationStepSummary::SendText {
            text: text.ok_or_else(invalid_stored_column)?,
            append_enter: append_enter.ok_or_else(invalid_stored_column)?,
            timeout_seconds: timeout,
        }),
        "send_secret" => Ok(LoginAutomationStepSummary::SendSecret {
            secret_label: label.ok_or_else(invalid_stored_column)?,
            append_enter: append_enter.ok_or_else(invalid_stored_column)?,
            timeout_seconds: timeout,
        }),
        _ => Err(invalid_stored_column()),
    }
}

fn read_login_automation_step_input(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LoginAutomationStepInput> {
    let kind = row.get::<_, String>(0)?;
    let text = row.get::<_, Option<String>>(1)?;
    let append_enter = row.get::<_, Option<bool>>(2)?;
    let timeout_seconds = u8::try_from(row.get::<_, i64>(3)?).map_err(invalid_column)?;
    let secret_ref_id = row.get::<_, Option<String>>(4)?;
    let secret_label = row.get::<_, Option<String>>(5)?;
    match kind.as_str() {
        "expect" => Ok(LoginAutomationStepInput::Expect {
            literal_text: text.ok_or_else(invalid_stored_column)?,
            timeout_seconds,
        }),
        "send_text" => Ok(LoginAutomationStepInput::SendText {
            text: text.ok_or_else(invalid_stored_column)?,
            append_enter: append_enter.ok_or_else(invalid_stored_column)?,
            timeout_seconds,
        }),
        "send_secret" => Ok(LoginAutomationStepInput::SendSecret {
            secret_ref_id: LoginAutomationSecretStageId::parse(
                secret_ref_id.ok_or_else(invalid_stored_column)?,
            )
            .map_err(|message| {
                invalid_column(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    message,
                ))
            })?,
            secret_label: secret_label.ok_or_else(invalid_stored_column)?,
            append_enter: append_enter.ok_or_else(invalid_stored_column)?,
            timeout_seconds,
        }),
        _ => Err(invalid_stored_column()),
    }
}

fn invalid_stored_column() -> rusqlite::Error {
    invalid_column(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "stored connection configuration is invalid",
    ))
}

fn validate_unique_bounded_ids<T: PartialEq>(
    values: &[T],
    maximum: usize,
    error: &'static str,
) -> Result<()> {
    if values.len() > maximum
        || values
            .iter()
            .enumerate()
            .any(|(index, value)| values[..index].contains(value))
    {
        return Err(AppPersistenceError::InvalidInput(error));
    }
    Ok(())
}

fn config_update_error(
    transaction: &Transaction<'_>,
    table: &str,
    host_id: &HostId,
) -> Result<AppPersistenceError> {
    let exists: bool = transaction.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE host_id = ?1)"),
        [host_id.as_str()],
        |row| row.get(0),
    )?;
    Ok(if exists {
        AppPersistenceError::Conflict
    } else {
        AppPersistenceError::NotFound
    })
}

fn next_revision(expected: WireSequence) -> Result<u64> {
    expected
        .get()
        .checked_add(1)
        .ok_or(AppPersistenceError::Conflict)
}

fn optional_u32(value: Option<i64>) -> Result<u32> {
    u32::try_from(value.ok_or(AppPersistenceError::InvalidStoredData)?)
        .map_err(|_| AppPersistenceError::InvalidStoredData)
}

fn usize_to_i64(value: usize) -> Result<i64> {
    i64::try_from(value).map_err(|_| AppPersistenceError::InvalidInput("index is too large"))
}

fn usize_to_u32(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| AppPersistenceError::InvalidStoredData)
}

fn validate_plugin_catalog(
    trust: &PluginCatalogTrustRecord,
    entries: &[PluginCatalogEntryRecord],
) -> Result<()> {
    if trust.root_key_id.is_empty()
        || trust.root_key_id.len() > 120
        || trust.sequence == 0
        || trust.catalog_signature_base64.len() < 80
        || trust.catalog_signature_base64.len() > 120
        || trust.catalog_revision.is_empty()
        || trust.catalog_revision.len() > 120
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin catalog trust state",
        ));
    }
    validate_digest(&trust.payload_sha256)?;
    let mut identities = std::collections::BTreeSet::new();
    for entry in entries {
        if entry.version.is_empty()
            || entry.version.len() > 80
            || entry.name.is_empty()
            || entry.publisher.is_empty()
            || !entry.package_url.starts_with("https://")
            || entry.package_url.len() > 2_048
            || entry.package_size == 0
            || entry.raw_capabilities.len() > 32
            || entry.raw_capabilities.iter().any(|name| {
                name.is_empty() || name.len() > 100 || name.chars().any(char::is_control)
            })
            || known_catalog_capabilities(&entry.raw_capabilities) != entry.capabilities
            || entry.capabilities.len() > 32
            || entry.details.as_ref().is_some_and(|details| {
                serde_json::to_vec(details).map_or(true, |bytes| bytes.len() > 64 * 1024)
            })
            || !identities.insert((entry.plugin_id.clone(), entry.version.clone()))
        {
            return Err(AppPersistenceError::InvalidInput(
                "invalid plugin catalog entry",
            ));
        }
        validate_digest(&entry.package_sha256)?;
    }
    Ok(())
}

fn validate_plugin_installed(candidate: &PluginInstalledRecord) -> Result<()> {
    if candidate.name.is_empty()
        || candidate.publisher.is_empty()
        || candidate.active_version.is_empty()
        || candidate.capabilities.len() > 32
        || candidate.state_version.get() == 0
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid installed plugin metadata",
        ));
    }
    validate_digest(&candidate.signer_fingerprint_sha256)?;
    validate_digest(&candidate.package_sha256)
}

fn complete_plugin_capability_decision(
    capabilities: &[PluginCapability],
    grants: &[(PluginCapability, bool)],
) -> bool {
    capabilities.len() == grants.len()
        && grants.len() <= 32
        && grants.iter().enumerate().all(|(index, (capability, _))| {
            capabilities.contains(capability)
                && !grants[..index].iter().any(|value| value.0 == *capability)
        })
}

fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppPersistenceError::InvalidInput("invalid sha256 digest"));
    }
    Ok(())
}

fn validate_plugin_permission_binding(binding: &PluginPermissionBinding) -> Result<()> {
    validate_digest(&binding.artifact_sha256)?;
    if binding.secure_surface_contract_revision == 0 {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin permission binding",
        ));
    }
    Ok(())
}

fn host_scoped_plugin_capability(capability: PluginCapability) -> bool {
    matches!(
        capability,
        PluginCapability::HostMetadataRead
            | PluginCapability::HostMutationPropose
            | PluginCapability::HostSessionRequest
    )
}

fn special_plugin_capability(capability: PluginCapability) -> bool {
    matches!(
        capability,
        PluginCapability::UiWebviewIsolated
            | PluginCapability::NetworkDomain
            | PluginCapability::LocalFiles
            | PluginCapability::LocalProcess
            | PluginCapability::UiHostDomObserve
            | PluginCapability::UiHostDomMutate
            | PluginCapability::UiHostCss
            | PluginCapability::HostMetadataRead
            | PluginCapability::HostMutationPropose
            | PluginCapability::HostSessionRequest
            | PluginCapability::RemoteInspect
            | PluginCapability::RemoteExecRequest
            | PluginCapability::SftpRead
            | PluginCapability::SftpWrite
            | PluginCapability::DeviceSerial
            | PluginCapability::TerminalProvider
            | PluginCapability::CredentialsPlugin
            | PluginCapability::SshSync
    )
}

fn permission_binding_from_columns(
    row: &rusqlite::Row<'_>,
    start: usize,
) -> rusqlite::Result<Option<PluginPermissionBinding>> {
    let artifact_sha256: Option<String> = row.get(start)?;
    let app_version_major: Option<i64> = row.get(start + 1)?;
    let app_version_minor: Option<i64> = row.get(start + 2)?;
    let secure_surface_contract_revision: Option<i64> = row.get(start + 3)?;
    match (
        artifact_sha256,
        app_version_major,
        app_version_minor,
        secure_surface_contract_revision,
    ) {
        (None, None, None, None) => Ok(None),
        (Some(artifact_sha256), Some(major), Some(minor), Some(contract)) => {
            Ok(Some(PluginPermissionBinding {
                artifact_sha256,
                app_version_major: u64::try_from(major).map_err(invalid_column)?,
                app_version_minor: u64::try_from(minor).map_err(invalid_column)?,
                secure_surface_contract_revision: u64::try_from(contract)
                    .map_err(invalid_column)?,
            }))
        }
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "incomplete plugin permission binding",
        ))),
    }
}

fn permission_binding_from_scope_row(
    row: &PluginPermissionScopeBindingRow,
) -> Option<PluginPermissionBinding> {
    Some(PluginPermissionBinding {
        artifact_sha256: row.1.clone()?,
        app_version_major: u64::try_from(row.2?).ok()?,
        app_version_minor: u64::try_from(row.3?).ok()?,
        secure_surface_contract_revision: u64::try_from(row.4?).ok()?,
    })
}

fn read_plugin_catalog_trust(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PluginCatalogTrustRecord> {
    Ok(PluginCatalogTrustRecord {
        root_key_id: row.get(0)?,
        sequence: u64::try_from(row.get::<_, i64>(1)?).map_err(invalid_column)?,
        payload_sha256: row.get(2)?,
        catalog_signature_base64: row.get(3)?,
        catalog_revision: row.get(4)?,
        expires_at_unix_ms: row.get(5)?,
        verified_at_unix_ms: row.get(6)?,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogCapabilityCache {
    format_version: u16,
    capabilities: Vec<String>,
    unsupported_manifest: bool,
}

/// Display projection only. Admission requires every original name to be recognized.
fn known_catalog_capabilities(raw: &[String]) -> Vec<PluginCapability> {
    raw.iter()
        .filter_map(|name| serde_json::from_value(serde_json::Value::String(name.clone())).ok())
        .collect()
}

fn decode_catalog_capabilities(
    value: &str,
) -> std::result::Result<(Vec<String>, Vec<PluginCapability>, bool), serde_json::Error> {
    let stored: serde_json::Value = serde_json::from_str(value)?;
    let (raw, unsupported) = if stored.is_array() {
        (serde_json::from_value::<Vec<String>>(stored)?, false)
    } else {
        let cache: CatalogCapabilityCache = serde_json::from_value(stored)?;
        if cache.format_version != 1 {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "unsupported catalog cache version",
            ));
        }
        (cache.capabilities, cache.unsupported_manifest)
    };
    let known = known_catalog_capabilities(&raw);
    Ok((raw, known, unsupported))
}

fn read_plugin_catalog_entry(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PluginCatalogEntryRecord> {
    let (raw_capabilities, capabilities, unsupported_manifest) =
        decode_catalog_capabilities(&row.get::<_, String>(13)?).map_err(invalid_column)?;
    Ok(PluginCatalogEntryRecord {
        plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
        version: row.get(1)?,
        name: row.get(2)?,
        publisher: row.get(3)?,
        protocol_major: u16::try_from(row.get::<_, i64>(4)?).map_err(invalid_column)?,
        protocol_minor: u16::try_from(row.get::<_, i64>(5)?).map_err(invalid_column)?,
        platform: row.get(6)?,
        architectures: parse_json_column(row.get(7)?)?,
        package_url: row.get(8)?,
        package_size: u64::try_from(row.get::<_, i64>(9)?).map_err(invalid_column)?,
        package_sha256: row.get(10)?,
        publisher_key_base64: row.get(11)?,
        publisher_signature_base64: row.get(12)?,
        capabilities,
        raw_capabilities,
        unsupported_manifest,
        minimum_app_version: row.get(14)?,
        published_at_unix_ms: row.get(15)?,
        details: row
            .get::<_, Option<String>>(16)?
            .map(parse_json_column)
            .transpose()?,
    })
}

fn read_plugin_installed(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginInstalledRecord> {
    Ok(PluginInstalledRecord {
        plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
        name: row.get(1)?,
        publisher: row.get(2)?,
        signer_fingerprint_sha256: row.get(3)?,
        active_version: row.get(4)?,
        package_sha256: row.get(5)?,
        capabilities: parse_json_column(row.get(6)?)?,
        state: plugin_install_state_from_db(&row.get::<_, String>(7)?)?,
        state_version: read_wire_sequence(row, 8)?,
        installed_at_unix_ms: row.get(9)?,
        updated_at_unix_ms: row.get(10)?,
    })
}

fn read_plugin_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginOperationRecord> {
    Ok(PluginOperationRecord {
        operation_id: parse_id(row.get::<_, String>(0)?, PluginOperationId::parse)?,
        plugin_id: optional_id(row.get(1)?, PluginId::parse)?,
        kind: plugin_operation_kind_from_db(&row.get::<_, String>(2)?)?,
        state: plugin_operation_state_from_db(&row.get::<_, String>(3)?)?,
        phase: plugin_operation_phase_from_db(&row.get::<_, String>(4)?)?,
        idempotency_key: row.get(5)?,
        request_fingerprint_sha256: row.get(6)?,
        candidate_version: row.get(7)?,
        expected_active_version: row.get(8)?,
        error_code: row.get(9)?,
        state_version: read_wire_sequence(row, 10)?,
        created_at_unix_ms: row.get(11)?,
        updated_at_unix_ms: row.get(12)?,
    })
}

fn read_plugin_capability_grant(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PluginCapabilityGrantRecord> {
    Ok(PluginCapabilityGrantRecord {
        plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
        signer_fingerprint_sha256: row.get(1)?,
        major_version: u64::try_from(row.get::<_, i64>(2)?).map_err(invalid_column)?,
        capability: plugin_capability_from_db(&row.get::<_, String>(3)?)?,
        granted: row.get(4)?,
        state_version: read_wire_sequence(row, 5)?,
        binding: permission_binding_from_columns(row, 6)?,
    })
}

fn parse_json_column<T: serde::de::DeserializeOwned>(value: String) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(invalid_column)
}

fn plugin_capability_to_db(value: PluginCapability) -> &'static str {
    match value {
        PluginCapability::UiPanel => "ui.panel",
        PluginCapability::UiNavigation => "ui.navigation",
        PluginCapability::UiPage => "ui.page",
        PluginCapability::UiWebviewIsolated => "ui.webview.isolated",
        PluginCapability::UiHostDomObserve => "ui.hostDom.observe",
        PluginCapability::UiHostDomMutate => "ui.hostDom.mutate",
        PluginCapability::UiHostCss => "ui.hostCss",
        PluginCapability::ClipboardWrite => "clipboard.write",
        PluginCapability::TerminalMetadata => "terminal.metadata",
        PluginCapability::TerminalObserve => "terminal.observe",
        PluginCapability::TerminalAnnotation => "terminal.annotation",
        PluginCapability::TerminalProposeInput => "terminal.proposeInput",
        PluginCapability::TerminalRequestInput => "terminal.requestInput",
        PluginCapability::TerminalProvider => "terminal.provider",
        PluginCapability::DeviceSerial => "device.serial",
        PluginCapability::HostMetadataRead => "host.metadata.read",
        PluginCapability::HostMutationPropose => "host.mutation.propose",
        PluginCapability::HostSessionRequest => "host.session.request",
        PluginCapability::RemoteInspect => "remote.inspect",
        PluginCapability::RemoteExecRequest => "remote.exec.request",
        PluginCapability::NetworkDomain => "network.domain",
        PluginCapability::LocalFiles => "local.files",
        PluginCapability::LocalProcess => "local.process",
        PluginCapability::StoragePlugin => "storage.plugin",
        PluginCapability::SftpRead => "sftp.read",
        PluginCapability::SftpWrite => "sftp.write",
        PluginCapability::CredentialsPlugin => "credentials.plugin",
        PluginCapability::MetricsRead => "metrics.read",
        PluginCapability::SshSync => "ssh.sync",
    }
}

fn plugin_capability_from_db(value: &str) -> rusqlite::Result<PluginCapability> {
    match value {
        "ui.panel" => Ok(PluginCapability::UiPanel),
        "ui.navigation" => Ok(PluginCapability::UiNavigation),
        "ui.page" => Ok(PluginCapability::UiPage),
        "ui.webview.isolated" => Ok(PluginCapability::UiWebviewIsolated),
        "ui.hostDom.observe" => Ok(PluginCapability::UiHostDomObserve),
        "ui.hostDom.mutate" => Ok(PluginCapability::UiHostDomMutate),
        "ui.hostCss" => Ok(PluginCapability::UiHostCss),
        "clipboard.write" => Ok(PluginCapability::ClipboardWrite),
        "terminal.metadata" => Ok(PluginCapability::TerminalMetadata),
        "terminal.observe" => Ok(PluginCapability::TerminalObserve),
        "terminal.annotation" => Ok(PluginCapability::TerminalAnnotation),
        "terminal.proposeInput" => Ok(PluginCapability::TerminalProposeInput),
        "terminal.requestInput" => Ok(PluginCapability::TerminalRequestInput),
        "terminal.provider" => Ok(PluginCapability::TerminalProvider),
        "device.serial" => Ok(PluginCapability::DeviceSerial),
        "host.metadata.read" => Ok(PluginCapability::HostMetadataRead),
        "host.mutation.propose" => Ok(PluginCapability::HostMutationPropose),
        "host.session.request" => Ok(PluginCapability::HostSessionRequest),
        "remote.inspect" => Ok(PluginCapability::RemoteInspect),
        "remote.exec.request" => Ok(PluginCapability::RemoteExecRequest),
        "network.domain" => Ok(PluginCapability::NetworkDomain),
        "local.files" => Ok(PluginCapability::LocalFiles),
        "local.process" => Ok(PluginCapability::LocalProcess),
        "storage.plugin" => Ok(PluginCapability::StoragePlugin),
        "sftp.read" => Ok(PluginCapability::SftpRead),
        "sftp.write" => Ok(PluginCapability::SftpWrite),
        "credentials.plugin" => Ok(PluginCapability::CredentialsPlugin),
        "metrics.read" => Ok(PluginCapability::MetricsRead),
        "ssh.sync" => Ok(PluginCapability::SshSync),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid plugin capability",
        ))),
    }
}

fn plugin_install_state_to_db(value: PluginInstallState) -> &'static str {
    match value {
        PluginInstallState::Enabled => "enabled",
        PluginInstallState::Disabled => "disabled",
        PluginInstallState::UpdateAvailable => "update_available",
        PluginInstallState::Crashed => "crashed",
        PluginInstallState::Quarantined => "quarantined",
        PluginInstallState::Incompatible => "incompatible",
    }
}

fn plugin_install_state_from_db(value: &str) -> rusqlite::Result<PluginInstallState> {
    match value {
        "enabled" => Ok(PluginInstallState::Enabled),
        "disabled" => Ok(PluginInstallState::Disabled),
        "update_available" => Ok(PluginInstallState::UpdateAvailable),
        "crashed" => Ok(PluginInstallState::Crashed),
        "quarantined" => Ok(PluginInstallState::Quarantined),
        "incompatible" => Ok(PluginInstallState::Incompatible),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid plugin install state",
        ))),
    }
}

fn plugin_operation_kind_to_db(value: PluginOperationKind) -> &'static str {
    match value {
        PluginOperationKind::CatalogRefresh => "catalog_refresh",
        PluginOperationKind::Install => "install",
        PluginOperationKind::Update => "update",
        PluginOperationKind::Disable => "disable",
        PluginOperationKind::Uninstall => "uninstall",
    }
}

fn plugin_operation_kind_from_db(value: &str) -> rusqlite::Result<PluginOperationKind> {
    match value {
        "catalog_refresh" => Ok(PluginOperationKind::CatalogRefresh),
        "install" => Ok(PluginOperationKind::Install),
        "update" => Ok(PluginOperationKind::Update),
        "disable" => Ok(PluginOperationKind::Disable),
        "uninstall" => Ok(PluginOperationKind::Uninstall),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid plugin operation kind",
        ))),
    }
}

fn plugin_operation_state_to_db(value: PluginOperationState) -> &'static str {
    match value {
        PluginOperationState::Pending => "pending",
        PluginOperationState::Running => "running",
        PluginOperationState::AwaitingCapabilities => "awaiting_capabilities",
        PluginOperationState::Succeeded => "succeeded",
        PluginOperationState::Failed => "failed",
        PluginOperationState::Cancelled => "cancelled",
    }
}

fn plugin_operation_state_from_db(value: &str) -> rusqlite::Result<PluginOperationState> {
    match value {
        "pending" => Ok(PluginOperationState::Pending),
        "running" => Ok(PluginOperationState::Running),
        "awaiting_capabilities" => Ok(PluginOperationState::AwaitingCapabilities),
        "succeeded" => Ok(PluginOperationState::Succeeded),
        "failed" => Ok(PluginOperationState::Failed),
        "cancelled" => Ok(PluginOperationState::Cancelled),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid plugin operation state",
        ))),
    }
}

fn plugin_operation_phase_to_db(value: PluginOperationPhase) -> &'static str {
    match value {
        PluginOperationPhase::Resolve => "resolve",
        PluginOperationPhase::Download => "download",
        PluginOperationPhase::VerifyCatalog => "verify_catalog",
        PluginOperationPhase::VerifyPackage => "verify_package",
        PluginOperationPhase::AwaitingCapabilities => "awaiting_capabilities",
        PluginOperationPhase::Staged => "staged",
        PluginOperationPhase::FilesystemActivated => "filesystem_activated",
        PluginOperationPhase::DatabaseCommitted => "database_committed",
        PluginOperationPhase::ReconcileRequired => "reconcile_required",
        PluginOperationPhase::Completed => "completed",
    }
}

fn plugin_operation_phase_from_db(value: &str) -> rusqlite::Result<PluginOperationPhase> {
    match value {
        "resolve" => Ok(PluginOperationPhase::Resolve),
        "download" => Ok(PluginOperationPhase::Download),
        "verify_catalog" => Ok(PluginOperationPhase::VerifyCatalog),
        "verify_package" => Ok(PluginOperationPhase::VerifyPackage),
        "awaiting_capabilities" => Ok(PluginOperationPhase::AwaitingCapabilities),
        "staged" => Ok(PluginOperationPhase::Staged),
        "filesystem_activated" => Ok(PluginOperationPhase::FilesystemActivated),
        "database_committed" => Ok(PluginOperationPhase::DatabaseCommitted),
        "reconcile_required" => Ok(PluginOperationPhase::ReconcileRequired),
        "completed" => Ok(PluginOperationPhase::Completed),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid plugin operation phase",
        ))),
    }
}

fn read_identity(row: &rusqlite::Row<'_>) -> rusqlite::Result<IdentitySummary> {
    Ok(IdentitySummary {
        identity_id: parse_id(row.get::<_, String>(0)?, IdentityId::parse)?,
        label: row.get(1)?,
        username: row.get(2)?,
        state_version: read_wire_sequence(row, 3)?,
    })
}

fn lookup_login_automation_secret_stage(
    connection: &Connection,
    field: &str,
    value: &str,
) -> Result<Option<LoginAutomationSecretStageRecord>> {
    let field = match field {
        "stage_id" => "stage_id",
        "operation_id" => "operation_id",
        "idempotency_key" => "idempotency_key",
        _ => return Err(AppPersistenceError::InvalidInput("invalid stage lookup")),
    };
    connection
        .query_row(
            &format!(
                "SELECT stage_id, secret_ref_id, host_id, expected_automation_revision,
                        operation_id, idempotency_key, label, expires_at_ms, state
                 FROM login_automation_secret_stages WHERE {field} = ?1"
            ),
            [value],
            read_login_automation_secret_stage,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn lookup_host_create_password_stage(
    connection: &Connection,
    field: &str,
    value: &str,
) -> Result<Option<HostCreatePasswordStageRecord>> {
    let field = match field {
        "stage_id" => "stage_id",
        "operation_id" => "operation_id",
        "idempotency_key" => "idempotency_key",
        _ => return Err(AppPersistenceError::InvalidInput("invalid stage lookup")),
    };
    connection
        .query_row(
            &format!(
                "SELECT stage_id, secret_ref_id, operation_id, idempotency_key,
                        identity_label, credential_label, expires_at_ms, state
                 FROM host_create_password_stages WHERE {field} = ?1"
            ),
            [value],
            read_host_create_password_stage,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn read_host_create_password_stage(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<HostCreatePasswordStageRecord> {
    Ok(HostCreatePasswordStageRecord {
        staged_password_id: parse_id(row.get::<_, String>(0)?, HostCreatePasswordStageId::parse)?,
        secret_ref_id: parse_id(row.get::<_, String>(1)?, SecretRefId::parse)?,
        operation_id: parse_id(row.get::<_, String>(2)?, OperationId::parse)?,
        idempotency_key: row.get(3)?,
        identity_label: row.get(4)?,
        credential_label: row.get(5)?,
        expires_at_unix_ms: row.get(6)?,
        state: match row.get::<_, String>(7)?.as_str() {
            "pending_vault" => HostCreatePasswordStageState::PendingVault,
            "staged" => HostCreatePasswordStageState::Staged,
            "cleanup_pending" => HostCreatePasswordStageState::CleanupPending,
            "cancelled" => HostCreatePasswordStageState::Cancelled,
            "consumed" => HostCreatePasswordStageState::Consumed,
            _ => return Err(invalid_stored_column()),
        },
    })
}

fn read_login_automation_secret_stage(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LoginAutomationSecretStageRecord> {
    Ok(LoginAutomationSecretStageRecord {
        staged_secret_id: parse_id(
            row.get::<_, String>(0)?,
            LoginAutomationSecretStageId::parse,
        )?,
        secret_ref_id: parse_id(row.get::<_, String>(1)?, SecretRefId::parse)?,
        host_id: parse_id(row.get::<_, String>(2)?, HostId::parse)?,
        expected_automation_revision: read_wire_sequence(row, 3)?,
        operation_id: parse_id(row.get::<_, String>(4)?, OperationId::parse)?,
        idempotency_key: row.get(5)?,
        label: row.get(6)?,
        expires_at_unix_ms: row.get(7)?,
        state: match row.get::<_, String>(8)?.as_str() {
            "pending_vault" => LoginAutomationSecretStageState::PendingVault,
            "staged" => LoginAutomationSecretStageState::Staged,
            "cleanup_pending" => LoginAutomationSecretStageState::CleanupPending,
            "cancelled" => LoginAutomationSecretStageState::Cancelled,
            "consumed" => LoginAutomationSecretStageState::Consumed,
            _ => return Err(invalid_stored_column()),
        },
    })
}

fn list_credential_ref_summaries(
    connection: &Connection,
    identity_id: &IdentityId,
    ready_only: bool,
) -> Result<Vec<CredentialRefSummary>> {
    let ready_filter = if ready_only {
        "AND credential_refs.import_state = 'ready'"
    } else {
        ""
    };
    let mut statement = connection.prepare(&format!(
        "SELECT credential_refs.id, credential_refs.identity_id, credential_refs.kind,
                credential_refs.priority, credential_refs.label,
                credential_private_key_details.public_key_algorithm,
                credential_private_key_details.public_key_fingerprint,
                credential_ssh_agent_details.public_key_blob,
                credential_ssh_agent_details.public_key_algorithm,
                credential_ssh_agent_details.public_key_fingerprint,
                credential_ssh_agent_details.agent_scope,
                credential_ssh_agent_details.identity_kind,
                credential_ssh_agent_details.hardware_application,
                credential_ssh_agent_details.certificate_blob,
                credential_ssh_agent_details.certificate_algorithm,
                credential_ssh_agent_details.certificate_fingerprint,
                credential_ssh_agent_details.certificate_ca_public_key_fingerprint,
                credential_ssh_agent_details.certificate_serial,
                credential_ssh_agent_details.certificate_key_id,
                credential_ssh_agent_details.certificate_valid_principals_json,
                credential_ssh_agent_details.certificate_type,
                credential_ssh_agent_details.certificate_valid_after_unix_seconds,
                credential_ssh_agent_details.certificate_valid_before_unix_seconds,
                credential_ssh_agent_details.certificate_critical_options_json,
                credential_ssh_agent_details.certificate_extensions_json,
                credential_refs.state_version,
                credential_keyboard_interactive_details.max_rounds
         FROM credential_refs
         LEFT JOIN credential_private_key_details
           ON credential_private_key_details.credential_ref_id = credential_refs.id
         LEFT JOIN credential_keyboard_interactive_details
           ON credential_keyboard_interactive_details.credential_ref_id = credential_refs.id
         LEFT JOIN credential_ssh_agent_details
           ON credential_ssh_agent_details.credential_ref_id = credential_refs.id
         WHERE credential_refs.identity_id = ?1 {ready_filter}
         ORDER BY credential_refs.priority, credential_refs.id"
    ))?;
    statement
        .query_map([identity_id.as_str()], read_credential_ref_summary)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn read_credential_ref_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialRefSummary> {
    let priority = row.get::<_, i64>(3)?;
    let database_kind = row.get::<_, String>(2)?;
    let agent_identity_kind = row
        .get::<_, Option<String>>(11)?
        .map(|value| agent_identity_kind_from_db(&value))
        .transpose()?;
    let method = authentication_method_from_db(&database_kind, agent_identity_kind)?;
    let details = match method {
        AuthenticationMethodKind::Password => CredentialRefDetails::Password,
        AuthenticationMethodKind::PrivateKey => CredentialRefDetails::PrivateKey {
            public_key_algorithm: row.get(5)?,
            public_key_fingerprint: row.get(6)?,
        },
        AuthenticationMethodKind::KeyboardInteractive => {
            CredentialRefDetails::KeyboardInteractive {
                max_rounds: u8::try_from(row.get::<_, i64>(26)?).map_err(invalid_column)?,
            }
        }
        AuthenticationMethodKind::SshAgent => CredentialRefDetails::SshAgent {
            public_key_blob: row.get(7)?,
            public_key_algorithm: row.get(8)?,
            public_key_fingerprint: row.get(9)?,
            scope: ssh_agent_scope_from_db(&row.get::<_, String>(10)?)?,
        },
        AuthenticationMethodKind::Certificate => CredentialRefDetails::Certificate {
            certificate: Box::new(read_certificate_metadata(
                row.get(13)?,
                row.get(14)?,
                row.get(15)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(16)?,
                row.get(17)?,
                row.get(18)?,
                row.get(19)?,
                row.get(20)?,
                row.get(21)?,
                row.get(22)?,
                row.get(23)?,
                row.get(24)?,
            )?),
            scope: ssh_agent_scope_from_db(&row.get::<_, String>(10)?)?,
        },
        AuthenticationMethodKind::HardwareKey => CredentialRefDetails::HardwareKey {
            public_key_blob: row.get(7)?,
            public_key_algorithm: row.get(8)?,
            public_key_fingerprint: row.get(9)?,
            application: row.get(12)?,
            scope: ssh_agent_scope_from_db(&row.get::<_, String>(10)?)?,
        },
    };
    Ok(CredentialRefSummary {
        credential_ref_id: parse_id(row.get::<_, String>(0)?, CredentialRefId::parse)?,
        identity_id: parse_id(row.get::<_, String>(1)?, IdentityId::parse)?,
        method,
        priority: u32::try_from(priority).map_err(invalid_column)?,
        label: row.get(4)?,
        details,
        state_version: read_wire_sequence(row, 25)?,
    })
}

fn read_credential_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialRecord> {
    let database_kind = row.get::<_, String>(2)?;
    let agent_identity_kind = row
        .get::<_, Option<String>>(14)?
        .map(|value| agent_identity_kind_from_db(&value))
        .transpose()?;
    let method = authentication_method_from_db(&database_kind, agent_identity_kind)?;
    let password_secret = optional_id(row.get(3)?, SecretRefId::parse)?;
    let private_key_secret = optional_id(row.get(4)?, SecretRefId::parse)?;
    let passphrase_secret = optional_id(row.get(5)?, SecretRefId::parse)?;
    let details = match method {
        AuthenticationMethodKind::Password => CredentialRecordDetails::Password {
            secret_ref_id: password_secret.ok_or_else(invalid_stored_column)?,
        },
        AuthenticationMethodKind::PrivateKey => CredentialRecordDetails::PrivateKey {
            secret_ref_id: private_key_secret.ok_or_else(invalid_stored_column)?,
            passphrase_secret_ref_id: passphrase_secret,
            public_key_algorithm: row.get(8)?,
            public_key_fingerprint: row.get(9)?,
        },
        AuthenticationMethodKind::KeyboardInteractive => {
            CredentialRecordDetails::KeyboardInteractive {
                max_rounds: u8::try_from(row.get::<_, i64>(32)?).map_err(invalid_column)?,
            }
        }
        AuthenticationMethodKind::SshAgent => CredentialRecordDetails::SshAgent {
            public_key_blob: row.get(10)?,
            public_key_algorithm: row.get(11)?,
            public_key_fingerprint: row.get(12)?,
            scope: ssh_agent_scope_from_db(&row.get::<_, String>(13)?)?,
        },
        AuthenticationMethodKind::Certificate => CredentialRecordDetails::Certificate {
            certificate: Box::new(read_certificate_metadata(
                row.get(16)?,
                row.get(17)?,
                row.get(18)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(19)?,
                row.get(20)?,
                row.get(21)?,
                row.get(22)?,
                row.get(23)?,
                row.get(24)?,
                row.get(25)?,
                row.get(26)?,
                row.get(27)?,
            )?),
            scope: ssh_agent_scope_from_db(&row.get::<_, String>(13)?)?,
        },
        AuthenticationMethodKind::HardwareKey => CredentialRecordDetails::HardwareKey {
            public_key_blob: row.get(10)?,
            public_key_algorithm: row.get(11)?,
            public_key_fingerprint: row.get(12)?,
            application: row.get(15)?,
            scope: ssh_agent_scope_from_db(&row.get::<_, String>(13)?)?,
        },
    };
    let import_operation_id = row
        .get::<_, Option<String>>(29)?
        .map(|value| parse_id(value, OperationId::parse))
        .transpose()?;
    let priority = row.get::<_, i64>(6)?;
    Ok(CredentialRecord {
        credential_ref_id: parse_id(row.get::<_, String>(0)?, CredentialRefId::parse)?,
        identity_id: parse_id(row.get::<_, String>(1)?, IdentityId::parse)?,
        method,
        details,
        priority: u32::try_from(priority).map_err(invalid_column)?,
        label: row.get(7)?,
        state_version: read_wire_sequence(row, 28)?,
        import_operation_id,
        import_idempotency_key: row.get(30)?,
        import_state: credential_import_state_from_db(&row.get::<_, String>(31)?)?,
    })
}

fn read_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostSummary> {
    let port = row.get::<_, i64>(4)?;
    let identity_id = row
        .get::<_, Option<String>>(6)?
        .map(|value| parse_id(value, IdentityId::parse))
        .transpose()?;
    Ok(HostSummary {
        host_id: parse_id(row.get::<_, String>(0)?, HostId::parse)?,
        label: row.get(1)?,
        address: row.get(2)?,
        normalized_address: row.get(3)?,
        port: u16::try_from(port).map_err(invalid_column)?,
        username: row.get(5)?,
        identity_id,
        favorite: row.get(7)?,
        has_ready_credential: false,
        state_version: read_wire_sequence(row, 8)?,
    })
}

fn read_host_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostGroupSummary> {
    Ok(HostGroupSummary {
        group_id: parse_id(row.get::<_, String>(0)?, HostGroupId::parse)?,
        label: row.get(1)?,
        state_version: read_wire_sequence(row, 2)?,
    })
}

fn read_host_tag(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostTagSummary> {
    Ok(HostTagSummary {
        tag_id: parse_id(row.get::<_, String>(0)?, HostTagId::parse)?,
        label: row.get(1)?,
        state_version: read_wire_sequence(row, 2)?,
    })
}

fn read_recent_connection(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecentConnectionSummary> {
    Ok(RecentConnectionSummary {
        host_id: parse_id(row.get::<_, String>(0)?, HostId::parse)?,
        connected_at_unix_ms: row.get(1)?,
        recency_sequence: read_wire_sequence(row, 2)?,
        successful_connection_count: read_wire_sequence(row, 3)?,
    })
}

fn read_host_catalog_base(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(
    HostSummary,
    Option<HostGroupSummary>,
    Option<RecentConnectionSummary>,
)> {
    let host = read_host(row)?;
    let group_id = row.get::<_, Option<String>>(9)?;
    let group = group_id
        .map(|group_id| {
            Ok::<HostGroupSummary, rusqlite::Error>(HostGroupSummary {
                group_id: parse_id(group_id, HostGroupId::parse)?,
                label: row.get(10)?,
                state_version: read_wire_sequence(row, 11)?,
            })
        })
        .transpose()?;
    let recent_host_id = row.get::<_, Option<String>>(12)?;
    let recent_connection = recent_host_id
        .map(|host_id| {
            Ok::<RecentConnectionSummary, rusqlite::Error>(RecentConnectionSummary {
                host_id: parse_id(host_id, HostId::parse)?,
                connected_at_unix_ms: row.get(13)?,
                recency_sequence: read_wire_sequence(row, 14)?,
                successful_connection_count: read_wire_sequence(row, 15)?,
            })
        })
        .transpose()?;
    Ok((host, group, recent_connection))
}

fn read_host_tag_ids(connection: &Connection, host_id: &HostId) -> Result<Vec<HostTagId>> {
    let mut statement = connection.prepare(
        "SELECT host_tags.id
         FROM host_tag_assignments
         JOIN host_tags ON host_tags.id = host_tag_assignments.tag_id
         WHERE host_tag_assignments.host_id = ?1
         ORDER BY host_tags.label COLLATE NOCASE, host_tags.id",
    )?;
    statement
        .query_map([host_id.as_str()], |row| {
            parse_id(row.get::<_, String>(0)?, HostTagId::parse)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn read_host_tags(connection: &Connection, host_id: &HostId) -> Result<Vec<HostTagSummary>> {
    let mut statement = connection.prepare(
        "SELECT host_tags.id, host_tags.label, host_tags.state_version
         FROM host_tag_assignments
         JOIN host_tags ON host_tags.id = host_tag_assignments.tag_id
         WHERE host_tag_assignments.host_id = ?1
         ORDER BY host_tags.label COLLATE NOCASE, host_tags.id",
    )?;
    statement
        .query_map([host_id.as_str()], read_host_tag)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn read_known_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnownHostSummary> {
    let port = row.get::<_, i64>(2)?;
    Ok(KnownHostSummary {
        known_host_id: parse_id(row.get::<_, String>(0)?, KnownHostId::parse)?,
        normalized_address: row.get(1)?,
        port: u16::try_from(port).map_err(invalid_column)?,
        key_algorithm: row.get(3)?,
        public_key_base64: row.get(4)?,
        fingerprint_sha256: row.get(5)?,
        first_trusted_at_unix_ms: row.get(6)?,
        last_verified_at_unix_ms: row.get(7)?,
        state_version: read_wire_sequence(row, 8)?,
    })
}

fn observed_host_key(
    address: &str,
    port: u16,
    key_algorithm: &str,
    public_key_blob: &[u8],
) -> Result<ObservedHostKey> {
    if public_key_blob.is_empty() || public_key_blob.len() > 64 * 1024 {
        return Err(AppPersistenceError::InvalidInput(
            "public host key size is invalid",
        ));
    }
    let endpoint = Endpoint::parse(address, port)?;
    let key_algorithm = normalized_required(
        key_algorithm,
        128,
        "host key algorithm is empty or too long",
    )?;
    Ok(ObservedHostKey {
        normalized_address: endpoint.normalized_address().to_owned(),
        port,
        key_algorithm,
        public_key_base64: BASE64.encode(public_key_blob),
        fingerprint_sha256: ssh_sha256_fingerprint(public_key_blob),
    })
}

fn lookup_known_host(
    connection: &Connection,
    normalized_address: &str,
    port: u16,
    key_algorithm: &str,
) -> Result<Option<KnownHostSummary>> {
    connection
        .query_row(
            "SELECT id, normalized_address, port, key_algorithm, public_key_base64,
                    fingerprint_sha256, first_trusted_at_ms, last_verified_at_ms, state_version
             FROM known_hosts
             WHERE normalized_address = ?1 AND port = ?2 AND key_algorithm = ?3",
            params![normalized_address, i64::from(port), key_algorithm],
            read_known_host,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn lookup_any_known_host(
    connection: &Connection,
    normalized_address: &str,
    port: u16,
) -> Result<Option<KnownHostSummary>> {
    connection
        .query_row(
            "SELECT id, normalized_address, port, key_algorithm, public_key_base64,
                    fingerprint_sha256, first_trusted_at_ms, last_verified_at_ms, state_version
             FROM known_hosts
             WHERE normalized_address = ?1 AND port = ?2
             ORDER BY first_trusted_at_ms, id
             LIMIT 1",
            params![normalized_address, i64::from(port)],
            read_known_host,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn lookup_credential_import(
    connection: &Connection,
    operation_id: &OperationId,
    idempotency_key: &str,
) -> Result<Vec<CredentialRecord>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {CREDENTIAL_RECORD_COLUMNS} FROM credential_refs
         {CREDENTIAL_RECORD_JOINS}
         WHERE credential_refs.import_operation_id = ?1
            OR credential_refs.import_idempotency_key = ?2
         ORDER BY credential_refs.id"
    ))?;
    statement
        .query_map(
            params![operation_id.as_str(), idempotency_key],
            read_credential_record,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

#[allow(clippy::too_many_arguments)]
fn same_credential_import(
    record: &CredentialRecord,
    operation_id: &OperationId,
    idempotency_key: &str,
    identity_id: &IdentityId,
    kind: CredentialKind,
    priority: u32,
    label: &str,
    has_passphrase: bool,
) -> bool {
    let base_matches = record.import_operation_id.as_ref() == Some(operation_id)
        && record.import_idempotency_key.as_deref() == Some(idempotency_key)
        && record.identity_id == *identity_id
        && record.method == authentication_method_from_import_kind(kind)
        && record.priority == priority
        && record.label == label;
    base_matches
        && match (&record.details, kind) {
            (CredentialRecordDetails::Password { .. }, CredentialKind::Password) => !has_passphrase,
            (
                CredentialRecordDetails::PrivateKey {
                    passphrase_secret_ref_id,
                    ..
                },
                CredentialKind::PrivateKey,
            ) => passphrase_secret_ref_id.is_some() == has_passphrase,
            _ => false,
        }
}

fn credential_kind_to_db(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::Password => "password",
        CredentialKind::PrivateKey => "private_key",
    }
}

fn authentication_method_from_import_kind(kind: CredentialKind) -> AuthenticationMethodKind {
    match kind {
        CredentialKind::Password => AuthenticationMethodKind::Password,
        CredentialKind::PrivateKey => AuthenticationMethodKind::PrivateKey,
    }
}

fn credential_import_kind(record: &CredentialRecord) -> Result<CredentialKind> {
    match record.method {
        AuthenticationMethodKind::Password => Ok(CredentialKind::Password),
        AuthenticationMethodKind::PrivateKey => Ok(CredentialKind::PrivateKey),
        AuthenticationMethodKind::KeyboardInteractive
        | AuthenticationMethodKind::SshAgent
        | AuthenticationMethodKind::Certificate
        | AuthenticationMethodKind::HardwareKey => Err(AppPersistenceError::InvalidStoredData),
    }
}

fn private_key_public_metadata(record: &CredentialRecord) -> (Option<&str>, Option<&str>) {
    match &record.details {
        CredentialRecordDetails::PrivateKey {
            public_key_algorithm,
            public_key_fingerprint,
            ..
        } => (
            public_key_algorithm.as_deref(),
            public_key_fingerprint.as_deref(),
        ),
        _ => (None, None),
    }
}

fn authentication_method_from_db(
    value: &str,
    agent_identity_kind: Option<AgentIdentityKind>,
) -> rusqlite::Result<AuthenticationMethodKind> {
    match (value, agent_identity_kind) {
        ("password", None) => Ok(AuthenticationMethodKind::Password),
        ("private_key", None) => Ok(AuthenticationMethodKind::PrivateKey),
        ("keyboard_interactive", None) => Ok(AuthenticationMethodKind::KeyboardInteractive),
        ("ssh_agent", Some(AgentIdentityKind::Ordinary)) => Ok(AuthenticationMethodKind::SshAgent),
        ("ssh_agent", Some(AgentIdentityKind::Certificate)) => {
            Ok(AuthenticationMethodKind::Certificate)
        }
        ("ssh_agent", Some(AgentIdentityKind::HardwareKey)) => {
            Ok(AuthenticationMethodKind::HardwareKey)
        }
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid authentication method and Agent identity kind: {value}"),
        ))),
    }
}

fn agent_identity_kind_to_db(value: AgentIdentityKind) -> &'static str {
    match value {
        AgentIdentityKind::Ordinary => "ordinary",
        AgentIdentityKind::Certificate => "certificate",
        AgentIdentityKind::HardwareKey => "hardware_key",
    }
}

fn agent_identity_kind_from_db(value: &str) -> rusqlite::Result<AgentIdentityKind> {
    match value {
        "ordinary" => Ok(AgentIdentityKind::Ordinary),
        "certificate" => Ok(AgentIdentityKind::Certificate),
        "hardware_key" => Ok(AgentIdentityKind::HardwareKey),
        _ => Err(invalid_stored_column()),
    }
}

fn ssh_agent_scope_from_db(value: &str) -> rusqlite::Result<SshAgentScope> {
    match value {
        "default_environment" => Ok(SshAgentScope::DefaultEnvironment),
        _ => Err(invalid_stored_column()),
    }
}

#[allow(clippy::too_many_arguments)]
fn read_certificate_metadata(
    certificate_blob: Option<Vec<u8>>,
    certificate_algorithm: Option<String>,
    certificate_fingerprint: Option<String>,
    subject_public_key_blob: Option<Vec<u8>>,
    subject_public_key_algorithm: Option<String>,
    subject_public_key_fingerprint: Option<String>,
    ca_public_key_fingerprint: Option<String>,
    serial: Option<String>,
    key_id: Option<String>,
    valid_principals_json: Option<String>,
    certificate_type: Option<String>,
    valid_after_unix_seconds: Option<i64>,
    valid_before_unix_seconds: Option<i64>,
    critical_options_json: Option<String>,
    extensions_json: Option<String>,
) -> rusqlite::Result<SshCertificateMetadata> {
    let valid_principals = serde_json::from_str(
        valid_principals_json
            .as_deref()
            .ok_or_else(invalid_stored_column)?,
    )
    .map_err(|_| invalid_stored_column())?;
    let critical_options = serde_json::from_str(
        critical_options_json
            .as_deref()
            .ok_or_else(invalid_stored_column)?,
    )
    .map_err(|_| invalid_stored_column())?;
    let extensions = serde_json::from_str(
        extensions_json
            .as_deref()
            .ok_or_else(invalid_stored_column)?,
    )
    .map_err(|_| invalid_stored_column())?;
    let certificate_type = match certificate_type.as_deref() {
        Some("user") => SshCertificateType::User,
        _ => return Err(invalid_stored_column()),
    };
    let metadata = SshCertificateMetadata {
        source: AgentIdentitySource::SystemSshAgent,
        certificate_blob: certificate_blob.ok_or_else(invalid_stored_column)?,
        certificate_algorithm: certificate_algorithm.ok_or_else(invalid_stored_column)?,
        certificate_fingerprint: certificate_fingerprint.ok_or_else(invalid_stored_column)?,
        serial: serial.ok_or_else(invalid_stored_column)?,
        subject_public_key_blob: subject_public_key_blob.ok_or_else(invalid_stored_column)?,
        subject_public_key_algorithm: subject_public_key_algorithm
            .ok_or_else(invalid_stored_column)?,
        subject_public_key_fingerprint: subject_public_key_fingerprint
            .ok_or_else(invalid_stored_column)?,
        ca_public_key_fingerprint: ca_public_key_fingerprint.ok_or_else(invalid_stored_column)?,
        key_id: key_id.ok_or_else(invalid_stored_column)?,
        valid_principals,
        certificate_type,
        valid_after_unix_seconds: valid_after_unix_seconds.ok_or_else(invalid_stored_column)?,
        valid_before_unix_seconds,
        critical_options,
        extensions,
    };
    validate_certificate_metadata(&metadata).map_err(|_| invalid_stored_column())?;
    Ok(metadata)
}

fn validated_agent_identity_details(
    identity_kind: AgentIdentityKind,
    public_key_blob: &[u8],
    public_key_algorithm: &str,
    public_key_fingerprint: &str,
    hardware_application: Option<&str>,
    certificate: Option<&SshCertificateMetadata>,
) -> Result<CredentialRecordDetails> {
    validate_agent_public_key(
        public_key_blob,
        public_key_algorithm,
        public_key_fingerprint,
    )?;
    match identity_kind {
        AgentIdentityKind::Ordinary => {
            if certificate.is_some()
                || hardware_application.is_some()
                || public_key_algorithm.starts_with("sk-")
                || public_key_algorithm.contains("-cert-")
            {
                return Err(AppPersistenceError::InvalidInput(
                    "ordinary SSH Agent identity metadata is invalid",
                ));
            }
            Ok(CredentialRecordDetails::SshAgent {
                public_key_blob: public_key_blob.to_vec(),
                public_key_algorithm: public_key_algorithm.to_owned(),
                public_key_fingerprint: public_key_fingerprint.to_owned(),
                scope: SshAgentScope::DefaultEnvironment,
            })
        }
        AgentIdentityKind::Certificate => {
            if hardware_application.is_some() {
                return Err(AppPersistenceError::InvalidInput(
                    "SSH Agent certificate cannot contain a hardware application projection",
                ));
            }
            let certificate = certificate.ok_or(AppPersistenceError::InvalidInput(
                "SSH Agent certificate metadata is required",
            ))?;
            validate_certificate_metadata(certificate)?;
            if certificate.subject_public_key_blob != public_key_blob
                || certificate.subject_public_key_algorithm != public_key_algorithm
                || certificate.subject_public_key_fingerprint != public_key_fingerprint
            {
                return Err(AppPersistenceError::InvalidInput(
                    "SSH Agent certificate subject key does not match",
                ));
            }
            Ok(CredentialRecordDetails::Certificate {
                certificate: Box::new(certificate.clone()),
                scope: SshAgentScope::DefaultEnvironment,
            })
        }
        AgentIdentityKind::HardwareKey => {
            let application = hardware_application.ok_or(AppPersistenceError::InvalidInput(
                "SSH Agent hardware-key application is required",
            ))?;
            if certificate.is_some()
                || !matches!(
                    public_key_algorithm,
                    "sk-ssh-ed25519@openssh.com" | "sk-ecdsa-sha2-nistp256@openssh.com"
                )
                || public_key_algorithm.contains("-cert-")
                || !application.starts_with("ssh:")
                || application.len() > 255
                || application.chars().any(char::is_control)
            {
                return Err(AppPersistenceError::InvalidInput(
                    "SSH Agent hardware-key metadata is invalid",
                ));
            }
            Ok(CredentialRecordDetails::HardwareKey {
                public_key_blob: public_key_blob.to_vec(),
                public_key_algorithm: public_key_algorithm.to_owned(),
                public_key_fingerprint: public_key_fingerprint.to_owned(),
                application: application.to_owned(),
                scope: SshAgentScope::DefaultEnvironment,
            })
        }
    }
}

fn validate_agent_public_key(
    public_key_blob: &[u8],
    public_key_algorithm: &str,
    public_key_fingerprint: &str,
) -> Result<()> {
    if public_key_blob.is_empty()
        || public_key_blob.len() > 64 * 1024
        || public_key_algorithm.is_empty()
        || public_key_algorithm.len() > 80
        || public_key_algorithm.chars().any(char::is_control)
        || public_key_fingerprint != ssh_sha256_fingerprint(public_key_blob)
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH Agent public key metadata is invalid",
        ));
    }
    Ok(())
}

fn validate_certificate_metadata(metadata: &SshCertificateMetadata) -> Result<()> {
    validate_agent_public_key(
        &metadata.subject_public_key_blob,
        &metadata.subject_public_key_algorithm,
        &metadata.subject_public_key_fingerprint,
    )?;
    if metadata.source != AgentIdentitySource::SystemSshAgent
        || metadata.certificate_blob.is_empty()
        || metadata.certificate_blob.len() > 64 * 1024
        || metadata.certificate_algorithm.is_empty()
        || metadata.certificate_algorithm.len() > 80
        || metadata.certificate_algorithm.chars().any(char::is_control)
        || !metadata
            .certificate_algorithm
            .contains("-cert-v01@openssh.com")
        || metadata.certificate_fingerprint != ssh_sha256_fingerprint(&metadata.certificate_blob)
        || !valid_fingerprint_label(&metadata.ca_public_key_fingerprint)
        || !canonical_u64_decimal(&metadata.serial)
        || metadata.key_id.len() > 1024
        || metadata.key_id.chars().any(char::is_control)
        || metadata.valid_after_unix_seconds < 0
        || metadata
            .valid_before_unix_seconds
            .is_some_and(|value| value <= metadata.valid_after_unix_seconds)
        || metadata.valid_principals.len() > 64
        || metadata.critical_options.len() > 32
        || metadata.extensions.len() > 64
        || metadata
            .valid_principals
            .iter()
            .map(String::len)
            .fold(0usize, usize::saturating_add)
            > 16 * 1024
        || metadata
            .critical_options
            .iter()
            .map(|option| option.name.len().saturating_add(option.value.len()))
            .fold(0usize, usize::saturating_add)
            > 48 * 1024
        || metadata
            .extensions
            .iter()
            .map(|extension| extension.name.len().saturating_add(extension.value.len()))
            .fold(0usize, usize::saturating_add)
            > 96 * 1024
    {
        return Err(AppPersistenceError::InvalidInput(
            "SSH certificate metadata is invalid",
        ));
    }
    let mut principal_names = std::collections::HashSet::new();
    for principal in &metadata.valid_principals {
        if principal.is_empty()
            || principal.len() > 256
            || principal.chars().any(char::is_control)
            || !principal_names.insert(principal)
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH certificate principals are invalid",
            ));
        }
    }
    let mut option_names = std::collections::HashSet::new();
    for option in &metadata.critical_options {
        let recognized = recognized_critical_option(option);
        if option.name.is_empty()
            || option.name.len() > 128
            || option.name.chars().any(char::is_control)
            || option.value.len() > 4096
            || !option_names.insert(&option.name)
            || option.recognized != recognized
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH certificate critical options are invalid",
            ));
        }
    }
    let mut extension_names = std::collections::HashSet::new();
    for extension in &metadata.extensions {
        let recognized_name = matches!(
            extension.name.as_str(),
            "no-touch-required"
                | "permit-X11-forwarding"
                | "permit-agent-forwarding"
                | "permit-port-forwarding"
                | "permit-pty"
                | "permit-user-rc"
        );
        let recognized = recognized_name && extension.value.is_empty();
        if extension.name.is_empty()
            || extension.name.len() > 128
            || extension.name.chars().any(char::is_control)
            || extension.value.len() > 4096
            || !extension_names.insert(&extension.name)
            || extension.recognized != recognized
        {
            return Err(AppPersistenceError::InvalidInput(
                "SSH certificate extensions are invalid",
            ));
        }
    }
    Ok(())
}

fn recognized_critical_option(option: &norishell_core_api::SshCertificateCriticalOption) -> bool {
    match option.name.as_str() {
        "verify-required" => option.value.is_empty(),
        "force-command" => std::str::from_utf8(&option.value)
            .is_ok_and(|value| !value.is_empty() && !value.contains('\0')),
        "source-address" => {
            std::str::from_utf8(&option.value).is_ok_and(valid_source_address_critical_option)
        }
        _ => false,
    }
}

fn valid_source_address_critical_option(value: &str) -> bool {
    !value.is_empty()
        && value.split(',').all(|entry| {
            let (address, prefix) = entry.split_once('/').unwrap_or((entry, ""));
            let Ok(address) = address.parse::<std::net::IpAddr>() else {
                return false;
            };
            if prefix.is_empty() {
                return true;
            }
            prefix.parse::<u8>().is_ok_and(|prefix| match address {
                std::net::IpAddr::V4(_) => prefix <= 32,
                std::net::IpAddr::V6(_) => prefix <= 128,
            })
        })
}

fn canonical_u64_decimal(value: &str) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

fn valid_fingerprint_label(value: &str) -> bool {
    (8..=120).contains(&value.len())
        && value.starts_with("SHA256:")
        && !value.chars().any(char::is_control)
}

fn credential_import_state_from_db(value: &str) -> rusqlite::Result<CredentialImportState> {
    match value {
        "pending" => Ok(CredentialImportState::Pending),
        "ready" => Ok(CredentialImportState::Ready),
        _ => Err(invalid_column(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown credential import state: {value}"),
        ))),
    }
}

fn normalized_public_key_metadata(
    public_key_algorithm: Option<&str>,
    public_key_fingerprint: Option<&str>,
) -> Result<(Option<String>, Option<String>)> {
    Ok((
        normalized_optional(
            public_key_algorithm,
            128,
            "public key algorithm is too long",
        )?,
        normalized_optional(
            public_key_fingerprint,
            256,
            "public key fingerprint is too long",
        )?,
    ))
}

fn validate_public_key_metadata(
    kind: CredentialKind,
    public_key_algorithm: &Option<String>,
    public_key_fingerprint: &Option<String>,
) -> Result<()> {
    match kind {
        CredentialKind::Password
            if public_key_algorithm.is_some() || public_key_fingerprint.is_some() =>
        {
            Err(AppPersistenceError::InvalidInput(
                "password credentials cannot contain public key metadata",
            ))
        }
        CredentialKind::PrivateKey
            if public_key_algorithm.is_none() || public_key_fingerprint.is_none() =>
        {
            Err(AppPersistenceError::InvalidInput(
                "private key credentials require derived public key metadata",
            ))
        }
        _ => Ok(()),
    }
}

fn normalized_label(value: &str) -> Result<String> {
    normalized_required(value, 120, "label is empty or too long")
}

fn validated_idempotency_key(value: &str) -> Result<String> {
    if value.trim() != value {
        return Err(AppPersistenceError::InvalidInput(
            "idempotency key is empty or invalid",
        ));
    }
    normalized_required(value, 128, "idempotency key is empty or invalid")
}

fn normalized_required(value: &str, maximum: usize, error: &'static str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(|character| character.is_control())
    {
        return Err(AppPersistenceError::InvalidInput(error));
    }
    Ok(value.to_owned())
}

fn normalized_optional(
    value: Option<&str>,
    maximum: usize,
    error: &'static str,
) -> Result<Option<String>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalized_required(value, maximum, error))
        .transpose()
}

fn parse_id<T>(
    value: String,
    parse: impl FnOnce(String) -> std::result::Result<T, &'static str>,
) -> rusqlite::Result<T> {
    parse(value.clone()).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(AppPersistenceError::InvalidStoredData),
        )
    })
}

fn optional_id<T>(
    value: Option<String>,
    parse: impl FnOnce(String) -> std::result::Result<T, &'static str>,
) -> rusqlite::Result<Option<T>> {
    value.map(|value| parse_id(value, parse)).transpose()
}

fn read_wire_sequence(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<WireSequence> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value)
        .map(WireSequence::new)
        .map_err(invalid_column)
}

fn read_optional_wire_sequence(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<WireSequence>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            u64::try_from(value)
                .map(WireSequence::new)
                .map_err(invalid_column)
        })
        .transpose()
}

fn invalid_column(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error))
}

fn u64_to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| AppPersistenceError::Conflict)
}

fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn ensure_private_parent(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or(AppPersistenceError::InvalidInput(
        "database path has no parent",
    ))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn ensure_private_file(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use norishell_core_api::{
        AgentIdentityKind, AgentIdentitySource, AlgorithmCategory, AlgorithmCompatibilityException,
        AuthenticationMethodKind, AuthenticationPlanMode, CredentialKind, CredentialRefDetails,
        CredentialRefId, DesktopProfile, DesktopProtocol, DiskResourceId, ForwardRuleId,
        HeartbeatPolicy, HostCatalogSort, HostConfiguredCreateRequest,
        HostCreateLoginAutomationStep, HostId, IdentityId, LoginAutomationStepInput,
        MonitoringPolicy, NetworkResourceId, OperationId, PluginCapability, PluginId,
        PluginInstallState, PluginOperationId, PluginOperationKind, PluginOperationState,
        PortForwardRule, ProxyDnsMode, ProxyEndpoint, RequestId, RequestMeta, RouteIngress,
        SecretRefId, ShellHeartbeatLineEnding, SshAgentScope, SshCertificateCriticalOption,
        SshCertificateExtension, SshCertificateMetadata, SshCertificateType,
        TerminalWorkspaceLayout, TerminalWorkspaceLayoutNode, TerminalWorkspacePane,
        TerminalWorkspaceTab, WireSequence,
    };
    use norishell_ssh_domain::ssh_sha256_fingerprint;
    use rusqlite::{Connection, params};

    use super::{
        AppPersistenceError, AppRepository, CredentialImportState, CredentialRecordDetails,
        DEFAULT_ALGORITHM_POLICY_ID, HostBatchCreateInput, HostBatchJumpHostInput,
        HostCreatePasswordStageState, KnownHostObservation, LoginAutomationSecretStageState,
        PluginActivationPermissions, PluginCatalogEntryRecord, PluginCatalogTrustRecord,
        PluginInstalledRecord, PluginOperationPhase, PluginPermissionBinding, SshSyncHttpMethod,
        SshSyncHttpUploadAttemptInput, SshSyncHttpUploadAttemptState,
        SshSyncHttpUploadCompletionFence, SshSyncHttpUploadCompletionProof, SshSyncLocalObjectId,
        SshSyncLoginAutomation, SshSyncLoginAutomationStep, SshSyncObjectKind,
        SshSyncObjectMappingInput, SshSyncOwnedCreateBatch, SshSyncOwnedCredentialDelete,
        SshSyncOwnedCredentialUpdate, SshSyncOwnedDesktopProfileDelete,
        SshSyncOwnedDesktopProfileUpdate, SshSyncOwnedHostBaseVersions, SshSyncOwnedHostDelete,
        SshSyncOwnedHostUpdate, SshSyncOwnedIdentityDelete, SshSyncOwnedIdentityUpdate,
        SshSyncOwnedMetadataDelta, SshSyncOwnedSecretDelete, SshSyncOwnedSecretReplacement,
        SshSyncProfileKeyBinding, SshSyncProfileRemoteBaseline, SshSyncProfileScope,
        SshSyncProfileScopeMode, SshSyncProfileStateCreate, SshSyncProfileStateKey,
        SshSyncRestoreCredentialInput, SshSyncRestoreCredentialMaterial,
        SshSyncRestoreDesktopProfileInput, SshSyncRestoreHostInput, SshSyncRestoreIdentityInput,
        SshSyncRestorePlan, SshSyncRestoreSagaInput, SshSyncScopeMembershipInput,
        SshSyncScopeMembershipState, migrate_v2_to_v3, migrate_v3_to_v4, migrate_v4_to_v5,
        migrate_v5_to_v6, migrate_v6_to_v7,
    };

    fn repository(directory: &tempfile::TempDir) -> AppRepository {
        AppRepository::open(directory.path().join("data").join("norishell.sqlite3"))
            .expect("open repository")
    }

    fn plugin_permission_binding(artifact_sha256: &str) -> PluginPermissionBinding {
        PluginPermissionBinding {
            artifact_sha256: artifact_sha256.to_owned(),
            app_version_major: 0,
            app_version_minor: 1,
            secure_surface_contract_revision: 1,
        }
    }

    fn initial_plugin_permissions<'a>(
        binding: &'a PluginPermissionBinding,
        grants: &'a [(PluginCapability, bool)],
    ) -> PluginActivationPermissions<'a> {
        PluginActivationPermissions {
            protocol_major: 1,
            binding,
            previous_binding: None,
            expected_previous_grant_state_version: None,
            expected_previous_scope_state_version: None,
            grants,
            carried_host_scope_capabilities: &[],
            approved_host_scopes: None,
        }
    }

    fn configured_host_request(operation_id: OperationId) -> HostConfiguredCreateRequest {
        HostConfiguredCreateRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id,
            idempotency_key: "configured-host-create-once".to_owned(),
            label: "Production Edge".to_owned(),
            address: "EDGE.EXAMPLE.".to_owned(),
            port: 2222,
            username: Some("deploy".to_owned()),
            identity_id: None,
            favorite: true,
            group_id: None,
            tag_ids: vec![],
            ingress: RouteIngress::DirectTcp,
            jump_host_ids: vec![],
            authentication_mode: AuthenticationPlanMode::Identity,
            credential_ref_ids: vec![],
            algorithm_policy_id: DEFAULT_ALGORITHM_POLICY_ID.to_owned(),
            compatibility_exceptions: vec![],
            heartbeat_policy: HeartbeatPolicy::TransportKeepalive {
                interval_seconds: 30,
                reply_timeout_seconds: 10,
                failure_threshold: 3,
            },
            monitoring_policy: MonitoringPolicy {
                enabled: true,
                sample_interval_seconds: 15,
                sample_timeout_seconds: 5,
                disk_mount_ids: vec![DiskResourceId::Root],
                network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
            },
            login_automation_enabled: true,
            login_automation_confirmed: true,
            login_automation_steps: vec![HostCreateLoginAutomationStep::SendText {
                text: "export TERM=xterm-256color".to_owned(),
                append_enter: true,
                timeout_seconds: 5,
            }],
            staged_password_id: None,
        }
    }

    fn ssh_sync_restore_fixture() -> (SshSyncRestoreSagaInput, SshSyncRestorePlan) {
        let attempt_id = OperationId::new();
        let plugin_id = PluginId::parse("org.norixor.restore-tests").expect("plugin id");
        let profile_id = "portable-profile-a".to_owned();
        let bundle_sha256 = "a".repeat(64);
        let plan_sha256 = "b".repeat(64);
        let identity_id = IdentityId::new();
        let credential_ref_id = CredentialRefId::new();
        let secret_ref_id = SecretRefId::new();
        let host_id = HostId::new();
        let mut host_request = configured_host_request(OperationId::new());
        host_request.idempotency_key = "restore-host-portable-a".to_owned();
        host_request.identity_id = Some(identity_id.clone());
        host_request.login_automation_enabled = false;
        host_request.login_automation_confirmed = false;
        host_request.login_automation_steps.clear();

        let saga = SshSyncRestoreSagaInput {
            attempt_id: attempt_id.clone(),
            plugin_id: plugin_id.clone(),
            profile_id: profile_id.clone(),
            bundle_sha256: bundle_sha256.clone(),
            plan_sha256: plan_sha256.clone(),
            secret_ref_ids: vec![secret_ref_id.clone()],
        };
        let plan = SshSyncRestorePlan {
            attempt_id,
            plugin_id,
            profile_id,
            bundle_sha256,
            plan_sha256,
            identities: vec![SshSyncRestoreIdentityInput {
                identity_id: identity_id.clone(),
                label: "Restored identity".to_owned(),
                username: Some("deploy".to_owned()),
            }],
            credentials: vec![SshSyncRestoreCredentialInput {
                credential_ref_id,
                identity_id,
                operation_id: OperationId::new(),
                idempotency_key: "restore-credential-portable-a".to_owned(),
                priority: 0,
                label: "Restored password".to_owned(),
                material: SshSyncRestoreCredentialMaterial::Password { secret_ref_id },
            }],
            hosts: vec![SshSyncRestoreHostInput {
                host_id,
                request: host_request,
                tag_labels: Vec::new(),
                login_automation: SshSyncLoginAutomation::disabled(),
            }],
            desktop_profiles: Vec::new(),
        };
        (saga, plan)
    }

    fn initial_ssh_sync_host_versions() -> SshSyncOwnedHostBaseVersions {
        SshSyncOwnedHostBaseVersions {
            host: WireSequence::new(1),
            route: WireSequence::new(1),
            authentication: WireSequence::new(1),
            algorithm: WireSequence::new(1),
            heartbeat: WireSequence::new(1),
            monitoring: WireSequence::new(1),
            login_automation: WireSequence::new(1),
        }
    }

    fn ssh_sync_profile_fixture(profile_id: &str) -> SshSyncProfileStateCreate {
        SshSyncProfileStateCreate {
            key: SshSyncProfileStateKey {
                plugin_id: PluginId::parse("org.norixor.profile-state-tests").expect("plugin id"),
                signer_fingerprint_sha256: "1".repeat(64),
                profile_id: profile_id.to_owned(),
            },
            scope: SshSyncProfileScope {
                mode: SshSyncProfileScopeMode::Custom,
                custom_host_ids: vec![HostId::new()],
                custom_credential_ref_ids: vec![CredentialRefId::new()],
                custom_desktop_profile_ids: Vec::new(),
            },
        }
    }

    fn ssh_sync_desktop_profile(
        host_id: HostId,
        credential_ref_id: CredentialRefId,
    ) -> DesktopProfile {
        DesktopProfile {
            id: uuid::Uuid::new_v4().to_string(),
            label: "Synced desktop".to_owned(),
            protocol: DesktopProtocol::Rdp,
            address: "rdp.example".to_owned(),
            port: 3389,
            username: "desktop-user".to_owned(),
            domain: String::new(),
            host_id: Some(host_id),
            gateway_host_id: None,
            credential_ref_id: Some(credential_ref_id),
            width: 1440,
            height: 900,
            clipboard_enabled: true,
            audio_playback_enabled: true,
            revision: WireSequence::new(0),
        }
    }

    fn empty_ssh_sync_owned_delta(
        owner: SshSyncProfileStateKey,
        delta_sha256: char,
    ) -> SshSyncOwnedMetadataDelta {
        SshSyncOwnedMetadataDelta {
            owner,
            attempt_id: OperationId::new(),
            delta_sha256: delta_sha256.to_string().repeat(64),
            creates: None,
            identity_updates: Vec::new(),
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        }
    }

    #[test]
    fn configured_host_create_is_atomic_and_exactly_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let group = repository.create_host_group("Production").expect("group");
        let tag = repository.create_host_tag("Linux").expect("tag");
        let operation_id = OperationId::new();
        let mut request = configured_host_request(operation_id.clone());
        request.group_id = Some(group.group_id.clone());
        request.tag_ids = vec![tag.tag_id.clone()];

        let created = repository
            .create_host_configured(&request)
            .expect("configured create");
        assert_eq!(created.host.normalized_address, "edge.example");
        assert_eq!(created.host.state_version, WireSequence::new(1));
        assert_eq!(created.organization.group_id, Some(group.group_id));
        assert_eq!(created.organization.tag_ids, [tag.tag_id]);
        assert_eq!(
            created.connection_config.route_plan.revision,
            WireSequence::new(1)
        );
        assert_eq!(
            created
                .connection_config
                .login_automation
                .confirmed_revision,
            Some(WireSequence::new(1))
        );
        assert!(created.connection_config.monitoring_policy.policy.enabled);

        let replay = repository
            .create_host_configured(&request)
            .expect("exact replay");
        assert_eq!(replay, created);
        assert_eq!(repository.list_hosts().expect("hosts").len(), 1);

        let mut changed = request;
        changed.label = "Changed label".to_owned();
        assert!(matches!(
            repository.create_host_configured(&changed),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert_eq!(repository.list_hosts().expect("hosts").len(), 1);

        let mut invalid = configured_host_request(OperationId::new());
        invalid.idempotency_key = "configured-host-create-invalid".to_owned();
        invalid.login_automation_enabled = false;
        assert!(matches!(
            repository.create_host_configured(&invalid),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert_eq!(repository.list_hosts().expect("hosts").len(), 1);
    }

    #[test]
    fn configured_host_create_validates_selected_identity_without_leaving_a_host_on_failure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Production deploy", Some("deploy"))
            .expect("identity");
        let mut request = configured_host_request(OperationId::new());
        request.idempotency_key = "configured-host-create-identity".to_owned();
        request.identity_id = Some(identity.identity_id.clone());

        let created = repository
            .create_host_configured(&request)
            .expect("configured create with identity");
        assert_eq!(created.host.identity_id, Some(identity.identity_id));

        let mut missing_identity_request = configured_host_request(OperationId::new());
        missing_identity_request.idempotency_key =
            "configured-host-create-missing-identity".to_owned();
        missing_identity_request.identity_id = Some(IdentityId::new());
        assert!(matches!(
            repository.create_host_configured(&missing_identity_request),
            Err(AppPersistenceError::NotFound)
        ));
        assert_eq!(repository.list_hosts().expect("hosts").len(), 1);
    }

    #[test]
    fn staged_password_is_consumed_with_dedicated_ready_identity_in_host_transaction() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let operation_id = OperationId::new();
        let begin = repository
            .begin_host_create_password_stage_with_outcome(
                &operation_id,
                "configured-host-create-once",
                "Production Edge identity",
                "Password",
            )
            .expect("begin stage");
        assert!(begin.created);
        let staged = repository
            .mark_host_create_password_staged(&begin.record.staged_password_id)
            .expect("mark staged");
        let replay = repository
            .begin_host_create_password_stage_with_outcome(
                &operation_id,
                "configured-host-create-once",
                "Production Edge identity",
                "Password",
            )
            .expect("stage replay");
        assert!(!replay.created);
        assert_eq!(replay.record.staged_password_id, staged.staged_password_id);

        let mut request = configured_host_request(operation_id.clone());
        request.staged_password_id = Some(staged.staged_password_id.clone());
        let created = repository
            .create_host_configured(&request)
            .expect("consume staged password");
        let identity_id = created.host.identity_id.expect("dedicated identity");
        let credentials = repository
            .list_ready_credential_records_for_host(&created.host.host_id)
            .expect("ready credentials");
        assert_eq!(credentials.len(), 1);
        assert_eq!(credentials[0].identity_id, identity_id);
        assert_eq!(credentials[0].method, AuthenticationMethodKind::Password);
        assert!(matches!(
            credentials[0].details,
            CredentialRecordDetails::Password { ref secret_ref_id }
                if *secret_ref_id == staged.secret_ref_id
        ));
        let consumed = super::lookup_host_create_password_stage(
            &repository.connection,
            "stage_id",
            staged.staged_password_id.as_str(),
        )
        .expect("lookup stage")
        .expect("stage");
        assert_eq!(consumed.state, HostCreatePasswordStageState::Consumed);
        assert!(
            !repository
                .begin_host_create_password_cleanup(&operation_id, "configured-host-create-once")
                .expect("cancel consumed")
                .is_none()
        );

        let replay = repository
            .create_host_configured(&request)
            .expect("configured replay");
        assert_eq!(replay.host.host_id, created.host.host_id);
        assert_eq!(repository.list_identities().expect("identities").len(), 1);
    }

    #[test]
    fn v16_migration_repairs_v15_database_missing_host_password_stage_storage() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        {
            let repository = AppRepository::open(&database_path).expect("current repository");
            super::remove_schema_added_after_fixture_version(&repository.connection, 15).unwrap();
            repository
                .connection
                .execute_batch(
                    "DROP TABLE plugin_host_scope_grants;
                     DROP TABLE plugin_host_scope_sets;
                     DROP TABLE host_create_password_stages;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 15;",
                )
                .expect("simulate affected v15 database");
        }

        let repository = AppRepository::open(&database_path).expect("repair v15 database");
        let stage_table_exists: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'host_create_password_stages'",
                [],
                |row| row.get(0),
            )
            .expect("read repaired stage table");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read repaired schema version");
        assert_eq!(stage_table_exists, 1);
        assert_eq!(version, super::SCHEMA_VERSION);
    }

    #[test]
    fn v20_migration_preserves_grants_and_expands_only_the_capability_check() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let plugin_id = PluginId::parse("com.norishell.migrate-sync").expect("plugin id");
        {
            let mut repository = AppRepository::open(&database_path).expect("current repository");
            super::remove_schema_added_after_fixture_version(&repository.connection, 19).unwrap();
            let plugin = PluginInstalledRecord {
                plugin_id: plugin_id.clone(),
                name: "Migration Fixture".to_owned(),
                publisher: "NoriShell".to_owned(),
                signer_fingerprint_sha256: "1".repeat(64),
                active_version: "1.0.0".to_owned(),
                package_sha256: "a".repeat(64),
                capabilities: vec![PluginCapability::UiPanel],
                state: PluginInstallState::Disabled,
                state_version: WireSequence::new(1),
                installed_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            };
            repository
                .activate_plugin_installation(
                    None,
                    &plugin,
                    100,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(initial_plugin_permissions(
                        &plugin_permission_binding(&plugin.package_sha256),
                        &[(PluginCapability::UiPanel, true)],
                    )),
                )
                .expect("install fixture");
            repository
                .connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v20;
                     CREATE TABLE plugin_capability_grants (
                       plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
                       signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
                       major_version INTEGER NOT NULL CHECK(major_version >= 0),
                       capability TEXT NOT NULL CHECK(capability IN
                         ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
                          'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
                          'clipboard.write', 'terminal.metadata', 'terminal.observe',
                          'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
                          'host.metadata.read', 'host.mutation.propose', 'host.session.request',
                          'network.domain', 'storage.plugin', 'sftp.read', 'sftp.write',
                          'metrics.read')),
                       granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
                       state_version INTEGER NOT NULL CHECK(state_version >= 1),
                       updated_at_ms INTEGER NOT NULL,
                       PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
                     ) STRICT;
                     INSERT INTO plugin_capability_grants
                       SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                              granted, state_version, updated_at_ms
                       FROM plugin_capability_grants_v20;
                     DROP TABLE plugin_capability_grants_v20;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 19;
                     COMMIT;",
                )
                .expect("construct v19 fixture");
        }

        let repository = AppRepository::open(&database_path).expect("migrate v19 repository");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read schema version");
        assert_eq!(version, super::SCHEMA_VERSION);
        let grants = repository
            .list_plugin_capability_grants(&plugin_id, &"1".repeat(64), 1)
            .expect("read preserved grant");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].capability, PluginCapability::UiPanel);
        assert!(grants[0].granted);
        assert_eq!(grants[0].state_version, WireSequence::new(1));

        repository
            .connection
            .execute(
                "INSERT INTO plugin_capability_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, capability,
                  granted, state_version, updated_at_ms)
                 VALUES (?1, ?2, 2, 'ssh.sync', 0, 1, 1)",
                params![plugin_id.as_str(), "1".repeat(64)],
            )
            .expect("new capability satisfies expanded CHECK");
        assert!(
            repository
                .connection
                .execute(
                    "INSERT INTO plugin_capability_grants
                     (plugin_id, signer_fingerprint_sha256, major_version, capability,
                      granted, state_version, updated_at_ms)
                     VALUES (?1, ?2, 3, 'network.anything', 0, 1, 1)",
                    params![plugin_id.as_str(), "1".repeat(64)],
                )
                .is_err()
        );
        assert!(
            repository
                .connection
                .execute(
                    "INSERT INTO plugin_capability_grants
                     (plugin_id, signer_fingerprint_sha256, major_version, capability,
                      granted, state_version, updated_at_ms)
                     VALUES (?1, ?2, 4, 'ssh.sync', 2, 1, 1)",
                    params![plugin_id.as_str(), "1".repeat(64)],
                )
                .is_err()
        );
        let foreign_key_violations: i64 = repository
            .connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("check foreign keys");
        assert_eq!(foreign_key_violations, 0);
    }

    #[test]
    fn v27_migration_binds_only_the_current_active_permission_set() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let plugin_id = PluginId::parse("org.norixor").expect("plugin id");
        let host_id;
        {
            let mut repository = AppRepository::open(&database_path).expect("current repository");
            super::remove_schema_added_after_fixture_version(&repository.connection, 26).unwrap();
            host_id = repository
                .create_host("Current", "current.example", 22, None, None, false)
                .expect("host")
                .host_id;
            let plugin = PluginInstalledRecord {
                plugin_id: plugin_id.clone(),
                name: "Norixor".to_owned(),
                publisher: "Norixor".to_owned(),
                signer_fingerprint_sha256: "1".repeat(64),
                active_version: "1.0.7".to_owned(),
                package_sha256: "a".repeat(64),
                capabilities: vec![
                    PluginCapability::SshSync,
                    PluginCapability::HostMetadataRead,
                ],
                state: PluginInstallState::Disabled,
                state_version: WireSequence::new(1),
                installed_at_unix_ms: 1,
                updated_at_unix_ms: 1,
            };
            let binding = plugin_permission_binding(&plugin.package_sha256);
            repository
                .activate_plugin_installation(
                    None,
                    &plugin,
                    100,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(initial_plugin_permissions(
                        &binding,
                        &[
                            (PluginCapability::SshSync, true),
                            (PluginCapability::HostMetadataRead, true),
                        ],
                    )),
                )
                .expect("activate current plugin");
            repository
                .replace_plugin_host_scope_grants(
                    &plugin_id,
                    &plugin.signer_fingerprint_sha256,
                    1,
                    plugin.state_version,
                    None,
                    &binding,
                    &[(host_id.clone(), PluginCapability::HostMetadataRead)],
                )
                .expect("current Host scope");
            repository
                .connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     ALTER TABLE plugin_host_scope_grants RENAME TO plugin_host_scope_grants_current;
                     ALTER TABLE plugin_host_scope_sets RENAME TO plugin_host_scope_sets_current;
                     ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_current;
                     CREATE TABLE plugin_capability_grants (
                       plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
                       signer_fingerprint_sha256 TEXT NOT NULL,
                       major_version INTEGER NOT NULL,
                       capability TEXT NOT NULL,
                       granted INTEGER NOT NULL,
                       state_version INTEGER NOT NULL,
                       updated_at_ms INTEGER NOT NULL,
                       PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability)
                     );
                     INSERT INTO plugin_capability_grants
                       SELECT plugin_id, signer_fingerprint_sha256, major_version, capability,
                              granted, state_version, updated_at_ms
                       FROM plugin_capability_grants_current;
                     INSERT INTO plugin_capability_grants
                       VALUES ('org.norixor',
                               '2222222222222222222222222222222222222222222222222222222222222222',
                               1, 'ssh.sync', 1, 9, 9);
                     CREATE TABLE plugin_host_scope_sets (
                       plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
                       signer_fingerprint_sha256 TEXT NOT NULL,
                       major_version INTEGER NOT NULL,
                       state_version INTEGER NOT NULL,
                       updated_at_ms INTEGER NOT NULL,
                       PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version)
                     );
                     INSERT INTO plugin_host_scope_sets
                       SELECT plugin_id, signer_fingerprint_sha256, major_version,
                              state_version, updated_at_ms
                       FROM plugin_host_scope_sets_current;
                     CREATE TABLE plugin_host_scope_grants (
                       plugin_id TEXT NOT NULL,
                       signer_fingerprint_sha256 TEXT NOT NULL,
                       major_version INTEGER NOT NULL,
                       host_id TEXT NOT NULL,
                       capability TEXT NOT NULL,
                       state_version INTEGER NOT NULL,
                       updated_at_ms INTEGER NOT NULL,
                       PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, host_id, capability),
                       FOREIGN KEY(plugin_id, signer_fingerprint_sha256, major_version)
                         REFERENCES plugin_host_scope_sets(plugin_id, signer_fingerprint_sha256, major_version)
                         ON DELETE CASCADE
                     );
                     INSERT INTO plugin_host_scope_grants SELECT * FROM plugin_host_scope_grants_current;
                     DROP TABLE plugin_host_scope_grants_current;
                     DROP TABLE plugin_host_scope_sets_current;
                     DROP TABLE plugin_capability_grants_current;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 26;",
                )
                .expect("simulate v26 permission schema");
        }

        let repository = AppRepository::open(&database_path).expect("migrate v26 repository");
        let grants = repository
            .list_plugin_capability_grants(&plugin_id, &"1".repeat(64), 1)
            .expect("current grants");
        assert_eq!(grants.len(), 2);
        assert!(grants.iter().any(|grant| {
            grant.capability == PluginCapability::SshSync
                && grant.granted
                && grant.binding == Some(plugin_permission_binding(&"a".repeat(64)))
        }));
        let historical = repository
            .list_plugin_capability_grants(&plugin_id, &"2".repeat(64), 1)
            .expect("historical signer grant");
        assert_eq!(historical.len(), 1);
        assert_eq!(historical[0].binding, None);
        let scope = repository
            .plugin_host_scope_set(&plugin_id, &"1".repeat(64), 1)
            .expect("scope set")
            .expect("current scope set");
        assert_eq!(
            scope.binding,
            Some(plugin_permission_binding(&"a".repeat(64)))
        );
        assert_eq!(
            repository
                .list_plugin_host_scope_grants(&plugin_id, &"1".repeat(64), 1)
                .expect("scope grants")[0]
                .host_id,
            host_id
        );
    }

    #[test]
    fn saved_forward_rules_are_versioned_templates_with_cas() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let host = repository
            .create_host("Edge", "edge.example", 22, Some("root"), None, false)
            .expect("host");
        let rule_id = ForwardRuleId::new();
        let local = PortForwardRule::Local {
            host_id: host.host_id.clone(),
            local_bind_address: "127.0.0.1".to_owned(),
            local_listen_port: 8080,
            remote_target_host: "127.0.0.1".to_owned(),
            remote_target_port: 80,
        };
        let created = repository
            .create_forward_rule(&rule_id, "Web preview", &local)
            .expect("create rule");
        assert_eq!(created.state_version, WireSequence::new(1));
        assert_eq!(
            repository.list_forward_rules().unwrap(),
            std::slice::from_ref(&created)
        );

        let dynamic = PortForwardRule::Dynamic {
            host_id: host.host_id,
            local_bind_address: "127.0.0.1".to_owned(),
            local_listen_port: 1080,
        };
        let updated = repository
            .update_forward_rule(&rule_id, created.state_version, "SOCKS workspace", &dynamic)
            .expect("update rule");
        assert_eq!(updated.state_version, WireSequence::new(2));
        assert!(matches!(
            repository.update_forward_rule(&rule_id, created.state_version, "stale", &dynamic,),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.delete_forward_rule(&rule_id, WireSequence::new(1)),
            Err(AppPersistenceError::Conflict)
        ));
        repository
            .delete_forward_rule(&rule_id, updated.state_version)
            .expect("delete rule");
        assert!(repository.list_forward_rules().unwrap().is_empty());
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, super::SCHEMA_VERSION);
    }

    #[test]
    fn login_automation_secret_staging_is_bound_one_time_and_recoverable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let target = repository
            .create_host("Target", "target.example", 22, Some("root"), None, false)
            .expect("target host");
        let other = repository
            .create_host("Other", "other.example", 22, Some("root"), None, false)
            .expect("other host");
        let revision = repository
            .get_host_connection_config(&target.host_id)
            .expect("target config")
            .login_automation
            .revision;
        let operation_id = OperationId::new();
        let first_begin = repository
            .begin_login_automation_secret_stage_with_outcome(
                &target.host_id,
                revision,
                &operation_id,
                "login-automation-stage-once",
                "Root password",
            )
            .expect("begin stage");
        assert!(first_begin.created);
        let staged = first_begin.record;
        let replay = repository
            .begin_login_automation_secret_stage_with_outcome(
                &target.host_id,
                revision,
                &operation_id,
                "login-automation-stage-once",
                "Root password",
            )
            .expect("exact replay");
        assert!(!replay.created);
        assert_eq!(replay.record, staged);
        assert!(matches!(
            repository.begin_login_automation_secret_stage(
                &target.host_id,
                revision,
                &operation_id,
                "login-automation-stage-once",
                "Changed label",
            ),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert!(matches!(
            repository.begin_login_automation_secret_stage(
                &other.host_id,
                WireSequence::new(1),
                &operation_id,
                "login-automation-stage-once",
                "Root password",
            ),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert!(matches!(
            repository.begin_login_automation_secret_stage(
                &other.host_id,
                WireSequence::new(2),
                &OperationId::new(),
                "stale-revision-stage",
                "Root password",
            ),
            Err(AppPersistenceError::Conflict)
        ));

        let staged = repository
            .mark_login_automation_secret_staged(&staged.staged_secret_id)
            .expect("mark staged");
        assert!(!format!("{staged:?}").contains(staged.secret_ref_id.as_str()));
        let step = LoginAutomationStepInput::SendSecret {
            secret_ref_id: staged.staged_secret_id.clone(),
            secret_label: "untrusted client label".to_owned(),
            append_enter: true,
            timeout_seconds: 10,
        };
        assert!(matches!(
            repository.replace_login_automation(
                &other.host_id,
                WireSequence::new(1),
                true,
                std::slice::from_ref(&step),
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.replace_login_automation(
                &target.host_id,
                revision,
                true,
                &[
                    step.clone(),
                    LoginAutomationStepInput::PreserveExistingSecret {
                        existing_ordinal: 31,
                        append_enter: true,
                        timeout_seconds: 10,
                    },
                ],
            ),
            Err(AppPersistenceError::Conflict)
        ));
        let state_after_rollback: String = repository
            .connection
            .query_row(
                "SELECT state FROM login_automation_secret_stages WHERE stage_id = ?1",
                [staged.staged_secret_id.as_str()],
                |row| row.get(0),
            )
            .expect("stage state");
        assert_eq!(state_after_rollback, "staged");

        let replaced = repository
            .replace_login_automation(&target.host_id, revision, true, std::slice::from_ref(&step))
            .expect("consume staged handle");
        assert_eq!(replaced.revision, WireSequence::new(2));
        assert_eq!(
            repository
                .begin_login_automation_secret_cleanup(
                    &operation_id,
                    "login-automation-stage-once",
                )
                .expect("consumed cleanup lookup")
                .expect("consumed stage")
                .state,
            LoginAutomationSecretStageState::Consumed
        );
        let stored_secret_ref: String = repository
            .connection
            .query_row(
                "SELECT secret_ref_id FROM host_login_automation_steps WHERE host_id = ?1",
                [target.host_id.as_str()],
                |row| row.get(0),
            )
            .expect("stored secret ref");
        assert_eq!(stored_secret_ref, staged.secret_ref_id.as_str());
        assert!(matches!(
            repository.replace_login_automation(
                &target.host_id,
                replaced.revision,
                true,
                std::slice::from_ref(&step),
            ),
            Err(AppPersistenceError::Conflict)
        ));
        let preserved = repository
            .replace_login_automation(
                &target.host_id,
                replaced.revision,
                true,
                &[LoginAutomationStepInput::PreserveExistingSecret {
                    existing_ordinal: 0,
                    append_enter: false,
                    timeout_seconds: 12,
                }],
            )
            .expect("preserve internal secret");
        assert_eq!(preserved.revision, WireSequence::new(3));
        let snapshot = repository
            .get_host_connection_snapshot(&target.host_id)
            .expect("legacy-compatible execution snapshot");
        assert!(matches!(
            snapshot.login_automation.steps.as_slice(),
            [LoginAutomationStepInput::SendSecret { secret_ref_id, secret_label, .. }]
                if secret_ref_id.as_str() == staged.secret_ref_id.as_str()
                    && secret_label == "Root password"
        ));

        let expiring_operation_id = OperationId::new();
        let expiring = repository
            .begin_login_automation_secret_stage(
                &other.host_id,
                WireSequence::new(1),
                &expiring_operation_id,
                "expiring-stage",
                "Expiring password",
            )
            .expect("begin expiring stage");
        repository
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_login_secret_mark
                 BEFORE UPDATE OF state ON login_automation_secret_stages
                 WHEN NEW.state = 'staged'
                 BEGIN SELECT RAISE(ABORT, 'injected mark failure'); END;",
            )
            .expect("install mark failure");
        assert!(matches!(
            repository.mark_login_automation_secret_staged(&expiring.staged_secret_id),
            Err(AppPersistenceError::Database(_))
        ));
        repository
            .connection
            .execute_batch("DROP TRIGGER fail_login_secret_mark;")
            .expect("remove mark failure");
        repository
            .mark_login_automation_secret_staged(&expiring.staged_secret_id)
            .expect("mark expiring stage");
        repository
            .connection
            .execute(
                "UPDATE login_automation_secret_stages SET expires_at_ms = 0 WHERE stage_id = ?1",
                [expiring.staged_secret_id.as_str()],
            )
            .expect("expire stage");
        assert!(
            repository
                .list_login_automation_secret_cleanup_candidates(64)
                .expect("expired cleanup candidates")
                .iter()
                .any(|candidate| candidate.staged_secret_id == expiring.staged_secret_id)
        );
        assert!(matches!(
            repository.replace_login_automation(
                &other.host_id,
                WireSequence::new(1),
                true,
                &[LoginAutomationStepInput::SendSecret {
                    secret_ref_id: expiring.staged_secret_id.clone(),
                    secret_label: "ignored".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                }],
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.begin_login_automation_secret_cleanup(&operation_id, "expiring-stage"),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert!(
            repository
                .begin_login_automation_secret_cleanup(&OperationId::new(), "missing-stage")
                .expect("missing cleanup")
                .is_none()
        );
        let cleanup = repository
            .begin_login_automation_secret_cleanup(&expiring_operation_id, "expiring-stage")
            .expect("begin cleanup")
            .expect("cleanup record");
        assert_eq!(
            cleanup.state,
            LoginAutomationSecretStageState::CleanupPending
        );
        repository
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_login_secret_finalize
                 BEFORE UPDATE OF state ON login_automation_secret_stages
                 WHEN NEW.state = 'cancelled'
                 BEGIN SELECT RAISE(ABORT, 'injected finalize failure'); END;",
            )
            .expect("install finalize failure");
        assert!(matches!(
            repository.finalize_login_automation_secret_cleanup(&expiring.staged_secret_id),
            Err(AppPersistenceError::Database(_))
        ));
        assert_eq!(
            repository
                .begin_login_automation_secret_cleanup(&expiring_operation_id, "expiring-stage")
                .expect("retry cleanup after definite failure")
                .expect("cleanup remains durable")
                .state,
            LoginAutomationSecretStageState::CleanupPending
        );
        repository
            .connection
            .execute_batch("DROP TRIGGER fail_login_secret_finalize;")
            .expect("remove finalize failure");
        assert!(
            repository
                .finalize_login_automation_secret_cleanup(&expiring.staged_secret_id)
                .expect("finalize cleanup")
                .cancelled
        );
        assert_eq!(
            repository
                .begin_login_automation_secret_cleanup(&expiring_operation_id, "expiring-stage")
                .expect("cancel replay")
                .expect("cancelled record")
                .state,
            LoginAutomationSecretStageState::Cancelled
        );
        assert!(
            !repository
                .finalize_login_automation_secret_cleanup(&expiring.staged_secret_id)
                .expect("finalize replay")
                .cancelled
        );
        let poison_operation_id = OperationId::new();
        let poison_stage = repository
            .begin_login_automation_secret_stage(
                &other.host_id,
                WireSequence::new(1),
                &poison_operation_id,
                "poisoned-replace-stage",
                "Poisoned password",
            )
            .expect("begin poison stage");
        repository
            .mark_login_automation_secret_staged(&poison_stage.staged_secret_id)
            .expect("mark poison stage");
        repository.login_automation_secret_stages_poisoned = true;
        assert!(matches!(
            repository
                .begin_login_automation_secret_cleanup(&expiring_operation_id, "expiring-stage"),
            Err(AppPersistenceError::RequiresReload)
        ));
        assert!(matches!(
            repository.replace_login_automation(
                &other.host_id,
                WireSequence::new(1),
                true,
                &[LoginAutomationStepInput::SendSecret {
                    secret_ref_id: poison_stage.staged_secret_id.clone(),
                    secret_label: "ignored".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                }],
            ),
            Err(AppPersistenceError::RequiresReload)
        ));
        let unchanged_revision: i64 = repository
            .connection
            .query_row(
                "SELECT revision FROM host_login_automations WHERE host_id = ?1",
                [other.host_id.as_str()],
                |row| row.get(0),
            )
            .expect("unchanged automation revision");
        assert_eq!(unchanged_revision, 1);
        let unconsumed_state: String = repository
            .connection
            .query_row(
                "SELECT state FROM login_automation_secret_stages WHERE stage_id = ?1",
                [poison_stage.staged_secret_id.as_str()],
                |row| row.get(0),
            )
            .expect("unconsumed stage state");
        assert_eq!(unconsumed_state, "staged");
    }

    #[test]
    fn v11_migration_preserves_legacy_login_automation_execution_data() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let host = repository
            .create_host("Legacy", "legacy.example", 22, None, None, false)
            .expect("host");
        let secret_ref_id = SecretRefId::new();
        super::remove_schema_added_after_fixture_version(&repository.connection, 10).unwrap();
        repository
            .connection
            .execute(
                "UPDATE host_login_automations SET enabled = 1, revision = 2 WHERE host_id = ?1",
                [host.host_id.as_str()],
            )
            .expect("enable legacy automation");
        repository
            .connection
            .execute(
                "INSERT INTO host_login_automation_steps
                 (host_id, ordinal, kind, literal_text, append_enter, timeout_seconds,
                  secret_ref_id, secret_label)
                 VALUES (?1, 0, 'send_secret', NULL, 1, 10, ?2, 'Legacy password')",
                params![host.host_id.as_str(), secret_ref_id.as_str()],
            )
            .expect("insert legacy step");
        repository
            .connection
            .execute_batch(
                "DROP TABLE plugin_host_scope_grants;
                 DROP TABLE plugin_host_scope_sets;
                 DROP TABLE forward_rules;
                 DROP TABLE login_automation_secret_stages;
                 DROP TABLE host_create_password_stages;
                 DROP TABLE host_configured_create_operations;
                 DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                 PRAGMA user_version = 10;",
            )
            .expect("restore v10 schema boundary");
        drop(repository);

        let repository = AppRepository::open(&database_path).expect("migrate v11");
        let snapshot = repository
            .get_host_connection_snapshot(&host.host_id)
            .expect("legacy execution snapshot");
        assert!(matches!(
            snapshot.login_automation.steps.as_slice(),
            [LoginAutomationStepInput::SendSecret { secret_ref_id: stored, secret_label, .. }]
                if stored.as_str() == secret_ref_id.as_str()
                    && secret_label == "Legacy password"
        ));
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, super::SCHEMA_VERSION);
    }

    #[test]
    fn v10_migration_normalizes_bounds_and_populates_canonical_monitoring_resources() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let host = repository
            .create_host("Legacy", "legacy.example", 22, None, None, false)
            .expect("host");
        super::remove_schema_added_after_fixture_version(&repository.connection, 9).unwrap();
        repository
            .connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 ALTER TABLE host_monitoring_policies RENAME TO host_monitoring_policies_v10;
                 CREATE TABLE host_monitoring_policies (
                   host_id TEXT PRIMARY KEY REFERENCES hosts(id) ON DELETE CASCADE,
                   revision INTEGER NOT NULL CHECK(revision >= 1),
                   enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
                   sample_interval_seconds INTEGER NOT NULL
                     CHECK(sample_interval_seconds BETWEEN 10 AND 3600),
                   sample_timeout_seconds INTEGER NOT NULL
                     CHECK(sample_timeout_seconds BETWEEN 1 AND 60),
                   created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
                   CHECK(sample_timeout_seconds < sample_interval_seconds)
                 ) STRICT;
                 INSERT INTO host_monitoring_policies
                   (host_id, revision, enabled, sample_interval_seconds,
                    sample_timeout_seconds, created_at_ms, updated_at_ms)
                   SELECT host_id, revision, enabled, 3600, 60, created_at_ms, updated_at_ms
                 FROM host_monitoring_policies_v10;
                 DROP TABLE host_monitoring_policies_v10;
                 DROP TABLE forward_rules;
                 DROP TABLE plugin_host_scope_grants;
                 DROP TABLE plugin_host_scope_sets;
                 DROP TABLE login_automation_secret_stages;
                 DROP TABLE host_create_password_stages;
                 DROP TABLE host_configured_create_operations;
                 DELETE FROM host_monitoring_selections;
                 DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                 PRAGMA user_version = 9;
                 PRAGMA foreign_keys = ON;",
            )
            .expect("prepare v9 monitoring fixture");
        drop(repository);

        let repository = AppRepository::open(&database_path).expect("migrate v10");
        let policy = repository
            .get_monitoring_policy(&host.host_id)
            .expect("monitoring policy");
        assert_eq!(policy.policy.sample_interval_seconds, 300);
        assert_eq!(policy.policy.sample_timeout_seconds, 30);
        assert_eq!(policy.policy.disk_mount_ids, [DiskResourceId::Root]);
        assert_eq!(
            policy.policy.network_interface_ids,
            [NetworkResourceId::AggregateNonLoopback]
        );
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, super::SCHEMA_VERSION);
    }

    #[test]
    fn monitoring_policy_requires_both_canonical_resources_in_storage_and_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let host = repository
            .create_host("Monitored", "metrics.example", 22, None, None, false)
            .expect("host");
        let initial = repository
            .get_monitoring_policy(&host.host_id)
            .expect("default policy");
        assert_eq!(initial.policy.disk_mount_ids, [DiskResourceId::Root]);
        assert_eq!(
            initial.policy.network_interface_ids,
            [NetworkResourceId::AggregateNonLoopback]
        );
        assert!(matches!(
            repository.replace_monitoring_policy(
                &host.host_id,
                initial.revision,
                &MonitoringPolicy {
                    enabled: true,
                    sample_interval_seconds: 15,
                    sample_timeout_seconds: 5,
                    disk_mount_ids: Vec::new(),
                    network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
                },
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        repository
            .connection
            .execute(
                "DELETE FROM host_monitoring_selections
                 WHERE host_id = ?1 AND kind = 'disk_mount'",
                [host.host_id.as_str()],
            )
            .expect("remove canonical disk selection");
        assert!(matches!(
            repository.get_monitoring_policy(&host.host_id),
            Err(AppPersistenceError::InvalidStoredData)
        ));
    }

    #[test]
    fn ssh_sync_identity_creation_is_idempotent_and_rejects_field_drift() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity_id = IdentityId::new();
        let first = repository
            .create_ssh_sync_identity(&identity_id, "Production", Some("deploy"))
            .expect("create deterministic identity");
        let replay = repository
            .create_ssh_sync_identity(&identity_id, "Production", Some("deploy"))
            .expect("replay deterministic identity");
        assert_eq!(first, replay);
        assert!(matches!(
            repository.create_ssh_sync_identity(&identity_id, "Changed", Some("deploy")),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
    }

    #[test]
    fn identity_update_and_delete_are_cas_bounded_reference_safe_and_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Production", Some("deploy"))
            .expect("identity");
        assert!(matches!(
            repository.update_identity(&identity.identity_id, WireSequence::new(2), "Stale", None,),
            Err(AppPersistenceError::Conflict)
        ));
        let updated = repository
            .update_identity(
                &identity.identity_id,
                identity.state_version,
                "Production updated",
                Some("release"),
            )
            .expect("update identity");
        assert_eq!(updated.state_version, WireSequence::new(2));
        assert_eq!(updated.label, "Production updated");
        assert_eq!(updated.username.as_deref(), Some("release"));

        let host = repository
            .create_host(
                "Referenced host",
                "identity.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("referencing host");
        let pending = repository
            .begin_credential_import(
                &OperationId::new(),
                "identity-impact-pending",
                &identity.identity_id,
                CredentialKind::Password,
                0,
                "Pending password",
                false,
            )
            .expect("pending credential");
        let impact = repository
            .identity_delete_impact(&identity.identity_id)
            .expect("delete impact");
        assert_eq!(impact.identity, updated);
        assert_eq!(impact.referencing_host_count, 1);
        assert_eq!(impact.referencing_hosts[0].host_id, host.host_id);
        assert_eq!(impact.referencing_credential_ref_count, 1);
        assert_eq!(
            impact.referencing_credential_refs[0].credential_ref_id,
            pending.credential_ref_id
        );
        let public_impact = serde_json::to_string(&impact).expect("serialize impact");
        if let CredentialRecordDetails::Password { secret_ref_id } = &pending.details {
            assert!(!public_impact.contains(secret_ref_id.as_str()));
        } else {
            panic!("expected password credential");
        }
        assert!(matches!(
            repository.delete_identity(&identity.identity_id, identity.state_version),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.delete_identity(&identity.identity_id, updated.state_version),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(repository.get_host(&host.host_id).expect("host"), host);
        assert_eq!(
            repository
                .get_credential_import_record(&pending.credential_ref_id)
                .expect("credential"),
            pending
        );

        let empty = repository
            .create_identity("Disposable", None)
            .expect("empty identity");
        assert!(matches!(
            repository.delete_identity(&empty.identity_id, WireSequence::new(2)),
            Err(AppPersistenceError::Conflict)
        ));
        let deleted = repository
            .delete_identity(&empty.identity_id, empty.state_version)
            .expect("delete identity");
        assert!(deleted.deleted);
        let replay = repository
            .delete_identity(&empty.identity_id, empty.state_version)
            .expect("idempotent replay");
        assert!(!replay.deleted);
        assert_eq!(replay.identity_id, empty.identity_id);
    }

    #[test]
    fn known_host_delete_is_cas_bounded_and_idempotent_without_replacement() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let key = b"known-host-delete-key";
        let trusted = repository
            .trust_known_host("delete.example", 22, "ssh-ed25519", key)
            .expect("trust known host");
        let verified = repository
            .record_known_host_verified("delete.example", 22, "ssh-ed25519", key)
            .expect("verify known host");
        assert!(matches!(
            repository.delete_known_host(&trusted.known_host_id, trusted.state_version),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository.list_known_hosts().expect("known hosts"),
            std::slice::from_ref(&verified)
        );

        let deleted = repository
            .delete_known_host(&verified.known_host_id, verified.state_version)
            .expect("delete known host");
        assert!(deleted.deleted);
        assert_eq!(deleted.deleted_known_host, Some(verified.clone()));
        assert!(
            repository
                .list_known_hosts()
                .expect("known hosts")
                .is_empty()
        );
        let replay = repository
            .delete_known_host(&verified.known_host_id, verified.state_version)
            .expect("idempotent replay");
        assert!(!replay.deleted);
        assert!(replay.deleted_known_host.is_none());
    }

    fn sample_certificate() -> SshCertificateMetadata {
        let subject_public_key_blob = b"subject-ed25519-public-key".to_vec();
        let certificate_blob = b"exact-openssh-user-certificate".to_vec();
        SshCertificateMetadata {
            source: AgentIdentitySource::SystemSshAgent,
            certificate_fingerprint: ssh_sha256_fingerprint(&certificate_blob),
            certificate_blob,
            certificate_algorithm: "ssh-ed25519-cert-v01@openssh.com".to_owned(),
            serial: "18446744073709551615".to_owned(),
            subject_public_key_fingerprint: ssh_sha256_fingerprint(&subject_public_key_blob),
            subject_public_key_blob,
            subject_public_key_algorithm: "ssh-ed25519".to_owned(),
            ca_public_key_fingerprint: "SHA256:trusted-ca-fingerprint".to_owned(),
            key_id: "deploy@example".to_owned(),
            valid_principals: vec!["deploy".to_owned(), "automation".to_owned()],
            certificate_type: SshCertificateType::User,
            valid_after_unix_seconds: 1_700_000_000,
            valid_before_unix_seconds: Some(1_800_000_000),
            critical_options: vec![SshCertificateCriticalOption {
                name: "future-policy".to_owned(),
                value: vec![0, 1],
                recognized: false,
            }],
            extensions: vec![
                SshCertificateExtension {
                    name: "no-touch-required".to_owned(),
                    value: Vec::new(),
                    recognized: true,
                },
                SshCertificateExtension {
                    name: "future-extension".to_owned(),
                    value: vec![2],
                    recognized: false,
                },
            ],
        }
    }

    fn create_v2_schema(connection: &Connection) {
        connection
            .execute_batch(
                "CREATE TABLE identities (
                   id TEXT PRIMARY KEY, label TEXT NOT NULL, username TEXT,
                   state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL
                 ) STRICT;
                 CREATE TABLE credential_refs (
                   id TEXT PRIMARY KEY,
                   identity_id TEXT NOT NULL REFERENCES identities(id) ON DELETE RESTRICT,
                   kind TEXT NOT NULL CHECK(kind IN ('password', 'private_key')),
                   secret_ref_id TEXT NOT NULL UNIQUE,
                   priority INTEGER NOT NULL CHECK(priority >= 0), label TEXT NOT NULL,
                   public_key_algorithm TEXT, public_key_fingerprint TEXT,
                   state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL,
                   passphrase_secret_ref_id TEXT,
                   import_operation_id TEXT,
                   import_idempotency_key TEXT,
                   import_state TEXT NOT NULL DEFAULT 'ready'
                     CHECK(import_state IN ('pending', 'ready')),
                   UNIQUE(identity_id, priority)
                 ) STRICT;
                 CREATE UNIQUE INDEX credential_refs_passphrase_secret_ref_unique
                   ON credential_refs(passphrase_secret_ref_id)
                   WHERE passphrase_secret_ref_id IS NOT NULL;
                 CREATE UNIQUE INDEX credential_refs_import_operation_unique
                   ON credential_refs(import_operation_id)
                   WHERE import_operation_id IS NOT NULL;
                 CREATE UNIQUE INDEX credential_refs_import_idempotency_unique
                   ON credential_refs(import_idempotency_key)
                   WHERE import_idempotency_key IS NOT NULL;
                 CREATE INDEX credential_refs_ready_identity_priority
                   ON credential_refs(identity_id, import_state, priority, id);
                 CREATE TABLE hosts (
                   id TEXT PRIMARY KEY, label TEXT NOT NULL, address TEXT NOT NULL,
                   normalized_address TEXT NOT NULL,
                   port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535), username TEXT,
                   identity_id TEXT REFERENCES identities(id) ON DELETE SET NULL,
                   favorite INTEGER NOT NULL CHECK(favorite IN (0, 1)),
                   state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL
                 ) STRICT;
                 CREATE INDEX hosts_normalized_endpoint ON hosts(normalized_address, port);
                 CREATE TABLE known_hosts (
                   id TEXT PRIMARY KEY, normalized_address TEXT NOT NULL,
                   port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
                   key_algorithm TEXT NOT NULL, public_key_base64 TEXT NOT NULL,
                   fingerprint_sha256 TEXT NOT NULL, first_trusted_at_ms INTEGER NOT NULL,
                   last_verified_at_ms INTEGER NOT NULL, state_version INTEGER NOT NULL,
                   UNIQUE(normalized_address, port, key_algorithm)
                 ) STRICT;
                 PRAGMA user_version = 2;",
            )
            .expect("create v2 schema");
    }

    #[test]
    fn credential_import_is_idempotent_and_pending_is_not_usable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Production", Some("deploy"))
            .expect("create identity");
        let operation_id = OperationId::new();
        let first = repository
            .begin_credential_import(
                &operation_id,
                "credential-import-once",
                &identity.identity_id,
                CredentialKind::PrivateKey,
                0,
                "Production key",
                true,
            )
            .expect("begin import");
        assert_eq!(first.import_state, CredentialImportState::Pending);
        assert!(matches!(
            first.details,
            CredentialRecordDetails::PrivateKey {
                passphrase_secret_ref_id: Some(_),
                ..
            }
        ));
        assert!(
            repository
                .list_credential_refs(&identity.identity_id)
                .expect("list public credentials")
                .is_empty()
        );
        assert!(matches!(
            repository.get_ready_credential_record(&first.credential_ref_id),
            Err(AppPersistenceError::NotFound)
        ));

        let replay = repository
            .begin_credential_import(
                &operation_id,
                "credential-import-once",
                &identity.identity_id,
                CredentialKind::PrivateKey,
                0,
                "Production key",
                true,
            )
            .expect("replay import");
        assert_eq!(replay, first);
        assert!(matches!(
            repository.begin_credential_import(
                &operation_id,
                "credential-import-once",
                &identity.identity_id,
                CredentialKind::PrivateKey,
                0,
                "Different label",
                true,
            ),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert!(matches!(
            repository.begin_credential_import(
                &OperationId::new(),
                "credential-import-once",
                &identity.identity_id,
                CredentialKind::PrivateKey,
                0,
                "Production key",
                true,
            ),
            Err(AppPersistenceError::IdempotencyConflict)
        ));

        let ready = repository
            .mark_credential_import_ready(
                &first.credential_ref_id,
                &operation_id,
                first.state_version,
                Some("ssh-ed25519"),
                Some("SHA256:public"),
            )
            .expect("mark ready");
        assert_eq!(ready.import_state, CredentialImportState::Ready);
        assert_eq!(ready.state_version, WireSequence::new(2));

        let reconciled = repository
            .mark_credential_import_ready(
                &first.credential_ref_id,
                &operation_id,
                first.state_version,
                Some("ssh-ed25519"),
                Some("SHA256:public"),
            )
            .expect("reconcile uncertain ready commit");
        assert_eq!(reconciled, ready);
        let summaries = repository
            .list_credential_refs(&identity.identity_id)
            .expect("list ready credentials");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].credential_ref_id, ready.credential_ref_id);
        assert_eq!(summaries[0].state_version, ready.state_version);
    }

    #[test]
    fn password_import_rejects_passphrase_and_public_key_metadata() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Password", None)
            .expect("create identity");
        assert!(matches!(
            repository.begin_credential_import(
                &OperationId::new(),
                "password-with-passphrase",
                &identity.identity_id,
                CredentialKind::Password,
                0,
                "Password",
                true,
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        let operation_id = OperationId::new();
        let pending = repository
            .begin_credential_import(
                &operation_id,
                "password-import",
                &identity.identity_id,
                CredentialKind::Password,
                0,
                "Password",
                false,
            )
            .expect("begin password import");
        assert!(matches!(
            repository.mark_credential_import_ready(
                &pending.credential_ref_id,
                &operation_id,
                pending.state_version,
                Some("ssh-ed25519"),
                Some("SHA256:invalid"),
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        repository
            .mark_credential_import_ready(
                &pending.credential_ref_id,
                &operation_id,
                pending.state_version,
                None,
                None,
            )
            .expect("mark password ready");
    }

    #[test]
    fn ssh_agent_credential_persists_exact_public_identity_without_secret_slots() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Agent identity", Some("deploy"))
            .expect("identity");
        let public_key_blob = b"bounded-fake-agent-public-key-blob";
        let fingerprint = ssh_sha256_fingerprint(public_key_blob);
        let operation_id = OperationId::new();
        let record = repository
            .create_ssh_agent_credential(
                &operation_id,
                "agent-credential-once",
                &identity.identity_id,
                0,
                "Agent key",
                public_key_blob,
                "ssh-ed25519",
                &fingerprint,
            )
            .expect("create agent credential");

        assert!(matches!(
            record.details,
            CredentialRecordDetails::SshAgent {
                ref public_key_blob,
                ref public_key_algorithm,
                ref public_key_fingerprint,
                scope: SshAgentScope::DefaultEnvironment,
            } if public_key_blob == b"bounded-fake-agent-public-key-blob"
                && public_key_algorithm == "ssh-ed25519"
                && public_key_fingerprint == &fingerprint
        ));
        let secret_slots: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM credential_secret_slots WHERE credential_ref_id = ?1",
                [record.credential_ref_id.as_str()],
                |row| row.get(0),
            )
            .expect("count secret slots");
        assert_eq!(secret_slots, 0);
        assert!(
            repository
                .connection
                .execute(
                    "INSERT INTO credential_secret_slots
                     (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
                     VALUES (?1, 0, 'password', ?2, 'invalid')",
                    params![
                        record.credential_ref_id.as_str(),
                        SecretRefId::new().as_str()
                    ],
                )
                .is_err(),
            "an Agent reference must never accept a secret slot"
        );

        let replay = repository
            .create_ssh_agent_credential(
                &operation_id,
                "agent-credential-once",
                &identity.identity_id,
                0,
                "Agent key",
                public_key_blob,
                "ssh-ed25519",
                &fingerprint,
            )
            .expect("replay agent credential");
        assert_eq!(replay.credential_ref_id, record.credential_ref_id);
    }

    #[test]
    fn agent_certificate_roundtrips_exact_public_metadata_and_unknown_critical_option() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Certificate identity", Some("deploy"))
            .expect("identity");
        let certificate = sample_certificate();
        let operation_id = OperationId::new();
        let record = repository
            .create_agent_identity_credential(
                &operation_id,
                "agent-certificate-once",
                &identity.identity_id,
                0,
                "System Agent certificate",
                AgentIdentityKind::Certificate,
                &certificate.subject_public_key_blob,
                &certificate.subject_public_key_algorithm,
                &certificate.subject_public_key_fingerprint,
                None,
                Some(&certificate),
            )
            .expect("create certificate credential");

        assert_eq!(record.method, AuthenticationMethodKind::Certificate);
        assert!(matches!(
            &record.details,
            CredentialRecordDetails::Certificate {
                certificate: stored,
                scope: SshAgentScope::DefaultEnvironment,
            } if stored.as_ref() == &certificate && stored.has_unknown_critical_options()
        ));
        let summary = repository
            .list_credential_refs(&identity.identity_id)
            .expect("list certificate")
            .pop()
            .expect("certificate summary");
        assert_eq!(summary.method, AuthenticationMethodKind::Certificate);
        assert!(matches!(
            summary.details,
            CredentialRefDetails::Certificate { certificate: stored, .. }
                if stored.as_ref() == &certificate && stored.has_unknown_critical_options()
        ));
        let (database_kind, identity_kind, secret_slots): (String, String, i64) = repository
            .connection
            .query_row(
                "SELECT credential_refs.kind, credential_ssh_agent_details.identity_kind,
                        (SELECT COUNT(*) FROM credential_secret_slots
                         WHERE credential_ref_id = credential_refs.id)
                 FROM credential_refs
                 JOIN credential_ssh_agent_details
                   ON credential_ssh_agent_details.credential_ref_id = credential_refs.id
                 WHERE credential_refs.id = ?1",
                [record.credential_ref_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read storage classification");
        assert_eq!(database_kind, "ssh_agent");
        assert_eq!(identity_kind, "certificate");
        assert_eq!(secret_slots, 0);

        let replay = repository
            .create_agent_identity_credential(
                &operation_id,
                "agent-certificate-once",
                &identity.identity_id,
                0,
                "System Agent certificate",
                AgentIdentityKind::Certificate,
                &certificate.subject_public_key_blob,
                &certificate.subject_public_key_algorithm,
                &certificate.subject_public_key_fingerprint,
                None,
                Some(&certificate),
            )
            .expect("replay exact certificate");
        assert_eq!(replay.credential_ref_id, record.credential_ref_id);
        let mut changed = certificate.clone();
        changed.key_id = "different".to_owned();
        assert!(matches!(
            repository.create_agent_identity_credential(
                &operation_id,
                "agent-certificate-once",
                &identity.identity_id,
                0,
                "System Agent certificate",
                AgentIdentityKind::Certificate,
                &changed.subject_public_key_blob,
                &changed.subject_public_key_algorithm,
                &changed.subject_public_key_fingerprint,
                None,
                Some(&changed),
            ),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
    }

    #[test]
    fn agent_hardware_key_requires_exact_supported_algorithm_and_ssh_application() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Hardware identity", Some("deploy"))
            .expect("identity");
        let public_key_blob = b"exact-sk-ed25519-public-key";
        let fingerprint = ssh_sha256_fingerprint(public_key_blob);
        let operation_id = OperationId::new();
        let record = repository
            .create_agent_identity_credential(
                &operation_id,
                "hardware-key-once",
                &identity.identity_id,
                0,
                "Security key",
                AgentIdentityKind::HardwareKey,
                public_key_blob,
                "sk-ssh-ed25519@openssh.com",
                &fingerprint,
                Some("ssh:norishell"),
                None,
            )
            .expect("create hardware key");
        assert_eq!(record.method, AuthenticationMethodKind::HardwareKey);
        assert!(matches!(
            record.details,
            CredentialRecordDetails::HardwareKey {
                application,
                scope: SshAgentScope::DefaultEnvironment,
                ..
            } if application == "ssh:norishell"
        ));
        assert!(matches!(
            repository.create_agent_identity_credential(
                &OperationId::new(),
                "unsupported-hardware-key",
                &identity.identity_id,
                1,
                "Unsupported key",
                AgentIdentityKind::HardwareKey,
                public_key_blob,
                "sk-rsa-sha2-512@openssh.com",
                &fingerprint,
                Some("ssh:norishell"),
                None,
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            repository.create_agent_identity_credential(
                &OperationId::new(),
                "foreign-hardware-application",
                &identity.identity_id,
                1,
                "Foreign application",
                AgentIdentityKind::HardwareKey,
                public_key_blob,
                "sk-ssh-ed25519@openssh.com",
                &fingerprint,
                Some("example.com"),
                None,
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn certificate_known_options_require_canonical_values() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Certificate validation", None)
            .expect("identity");
        let mut certificate = sample_certificate();
        certificate.critical_options = vec![SshCertificateCriticalOption {
            name: "verify-required".to_owned(),
            value: vec![1],
            recognized: true,
        }];
        assert!(matches!(
            repository.create_agent_identity_credential(
                &OperationId::new(),
                "malformed-verify-required",
                &identity.identity_id,
                0,
                "Malformed certificate",
                AgentIdentityKind::Certificate,
                &certificate.subject_public_key_blob,
                &certificate.subject_public_key_algorithm,
                &certificate.subject_public_key_fingerprint,
                None,
                Some(&certificate),
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        certificate.critical_options[0].value.clear();
        certificate.extensions[0].value.push(1);
        assert!(matches!(
            repository.create_agent_identity_credential(
                &OperationId::new(),
                "malformed-no-touch",
                &identity.identity_id,
                0,
                "Malformed extension",
                AgentIdentityKind::Certificate,
                &certificate.subject_public_key_blob,
                &certificate.subject_public_key_algorithm,
                &certificate.subject_public_key_fingerprint,
                None,
                Some(&certificate),
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn keyboard_interactive_credential_round_limit_is_persisted_and_idempotent_without_secrets() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Interactive identity", Some("deploy"))
            .expect("identity");
        let operation_id = OperationId::new();
        let record = repository
            .create_keyboard_interactive_credential(
                &operation_id,
                "keyboard-interactive-once",
                &identity.identity_id,
                7,
                "Server challenge",
                12,
            )
            .expect("create keyboard-interactive credential");
        assert!(matches!(
            record.details,
            CredentialRecordDetails::KeyboardInteractive { max_rounds: 12 }
        ));
        let replay = repository
            .create_keyboard_interactive_credential(
                &operation_id,
                "keyboard-interactive-once",
                &identity.identity_id,
                7,
                "Server challenge",
                12,
            )
            .expect("replay keyboard-interactive credential");
        assert_eq!(replay, record);
        let summary = repository
            .list_credential_refs(&identity.identity_id)
            .expect("list credential")
            .into_iter()
            .next()
            .expect("summary");
        assert!(matches!(
            summary.details,
            norishell_core_api::CredentialRefDetails::KeyboardInteractive { max_rounds: 12 }
        ));
        let secret_slots: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM credential_secret_slots WHERE credential_ref_id = ?1",
                [record.credential_ref_id.as_str()],
                |row| row.get(0),
            )
            .expect("count secret slots");
        assert_eq!(secret_slots, 0);
        assert!(matches!(
            repository.create_keyboard_interactive_credential(
                &operation_id,
                "keyboard-interactive-once",
                &identity.identity_id,
                7,
                "Server challenge",
                13,
            ),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert!(matches!(
            repository.create_keyboard_interactive_credential(
                &OperationId::new(),
                "keyboard-interactive-invalid",
                &identity.identity_id,
                8,
                "Invalid",
                0,
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn host_identity_and_ready_credential_records_are_readable_by_id() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Production", Some("deploy"))
            .expect("create identity");
        let operation_id = OperationId::new();
        let pending = repository
            .begin_credential_import(
                &operation_id,
                "host-credential",
                &identity.identity_id,
                CredentialKind::Password,
                0,
                "Password",
                false,
            )
            .expect("begin credential");
        let ready = repository
            .mark_credential_import_ready(
                &pending.credential_ref_id,
                &operation_id,
                pending.state_version,
                None,
                None,
            )
            .expect("mark ready");
        let host = repository
            .create_host(
                "API",
                "EXAMPLE.COM.",
                22,
                None,
                Some(&identity.identity_id),
                true,
            )
            .expect("create host");

        assert_eq!(repository.get_host(&host.host_id).expect("get host"), host);
        assert_eq!(
            repository
                .get_identity(&identity.identity_id)
                .expect("get identity"),
            identity
        );
        assert_eq!(
            repository
                .get_ready_credential_record(&ready.credential_ref_id)
                .expect("get ready credential"),
            ready
        );
        assert_eq!(
            repository
                .list_ready_credential_records_for_host(&host.host_id)
                .expect("resolve host credentials"),
            vec![ready]
        );
    }

    #[test]
    fn migrates_v1_credentials_to_ready_without_changing_secret_references() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("legacy.sqlite3");
        let identity_id = IdentityId::new();
        let credential_ref_id = norishell_core_api::CredentialRefId::new();
        let secret_ref_id = SecretRefId::new();
        let connection = Connection::open(&database_path).expect("open v1 database");
        connection
            .execute_batch(
                "CREATE TABLE identities (
                   id TEXT PRIMARY KEY, label TEXT NOT NULL, username TEXT,
                   state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL
                 ) STRICT;
                 CREATE TABLE credential_refs (
                   id TEXT PRIMARY KEY,
                   identity_id TEXT NOT NULL REFERENCES identities(id) ON DELETE RESTRICT,
                   kind TEXT NOT NULL CHECK(kind IN ('password', 'private_key')),
                   secret_ref_id TEXT NOT NULL UNIQUE,
                   priority INTEGER NOT NULL CHECK(priority >= 0), label TEXT NOT NULL,
                   public_key_algorithm TEXT, public_key_fingerprint TEXT,
                   state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL,
                   UNIQUE(identity_id, priority)
                 ) STRICT;
                 CREATE TABLE hosts (
                   id TEXT PRIMARY KEY, label TEXT NOT NULL, address TEXT NOT NULL,
                   normalized_address TEXT NOT NULL,
                   port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535), username TEXT,
                   identity_id TEXT REFERENCES identities(id) ON DELETE SET NULL,
                   favorite INTEGER NOT NULL CHECK(favorite IN (0, 1)),
                   state_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                   updated_at_ms INTEGER NOT NULL
                 ) STRICT;
                 CREATE INDEX hosts_normalized_endpoint ON hosts(normalized_address, port);
                 CREATE TABLE known_hosts (
                   id TEXT PRIMARY KEY, normalized_address TEXT NOT NULL,
                   port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
                   key_algorithm TEXT NOT NULL, public_key_base64 TEXT NOT NULL,
                   fingerprint_sha256 TEXT NOT NULL, first_trusted_at_ms INTEGER NOT NULL,
                   last_verified_at_ms INTEGER NOT NULL, state_version INTEGER NOT NULL,
                   UNIQUE(normalized_address, port, key_algorithm)
                 ) STRICT;
                 PRAGMA user_version = 1;",
            )
            .expect("create v1 schema");
        connection
            .execute(
                "INSERT INTO identities
                 (id, label, username, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, 'Legacy', NULL, 1, 1, 1)",
                [identity_id.as_str()],
            )
            .expect("insert identity");
        connection
            .execute(
                "INSERT INTO credential_refs
                 (id, identity_id, kind, secret_ref_id, priority, label, public_key_algorithm,
                  public_key_fingerprint, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, 'password', ?3, 0, 'Legacy password', NULL, NULL, 1, 1, 1)",
                params![
                    credential_ref_id.as_str(),
                    identity_id.as_str(),
                    secret_ref_id.as_str()
                ],
            )
            .expect("insert v1 credential");
        drop(connection);

        let repository = AppRepository::open(&database_path).expect("migrate repository");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read schema version");
        assert_eq!(version, super::SCHEMA_VERSION);
        let record = repository
            .get_ready_credential_record(&credential_ref_id)
            .expect("read migrated credential");
        assert!(matches!(
            record.details,
            CredentialRecordDetails::Password {
                secret_ref_id: ref stored_secret_ref_id
            } if *stored_secret_ref_id == secret_ref_id
        ));
        assert_eq!(record.import_operation_id, None);
        assert_eq!(record.import_idempotency_key, None);
        assert_eq!(record.import_state, CredentialImportState::Ready);
        let summaries = repository
            .list_credential_refs(&identity_id)
            .expect("list migrated credential");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].credential_ref_id, record.credential_ref_id);
    }

    #[test]
    fn migrates_real_v2_pending_and_ready_credentials_without_rebinding_references() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("v2.sqlite3");
        let identity_id = IdentityId::new();
        let host_id = HostId::new();
        let pending_id = CredentialRefId::new();
        let pending_secret_ref = SecretRefId::new();
        let passphrase_secret_ref = SecretRefId::new();
        let operation_id = OperationId::new();
        let ready_id = CredentialRefId::new();
        let ready_secret_ref = SecretRefId::new();
        let connection = Connection::open(&database_path).expect("open v2 database");
        create_v2_schema(&connection);
        connection
            .execute(
                "INSERT INTO identities
                 (id, label, username, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, 'Migrated', 'deploy', 3, 1, 2)",
                [identity_id.as_str()],
            )
            .expect("insert identity");
        connection
            .execute(
                "INSERT INTO hosts
                 (id, label, address, normalized_address, port, username, identity_id,
                  favorite, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, 'Legacy host', 'example.com', 'example.com', 22, 'deploy', ?2,
                         1, 5, 1, 2)",
                params![host_id.as_str(), identity_id.as_str()],
            )
            .expect("insert host");
        connection
            .execute(
                "INSERT INTO credential_refs
                 (id, identity_id, kind, secret_ref_id, priority, label,
                  public_key_algorithm, public_key_fingerprint, state_version,
                  created_at_ms, updated_at_ms, passphrase_secret_ref_id,
                  import_operation_id, import_idempotency_key, import_state)
                 VALUES (?1, ?2, 'private_key', ?3, 0, 'Pending key', NULL, NULL, 7,
                         1, 2, ?4, ?5, 'pending-key-once', 'pending')",
                params![
                    pending_id.as_str(),
                    identity_id.as_str(),
                    pending_secret_ref.as_str(),
                    passphrase_secret_ref.as_str(),
                    operation_id.as_str(),
                ],
            )
            .expect("insert pending key");
        connection
            .execute(
                "INSERT INTO credential_refs
                 (id, identity_id, kind, secret_ref_id, priority, label,
                  public_key_algorithm, public_key_fingerprint, state_version,
                  created_at_ms, updated_at_ms, import_state)
                 VALUES (?1, ?2, 'password', ?3, 1, 'Ready password', NULL, NULL, 4,
                         1, 2, 'ready')",
                params![
                    ready_id.as_str(),
                    identity_id.as_str(),
                    ready_secret_ref.as_str(),
                ],
            )
            .expect("insert ready password");
        drop(connection);

        let repository = AppRepository::open(&database_path).expect("migrate v2 repository");
        let pending = repository
            .get_credential_import_record(&pending_id)
            .expect("read pending key");
        assert!(matches!(
            pending.details,
            CredentialRecordDetails::PrivateKey {
                secret_ref_id: ref stored_secret_ref_id,
                passphrase_secret_ref_id: Some(ref stored_passphrase_secret_ref_id),
                ..
            } if *stored_secret_ref_id == pending_secret_ref
                && *stored_passphrase_secret_ref_id == passphrase_secret_ref
        ));
        assert_eq!(pending.import_operation_id, Some(operation_id));
        assert_eq!(
            pending.import_idempotency_key.as_deref(),
            Some("pending-key-once")
        );
        assert_eq!(pending.import_state, CredentialImportState::Pending);
        assert_eq!(pending.state_version, WireSequence::new(7));
        assert_eq!(pending.priority, 0);
        let ready = repository
            .get_ready_credential_record(&ready_id)
            .expect("read ready password");
        assert!(matches!(
            ready.details,
            CredentialRecordDetails::Password {
                secret_ref_id: ref stored_secret_ref_id
            } if *stored_secret_ref_id == ready_secret_ref
        ));
        assert_eq!(ready.import_state, CredentialImportState::Ready);
        assert_eq!(ready.state_version, WireSequence::new(4));
        assert_eq!(ready.priority, 1);
        assert!(matches!(
            repository.get_ready_credential_record(&pending_id),
            Err(AppPersistenceError::NotFound)
        ));
        let config = repository
            .get_host_connection_config(&host_id)
            .expect("read migrated defaults");
        assert!(matches!(config.route_plan.ingress, RouteIngress::DirectTcp));
        assert!(matches!(
            config.heartbeat_policy.policy,
            HeartbeatPolicy::Disabled
        ));
        assert!(!config.monitoring_policy.policy.enabled);
        assert_eq!(config.monitoring_policy.policy.sample_interval_seconds, 15);
        assert_eq!(config.monitoring_policy.policy.sample_timeout_seconds, 5);
        let violations: i64 = repository
            .connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("foreign key check");
        assert_eq!(violations, 0);
        let base_columns = repository
            .connection
            .prepare("SELECT name FROM pragma_table_info('credential_refs')")
            .expect("prepare columns")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query columns")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("collect columns");
        assert!(!base_columns.iter().any(|name| name.contains("secret_ref")));
    }

    #[test]
    fn v8_migration_classifies_existing_agent_rows_as_ordinary_without_guessing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("v7-agent.sqlite3");
        let identity_id = IdentityId::new();
        let credential_ref_id = CredentialRefId::new();
        let public_key_blob = b"legacy-exact-agent-public-key";
        let fingerprint = ssh_sha256_fingerprint(public_key_blob);
        let connection = Connection::open(&database_path).expect("open v2 fixture");
        create_v2_schema(&connection);
        connection
            .execute(
                "INSERT INTO identities
                 (id, label, username, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, 'Legacy Agent', 'deploy', 1, 1, 1)",
                [identity_id.as_str()],
            )
            .expect("insert identity");
        migrate_v2_to_v3(&connection).expect("migrate v3");
        migrate_v3_to_v4(&connection).expect("migrate v4");
        migrate_v4_to_v5(&connection).expect("migrate v5");
        connection
            .execute(
                "INSERT INTO credential_refs
                 (id, identity_id, kind, priority, label, state_version, created_at_ms,
                  updated_at_ms, import_state)
                 VALUES (?1, ?2, 'ssh_agent', 0, 'Legacy key', 1, 1, 1, 'ready')",
                params![credential_ref_id.as_str(), identity_id.as_str()],
            )
            .expect("insert v5 credential");
        connection
            .execute(
                "INSERT INTO credential_ssh_agent_details
                 (credential_ref_id, agent_scope, public_key_blob, public_key_algorithm,
                  public_key_fingerprint)
                 VALUES (?1, 'default_environment', ?2, 'ssh-ed25519', ?3)",
                params![credential_ref_id.as_str(), public_key_blob, fingerprint],
            )
            .expect("insert v5 Agent detail");
        migrate_v5_to_v6(&connection).expect("migrate v6");
        migrate_v6_to_v7(&connection).expect("migrate v7");
        drop(connection);

        let repository = AppRepository::open(&database_path).expect("migrate v8");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read version");
        assert_eq!(version, super::SCHEMA_VERSION);
        let identity_kind: String = repository
            .connection
            .query_row(
                "SELECT identity_kind FROM credential_ssh_agent_details
                 WHERE credential_ref_id = ?1",
                [credential_ref_id.as_str()],
                |row| row.get(0),
            )
            .expect("read migrated identity kind");
        assert_eq!(identity_kind, "ordinary");
        let record = repository
            .get_ready_credential_record(&credential_ref_id)
            .expect("read migrated Agent");
        assert_eq!(record.method, AuthenticationMethodKind::SshAgent);
        assert!(matches!(
            record.details,
            CredentialRecordDetails::SshAgent { .. }
        ));
    }

    #[test]
    fn failed_v3_foreign_key_check_rolls_the_structural_migration_back() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("broken-v2.sqlite3");
        let connection = Connection::open(&database_path).expect("open v2 database");
        create_v2_schema(&connection);
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("disable fixture foreign keys");
        let orphan_identity = IdentityId::new();
        connection
            .execute(
                "INSERT INTO credential_refs
                 (id, identity_id, kind, secret_ref_id, priority, label, state_version,
                  created_at_ms, updated_at_ms, import_state)
                 VALUES (?1, ?2, 'password', ?3, 0, 'Orphan', 1, 1, 1, 'ready')",
                params![
                    CredentialRefId::new().as_str(),
                    orphan_identity.as_str(),
                    SecretRefId::new().as_str(),
                ],
            )
            .expect("insert orphan with foreign keys disabled");
        drop(connection);

        assert!(matches!(
            AppRepository::open(&database_path),
            Err(AppPersistenceError::InvalidStoredData)
        ));
        let connection = Connection::open(&database_path).expect("reopen rolled back database");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read rolled back version");
        assert_eq!(version, 2);
        let legacy_secret_column: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('credential_refs') WHERE name = 'secret_ref_id'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("legacy table restored");
        assert!(legacy_secret_column);
    }

    #[test]
    fn host_connection_policies_are_independently_revisioned_and_bounded() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("create identity");
        let operation_id = OperationId::new();
        let pending = repository
            .begin_credential_import(
                &operation_id,
                "policy-password",
                &identity.identity_id,
                CredentialKind::Password,
                0,
                "Proxy password",
                false,
            )
            .expect("begin password");
        let ready = repository
            .mark_credential_import_ready(
                &pending.credential_ref_id,
                &operation_id,
                pending.state_version,
                None,
                None,
            )
            .expect("ready password");
        let target = repository
            .create_host(
                "Target",
                "target.example",
                22,
                Some("deploy"),
                Some(&identity.identity_id),
                false,
            )
            .expect("create target");
        let jump = repository
            .create_host(
                "Jump",
                "jump.example",
                22,
                Some("deploy"),
                Some(&identity.identity_id),
                false,
            )
            .expect("create jump");

        let initial = repository
            .get_host_connection_config(&target.host_id)
            .expect("initial config");
        assert_eq!(initial.route_plan.revision, WireSequence::new(1));
        assert_eq!(initial.authentication_plan.revision, WireSequence::new(1));
        assert_eq!(
            initial.algorithm_policy.policy_id,
            DEFAULT_ALGORITHM_POLICY_ID
        );
        let route = repository
            .replace_route_plan(
                &target.host_id,
                initial.route_plan.revision,
                &RouteIngress::Socks5Proxy {
                    endpoint: ProxyEndpoint {
                        address: "Proxy.Example.".to_owned(),
                        normalized_address: "ignored-by-persistence".to_owned(),
                        port: 1080,
                    },
                    dns_mode: ProxyDnsMode::Proxy,
                    proxy_auth_credential_ref_id: Some(ready.credential_ref_id.clone()),
                },
                std::slice::from_ref(&jump.host_id),
            )
            .expect("replace route");
        assert_eq!(route.revision, WireSequence::new(2));
        assert_eq!(route.jump_host_ids, vec![jump.host_id.clone()]);
        match route.ingress {
            RouteIngress::Socks5Proxy { endpoint, .. } => {
                assert_eq!(endpoint.normalized_address, "proxy.example");
            }
            _ => panic!("expected SOCKS5 route"),
        }
        let jump_route = repository
            .get_route_plan(&jump.host_id)
            .expect("jump route");
        assert!(matches!(
            repository.replace_route_plan(
                &jump.host_id,
                jump_route.revision,
                &RouteIngress::DirectTcp,
                std::slice::from_ref(&target.host_id),
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        let authentication = repository
            .replace_authentication_plan(
                &target.host_id,
                initial.authentication_plan.revision,
                AuthenticationPlanMode::HostOverride,
                std::slice::from_ref(&ready.credential_ref_id),
            )
            .expect("replace authentication plan");
        assert_eq!(authentication.revision, WireSequence::new(2));
        assert_eq!(
            authentication.credential_ref_ids,
            vec![ready.credential_ref_id]
        );
        let other_identity = repository
            .create_identity("Other identity", Some("other"))
            .expect("create other identity");
        for next_identity_id in [Some(&other_identity.identity_id), None] {
            assert!(matches!(
                repository.update_host(
                    &target.host_id,
                    target.state_version,
                    &target.label,
                    &target.address,
                    target.port,
                    target.username.as_deref(),
                    next_identity_id,
                    target.favorite,
                ),
                Err(AppPersistenceError::InvalidInput(_))
            ));
        }
        assert_eq!(
            repository
                .get_host(&target.host_id)
                .expect("read unchanged target")
                .identity_id,
            Some(identity.identity_id.clone())
        );

        let algorithm = repository
            .replace_algorithm_policy(
                &target.host_id,
                initial.algorithm_policy.revision,
                "secure-default-v2",
                &[AlgorithmCompatibilityException {
                    category: AlgorithmCategory::HostKey,
                    exception_id: "legacy-host-key-1".to_owned(),
                    reason: Some("Controlled exception".to_owned()),
                }],
            )
            .expect("replace algorithm policy");
        assert_eq!(algorithm.revision, WireSequence::new(2));
        assert!(matches!(
            repository.replace_algorithm_policy(
                &target.host_id,
                algorithm.revision,
                "arbitrary ssh-rsa list",
                &[],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        let heartbeat = repository
            .replace_heartbeat_policy(
                &target.host_id,
                initial.heartbeat_policy.revision,
                &HeartbeatPolicy::TransportKeepalive {
                    interval_seconds: 30,
                    reply_timeout_seconds: 10,
                    failure_threshold: 3,
                },
            )
            .expect("replace heartbeat");
        assert_eq!(heartbeat.revision, WireSequence::new(2));
        assert!(matches!(
            repository.replace_heartbeat_policy(
                &target.host_id,
                heartbeat.revision,
                &HeartbeatPolicy::ShellHeartbeat {
                    payload_text: "\u{1b}".to_owned(),
                    line_ending: ShellHeartbeatLineEnding::Cr,
                    interval_seconds: 30,
                    user_idle_seconds: 5,
                },
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            repository.replace_heartbeat_policy(
                &target.host_id,
                heartbeat.revision,
                &HeartbeatPolicy::ShellHeartbeat {
                    payload_text: "TOKEN=ghp_123456789012345678901234".to_owned(),
                    line_ending: ShellHeartbeatLineEnding::Cr,
                    interval_seconds: 30,
                    user_idle_seconds: 5,
                },
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        let monitoring = repository
            .replace_monitoring_policy(
                &target.host_id,
                initial.monitoring_policy.revision,
                &MonitoringPolicy {
                    enabled: true,
                    sample_interval_seconds: 30,
                    sample_timeout_seconds: 10,
                    disk_mount_ids: vec![DiskResourceId::Root],
                    network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
                },
            )
            .expect("replace monitoring");
        assert_eq!(monitoring.revision, WireSequence::new(2));
        assert_eq!(monitoring.policy.disk_mount_ids, [DiskResourceId::Root]);
        assert!(matches!(
            repository.replace_monitoring_policy(
                &target.host_id,
                monitoring.revision,
                &MonitoringPolicy {
                    enabled: true,
                    sample_interval_seconds: 30,
                    sample_timeout_seconds: 10,
                    disk_mount_ids: vec![DiskResourceId::Root, DiskResourceId::Root],
                    network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
                },
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        for invalid_policy in [
            MonitoringPolicy {
                enabled: true,
                sample_interval_seconds: 4,
                sample_timeout_seconds: 2,
                disk_mount_ids: vec![DiskResourceId::Root],
                network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
            },
            MonitoringPolicy {
                enabled: true,
                sample_interval_seconds: 301,
                sample_timeout_seconds: 2,
                disk_mount_ids: vec![DiskResourceId::Root],
                network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
            },
            MonitoringPolicy {
                enabled: true,
                sample_interval_seconds: 15,
                sample_timeout_seconds: 1,
                disk_mount_ids: vec![DiskResourceId::Root],
                network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
            },
            MonitoringPolicy {
                enabled: true,
                sample_interval_seconds: 15,
                sample_timeout_seconds: 31,
                disk_mount_ids: vec![DiskResourceId::Root],
                network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
            },
        ] {
            assert!(matches!(
                repository.replace_monitoring_policy(
                    &target.host_id,
                    monitoring.revision,
                    &invalid_policy,
                ),
                Err(AppPersistenceError::InvalidInput(_))
            ));
        }

        let automation_secret = repository
            .begin_login_automation_secret_stage(
                &target.host_id,
                initial.login_automation.revision,
                &OperationId::new(),
                "automation-secret-once",
                "Escalation password",
            )
            .expect("begin secret stage");
        let automation_secret = repository
            .mark_login_automation_secret_staged(&automation_secret.staged_secret_id)
            .expect("mark secret staged");
        let automation = repository
            .replace_login_automation(
                &target.host_id,
                initial.login_automation.revision,
                true,
                &[
                    LoginAutomationStepInput::Expect {
                        literal_text: "Password:".to_owned(),
                        timeout_seconds: 10,
                    },
                    LoginAutomationStepInput::SendSecret {
                        secret_ref_id: automation_secret.staged_secret_id.clone(),
                        secret_label: "Escalation password".to_owned(),
                        append_enter: true,
                        timeout_seconds: 10,
                    },
                ],
            )
            .expect("replace automation");
        assert_eq!(automation.revision, WireSequence::new(2));
        assert_eq!(automation.confirmed_revision, None);
        let rendered = format!("{automation:?}");
        assert!(!rendered.contains(automation_secret.secret_ref_id.as_str()));
        let stored_secret_ref: String = repository
            .connection
            .query_row(
                "SELECT secret_ref_id FROM host_login_automation_steps
                 WHERE host_id = ?1 AND kind = 'send_secret'",
                [target.host_id.as_str()],
                |row| row.get(0),
            )
            .expect("read internal secret binding");
        assert_eq!(stored_secret_ref, automation_secret.secret_ref_id.as_str());
        let confirmed = repository
            .confirm_login_automation(&target.host_id, automation.revision)
            .expect("confirm exact automation revision");
        assert_eq!(confirmed.confirmed_revision, Some(automation.revision));
        assert!(matches!(
            repository.confirm_login_automation(&target.host_id, WireSequence::new(1)),
            Err(AppPersistenceError::Conflict)
        ));
        let replaced = repository
            .replace_login_automation(
                &target.host_id,
                automation.revision,
                true,
                &[
                    LoginAutomationStepInput::Expect {
                        literal_text: "Password:".to_owned(),
                        timeout_seconds: 10,
                    },
                    LoginAutomationStepInput::PreserveExistingSecret {
                        existing_ordinal: 1,
                        append_enter: true,
                        timeout_seconds: 10,
                    },
                ],
            )
            .expect("replace while preserving hidden secret");
        assert_eq!(replaced.revision, WireSequence::new(3));
        assert_eq!(replaced.confirmed_revision, None);
        let preserved_secret_ref: String = repository
            .connection
            .query_row(
                "SELECT secret_ref_id FROM host_login_automation_steps
                 WHERE host_id = ?1 AND ordinal = 1 AND kind = 'send_secret'",
                [target.host_id.as_str()],
                |row| row.get(0),
            )
            .expect("read preserved secret binding");
        assert_eq!(
            preserved_secret_ref,
            automation_secret.secret_ref_id.as_str()
        );

        let current = repository
            .get_host_connection_config(&target.host_id)
            .expect("current config");
        assert_eq!(current.route_plan.revision, WireSequence::new(2));
        assert_eq!(current.authentication_plan.revision, WireSequence::new(2));
        assert_eq!(current.algorithm_policy.revision, WireSequence::new(2));
        assert_eq!(current.heartbeat_policy.revision, WireSequence::new(2));
        assert_eq!(current.monitoring_policy.revision, WireSequence::new(2));
        assert_eq!(current.login_automation.revision, WireSequence::new(3));
    }

    #[test]
    fn unknown_host_observation_is_read_only_and_explicit_trust_is_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let observed = repository
            .observe_known_host("Example.com", 22, "ssh-ed25519", b"key-a")
            .expect("observe first");
        assert!(matches!(observed, KnownHostObservation::Unknown(_)));
        assert!(
            repository
                .list_known_hosts()
                .expect("list before trust")
                .is_empty()
        );

        let first = repository
            .trust_known_host("Example.com", 22, "ssh-ed25519", b"key-a")
            .expect("trust first");
        let second = repository
            .trust_known_host("example.com.", 22, "ssh-ed25519", b"key-a")
            .expect("trust same key again");
        assert_eq!(first, second);

        let verified = repository
            .record_known_host_verified("example.com", 22, "ssh-ed25519", b"key-a")
            .expect("record verification");
        assert_eq!(verified.state_version, WireSequence::new(2));
        assert!(matches!(
            repository
                .observe_known_host("example.com", 22, "ssh-ed25519", b"key-a")
                .expect("observe trusted"),
            KnownHostObservation::Trusted(_)
        ));

        let error = repository
            .trust_known_host("example.com", 22, "ssh-ed25519", b"key-b")
            .expect_err("mismatch must fail");
        assert!(matches!(
            error,
            AppPersistenceError::KnownHostMismatch { .. }
        ));
        let trusted = repository.list_known_hosts().expect("list known hosts");
        assert_eq!(trusted.len(), 1);
        assert_eq!(trusted[0].public_key_base64, BASE64.encode(b"key-a"));
    }

    #[test]
    fn changed_host_key_is_reported_without_mutating_trust() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        repository
            .trust_known_host("example.com", 22, "ssh-ed25519", b"key-a")
            .expect("trust first key");

        let observation = repository
            .observe_known_host("example.com", 22, "ssh-ed25519", b"key-b")
            .expect("observe changed key");
        let KnownHostObservation::Mismatch { trusted, observed } = observation else {
            panic!("changed host key must be a mismatch");
        };
        assert_eq!(trusted.public_key_base64, BASE64.encode(b"key-a"));
        assert_eq!(observed.public_key_base64, BASE64.encode(b"key-b"));

        let stored = repository.list_known_hosts().expect("list known hosts");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].public_key_base64, BASE64.encode(b"key-a"));
    }

    #[test]
    fn changed_host_key_algorithm_is_blocked_for_an_already_pinned_endpoint() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        repository
            .trust_known_host("example.com", 22, "ssh-ed25519", b"key-a")
            .expect("trust first key");

        let observation = repository
            .observe_known_host("example.com", 22, "rsa-sha2-512", b"key-b")
            .expect("observe changed algorithm");
        assert!(matches!(
            observation,
            KnownHostObservation::AlgorithmChanged { .. }
        ));
        assert!(matches!(
            repository.trust_known_host("example.com", 22, "rsa-sha2-512", b"key-b"),
            Err(AppPersistenceError::KnownHostAlgorithmChanged { .. })
        ));
        assert!(matches!(
            repository.record_known_host_verified("example.com", 22, "rsa-sha2-512", b"key-b"),
            Err(AppPersistenceError::KnownHostAlgorithmChanged { .. })
        ));

        let stored = repository.list_known_hosts().expect("list known hosts");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key_algorithm, "ssh-ed25519");
    }

    #[test]
    fn concurrent_algorithm_pins_allow_exactly_one_endpoint_algorithm() {
        let directory = tempfile::tempdir().expect("tempdir");
        drop(repository(&directory));
        let first_repository = repository(&directory);
        let second_repository = repository(&directory);
        let barrier = Arc::new(Barrier::new(2));

        let attempts = [
            (first_repository, "ssh-ed25519", b"key-a".as_slice()),
            (second_repository, "rsa-sha2-512", b"key-b".as_slice()),
        ]
        .map(|(mut repository, algorithm, key)| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                match repository.trust_known_host("example.com", 22, algorithm, key) {
                    Ok(_) => "trusted",
                    Err(AppPersistenceError::KnownHostAlgorithmChanged { .. }) => {
                        "algorithm-changed"
                    }
                    Err(error) => panic!("unexpected trust result: {error}"),
                }
            })
        });
        let outcomes = attempts.map(|attempt| attempt.join().expect("join trust attempt"));
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == "trusted")
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == "algorithm-changed")
                .count(),
            1
        );
        assert_eq!(repository(&directory).list_known_hosts().unwrap().len(), 1);
    }

    #[test]
    fn concurrent_known_host_verification_serializes_cas_versions() {
        let directory = tempfile::tempdir().expect("tempdir");
        repository(&directory)
            .trust_known_host("example.com", 22, "ssh-ed25519", b"key-a")
            .expect("trust key");
        let first_repository = repository(&directory);
        let second_repository = repository(&directory);
        let barrier = Arc::new(Barrier::new(2));
        let attempts = [first_repository, second_repository].map(|mut repository| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                repository
                    .record_known_host_verified("example.com", 22, "ssh-ed25519", b"key-a")
                    .expect("record concurrent verification")
                    .state_version
                    .get()
            })
        });
        let mut versions =
            attempts.map(|attempt| attempt.join().expect("join verification attempt"));
        versions.sort_unstable();
        assert_eq!(versions, [2, 3]);
        assert_eq!(
            repository(&directory).list_known_hosts().unwrap()[0].state_version,
            WireSequence::new(3)
        );
    }

    #[test]
    fn host_groups_tags_favorites_and_recent_successes_are_cas_bounded() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let production = repository
            .create_host_group("Production")
            .expect("create group");
        let backend = repository
            .create_host_tag("Backend")
            .expect("create backend tag");
        let critical = repository
            .create_host_tag("Critical")
            .expect("create critical tag");
        let first = repository
            .create_host("Alpha", "alpha.test", 22, None, None, false)
            .expect("create first host");
        let second = repository
            .create_host("Beta", "beta.test", 22, None, None, true)
            .expect("create second host");

        let organization = repository
            .replace_host_organization(
                &first.host_id,
                first.state_version,
                Some(&production.group_id),
                &[critical.tag_id.clone(), backend.tag_id.clone()],
            )
            .expect("classify host");
        assert_eq!(organization.host_state_version, WireSequence::new(2));
        assert_eq!(organization.group_id, Some(production.group_id.clone()));
        assert_eq!(
            organization.tag_ids,
            vec![backend.tag_id.clone(), critical.tag_id.clone()]
        );
        assert!(matches!(
            repository.replace_host_organization(
                &first.host_id,
                first.state_version,
                None,
                std::slice::from_ref(&backend.tag_id),
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.replace_host_organization(
                &first.host_id,
                organization.host_state_version,
                Some(&production.group_id),
                &[backend.tag_id.clone(), backend.tag_id.clone()],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            repository.delete_host_group(&production.group_id, production.state_version),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.delete_host_tag(&backend.tag_id, backend.state_version),
            Err(AppPersistenceError::Conflict)
        ));

        let favorite = repository
            .update_host_favorite(&first.host_id, organization.host_state_version, true)
            .expect("favorite host");
        assert!(favorite.favorite);
        assert_eq!(favorite.state_version, WireSequence::new(3));
        assert!(matches!(
            repository
                .update_host_favorite(&first.host_id, organization.host_state_version, false,),
            Err(AppPersistenceError::Conflict)
        ));

        let first_success = repository
            .record_successful_connection_at(&first.host_id, 1_000)
            .expect("record first success");
        let second_success = repository
            .record_successful_connection_at(&second.host_id, 2_000)
            .expect("record second success");
        let first_again = repository
            .record_successful_connection_at(&first.host_id, 3_000)
            .expect("record latest success");
        assert_eq!(
            first_success.successful_connection_count,
            WireSequence::new(1)
        );
        assert_eq!(second_success.recency_sequence, WireSequence::new(2));
        assert_eq!(
            first_again.successful_connection_count,
            WireSequence::new(2)
        );
        let recent = repository.list_recent_connections(2).expect("list recents");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].host_id, first.host_id);
        assert_eq!(recent[1].host_id, second.host_id);
        assert!(matches!(
            repository.list_recent_connections(0),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            repository.list_recent_connections(101),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        let recent_catalog = repository
            .list_host_catalog(HostCatalogSort::RecentlyConnected)
            .expect("sort catalog by recent success");
        assert_eq!(recent_catalog[0].host.host_id, first.host_id);
        assert_eq!(recent_catalog[0].tags.len(), 2);
        assert_eq!(
            recent_catalog[0]
                .group
                .as_ref()
                .map(|group| &group.group_id),
            Some(&production.group_id)
        );
        let favorite_catalog = repository
            .list_host_catalog(HostCatalogSort::FavoriteThenLabel)
            .expect("sort favorite catalog");
        assert!(favorite_catalog[0].host.favorite);

        let cleared = repository
            .replace_host_organization(&first.host_id, favorite.state_version, None, &[])
            .expect("clear classification");
        assert_eq!(cleared.host_state_version, WireSequence::new(4));
        repository
            .delete_host_group(&production.group_id, production.state_version)
            .expect("delete unused group");
        repository
            .delete_host_tag(&backend.tag_id, backend.state_version)
            .expect("delete unused backend tag");
        repository
            .delete_host_tag(&critical.tag_id, critical.state_version)
            .expect("delete unused critical tag");
    }

    #[test]
    fn group_and_tag_updates_reject_stale_versions_and_duplicate_labels() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let group = repository
            .create_host_group("Region A")
            .expect("create group");
        repository
            .create_host_group("Region B")
            .expect("create second group");
        let updated_group = repository
            .update_host_group(&group.group_id, group.state_version, "Region C")
            .expect("rename group");
        assert_eq!(updated_group.state_version, WireSequence::new(2));
        assert!(matches!(
            repository.update_host_group(&group.group_id, group.state_version, "Region D"),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.update_host_group(&group.group_id, updated_group.state_version, "region b",),
            Err(AppPersistenceError::Conflict)
        ));

        let tag = repository.create_host_tag("Linux").expect("create tag");
        repository
            .create_host_tag("Windows")
            .expect("create second tag");
        let updated_tag = repository
            .update_host_tag(&tag.tag_id, tag.state_version, "Unix")
            .expect("rename tag");
        assert_eq!(updated_tag.state_version, WireSequence::new(2));
        assert!(matches!(
            repository.update_host_tag(&tag.tag_id, tag.state_version, "Server"),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.update_host_tag(&tag.tag_id, updated_tag.state_version, "windows"),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn v3_to_v4_migration_preserves_existing_hosts_and_versions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("v3.sqlite3");
        let host_id = HostId::new();
        let connection = Connection::open(&database_path).expect("open fixture");
        create_v2_schema(&connection);
        connection
            .execute(
                "INSERT INTO hosts
                 (id, label, address, normalized_address, port, username, identity_id,
                  favorite, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, 'Preserved', 'preserved.test', 'preserved.test', 22, NULL, NULL,
                         1, 9, 10, 20)",
                [host_id.as_str()],
            )
            .expect("insert v2 host");
        migrate_v2_to_v3(&connection).expect("prepare real v3 fixture");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read v3 version");
        assert_eq!(version, 3);
        drop(connection);

        let repository = AppRepository::open(&database_path).expect("migrate v3 repository");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read v4 version");
        assert_eq!(version, super::SCHEMA_VERSION);
        let host = repository.get_host(&host_id).expect("read preserved host");
        assert_eq!(host.label, "Preserved");
        assert!(host.favorite);
        assert_eq!(host.state_version, WireSequence::new(9));
        let organization = repository
            .get_host_organization(&host_id)
            .expect("read new empty metadata");
        assert_eq!(organization.group_id, None);
        assert!(organization.tag_ids.is_empty());
        assert_eq!(organization.host_state_version, WireSequence::new(9));
        assert!(repository.list_recent_connections(10).unwrap().is_empty());
    }

    #[test]
    fn stale_host_mutations_fail_closed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let host = repository
            .create_host("Server", "server.test", 22, Some("root"), None, false)
            .expect("create host");
        let updated = repository
            .update_host(
                &host.host_id,
                host.state_version,
                "Server 2",
                "server.test",
                2222,
                Some("root"),
                None,
                false,
            )
            .expect("update host");
        assert_eq!(updated.state_version, WireSequence::new(2));
        assert!(matches!(
            repository.delete_host(&host.host_id, host.state_version),
            Err(AppPersistenceError::Conflict)
        ));
        repository
            .delete_host(&host.host_id, updated.state_version)
            .expect("delete current version");
    }

    #[test]
    fn host_batch_create_is_atomic_and_initializes_connection_defaults() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let hosts = repository
            .create_hosts_atomically(&[
                HostBatchCreateInput {
                    label: "Production".to_owned(),
                    address: "prod.example.test".to_owned(),
                    port: 22,
                    username: Some("deploy".to_owned()),
                    ingress: RouteIngress::DirectTcp,
                    jump_hosts: Vec::new(),
                },
                HostBatchCreateInput {
                    label: "Staging".to_owned(),
                    address: "staging.example.test".to_owned(),
                    port: 2222,
                    username: None,
                    ingress: RouteIngress::DirectTcp,
                    jump_hosts: Vec::new(),
                },
            ])
            .expect("create host batch");
        assert_eq!(hosts.len(), 2);
        for host in &hosts {
            let config = repository
                .get_host_connection_config(&host.host_id)
                .expect("default connection config");
            assert!(matches!(config.route_plan.ingress, RouteIngress::DirectTcp));
            assert_eq!(
                config.authentication_plan.mode,
                AuthenticationPlanMode::Identity
            );
        }

        assert!(
            repository
                .create_hosts_atomically(&[
                    HostBatchCreateInput {
                        label: "Valid".to_owned(),
                        address: "valid.example.test".to_owned(),
                        port: 22,
                        username: None,
                        ingress: RouteIngress::DirectTcp,
                        jump_hosts: Vec::new(),
                    },
                    HostBatchCreateInput {
                        label: "Invalid".to_owned(),
                        address: "bad host".to_owned(),
                        port: 22,
                        username: None,
                        ingress: RouteIngress::DirectTcp,
                        jump_hosts: Vec::new(),
                    },
                ])
                .is_err()
        );
        assert_eq!(repository.list_hosts().expect("list hosts").len(), 2);
    }

    #[test]
    fn host_batch_create_atomically_persists_supported_proxy_and_jump_routes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let shared_jump = HostBatchJumpHostInput {
            address: "jump.example.test".to_owned(),
            port: 2222,
            username: Some("jump-user".to_owned()),
        };
        let hosts = repository
            .create_hosts_atomically(&[
                HostBatchCreateInput {
                    label: "Shared jump".to_owned(),
                    address: "jump.example.test".to_owned(),
                    port: 2222,
                    username: Some("jump-user".to_owned()),
                    ingress: RouteIngress::DirectTcp,
                    jump_hosts: Vec::new(),
                },
                HostBatchCreateInput {
                    label: "HTTP target".to_owned(),
                    address: "http-target.example.test".to_owned(),
                    port: 22,
                    username: Some("deploy".to_owned()),
                    ingress: RouteIngress::HttpConnectProxy {
                        endpoint: ProxyEndpoint {
                            address: "http-proxy.example.test".to_owned(),
                            normalized_address: "http-proxy.example.test".to_owned(),
                            port: 8080,
                        },
                        proxy_auth_credential_ref_id: None,
                    },
                    jump_hosts: Vec::new(),
                },
                HostBatchCreateInput {
                    label: "SOCKS target".to_owned(),
                    address: "socks-target.example.test".to_owned(),
                    port: 22,
                    username: None,
                    ingress: RouteIngress::Socks5Proxy {
                        endpoint: ProxyEndpoint {
                            address: "socks-proxy.example.test".to_owned(),
                            normalized_address: "socks-proxy.example.test".to_owned(),
                            port: 1080,
                        },
                        dns_mode: ProxyDnsMode::Proxy,
                        proxy_auth_credential_ref_id: None,
                    },
                    jump_hosts: Vec::new(),
                },
                HostBatchCreateInput {
                    label: "Jump target A".to_owned(),
                    address: "jump-target-a.example.test".to_owned(),
                    port: 22,
                    username: None,
                    ingress: RouteIngress::DirectTcp,
                    jump_hosts: vec![
                        shared_jump.clone(),
                        HostBatchJumpHostInput {
                            address: "edge.example.test".to_owned(),
                            port: 22,
                            username: None,
                        },
                    ],
                },
                HostBatchCreateInput {
                    label: "Jump target B".to_owned(),
                    address: "jump-target-b.example.test".to_owned(),
                    port: 22,
                    username: None,
                    ingress: RouteIngress::DirectTcp,
                    jump_hosts: vec![shared_jump],
                },
            ])
            .expect("atomically import non-direct routes");
        assert_eq!(
            hosts.len(),
            6,
            "five selected targets plus one generated route host"
        );

        let by_label = hosts
            .iter()
            .map(|host| (host.label.as_str(), host))
            .collect::<std::collections::BTreeMap<_, _>>();
        let http = repository
            .get_host_connection_config(&by_label["HTTP target"].host_id)
            .expect("HTTP route");
        assert!(matches!(
            http.route_plan.ingress,
            RouteIngress::HttpConnectProxy {
                proxy_auth_credential_ref_id: None,
                ..
            }
        ));
        let socks = repository
            .get_host_connection_config(&by_label["SOCKS target"].host_id)
            .expect("SOCKS route");
        assert!(matches!(
            socks.route_plan.ingress,
            RouteIngress::Socks5Proxy {
                dns_mode: ProxyDnsMode::Proxy,
                proxy_auth_credential_ref_id: None,
                ..
            }
        ));
        let jump_a = repository
            .get_host_connection_config(&by_label["Jump target A"].host_id)
            .expect("jump route A");
        let jump_b = repository
            .get_host_connection_config(&by_label["Jump target B"].host_id)
            .expect("jump route B");
        assert_eq!(jump_a.route_plan.jump_host_ids.len(), 2);
        assert_eq!(jump_b.route_plan.jump_host_ids.len(), 1);
        assert_eq!(
            jump_a.route_plan.jump_host_ids[0], jump_b.route_plan.jump_host_ids[0],
            "the same frozen-preview hop is created once"
        );
        assert_eq!(
            jump_a.route_plan.jump_host_ids[0], by_label["Shared jump"].host_id,
            "an exact selected target is reused instead of duplicated"
        );
        for host in &hosts {
            let config = repository
                .get_host_connection_config(&host.host_id)
                .expect("imported connection config");
            assert_eq!(
                config.authentication_plan.mode,
                AuthenticationPlanMode::Identity
            );
            assert!(config.authentication_plan.credential_ref_ids.is_empty());
        }
    }

    #[test]
    fn host_batch_route_validation_fails_closed_without_partial_hosts() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let direct =
            |label: &str, address: &str, jumps: Vec<HostBatchJumpHostInput>| HostBatchCreateInput {
                label: label.to_owned(),
                address: address.to_owned(),
                port: 22,
                username: None,
                ingress: RouteIngress::DirectTcp,
                jump_hosts: jumps,
            };
        let jump = |address: &str, username: Option<&str>| HostBatchJumpHostInput {
            address: address.to_owned(),
            port: 22,
            username: username.map(str::to_owned),
        };

        for invalid in [
            vec![direct(
                "self",
                "self.example.test",
                vec![jump("self.example.test", None)],
            )],
            vec![direct(
                "repeat",
                "repeat.example.test",
                vec![
                    jump("hop.example.test", None),
                    jump("hop.example.test", None),
                ],
            )],
            vec![direct(
                "too-many",
                "many.example.test",
                (0..6)
                    .map(|index| jump(&format!("hop-{index}.example.test"), None))
                    .collect(),
            )],
            vec![
                direct(
                    "conflict-a",
                    "a.example.test",
                    vec![jump("shared.example.test", Some("alice"))],
                ),
                direct(
                    "conflict-b",
                    "b.example.test",
                    vec![jump("shared.example.test", Some("bob"))],
                ),
            ],
            vec![
                HostBatchCreateInput {
                    label: "selected-alice".to_owned(),
                    address: "selected.example.test".to_owned(),
                    port: 22,
                    username: Some("alice".to_owned()),
                    ingress: RouteIngress::DirectTcp,
                    jump_hosts: Vec::new(),
                },
                direct(
                    "selected-conflict",
                    "selected-target.example.test",
                    vec![jump("selected.example.test", Some("bob"))],
                ),
            ],
            vec![
                direct(
                    "cycle-a",
                    "cycle-a.example.test",
                    vec![jump("cycle-b.example.test", None)],
                ),
                direct(
                    "cycle-b",
                    "cycle-b.example.test",
                    vec![jump("cycle-a.example.test", None)],
                ),
            ],
            vec![
                direct("duplicate-a", "duplicate.example.test", Vec::new()),
                direct("duplicate-b", "duplicate.example.test", Vec::new()),
                direct(
                    "ambiguous-route",
                    "target.example.test",
                    vec![jump("duplicate.example.test", None)],
                ),
            ],
        ] {
            assert!(repository.create_hosts_atomically(&invalid).is_err());
            assert!(repository.list_hosts().expect("list hosts").is_empty());
        }

        let missing_proxy_credential = HostBatchCreateInput {
            label: "rollback".to_owned(),
            address: "rollback.example.test".to_owned(),
            port: 22,
            username: None,
            ingress: RouteIngress::HttpConnectProxy {
                endpoint: ProxyEndpoint {
                    address: "proxy.example.test".to_owned(),
                    normalized_address: "proxy.example.test".to_owned(),
                    port: 8080,
                },
                proxy_auth_credential_ref_id: Some(CredentialRefId::new()),
            },
            jump_hosts: Vec::new(),
        };
        assert!(
            repository
                .create_hosts_atomically(&[missing_proxy_credential])
                .is_err()
        );
        assert!(repository.list_hosts().expect("list hosts").is_empty());
    }

    #[test]
    fn selected_target_reuse_rejects_route_cycles_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let result = repository.create_hosts_atomically(&[
            HostBatchCreateInput {
                label: "Cycle A".to_owned(),
                address: "cycle-a.example.test".to_owned(),
                port: 22,
                username: None,
                ingress: RouteIngress::DirectTcp,
                jump_hosts: vec![HostBatchJumpHostInput {
                    address: "cycle-b.example.test".to_owned(),
                    port: 22,
                    username: None,
                }],
            },
            HostBatchCreateInput {
                label: "Cycle B".to_owned(),
                address: "cycle-b.example.test".to_owned(),
                port: 22,
                username: None,
                ingress: RouteIngress::DirectTcp,
                jump_hosts: vec![HostBatchJumpHostInput {
                    address: "cycle-a.example.test".to_owned(),
                    port: 22,
                    username: None,
                }],
            },
        ]);
        assert!(matches!(result, Err(AppPersistenceError::InvalidInput(_))));
        assert!(
            repository
                .list_hosts()
                .expect("list after rejected cycle")
                .is_empty(),
            "cycle rejection must roll back both selected targets and their routes"
        );
    }

    #[test]
    fn terminal_workspace_layout_roundtrips_and_rejects_stale_replace() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let initial = repository
            .get_terminal_workspace_layout()
            .expect("initial workspace");
        assert_eq!(initial.revision, WireSequence::new(1));
        assert!(initial.layout.tabs.is_empty());

        let layout = TerminalWorkspaceLayout {
            schema_version: 1,
            active_tab_id: Some("tab-1".to_owned()),
            tabs: vec![TerminalWorkspaceTab {
                tab_id: "tab-1".to_owned(),
                layout: TerminalWorkspaceLayoutNode::Pane {
                    pane_id: "pane-1".to_owned(),
                    terminal_id: "pane-1".to_owned(),
                },
                active_pane_id: "pane-1".to_owned(),
                panes: vec![TerminalWorkspacePane::Local {
                    pane_id: "pane-1".to_owned(),
                    label: "zsh".to_owned(),
                }],
            }],
        };
        let replaced = repository
            .replace_terminal_workspace_layout(initial.revision, &layout)
            .expect("replace workspace");
        assert_eq!(replaced.revision, WireSequence::new(2));
        assert_eq!(
            repository
                .get_terminal_workspace_layout()
                .expect("read workspace")
                .layout,
            layout
        );
        assert!(matches!(
            repository.replace_terminal_workspace_layout(initial.revision, &layout),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn terminal_workspace_layout_rejects_corrupt_stored_json() {
        let directory = tempfile::tempdir().expect("tempdir");
        let repository = repository(&directory);
        repository
            .connection
            .execute(
                "UPDATE terminal_workspace_layout SET layout_json = ?1 WHERE singleton = 1",
                ["{\"schemaVersion\":1,\"activeTabId\":\"missing\",\"tabs\":[]}"],
            )
            .expect("corrupt workspace fixture");
        assert!(matches!(
            repository.get_terminal_workspace_layout(),
            Err(AppPersistenceError::InvalidStoredData)
        ));
    }

    #[test]
    fn catalog_capability_cache_preserves_unknowns_and_rejects_unknown_formats() {
        let (raw, known, unsupported) =
            super::decode_catalog_capabilities(r#"["uiPanel","futureCapability"]"#).unwrap();
        assert_eq!(raw, ["uiPanel", "futureCapability"]);
        assert_eq!(known, [PluginCapability::UiPanel]);
        assert!(!unsupported);
        let (_, known, unsupported) = super::decode_catalog_capabilities(
            r#"{"formatVersion":1,"capabilities":["uiPanel"],"unsupportedManifest":true}"#,
        )
        .unwrap();
        assert_eq!(known, [PluginCapability::UiPanel]);
        assert!(unsupported);
        for value in [
            r#"{"formatVersion":2,"capabilities":[],"unsupportedManifest":false}"#,
            r#"{"capabilities":[],"unsupportedManifest":true}"#,
            r#"{"formatVersion":1,"capabilities":[],"unsupportedManifest":true,"unknown":true}"#,
        ] {
            assert!(super::decode_catalog_capabilities(value).is_err());
        }
    }

    fn plugin_catalog_entry(plugin_id: &PluginId, version: &str) -> PluginCatalogEntryRecord {
        PluginCatalogEntryRecord {
            plugin_id: plugin_id.clone(),
            version: version.to_owned(),
            name: "Fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            protocol_major: 1,
            protocol_minor: 0,
            platform: "desktop".to_owned(),
            architectures: vec!["universal".to_owned()],
            package_url: "https://plugins.example.test/fixture.zip".to_owned(),
            package_size: 1024,
            package_sha256: "a".repeat(64),
            publisher_key_base64: "A".repeat(44),
            publisher_signature_base64: "B".repeat(88),
            capabilities: vec![PluginCapability::UiPanel],
            raw_capabilities: serde_json::from_value(
                serde_json::to_value(vec![PluginCapability::UiPanel]).unwrap(),
            )
            .unwrap(),
            unsupported_manifest: false,
            minimum_app_version: "0.1.0".to_owned(),
            published_at_unix_ms: 100,
            details: None,
        }
    }

    #[test]
    fn catalog_release_details_roundtrip_and_v29_migration_preserve_catalog_trust() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("catalog.sqlite3");
        let plugin_id = PluginId::parse("org.norishell.systemd").unwrap();
        let trust = PluginCatalogTrustRecord {
            root_key_id: "root-1".into(),
            sequence: 10,
            payload_sha256: "1".repeat(64),
            catalog_signature_base64: "S".repeat(88),
            catalog_revision: "revision-1".into(),
            expires_at_unix_ms: 10000,
            verified_at_unix_ms: 1000,
        };
        let mut entry = plugin_catalog_entry(&plugin_id, "1.0.2");
        entry.details = Some(norishell_core_api::PluginCatalogReleaseDetails {
            description: "Service management".into(),
            release_notes: vec!["Preserve the service list".into()],
            extension_targets: vec!["terminal.tools".into()],
            release_published_at_unix_ms: Some(1000),
        });
        {
            let mut repository = AppRepository::open(&database_path).unwrap();
            repository
                .replace_plugin_catalog(None, &trust, &[entry.clone()])
                .unwrap();
        }
        {
            let repository = AppRepository::open(&database_path).unwrap();
            assert_eq!(
                repository.list_plugin_catalog_entries().unwrap(),
                vec![entry.clone()]
            );
            super::remove_schema_added_after_fixture_version(&repository.connection, 29).unwrap();
            repository.connection.execute_batch("DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json; PRAGMA user_version = 29;").unwrap();
        }
        let repository = AppRepository::open(&database_path).unwrap();
        entry.details = None;
        assert_eq!(
            repository.list_plugin_catalog_entries().unwrap(),
            vec![entry]
        );
        assert_eq!(repository.plugin_catalog_trust().unwrap(), Some(trust));
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
    }

    #[test]
    fn plugin_catalog_trust_sequence_and_digest_use_cas() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.fixture").expect("plugin id");
        let first = PluginCatalogTrustRecord {
            root_key_id: "root-1".to_owned(),
            sequence: 1,
            payload_sha256: "1".repeat(64),
            catalog_signature_base64: "S".repeat(88),
            catalog_revision: "revision-1".to_owned(),
            expires_at_unix_ms: 10_000,
            verified_at_unix_ms: 1_000,
        };
        repository
            .replace_plugin_catalog(None, &first, &[plugin_catalog_entry(&plugin_id, "1.0.0")])
            .expect("store first catalog");
        assert_eq!(
            repository.plugin_catalog_trust().expect("trust"),
            Some(first.clone())
        );
        assert!(matches!(
            repository.replace_plugin_catalog(None, &first, &[]),
            Err(AppPersistenceError::Conflict)
        ));
        let second = PluginCatalogTrustRecord {
            root_key_id: "root-2".to_owned(),
            sequence: 2,
            payload_sha256: "2".repeat(64),
            catalog_signature_base64: "T".repeat(88),
            catalog_revision: "revision-2".to_owned(),
            expires_at_unix_ms: 20_000,
            verified_at_unix_ms: 2_000,
        };
        repository
            .replace_plugin_catalog(
                Some(&first),
                &second,
                &[plugin_catalog_entry(&plugin_id, "2.0.0")],
            )
            .expect("explicit root rotation");
        assert_eq!(
            repository.list_plugin_catalog_entries().expect("entries")[0].version,
            "2.0.0"
        );
    }

    #[test]
    fn plugin_operation_saga_is_idempotent_and_phase_cas_rejects_stale_writers() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let operation_id = PluginOperationId::new();
        let plugin_id = PluginId::parse("com.norishell.fixture").expect("plugin id");
        let first = repository
            .begin_plugin_operation(
                &operation_id,
                Some(&plugin_id),
                PluginOperationKind::Install,
                "install-fixture-once",
                &"a".repeat(64),
                Some("1.0.0"),
                None,
            )
            .expect("begin operation");
        assert_eq!(first.phase, PluginOperationPhase::Resolve);
        assert!(matches!(
            repository.begin_plugin_operation(
                &PluginOperationId::new(),
                Some(&plugin_id),
                PluginOperationKind::Update,
                "concurrent-update",
                &"b".repeat(64),
                Some("1.1.0"),
                None,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .begin_plugin_operation(
                    &operation_id,
                    Some(&plugin_id),
                    PluginOperationKind::Install,
                    "install-fixture-once",
                    &"a".repeat(64),
                    Some("1.0.0"),
                    None,
                )
                .expect("exact replay"),
            first
        );
        let staged = repository
            .advance_plugin_operation(
                &operation_id,
                first.state_version,
                PluginOperationState::Running,
                PluginOperationPhase::Staged,
                None,
            )
            .expect("advance");
        assert_eq!(staged.state_version, WireSequence::new(2));
        assert!(matches!(
            repository.advance_plugin_operation(
                &operation_id,
                first.state_version,
                PluginOperationState::Succeeded,
                PluginOperationPhase::Completed,
                None,
            ),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn plugin_activation_and_same_major_grants_commit_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.atomic").expect("plugin id");
        let first = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Atomic".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &first,
                100,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                Some(initial_plugin_permissions(
                    &plugin_permission_binding(&first.package_sha256),
                    &[(PluginCapability::UiPanel, true)],
                )),
            )
            .expect("first atomic activation");
        assert!(matches!(
            repository.replace_plugin_capability_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(2),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&first.package_sha256),
                &[(PluginCapability::UiPanel, true)],
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.replace_plugin_capability_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&first.package_sha256),
                &[],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            repository.replace_plugin_capability_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&first.package_sha256),
                &[(PluginCapability::TerminalObserve, true)],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let second = PluginInstalledRecord {
            active_version: "1.1.0".to_owned(),
            package_sha256: "b".repeat(64),
            state_version: WireSequence::new(2),
            updated_at_unix_ms: 2,
            ..first
        };
        repository
            .activate_plugin_installation(
                Some(WireSequence::new(1)),
                &second,
                101,
                &"A".repeat(44),
                &"C".repeat(88),
                true,
                Some(PluginActivationPermissions {
                    protocol_major: 1,
                    binding: &plugin_permission_binding(&second.package_sha256),
                    previous_binding: Some(&plugin_permission_binding("a".repeat(64).as_str())),
                    expected_previous_grant_state_version: Some(WireSequence::new(1)),
                    expected_previous_scope_state_version: None,
                    grants: &[(PluginCapability::UiPanel, false)],
                    carried_host_scope_capabilities: &[],
                    approved_host_scopes: None,
                }),
            )
            .expect("same-major atomic update");
        let grants = repository
            .list_plugin_capability_grants(&plugin_id, &"1".repeat(64), 1)
            .expect("updated grants");
        assert_eq!(grants.len(), 1);
        assert!(!grants[0].granted);
        assert_eq!(grants[0].state_version, WireSequence::new(2));
        assert_eq!(
            repository
                .get_plugin_installation(&plugin_id)
                .expect("installation")
                .active_version,
            "1.1.0"
        );
    }

    #[test]
    fn plugin_update_rebinds_same_publisher_decisions_prunes_scopes_and_fences_rollback() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let host = repository
            .create_host("Scoped", "scoped.example", 22, None, None, false)
            .expect("host");
        let plugin_id = PluginId::parse("com.norishell.carryover").expect("plugin id");
        let first = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Carryover".to_owned(),
            publisher: "Verified Publisher".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.7".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![
                PluginCapability::UiPanel,
                PluginCapability::SshSync,
                PluginCapability::HostMetadataRead,
                PluginCapability::HostMutationPropose,
            ],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let first_binding = plugin_permission_binding(&first.package_sha256);
        let first_decisions = [
            (PluginCapability::UiPanel, true),
            (PluginCapability::SshSync, false),
            (PluginCapability::HostMetadataRead, true),
            (PluginCapability::HostMutationPropose, true),
        ];
        repository
            .activate_plugin_installation(
                None,
                &first,
                100,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                Some(initial_plugin_permissions(&first_binding, &first_decisions)),
            )
            .expect("initial activation");
        repository
            .replace_plugin_host_scope_grants(
                &plugin_id,
                &first.signer_fingerprint_sha256,
                1,
                first.state_version,
                None,
                &first_binding,
                &[
                    (host.host_id.clone(), PluginCapability::HostMetadataRead),
                    (host.host_id.clone(), PluginCapability::HostMutationPropose),
                ],
            )
            .expect("initial scopes");

        let second = PluginInstalledRecord {
            active_version: "1.0.8".to_owned(),
            package_sha256: "b".repeat(64),
            capabilities: vec![
                PluginCapability::UiPanel,
                PluginCapability::SshSync,
                PluginCapability::HostMetadataRead,
            ],
            state_version: WireSequence::new(2),
            updated_at_unix_ms: 2,
            ..first.clone()
        };
        let second_binding = plugin_permission_binding(&second.package_sha256);
        let second_decisions = [
            (PluginCapability::UiPanel, true),
            (PluginCapability::SshSync, false),
            (PluginCapability::HostMetadataRead, true),
        ];
        repository
            .activate_plugin_installation(
                Some(first.state_version),
                &second,
                101,
                &"A".repeat(44),
                &"C".repeat(88),
                true,
                Some(PluginActivationPermissions {
                    protocol_major: 1,
                    binding: &second_binding,
                    previous_binding: Some(&first_binding),
                    expected_previous_grant_state_version: Some(WireSequence::new(1)),
                    expected_previous_scope_state_version: Some(WireSequence::new(1)),
                    grants: &second_decisions,
                    carried_host_scope_capabilities: &[PluginCapability::HostMetadataRead],
                    approved_host_scopes: None,
                }),
            )
            .expect("same-publisher update");
        let rebound = repository
            .list_plugin_capability_grants(&plugin_id, &second.signer_fingerprint_sha256, 1)
            .expect("rebound grants");
        assert_eq!(rebound.len(), 3);
        assert!(rebound.iter().all(|grant| {
            grant.binding.as_ref() == Some(&second_binding)
                && grant.state_version == WireSequence::new(2)
        }));
        assert!(
            rebound
                .iter()
                .any(|grant| { grant.capability == PluginCapability::UiPanel && grant.granted })
        );
        assert!(
            rebound
                .iter()
                .any(|grant| { grant.capability == PluginCapability::SshSync && !grant.granted })
        );
        let scopes = repository
            .list_plugin_host_scope_grants(&plugin_id, &second.signer_fingerprint_sha256, 1)
            .expect("carried scopes");
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].capability, PluginCapability::HostMetadataRead);
        assert_eq!(
            repository
                .plugin_host_scope_set(&plugin_id, &second.signer_fingerprint_sha256, 1)
                .expect("scope set")
                .expect("scope set exists")
                .binding,
            Some(second_binding.clone())
        );

        let revoked = repository
            .replace_plugin_capability_grants(
                &plugin_id,
                &second.signer_fingerprint_sha256,
                1,
                second.state_version,
                Some(WireSequence::new(2)),
                &second_binding,
                &[
                    (PluginCapability::UiPanel, false),
                    (PluginCapability::SshSync, false),
                    (PluginCapability::HostMetadataRead, true),
                ],
            )
            .expect("revoke ordinary grant");
        assert!(
            revoked
                .iter()
                .any(|grant| { grant.capability == PluginCapability::UiPanel && !grant.granted })
        );
        let third = PluginInstalledRecord {
            active_version: "1.0.9".to_owned(),
            package_sha256: "c".repeat(64),
            state_version: WireSequence::new(3),
            updated_at_unix_ms: 3,
            ..second.clone()
        };
        let third_binding = plugin_permission_binding(&third.package_sha256);
        assert!(matches!(
            repository.activate_plugin_installation(
                Some(second.state_version),
                &third,
                102,
                &"A".repeat(44),
                &"D".repeat(88),
                true,
                Some(PluginActivationPermissions {
                    protocol_major: 1,
                    binding: &third_binding,
                    previous_binding: Some(&second_binding),
                    expected_previous_grant_state_version: Some(WireSequence::new(2)),
                    expected_previous_scope_state_version: Some(WireSequence::new(2)),
                    grants: &second_decisions,
                    carried_host_scope_capabilities: &[PluginCapability::HostMetadataRead],
                    approved_host_scopes: None,
                }),
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .get_plugin_installation(&plugin_id)
                .expect("installation after rejected update"),
            second
        );
        let after_rejected = repository
            .list_plugin_capability_grants(&plugin_id, &second.signer_fingerprint_sha256, 1)
            .expect("grants after rejected update");
        assert!(after_rejected.iter().any(|grant| {
            grant.capability == PluginCapability::UiPanel
                && !grant.granted
                && grant.state_version == WireSequence::new(3)
        }));
    }

    #[test]
    fn ssh_sync_capability_round_trips_and_grant_replacement_uses_cas() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.ssh-sync").expect("plugin id");
        let plugin = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "SSH Sync".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::SshSync],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &plugin,
                100,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                Some(initial_plugin_permissions(
                    &plugin_permission_binding(&plugin.package_sha256),
                    &[(PluginCapability::SshSync, true)],
                )),
            )
            .expect("activate SSH sync plugin");

        let initial = repository
            .list_plugin_capability_grants(&plugin_id, &"1".repeat(64), 1)
            .expect("list initial grants");
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].capability, PluginCapability::SshSync);
        assert!(initial[0].granted);
        assert_eq!(initial[0].state_version, WireSequence::new(1));

        let replaced = repository
            .replace_plugin_capability_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&plugin.package_sha256),
                &[(PluginCapability::SshSync, false)],
            )
            .expect("replace SSH sync grant");
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].capability, PluginCapability::SshSync);
        assert!(!replaced[0].granted);
        assert_eq!(replaced[0].state_version, WireSequence::new(2));
        assert!(matches!(
            repository.replace_plugin_capability_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&plugin.package_sha256),
                &[(PluginCapability::SshSync, true)],
            ),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn ordinary_reapproval_rebinds_only_ordinary_and_keeps_special_contract_stale() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.reapprove").expect("plugin id");
        let plugin = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Reapprove".to_owned(),
            publisher: "Publisher".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel, PluginCapability::SshSync],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let stale_binding = PluginPermissionBinding {
            artifact_sha256: plugin.package_sha256.clone(),
            app_version_major: 0,
            app_version_minor: 2,
            secure_surface_contract_revision: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &plugin,
                100,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                Some(initial_plugin_permissions(
                    &stale_binding,
                    &[
                        (PluginCapability::UiPanel, true),
                        (PluginCapability::SshSync, true),
                    ],
                )),
            )
            .expect("stale binding fixture");
        let current_binding = plugin_permission_binding(&plugin.package_sha256);
        let replaced = repository
            .replace_plugin_capability_grants(
                &plugin_id,
                &plugin.signer_fingerprint_sha256,
                1,
                plugin.state_version,
                Some(WireSequence::new(1)),
                &current_binding,
                &[
                    (PluginCapability::UiPanel, true),
                    (PluginCapability::SshSync, true),
                ],
            )
            .expect("ordinary reapproval");
        assert_eq!(
            replaced
                .iter()
                .find(|grant| grant.capability == PluginCapability::UiPanel)
                .and_then(|grant| grant.binding.clone()),
            Some(current_binding)
        );
        assert_eq!(
            replaced
                .iter()
                .find(|grant| grant.capability == PluginCapability::SshSync)
                .and_then(|grant| grant.binding.clone()),
            Some(stale_binding)
        );
    }

    #[test]
    fn plugin_host_scopes_require_global_grants_and_preserve_empty_set_cas() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let host = repository
            .create_host("Scoped", "scoped.example", 22, None, None, false)
            .expect("host");
        let plugin_id = PluginId::parse("com.norishell.host-scope").expect("plugin id");
        let plugin = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Host Scope".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![
                PluginCapability::UiPanel,
                PluginCapability::HostMetadataRead,
                PluginCapability::HostMutationPropose,
            ],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &plugin,
                100,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                Some(initial_plugin_permissions(
                    &plugin_permission_binding(&plugin.package_sha256),
                    &[
                        (PluginCapability::UiPanel, true),
                        (PluginCapability::HostMetadataRead, true),
                        (PluginCapability::HostMutationPropose, false),
                    ],
                )),
            )
            .expect("activate plugin");
        let grants = repository
            .replace_plugin_host_scope_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                None,
                &plugin_permission_binding(&plugin.package_sha256),
                &[(host.host_id.clone(), PluginCapability::HostMetadataRead)],
            )
            .expect("grant selected Host");
        assert_eq!(grants[0].state_version, WireSequence::new(1));
        assert!(matches!(
            repository.replace_plugin_host_scope_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                None,
                &plugin_permission_binding(&plugin.package_sha256),
                &[],
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.replace_plugin_host_scope_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&plugin.package_sha256),
                &[(host.host_id.clone(), PluginCapability::HostMutationPropose)],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let cleared = repository
            .replace_plugin_host_scope_grants(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&plugin.package_sha256),
                &[],
            )
            .expect("clear selected Hosts");
        assert!(cleared.is_empty());
        assert_eq!(
            repository
                .plugin_host_scope_state_version(&plugin_id, &"1".repeat(64), 1)
                .expect("scope version"),
            Some(WireSequence::new(2))
        );
    }

    #[test]
    fn protected_special_permissions_commit_global_and_host_scope_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let host = repository
            .create_host("Scoped", "scoped.example", 22, None, None, false)
            .expect("host");
        let plugin_id = PluginId::parse("com.norishell.special-scope").expect("plugin id");
        let plugin = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Special Scope".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![
                PluginCapability::UiPanel,
                PluginCapability::HostMetadataRead,
                PluginCapability::HostMutationPropose,
            ],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &plugin,
                100,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                Some(initial_plugin_permissions(
                    &plugin_permission_binding(&plugin.package_sha256),
                    &[
                        (PluginCapability::UiPanel, true),
                        (PluginCapability::HostMetadataRead, false),
                        (PluginCapability::HostMutationPropose, false),
                    ],
                )),
            )
            .expect("activate plugin");
        let decisions = [
            (PluginCapability::UiPanel, true),
            (PluginCapability::HostMetadataRead, true),
            (PluginCapability::HostMutationPropose, true),
        ];
        let (grants, scopes) = repository
            .replace_plugin_special_permissions(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(1)),
                None,
                &plugin_permission_binding(&plugin.package_sha256),
                &decisions,
                &[
                    (host.host_id.clone(), PluginCapability::HostMetadataRead),
                    (host.host_id.clone(), PluginCapability::HostMutationPropose),
                ],
            )
            .expect("atomic special permission update");
        assert!(
            grants
                .iter()
                .all(|grant| grant.state_version == WireSequence::new(2))
        );
        assert_eq!(scopes.len(), 2);
        assert!(
            scopes
                .iter()
                .all(|scope| scope.state_version == WireSequence::new(1))
        );

        assert!(matches!(
            repository.replace_plugin_special_permissions(
                &plugin_id,
                &"1".repeat(64),
                1,
                WireSequence::new(1),
                Some(WireSequence::new(2)),
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&plugin.package_sha256),
                &[
                    (PluginCapability::UiPanel, true),
                    (PluginCapability::HostMetadataRead, false),
                    (PluginCapability::HostMutationPropose, true),
                ],
                &[(host.host_id, PluginCapability::HostMetadataRead)],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let persisted = repository
            .list_plugin_capability_grants(&plugin_id, &"1".repeat(64), 1)
            .expect("persisted grants");
        assert!(persisted.iter().any(|grant| {
            grant.capability == PluginCapability::HostMetadataRead && grant.granted
        }));
    }

    #[test]
    fn plugin_audit_projection_is_bounded_and_newest_first() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.audit").expect("plugin id");
        repository
            .append_plugin_audit(Some(&plugin_id), None, "runtime", "started", None)
            .expect("first audit");
        repository
            .append_plugin_audit(
                Some(&plugin_id),
                None,
                "permission.special",
                "committed",
                Some("protected_window"),
            )
            .expect("second audit");

        let entries = repository.list_plugin_audit(1).expect("bounded audit");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "permission.special");
        assert_eq!(entries[0].plugin_id.as_deref(), Some(plugin_id.as_str()));
        assert_eq!(entries[0].detail_code.as_deref(), Some("protected_window"));
        assert!(matches!(
            repository.list_plugin_audit(0),
            Err(AppPersistenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn plugin_lifecycle_and_permission_commits_are_audited_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.audit-lifecycle").expect("plugin id");
        let installed = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Audit lifecycle".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &installed,
                100,
                "",
                "",
                true,
                Some(initial_plugin_permissions(
                    &plugin_permission_binding(&installed.package_sha256),
                    &[(PluginCapability::UiPanel, true)],
                )),
            )
            .expect("install with audit");
        let enabled = repository
            .set_plugin_install_state(
                &plugin_id,
                WireSequence::new(1),
                PluginInstallState::Enabled,
            )
            .expect("enable with audit");
        let disabled = repository
            .set_plugin_install_state(
                &plugin_id,
                enabled.state_version,
                PluginInstallState::Disabled,
            )
            .expect("disable with audit");
        repository
            .replace_plugin_capability_grants(
                &plugin_id,
                &installed.signer_fingerprint_sha256,
                1,
                disabled.state_version,
                Some(WireSequence::new(1)),
                &plugin_permission_binding(&installed.package_sha256),
                &[(PluginCapability::UiPanel, false)],
            )
            .expect("replace permissions with audit");

        let entries = repository.list_plugin_audit(10).expect("audit entries");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.action.as_str())
                .collect::<Vec<_>>(),
            vec![
                "permission.replace",
                "lifecycle.disable",
                "lifecycle.enable",
                "lifecycle.install",
            ]
        );
        assert!(entries.iter().all(|entry| entry.outcome == "committed"));
        assert_eq!(
            entries
                .last()
                .and_then(|entry| entry.detail_code.as_deref()),
            Some("package_and_grants")
        );
    }

    #[test]
    fn plugin_installation_requires_approval_for_expansion_and_artifact_identity_change() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let plugin_id = PluginId::parse("com.norishell.fixture").expect("plugin id");
        let first = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel],
            state: PluginInstallState::Enabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &first,
                1024,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                None,
            )
            .expect("first activation");
        let expanded = PluginInstalledRecord {
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "2.0.0".to_owned(),
            package_sha256: "b".repeat(64),
            capabilities: vec![PluginCapability::UiPanel, PluginCapability::TerminalObserve],
            state_version: WireSequence::new(2),
            updated_at_unix_ms: 2,
            ..first
        };
        assert!(matches!(
            repository.activate_plugin_installation(
                Some(WireSequence::new(1)),
                &expanded,
                1024,
                &"C".repeat(44),
                &"D".repeat(88),
                false,
                None,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        repository
            .activate_plugin_installation(
                Some(WireSequence::new(1)),
                &expanded,
                1024,
                &"C".repeat(44),
                &"D".repeat(88),
                true,
                None,
            )
            .expect("approved activation");
        let signer_changed = PluginInstalledRecord {
            signer_fingerprint_sha256: "2".repeat(64),
            active_version: "2.1.0".to_owned(),
            package_sha256: "c".repeat(64),
            state_version: WireSequence::new(3),
            updated_at_unix_ms: 3,
            ..expanded.clone()
        };
        assert!(matches!(
            repository.activate_plugin_installation(
                Some(WireSequence::new(2)),
                &signer_changed,
                1024,
                &"E".repeat(44),
                &"F".repeat(88),
                false,
                None,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        let grants = repository
            .replace_plugin_capability_grants(
                &plugin_id,
                &expanded.signer_fingerprint_sha256,
                1,
                WireSequence::new(2),
                None,
                &plugin_permission_binding(&expanded.package_sha256),
                &[
                    (PluginCapability::UiPanel, false),
                    (PluginCapability::TerminalObserve, true),
                ],
            )
            .expect("store signer and major scoped grant");
        assert_eq!(grants.len(), 2);
        assert!(grants.iter().any(|grant| {
            grant.capability == PluginCapability::TerminalObserve && grant.granted
        }));

        let disabled = repository
            .set_plugin_install_state(
                &plugin_id,
                WireSequence::new(2),
                PluginInstallState::Disabled,
            )
            .expect("disable with CAS");
        assert_eq!(disabled.state_version, WireSequence::new(3));
        assert!(matches!(
            repository.set_plugin_install_state(
                &plugin_id,
                WireSequence::new(2),
                PluginInstallState::Enabled,
            ),
            Err(AppPersistenceError::Conflict)
        ));

        let uninstall_operation_id = PluginOperationId::new();
        let uninstall = repository
            .begin_plugin_operation(
                &uninstall_operation_id,
                Some(&plugin_id),
                PluginOperationKind::Uninstall,
                "uninstall-fixture-once",
                &"e".repeat(64),
                None,
                Some("2.0.0"),
            )
            .expect("begin uninstall saga");
        let committed = repository
            .uninstall_plugin(
                &plugin_id,
                disabled.state_version,
                &uninstall_operation_id,
                uninstall.state_version,
                true,
            )
            .expect("commit uninstall database phase");
        assert_eq!(committed.phase, PluginOperationPhase::DatabaseCommitted);
        assert!(matches!(
            repository.get_plugin_installation(&plugin_id),
            Err(AppPersistenceError::NotFound)
        ));
        let retained_versions: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM plugin_installed_versions WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                |row| row.get(0),
            )
            .expect("retained versions");
        assert_eq!(retained_versions, 2);
    }

    #[test]
    fn v21_migration_adds_durable_ssh_sync_restore_sagas() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        {
            let repository = AppRepository::open(&database_path).expect("current repository");
            super::remove_schema_added_after_fixture_version(&repository.connection, 20).unwrap();
            repository
                .connection
                .execute_batch(
                    "DROP TABLE ssh_sync_restore_sagas;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 20;",
                )
                .expect("simulate v20 schema");
        }

        let repository = AppRepository::open(&database_path).expect("migrate v20 repository");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        let saga_table_exists: bool = repository
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'ssh_sync_restore_sagas')",
                [],
                |row| row.get(0),
            )
            .expect("saga table");
        assert_eq!(version, super::SCHEMA_VERSION);
        assert!(saga_table_exists);
    }

    #[test]
    fn ssh_sync_restore_saga_replay_is_exact_and_drift_fails_closed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, _) = ssh_sync_restore_fixture();

        let first = repository
            .begin_ssh_sync_restore_saga(&saga)
            .expect("begin restore saga");
        assert!(first.created);
        let replay = repository
            .begin_ssh_sync_restore_saga(&saga)
            .expect("replay restore saga");
        assert!(!replay.created);
        assert_eq!(first.record, replay.record);
        assert_eq!(
            repository
                .list_pending_ssh_sync_restore_sagas()
                .unwrap()
                .len(),
            1
        );

        let mut drift = saga;
        drift.plan_sha256 = "c".repeat(64);
        assert!(matches!(
            repository.begin_ssh_sync_restore_saga(&drift),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
    }

    #[test]
    fn ssh_sync_restore_commit_is_atomic_and_cross_revision_replay_is_stable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository
            .begin_ssh_sync_restore_saga(&saga)
            .expect("begin restore saga");
        assert_eq!(
            repository
                .preview_ssh_sync_restore_plan(&plan)
                .expect("preview restore"),
            super::SshSyncRestorePreview {
                create_count: 3,
                already_applied_count: 0,
                conflict_count: 0,
                new_secret_ref_ids: saga.secret_ref_ids.clone(),
            }
        );

        let committed = repository
            .commit_ssh_sync_restore_plan(&plan)
            .expect("commit restore");
        assert_eq!(committed.created_count, 3);
        assert_eq!(committed.already_applied_count, 0);
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&saga.attempt_id)
                .unwrap()
                .is_none()
        );
        repository
            .get_identity(&plan.identities[0].identity_id)
            .expect("restored identity");
        repository
            .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
            .expect("restored ready credential");
        repository
            .get_host(&plan.hosts[0].host_id)
            .expect("restored host");

        let mut later_saga = saga;
        later_saga.attempt_id = OperationId::new();
        later_saga.bundle_sha256 = "d".repeat(64);
        later_saga.plan_sha256 = "e".repeat(64);
        later_saga.secret_ref_ids.clear();
        let mut later_plan = plan;
        later_plan.attempt_id = later_saga.attempt_id.clone();
        later_plan.bundle_sha256 = later_saga.bundle_sha256.clone();
        later_plan.plan_sha256 = later_saga.plan_sha256.clone();
        let preview = repository
            .preview_ssh_sync_restore_plan(&later_plan)
            .expect("preview later revision");
        assert!(preview.new_secret_ref_ids.is_empty());
        repository
            .begin_ssh_sync_restore_saga(&later_saga)
            .expect("begin later revision");
        let replay = repository
            .commit_ssh_sync_restore_plan(&later_plan)
            .expect("replay stable objects");
        assert_eq!(replay.created_count, 0);
        assert_eq!(replay.already_applied_count, 3);
        let counts: (i64, i64, i64) = repository
            .connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM identities),
                   (SELECT COUNT(*) FROM credential_refs),
                   (SELECT COUNT(*) FROM hosts)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("metadata counts");
        assert_eq!(counts, (1, 1, 1));
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&later_saga.attempt_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn ssh_sync_restore_field_drift_conflicts_without_overwriting_metadata() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();

        let mut drift_saga = saga;
        drift_saga.attempt_id = OperationId::new();
        drift_saga.bundle_sha256 = "c".repeat(64);
        drift_saga.plan_sha256 = "d".repeat(64);
        drift_saga.secret_ref_ids.clear();
        let mut drift_plan = plan;
        drift_plan.attempt_id = drift_saga.attempt_id.clone();
        drift_plan.bundle_sha256 = drift_saga.bundle_sha256.clone();
        drift_plan.plan_sha256 = drift_saga.plan_sha256.clone();
        drift_plan.identities[0].label = "Drifted identity".to_owned();
        repository
            .begin_ssh_sync_restore_saga(&drift_saga)
            .expect("begin drift attempt");

        let preview = repository
            .preview_ssh_sync_restore_plan(&drift_plan)
            .expect("preview drift");
        assert!(preview.conflict_count > 0);
        assert!(preview.new_secret_ref_ids.is_empty());
        assert!(matches!(
            repository.commit_ssh_sync_restore_plan(&drift_plan),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .get_identity(&drift_plan.identities[0].identity_id)
                .unwrap()
                .label,
            "Restored identity"
        );
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&drift_saga.attempt_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn ssh_sync_restore_failure_rolls_back_all_metadata_and_keeps_saga() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_ssh_sync_restore_host
                 BEFORE INSERT ON hosts
                 BEGIN
                   SELECT RAISE(ABORT, 'injected restore failure');
                 END;",
            )
            .expect("install failure trigger");

        assert!(matches!(
            repository.commit_ssh_sync_restore_plan(&plan),
            Err(AppPersistenceError::Database(_))
        ));
        let counts: (i64, i64, i64) = repository
            .connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM identities),
                   (SELECT COUNT(*) FROM credential_refs),
                   (SELECT COUNT(*) FROM hosts)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("metadata counts");
        assert_eq!(counts, (0, 0, 0));
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&saga.attempt_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn restore_preview_cleanup_refs_exclude_exact_published_credentials() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (old_saga, old_plan) = ssh_sync_restore_fixture();
        let old_secret_ref = old_saga.secret_ref_ids[0].clone();
        repository.begin_ssh_sync_restore_saga(&old_saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&old_plan).unwrap();

        let new_identity_id = IdentityId::new();
        let new_secret_ref = SecretRefId::new();
        let mut new_plan = old_plan;
        new_plan.attempt_id = OperationId::new();
        new_plan.bundle_sha256 = "c".repeat(64);
        new_plan.plan_sha256 = "d".repeat(64);
        new_plan.identities.push(SshSyncRestoreIdentityInput {
            identity_id: new_identity_id.clone(),
            label: "Second restored identity".to_owned(),
            username: Some("operator".to_owned()),
        });
        new_plan.credentials.push(SshSyncRestoreCredentialInput {
            credential_ref_id: CredentialRefId::new(),
            identity_id: new_identity_id.clone(),
            operation_id: OperationId::new(),
            idempotency_key: "restore-credential-portable-b".to_owned(),
            priority: 0,
            label: "Second restored password".to_owned(),
            material: SshSyncRestoreCredentialMaterial::Password {
                secret_ref_id: new_secret_ref.clone(),
            },
        });
        let mut new_host_request = configured_host_request(OperationId::new());
        new_host_request.idempotency_key = "restore-host-portable-b".to_owned();
        new_host_request.label = "Second restored Host".to_owned();
        new_host_request.address = "second.example".to_owned();
        new_host_request.identity_id = Some(new_identity_id);
        new_host_request.login_automation_enabled = false;
        new_host_request.login_automation_confirmed = false;
        new_host_request.login_automation_steps.clear();
        new_plan.hosts.push(SshSyncRestoreHostInput {
            host_id: HostId::new(),
            request: new_host_request,
            tag_labels: Vec::new(),
            login_automation: SshSyncLoginAutomation::disabled(),
        });

        let preview = repository
            .preview_ssh_sync_restore_plan(&new_plan)
            .expect("preview mixed exact and new plan");
        assert_eq!(
            preview.new_secret_ref_ids.as_slice(),
            std::slice::from_ref(&new_secret_ref)
        );
        assert!(!preview.new_secret_ref_ids.contains(&old_secret_ref));
        let new_saga = SshSyncRestoreSagaInput {
            attempt_id: new_plan.attempt_id.clone(),
            plugin_id: new_plan.plugin_id.clone(),
            profile_id: new_plan.profile_id.clone(),
            bundle_sha256: new_plan.bundle_sha256.clone(),
            plan_sha256: new_plan.plan_sha256.clone(),
            secret_ref_ids: preview.new_secret_ref_ids,
        };
        repository.begin_ssh_sync_restore_saga(&new_saga).unwrap();
        repository
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_second_ssh_sync_restore_host
                 BEFORE INSERT ON hosts
                 BEGIN
                   SELECT RAISE(ABORT, 'injected second restore failure');
                 END;",
            )
            .expect("install failure trigger");
        assert!(matches!(
            repository.commit_ssh_sync_restore_plan(&new_plan),
            Err(AppPersistenceError::Database(_))
        ));

        let pending = repository
            .get_pending_ssh_sync_restore_saga(&new_saga.attempt_id)
            .unwrap()
            .expect("pending mixed restore saga");
        assert_eq!(pending.secret_ref_ids, [new_secret_ref]);
        assert!(!pending.secret_ref_ids.contains(&old_secret_ref));
        let counts: (i64, i64, i64) = repository
            .connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM identities),
                   (SELECT COUNT(*) FROM credential_refs),
                   (SELECT COUNT(*) FROM hosts)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("metadata counts");
        assert_eq!(counts, (1, 1, 1));
    }

    #[test]
    fn ssh_sync_restore_commits_all_portable_ready_credential_kinds() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (mut saga, mut plan) = ssh_sync_restore_fixture();
        plan.hosts.clear();
        let private_key_ref = SecretRefId::new();
        let passphrase_ref = SecretRefId::new();
        let private_key_credential_id = CredentialRefId::new();
        let keyboard_credential_id = CredentialRefId::new();
        plan.credentials.push(SshSyncRestoreCredentialInput {
            credential_ref_id: private_key_credential_id.clone(),
            identity_id: plan.identities[0].identity_id.clone(),
            operation_id: OperationId::new(),
            idempotency_key: "restore-private-key-portable".to_owned(),
            priority: 1,
            label: "Restored private key".to_owned(),
            material: SshSyncRestoreCredentialMaterial::PrivateKey {
                secret_ref_id: private_key_ref.clone(),
                passphrase_secret_ref_id: Some(passphrase_ref.clone()),
                public_key_algorithm: "ssh-ed25519".to_owned(),
                public_key_fingerprint: "SHA256:portable-key".to_owned(),
            },
        });
        plan.credentials.push(SshSyncRestoreCredentialInput {
            credential_ref_id: keyboard_credential_id.clone(),
            identity_id: plan.identities[0].identity_id.clone(),
            operation_id: OperationId::new(),
            idempotency_key: "restore-keyboard-interactive-portable".to_owned(),
            priority: 2,
            label: "Restored keyboard interactive".to_owned(),
            material: SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds: 4 },
        });
        let preview = repository.preview_ssh_sync_restore_plan(&plan).unwrap();
        assert_eq!(preview.create_count, 4);
        saga.secret_ref_ids = preview.new_secret_ref_ids;
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();

        assert!(matches!(
            repository
                .get_ready_credential_record(&private_key_credential_id)
                .unwrap()
                .details,
            CredentialRecordDetails::PrivateKey {
                secret_ref_id,
                passphrase_secret_ref_id: Some(passphrase),
                ..
            } if secret_ref_id == private_key_ref && passphrase == passphrase_ref
        ));
        assert!(matches!(
            repository
                .get_ready_credential_record(&keyboard_credential_id)
                .unwrap()
                .details,
            CredentialRecordDetails::KeyboardInteractive { max_rounds: 4 }
        ));
    }

    #[test]
    fn ssh_sync_restore_topologically_publishes_jump_hosts() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, mut plan) = ssh_sync_restore_fixture();
        let jump_host_id = HostId::new();
        plan.hosts[0].request.jump_host_ids = vec![jump_host_id.clone()];
        let target_host_id = plan.hosts[0].host_id.clone();
        let mut jump_request = configured_host_request(OperationId::new());
        jump_request.idempotency_key = "restore-jump-host-portable".to_owned();
        jump_request.label = "Restored jump Host".to_owned();
        jump_request.address = "jump.example".to_owned();
        jump_request.identity_id = Some(plan.identities[0].identity_id.clone());
        jump_request.login_automation_enabled = false;
        jump_request.login_automation_confirmed = false;
        jump_request.login_automation_steps.clear();
        plan.hosts.push(SshSyncRestoreHostInput {
            host_id: jump_host_id.clone(),
            request: jump_request,
            tag_labels: Vec::new(),
            login_automation: SshSyncLoginAutomation::disabled(),
        });

        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let route = repository
            .get_host_connection_config(&target_host_id)
            .unwrap()
            .route_plan;
        assert_eq!(route.jump_host_ids, [jump_host_id]);
    }

    #[test]
    fn v22_migration_adds_signer_bound_ssh_sync_profile_states() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        {
            let repository = AppRepository::open(&database_path).expect("current repository");
            super::remove_schema_added_after_fixture_version(&repository.connection, 21).unwrap();
            repository
                .connection
                .execute_batch(
                    "DROP TABLE ssh_sync_profile_states;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 21;",
                )
                .expect("simulate v21 schema");
        }

        let repository = AppRepository::open(&database_path).expect("migrate v21 repository");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        let table_exists: bool = repository
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'ssh_sync_profile_states')",
                [],
                |row| row.get(0),
            )
            .expect("profile state table");
        assert_eq!(version, super::SCHEMA_VERSION);
        assert!(table_exists);
    }

    #[test]
    fn ssh_sync_profile_key_binding_is_exactly_idempotent_and_recovers_after_reopen() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let input = ssh_sync_profile_fixture("primary");
        let sync_key_secret_ref_id = SecretRefId::new();
        let binding = SshSyncProfileKeyBinding {
            sync_key_secret_ref_id: sync_key_secret_ref_id.clone(),
            password_wrapped_sync_key_envelope: Some(vec![0xa5; 64]),
        };
        {
            let mut repository = AppRepository::open(&database_path).expect("repository");
            let created = repository
                .ensure_ssh_sync_profile_state(&input)
                .expect("create profile state");
            assert_eq!(created.state_version, WireSequence::new(1));
            assert_eq!(
                repository.ensure_ssh_sync_profile_state(&input).unwrap(),
                created
            );
            let bound = repository
                .bind_ssh_sync_profile_key(&input.key, created.state_version, &binding)
                .expect("bind sync key");
            assert_eq!(bound.state_version, WireSequence::new(2));
            assert_eq!(
                repository
                    .bind_ssh_sync_profile_key(&input.key, created.state_version, &binding)
                    .expect("replay lost binding response"),
                bound
            );
            let drift = SshSyncProfileKeyBinding {
                sync_key_secret_ref_id: SecretRefId::new(),
                password_wrapped_sync_key_envelope: Some(vec![0xa5; 64]),
            };
            assert!(matches!(
                repository.bind_ssh_sync_profile_key(&input.key, bound.state_version, &drift),
                Err(AppPersistenceError::Conflict)
            ));
            let envelope_drift = SshSyncProfileKeyBinding {
                sync_key_secret_ref_id: sync_key_secret_ref_id.clone(),
                password_wrapped_sync_key_envelope: Some(vec![0x5a; 64]),
            };
            assert!(matches!(
                repository.bind_ssh_sync_profile_key(
                    &input.key,
                    bound.state_version,
                    &envelope_drift,
                ),
                Err(AppPersistenceError::Conflict)
            ));
        }

        let repository = AppRepository::open(&database_path).expect("reopen repository");
        let recovered = repository
            .get_ssh_sync_profile_state(&input.key)
            .unwrap()
            .expect("recover state");
        assert_eq!(recovered.key_binding, Some(binding));
        let debug = format!("{recovered:?}");
        assert!(!debug.contains(sync_key_secret_ref_id.as_str()));
        assert!(!debug.contains("165, 165"));
    }

    #[test]
    fn ssh_sync_profile_scope_and_remote_baseline_updates_are_strict_cas() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let input = ssh_sync_profile_fixture("cas-profile");
        let created = repository.ensure_ssh_sync_profile_state(&input).unwrap();
        let invalid_profile = ssh_sync_profile_fixture(" padded ");
        assert!(matches!(
            repository.ensure_ssh_sync_profile_state(&invalid_profile),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let mut invalid_signer = ssh_sync_profile_fixture("invalid-signer");
        invalid_signer.key.signer_fingerprint_sha256 = "A".repeat(64);
        assert!(matches!(
            repository.ensure_ssh_sync_profile_state(&invalid_signer),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let all_eligible = SshSyncProfileScope {
            mode: SshSyncProfileScopeMode::AllEligible,
            custom_host_ids: Vec::new(),
            custom_credential_ref_ids: Vec::new(),
            custom_desktop_profile_ids: Vec::new(),
        };
        let scoped = repository
            .replace_ssh_sync_profile_scope(&input.key, created.state_version, &all_eligible)
            .expect("replace scope");
        assert_eq!(scoped.state_version, WireSequence::new(2));
        assert!(matches!(
            repository.replace_ssh_sync_profile_scope(
                &input.key,
                created.state_version,
                &input.scope,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.replace_ssh_sync_profile_remote_baseline(
                &input.key,
                scoped.state_version,
                Some(&SshSyncProfileRemoteBaseline {
                    remote_revision: 7,
                    remote_etag: "W/\"weak\"".to_owned(),
                    baseline_content_sha256: "b".repeat(64),
                    baseline_exchange_sha256: "c".repeat(64),
                }),
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let baseline = SshSyncProfileRemoteBaseline {
            remote_revision: 7,
            remote_etag: "\"revision-7\"".to_owned(),
            baseline_content_sha256: "b".repeat(64),
            baseline_exchange_sha256: "c".repeat(64),
        };
        let updated = repository
            .replace_ssh_sync_profile_remote_baseline(
                &input.key,
                scoped.state_version,
                Some(&baseline),
            )
            .expect("replace baseline");
        assert_eq!(updated.remote_baseline, Some(baseline));
        assert_eq!(updated.state_version, WireSequence::new(3));
        assert_eq!(
            repository
                .replace_ssh_sync_profile_remote_baseline(
                    &input.key,
                    updated.state_version,
                    updated.remote_baseline.as_ref(),
                )
                .expect("exact baseline no-op")
                .state_version,
            updated.state_version
        );

        let duplicate_host = HostId::new();
        assert!(matches!(
            repository.replace_ssh_sync_profile_scope(
                &input.key,
                updated.state_version,
                &SshSyncProfileScope {
                    mode: SshSyncProfileScopeMode::Custom,
                    custom_host_ids: vec![duplicate_host.clone(), duplicate_host],
                    custom_credential_ref_ids: Vec::new(),
                    custom_desktop_profile_ids: Vec::new(),
                },
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
    }

    #[test]
    fn ssh_sync_remote_reset_preserves_local_profile_and_clears_uncertain_upload() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let input = ssh_sync_profile_fixture("remote-reset");
        let created = repository.ensure_ssh_sync_profile_state(&input).unwrap();
        let baseline = SshSyncProfileRemoteBaseline {
            remote_revision: 4,
            remote_etag: "\"revision-4\"".to_owned(),
            baseline_content_sha256: "a".repeat(64),
            baseline_exchange_sha256: "b".repeat(64),
        };
        let with_baseline = repository
            .replace_ssh_sync_profile_remote_baseline(
                &input.key,
                created.state_version,
                Some(&baseline),
            )
            .unwrap();
        repository
            .ensure_ssh_sync_http_upload_attempt(&SshSyncHttpUploadAttemptInput {
                owner: input.key.clone(),
                canonical_url: "https://api.example.test/exchange".to_owned(),
                http_method: SshSyncHttpMethod::Put,
                use_oauth: true,
                authorization_revision: WireSequence::new(2),
                configuration_revision: WireSequence::new(3),
                base_revision: 4,
                base_etag: Some("\"revision-4\"".to_owned()),
                target_revision: 5,
                keyed_content_sha256: "c".repeat(64),
                body_sha256: "d".repeat(64),
                idempotency_key: uuid::Uuid::new_v4().to_string(),
            })
            .unwrap();
        let fence = SshSyncHttpUploadCompletionFence {
            canonical_url: "https://api.example.test/exchange".to_owned(),
            http_method: SshSyncHttpMethod::Put,
            use_oauth: true,
            authorization_revision: WireSequence::new(2),
            configuration_revision: WireSequence::new(3),
        };
        let mismatched_fence = SshSyncHttpUploadCompletionFence {
            configuration_revision: WireSequence::new(4),
            ..fence.clone()
        };
        assert!(matches!(
            repository.reset_ssh_sync_profile_remote_state(
                &input.key,
                with_baseline.state_version,
                &mismatched_fence,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(
            repository
                .get_ssh_sync_http_upload_attempt(&input.key)
                .unwrap()
                .is_some()
        );

        let reset = repository
            .reset_ssh_sync_profile_remote_state(&input.key, with_baseline.state_version, &fence)
            .expect("reset remote projection");
        assert_eq!(reset.remote_baseline, None);
        assert_eq!(reset.scope, input.scope);
        assert_eq!(reset.key_binding, None);
        assert_eq!(reset.state_version, WireSequence::new(3));
        assert!(
            repository
                .get_ssh_sync_http_upload_attempt(&input.key)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repository
                .reset_ssh_sync_profile_remote_state(&input.key, reset.state_version, &fence)
                .expect("idempotent empty reset")
                .state_version,
            reset.state_version
        );
        assert!(matches!(
            repository.reset_ssh_sync_profile_remote_state(
                &input.key,
                with_baseline.state_version,
                &fence,
            ),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn deleting_plugin_profile_states_returns_only_its_vault_cleanup_refs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let first = ssh_sync_profile_fixture("first");
        let second = ssh_sync_profile_fixture("second");
        let mut other = ssh_sync_profile_fixture("other");
        other.key.plugin_id = PluginId::parse("org.norixor.other-plugin").expect("other plugin");
        let first_ref = SecretRefId::new();
        let second_ref = SecretRefId::new();
        let other_ref = SecretRefId::new();
        for (input, secret_ref) in [
            (&first, &first_ref),
            (&second, &second_ref),
            (&other, &other_ref),
        ] {
            let created = repository.ensure_ssh_sync_profile_state(input).unwrap();
            repository
                .bind_ssh_sync_profile_key(
                    &input.key,
                    created.state_version,
                    &SshSyncProfileKeyBinding {
                        sync_key_secret_ref_id: secret_ref.clone(),
                        password_wrapped_sync_key_envelope: None,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            repository
                .list_ssh_sync_profile_states_for_plugin(&first.key.plugin_id)
                .unwrap()
                .len(),
            2
        );

        let deleted = repository
            .delete_ssh_sync_profile_states_for_plugin(&first.key.plugin_id)
            .expect("delete plugin states");
        assert_eq!(deleted.deleted_count, 2);
        assert_eq!(deleted.sync_key_secret_ref_ids.len(), 2);
        assert!(deleted.sync_key_secret_ref_ids.contains(&first_ref));
        assert!(deleted.sync_key_secret_ref_ids.contains(&second_ref));
        assert!(!deleted.sync_key_secret_ref_ids.contains(&other_ref));
        assert!(
            repository
                .list_ssh_sync_profile_states_for_plugin(&first.key.plugin_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            repository
                .get_ssh_sync_profile_state(&other.key)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn v23_migration_clears_unverifiable_v22_baseline_and_adds_object_mappings() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let input = ssh_sync_profile_fixture("migration-v23");
        {
            let mut repository = AppRepository::open(&database_path).expect("current repository");
            let created = repository.ensure_ssh_sync_profile_state(&input).unwrap();
            repository
                .replace_ssh_sync_profile_remote_baseline(
                    &input.key,
                    created.state_version,
                    Some(&SshSyncProfileRemoteBaseline {
                        remote_revision: 4,
                        remote_etag: "\"revision-4\"".to_owned(),
                        baseline_content_sha256: "a".repeat(64),
                        baseline_exchange_sha256: "b".repeat(64),
                    }),
                )
                .unwrap();
            super::remove_schema_added_after_fixture_version(&repository.connection, 22).unwrap();
            repository
                .connection
                .execute_batch(
                    "DROP TABLE ssh_sync_object_mappings;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 22;",
                )
                .expect("simulate v22 schema");
        }

        let repository = AppRepository::open(&database_path).expect("migrate v22 repository");
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let mapping_table_exists: bool = repository
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'ssh_sync_object_mappings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        assert!(mapping_table_exists);
        let migrated = repository
            .get_ssh_sync_profile_state(&input.key)
            .unwrap()
            .expect("migrated profile state");
        assert_eq!(migrated.scope, input.scope);
        assert_eq!(migrated.remote_baseline, None);
    }

    #[test]
    fn ssh_sync_object_mapping_batch_is_atomic_bijective_and_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = ssh_sync_profile_fixture("object-map");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let secret_ref_id = SecretRefId::new();
        let inputs = vec![
            SshSyncObjectMappingInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                local_object_id: SshSyncLocalObjectId::Host(HostId::new()),
            },
            SshSyncObjectMappingInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                local_object_id: SshSyncLocalObjectId::Identity(IdentityId::new()),
            },
            SshSyncObjectMappingInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                local_object_id: SshSyncLocalObjectId::Credential(CredentialRefId::new()),
            },
            SshSyncObjectMappingInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                local_object_id: SshSyncLocalObjectId::Secret(secret_ref_id.clone()),
            },
        ];
        let created = repository
            .resolve_or_create_ssh_sync_object_mappings(&owner.key, &inputs)
            .expect("create mapping batch");
        assert!(created.iter().all(|resolution| resolution.created));
        let replay = repository
            .resolve_or_create_ssh_sync_object_mappings(&owner.key, &inputs)
            .expect("replay mapping batch");
        assert!(replay.iter().all(|resolution| !resolution.created));
        assert_eq!(
            created
                .iter()
                .map(|value| &value.mapping)
                .collect::<Vec<_>>(),
            replay
                .iter()
                .map(|value| &value.mapping)
                .collect::<Vec<_>>()
        );
        let listed = repository
            .list_ssh_sync_object_mappings(&owner.key)
            .unwrap();
        assert_eq!(listed.len(), 4);
        assert!(listed.iter().any(|mapping| {
            mapping.object_kind == SshSyncObjectKind::Secret
                && mapping.local_object_id == SshSyncLocalObjectId::Secret(secret_ref_id.clone())
        }));
        let secret_debug = format!(
            "{:?}",
            listed
                .iter()
                .find(|mapping| mapping.object_kind == SshSyncObjectKind::Secret)
                .unwrap()
        );
        assert!(!secret_debug.contains(secret_ref_id.as_str()));

        let portable_drift = SshSyncObjectMappingInput {
            portable_object_id: inputs[0].portable_object_id.clone(),
            local_object_id: SshSyncLocalObjectId::Host(HostId::new()),
        };
        assert!(matches!(
            repository.resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                std::slice::from_ref(&portable_drift),
            ),
            Err(AppPersistenceError::Conflict)
        ));
        let local_drift = SshSyncObjectMappingInput {
            portable_object_id: uuid::Uuid::new_v4().to_string(),
            local_object_id: inputs[1].local_object_id.clone(),
        };
        let never_committed = SshSyncObjectMappingInput {
            portable_object_id: uuid::Uuid::new_v4().to_string(),
            local_object_id: SshSyncLocalObjectId::Host(HostId::new()),
        };
        assert!(matches!(
            repository.resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[never_committed, local_drift],
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .list_ssh_sync_object_mappings(&owner.key)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn ssh_sync_object_mapping_validation_and_plugin_delete_are_owner_scoped() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let first = ssh_sync_profile_fixture("map-first");
        let mut other = ssh_sync_profile_fixture("map-other");
        other.key.plugin_id = PluginId::parse("org.norixor.map-other").unwrap();
        repository.ensure_ssh_sync_profile_state(&first).unwrap();
        repository.ensure_ssh_sync_profile_state(&other).unwrap();
        let shared_portable_id = uuid::Uuid::new_v4().to_string();
        let first_input = SshSyncObjectMappingInput {
            portable_object_id: shared_portable_id.clone(),
            local_object_id: SshSyncLocalObjectId::Host(HostId::new()),
        };
        let other_input = SshSyncObjectMappingInput {
            portable_object_id: shared_portable_id,
            local_object_id: SshSyncLocalObjectId::Host(HostId::new()),
        };
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &first.key,
                std::slice::from_ref(&first_input),
            )
            .unwrap();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &other.key,
                std::slice::from_ref(&other_input),
            )
            .unwrap();
        assert!(matches!(
            repository.resolve_or_create_ssh_sync_object_mappings(
                &first.key,
                &[first_input.clone(), first_input],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert!(matches!(
            repository.resolve_or_create_ssh_sync_object_mappings(
                &first.key,
                &[SshSyncObjectMappingInput {
                    portable_object_id: uuid::Uuid::nil().to_string(),
                    local_object_id: SshSyncLocalObjectId::Host(HostId::new()),
                }],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        assert_eq!(
            repository
                .delete_ssh_sync_object_mappings_for_plugin(&first.key.plugin_id)
                .unwrap(),
            1
        );
        assert!(
            repository
                .list_ssh_sync_object_mappings(&first.key)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            repository
                .list_ssh_sync_object_mappings(&other.key)
                .unwrap()
                .len(),
            1
        );
        repository
            .delete_ssh_sync_profile_states_for_plugin(&other.key.plugin_id)
            .unwrap();
        assert!(
            repository
                .list_ssh_sync_object_mappings(&other.key)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn v24_migration_and_explicit_success_timestamp_are_cas_safe() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let input = ssh_sync_profile_fixture("sync-success");
        {
            let repository = AppRepository::open(&database_path).unwrap();
            super::remove_schema_added_after_fixture_version(&repository.connection, 23).unwrap();
            repository
                .connection
                .execute_batch(
                    "DROP TABLE ssh_sync_vault_gc_queue;
                     DROP TABLE ssh_sync_owned_delta_operations;
                     ALTER TABLE ssh_sync_profile_states DROP COLUMN last_successful_sync_at_ms;
                     DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json;
                     DROP TABLE plugin_settings;
                     PRAGMA user_version = 23;",
                )
                .unwrap();
        }
        let mut repository = AppRepository::open(&database_path).unwrap();
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        let created = repository.ensure_ssh_sync_profile_state(&input).unwrap();
        assert_eq!(created.last_successful_sync_at_unix_ms, None);
        let scoped = repository
            .replace_ssh_sync_profile_scope(
                &input.key,
                created.state_version,
                &SshSyncProfileScope {
                    mode: SshSyncProfileScopeMode::AllEligible,
                    custom_host_ids: Vec::new(),
                    custom_credential_ref_ids: Vec::new(),
                    custom_desktop_profile_ids: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(scoped.last_successful_sync_at_unix_ms, None);
        let succeeded = repository
            .mark_ssh_sync_profile_sync_succeeded(&input.key, scoped.state_version)
            .unwrap();
        assert!(succeeded.last_successful_sync_at_unix_ms.is_some());
        assert!(matches!(
            repository.mark_ssh_sync_profile_sync_succeeded(&input.key, scoped.state_version),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn owned_delete_is_atomic_owner_fenced_replayable_and_gc_ack_is_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let owner = ssh_sync_profile_fixture("owned-delete");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_identity = uuid::Uuid::new_v4().to_string();
        let portable_credential = uuid::Uuid::new_v4().to_string();
        let portable_host = uuid::Uuid::new_v4().to_string();
        let portable_secret = uuid::Uuid::new_v4().to_string();
        let secret_ref_id = saga.secret_ref_ids[0].clone();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_identity.clone(),
                        local_object_id: SshSyncLocalObjectId::Identity(
                            plan.identities[0].identity_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_credential.clone(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            plan.credentials[0].credential_ref_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_host.clone(),
                        local_object_id: SshSyncLocalObjectId::Host(plan.hosts[0].host_id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_secret.clone(),
                        local_object_id: SshSyncLocalObjectId::Secret(secret_ref_id.clone()),
                    },
                ],
            )
            .unwrap();
        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: OperationId::new(),
            delta_sha256: "d".repeat(64),
            creates: None,
            identity_updates: Vec::new(),
            identity_deletes: vec![SshSyncOwnedIdentityDelete {
                portable_object_id: portable_identity,
                identity_id: plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
            }],
            credential_updates: Vec::new(),
            credential_deletes: vec![SshSyncOwnedCredentialDelete {
                portable_object_id: portable_credential,
                credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                expected_state_version: WireSequence::new(1),
            }],
            host_updates: Vec::new(),
            host_deletes: vec![SshSyncOwnedHostDelete {
                portable_object_id: portable_host,
                host_id: plan.hosts[0].host_id.clone(),
                expected: initial_ssh_sync_host_versions(),
            }],
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: vec![SshSyncOwnedSecretDelete {
                portable_object_id: portable_secret,
                secret_ref_id: secret_ref_id.clone(),
            }],
        };
        let mut wrong_owner = delta.clone();
        wrong_owner.owner.profile_id = "wrong-owner".to_owned();
        wrong_owner.attempt_id = OperationId::new();
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&wrong_owner),
            Err(AppPersistenceError::NotFound | AppPersistenceError::Conflict)
        ));
        let mut drift = delta.clone();
        drift.attempt_id = OperationId::new();
        drift.host_deletes[0].expected.route = WireSequence::new(2);
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&drift),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(repository.get_host(&plan.hosts[0].host_id).is_ok());

        let applied = repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        assert_eq!(applied.deleted_count, 4);
        assert_eq!(
            applied.pending_vault_gc_secret_ref_ids.as_slice(),
            std::slice::from_ref(&secret_ref_id)
        );
        assert!(matches!(
            repository.get_host(&plan.hosts[0].host_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(matches!(
            repository.get_ready_credential_record(&plan.credentials[0].credential_ref_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(matches!(
            repository.get_identity(&plan.identities[0].identity_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(
            repository
                .list_ssh_sync_object_mappings(&owner.key)
                .unwrap()
                .is_empty()
        );
        let replay = repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.deleted_count, 4);
        let queued = repository.list_ssh_sync_vault_gc(&owner.key).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(
            repository
                .ack_ssh_sync_vault_gc(&owner.key, std::slice::from_ref(&secret_ref_id))
                .unwrap(),
            1
        );
        assert_eq!(
            repository
                .ack_ssh_sync_vault_gc(&owner.key, std::slice::from_ref(&secret_ref_id))
                .unwrap(),
            0
        );
        assert!(
            repository
                .apply_ssh_sync_owned_metadata_delta(&delta)
                .unwrap()
                .pending_vault_gc_secret_ref_ids
                .is_empty()
        );
    }

    #[test]
    fn owned_delete_rejects_every_object_kind_mapped_by_another_profile() {
        for shared_kind in [
            SshSyncObjectKind::Identity,
            SshSyncObjectKind::Credential,
            SshSyncObjectKind::Host,
            SshSyncObjectKind::Secret,
        ] {
            let directory = tempfile::tempdir().expect("tempdir");
            let mut repository = repository(&directory);
            let (saga, plan) = ssh_sync_restore_fixture();
            repository.begin_ssh_sync_restore_saga(&saga).unwrap();
            repository.commit_ssh_sync_restore_plan(&plan).unwrap();
            let owner = ssh_sync_profile_fixture("shared-delete-owner");
            let other_owner = ssh_sync_profile_fixture("shared-delete-other");
            repository.ensure_ssh_sync_profile_state(&owner).unwrap();
            repository
                .ensure_ssh_sync_profile_state(&other_owner)
                .unwrap();
            let portable_identity = uuid::Uuid::new_v4().to_string();
            let portable_credential = uuid::Uuid::new_v4().to_string();
            let portable_host = uuid::Uuid::new_v4().to_string();
            let portable_secret = uuid::Uuid::new_v4().to_string();
            let secret_ref_id = saga.secret_ref_ids[0].clone();
            repository
                .resolve_or_create_ssh_sync_object_mappings(
                    &owner.key,
                    &[
                        SshSyncObjectMappingInput {
                            portable_object_id: portable_identity.clone(),
                            local_object_id: SshSyncLocalObjectId::Identity(
                                plan.identities[0].identity_id.clone(),
                            ),
                        },
                        SshSyncObjectMappingInput {
                            portable_object_id: portable_credential.clone(),
                            local_object_id: SshSyncLocalObjectId::Credential(
                                plan.credentials[0].credential_ref_id.clone(),
                            ),
                        },
                        SshSyncObjectMappingInput {
                            portable_object_id: portable_host.clone(),
                            local_object_id: SshSyncLocalObjectId::Host(
                                plan.hosts[0].host_id.clone(),
                            ),
                        },
                        SshSyncObjectMappingInput {
                            portable_object_id: portable_secret.clone(),
                            local_object_id: SshSyncLocalObjectId::Secret(secret_ref_id.clone()),
                        },
                    ],
                )
                .unwrap();
            let shared_local_object_id = match shared_kind {
                SshSyncObjectKind::Identity => {
                    SshSyncLocalObjectId::Identity(plan.identities[0].identity_id.clone())
                }
                SshSyncObjectKind::Credential => {
                    SshSyncLocalObjectId::Credential(plan.credentials[0].credential_ref_id.clone())
                }
                SshSyncObjectKind::Host => {
                    SshSyncLocalObjectId::Host(plan.hosts[0].host_id.clone())
                }
                SshSyncObjectKind::Secret => SshSyncLocalObjectId::Secret(secret_ref_id.clone()),
                SshSyncObjectKind::DesktopProfile => {
                    unreachable!("the test's shared-kind fixture excludes desktop profiles")
                }
            };
            repository
                .resolve_or_create_ssh_sync_object_mappings(
                    &other_owner.key,
                    &[SshSyncObjectMappingInput {
                        portable_object_id: uuid::Uuid::new_v4().to_string(),
                        local_object_id: shared_local_object_id,
                    }],
                )
                .unwrap();
            let delta = SshSyncOwnedMetadataDelta {
                owner: owner.key.clone(),
                attempt_id: OperationId::new(),
                delta_sha256: "7".repeat(64),
                creates: None,
                identity_updates: Vec::new(),
                identity_deletes: vec![SshSyncOwnedIdentityDelete {
                    portable_object_id: portable_identity,
                    identity_id: plan.identities[0].identity_id.clone(),
                    expected_state_version: WireSequence::new(1),
                }],
                credential_updates: Vec::new(),
                credential_deletes: vec![SshSyncOwnedCredentialDelete {
                    portable_object_id: portable_credential,
                    credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                    expected_state_version: WireSequence::new(1),
                }],
                host_updates: Vec::new(),
                host_deletes: vec![SshSyncOwnedHostDelete {
                    portable_object_id: portable_host,
                    host_id: plan.hosts[0].host_id.clone(),
                    expected: initial_ssh_sync_host_versions(),
                }],
                desktop_profile_updates: Vec::new(),
                desktop_profile_deletes: Vec::new(),
                secret_replacements: Vec::new(),
                secret_deletes: vec![SshSyncOwnedSecretDelete {
                    portable_object_id: portable_secret,
                    secret_ref_id: secret_ref_id.clone(),
                }],
            };

            assert!(matches!(
                repository.apply_ssh_sync_owned_metadata_delta(&delta),
                Err(AppPersistenceError::Conflict)
            ));
            assert!(repository.get_host(&plan.hosts[0].host_id).is_ok());
            assert!(
                repository
                    .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
                    .is_ok()
            );
            assert!(
                repository
                    .get_identity(&plan.identities[0].identity_id)
                    .is_ok()
            );
            assert_eq!(
                repository
                    .list_ssh_sync_object_mappings(&owner.key)
                    .unwrap()
                    .len(),
                4
            );
            assert_eq!(
                repository
                    .list_ssh_sync_object_mappings(&other_owner.key)
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                repository
                    .list_ssh_sync_vault_gc(&owner.key)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn owned_credential_delete_does_not_queue_secret_mapped_by_another_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let owner = ssh_sync_profile_fixture("shared-secret-owner");
        let other_owner = ssh_sync_profile_fixture("shared-secret-other");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        repository
            .ensure_ssh_sync_profile_state(&other_owner)
            .unwrap();
        let portable_identity = uuid::Uuid::new_v4().to_string();
        let portable_credential = uuid::Uuid::new_v4().to_string();
        let portable_host = uuid::Uuid::new_v4().to_string();
        let secret_ref_id = saga.secret_ref_ids[0].clone();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_identity.clone(),
                        local_object_id: SshSyncLocalObjectId::Identity(
                            plan.identities[0].identity_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_credential.clone(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            plan.credentials[0].credential_ref_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_host.clone(),
                        local_object_id: SshSyncLocalObjectId::Host(plan.hosts[0].host_id.clone()),
                    },
                ],
            )
            .unwrap();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &other_owner.key,
                &[SshSyncObjectMappingInput {
                    portable_object_id: uuid::Uuid::new_v4().to_string(),
                    local_object_id: SshSyncLocalObjectId::Secret(secret_ref_id.clone()),
                }],
            )
            .unwrap();
        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: OperationId::new(),
            delta_sha256: "6".repeat(64),
            creates: None,
            identity_updates: Vec::new(),
            identity_deletes: vec![SshSyncOwnedIdentityDelete {
                portable_object_id: portable_identity,
                identity_id: plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
            }],
            credential_updates: Vec::new(),
            credential_deletes: vec![SshSyncOwnedCredentialDelete {
                portable_object_id: portable_credential,
                credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                expected_state_version: WireSequence::new(1),
            }],
            host_updates: Vec::new(),
            host_deletes: vec![SshSyncOwnedHostDelete {
                portable_object_id: portable_host,
                host_id: plan.hosts[0].host_id.clone(),
                expected: initial_ssh_sync_host_versions(),
            }],
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        };

        let applied = repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        assert!(applied.pending_vault_gc_secret_ref_ids.is_empty());
        assert!(
            repository
                .list_ssh_sync_vault_gc(&owner.key)
                .unwrap()
                .is_empty()
        );
        let other_mappings = repository
            .list_ssh_sync_object_mappings(&other_owner.key)
            .unwrap();
        assert_eq!(other_mappings.len(), 1);
        assert_eq!(
            other_mappings[0].local_object_id.as_str(),
            secret_ref_id.as_str()
        );
    }

    #[test]
    fn owned_remote_credential_update_preserves_mapping_and_queues_replaced_secret() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let owner = ssh_sync_profile_fixture("credential-update");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable = uuid::Uuid::new_v4().to_string();
        let portable_identity = uuid::Uuid::new_v4().to_string();
        let portable_secret = uuid::Uuid::new_v4().to_string();
        let old_secret_ref_id = saga.secret_ref_ids[0].clone();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable.clone(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            plan.credentials[0].credential_ref_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_secret.clone(),
                        local_object_id: SshSyncLocalObjectId::Secret(old_secret_ref_id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_identity.clone(),
                        local_object_id: SshSyncLocalObjectId::Identity(
                            plan.identities[0].identity_id.clone(),
                        ),
                    },
                ],
            )
            .unwrap();
        let replacement = SecretRefId::new();
        let attempt_id = OperationId::new();
        let create_fence = SshSyncRestorePlan {
            attempt_id: attempt_id.clone(),
            plugin_id: owner.key.plugin_id.clone(),
            profile_id: owner.key.profile_id.clone(),
            bundle_sha256: "8".repeat(64),
            plan_sha256: "9".repeat(64),
            identities: Vec::new(),
            credentials: Vec::new(),
            hosts: Vec::new(),
            desktop_profiles: Vec::new(),
        };
        let staged_change_fence = repository.ssh_sync_change_fence().unwrap();
        let saga_begin = repository
            .begin_ssh_sync_restore_saga_with_fence(
                &SshSyncRestoreSagaInput {
                    attempt_id: attempt_id.clone(),
                    plugin_id: owner.key.plugin_id.clone(),
                    profile_id: owner.key.profile_id.clone(),
                    bundle_sha256: create_fence.bundle_sha256.clone(),
                    plan_sha256: create_fence.plan_sha256.clone(),
                    secret_ref_ids: vec![replacement.clone()],
                },
                &staged_change_fence,
            )
            .unwrap();
        assert!(saga_begin.begin.created);
        assert_ne!(saga_begin.change_fence, staged_change_fence);
        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id,
            delta_sha256: "c".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: create_fence,
                mappings: Vec::new(),
            }),
            identity_updates: Vec::new(),
            identity_deletes: Vec::new(),
            credential_updates: vec![SshSyncOwnedCredentialUpdate {
                portable_object_id: portable,
                credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                expected_state_version: WireSequence::new(1),
                identity_id: plan.identities[0].identity_id.clone(),
                priority: 0,
                label: "Remote updated password".to_owned(),
                material: SshSyncRestoreCredentialMaterial::Password {
                    secret_ref_id: replacement.clone(),
                },
            }],
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: vec![SshSyncOwnedSecretReplacement {
                portable_object_id: portable_secret.clone(),
                expected_secret_ref_id: old_secret_ref_id.clone(),
                new_secret_ref_id: replacement.clone(),
            }],
            secret_deletes: Vec::new(),
        };
        let result = repository
            .apply_ssh_sync_owned_metadata_delta_with_scope_memberships_and_fence(
                &delta,
                None,
                &saga_begin.change_fence,
            )
            .unwrap();
        assert_eq!(result.created_count, 0);
        assert_eq!(result.updated_count, 2);
        assert_eq!(
            result.pending_vault_gc_secret_ref_ids,
            vec![old_secret_ref_id.clone()]
        );
        let credential = repository
            .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
            .unwrap();
        assert_eq!(credential.state_version, WireSequence::new(2));
        assert!(matches!(
            credential.details,
            CredentialRecordDetails::Password { secret_ref_id } if secret_ref_id == replacement
        ));
        let mappings = repository
            .list_ssh_sync_object_mappings(&owner.key)
            .unwrap();
        assert_eq!(mappings.len(), 3);
        assert!(mappings.iter().any(|mapping| {
            mapping.object_kind == SshSyncObjectKind::Secret
                && mapping.portable_object_id == portable_secret
                && mapping.local_object_id.as_str() == replacement.as_str()
        }));
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&delta.attempt_id)
                .unwrap()
                .is_none()
        );
        assert!(
            repository
                .apply_ssh_sync_owned_metadata_delta(&delta)
                .unwrap()
                .replayed
        );

        let failed_new_ref = SecretRefId::new();
        let failed_attempt_id = OperationId::new();
        let failed_fence = SshSyncRestorePlan {
            attempt_id: failed_attempt_id.clone(),
            plugin_id: owner.key.plugin_id.clone(),
            profile_id: owner.key.profile_id.clone(),
            bundle_sha256: "a".repeat(64),
            plan_sha256: "d".repeat(64),
            identities: Vec::new(),
            credentials: Vec::new(),
            hosts: Vec::new(),
            desktop_profiles: Vec::new(),
        };
        repository
            .begin_ssh_sync_restore_saga(&SshSyncRestoreSagaInput {
                attempt_id: failed_attempt_id.clone(),
                plugin_id: owner.key.plugin_id.clone(),
                profile_id: owner.key.profile_id.clone(),
                bundle_sha256: failed_fence.bundle_sha256.clone(),
                plan_sha256: failed_fence.plan_sha256.clone(),
                secret_ref_ids: vec![failed_new_ref.clone()],
            })
            .unwrap();
        let failed_delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: failed_attempt_id.clone(),
            delta_sha256: "e".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: failed_fence,
                mappings: Vec::new(),
            }),
            identity_updates: Vec::new(),
            identity_deletes: vec![SshSyncOwnedIdentityDelete {
                portable_object_id: portable_identity,
                identity_id: plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
            }],
            credential_updates: vec![SshSyncOwnedCredentialUpdate {
                portable_object_id: delta.credential_updates[0].portable_object_id.clone(),
                credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                expected_state_version: WireSequence::new(2),
                identity_id: plan.identities[0].identity_id.clone(),
                priority: 0,
                label: "Must rollback replacement".to_owned(),
                material: SshSyncRestoreCredentialMaterial::Password {
                    secret_ref_id: failed_new_ref.clone(),
                },
            }],
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: vec![SshSyncOwnedSecretReplacement {
                portable_object_id: portable_secret.clone(),
                expected_secret_ref_id: replacement.clone(),
                new_secret_ref_id: failed_new_ref,
            }],
            secret_deletes: Vec::new(),
        };
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&failed_delta),
            Err(AppPersistenceError::Conflict)
        ));
        let credential = repository
            .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
            .unwrap();
        assert_eq!(credential.state_version, WireSequence::new(2));
        assert!(matches!(
            credential.details,
            CredentialRecordDetails::Password { secret_ref_id } if secret_ref_id == replacement
        ));
        assert!(
            repository
                .list_ssh_sync_object_mappings(&owner.key)
                .unwrap()
                .iter()
                .any(|mapping| {
                    mapping.portable_object_id == portable_secret
                        && mapping.local_object_id.as_str() == replacement.as_str()
                })
        );
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&failed_attempt_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn owned_mixed_create_update_delete_commits_or_rolls_back_as_one_unit() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (existing_saga, existing_plan) = ssh_sync_restore_fixture();
        repository
            .begin_ssh_sync_restore_saga(&existing_saga)
            .unwrap();
        repository
            .commit_ssh_sync_restore_plan(&existing_plan)
            .unwrap();
        let owner = ssh_sync_profile_fixture("mixed-owned-delta");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_existing_identity = uuid::Uuid::new_v4().to_string();
        let portable_existing_host = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_existing_identity.clone(),
                        local_object_id: SshSyncLocalObjectId::Identity(
                            existing_plan.identities[0].identity_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_existing_host.clone(),
                        local_object_id: SshSyncLocalObjectId::Host(
                            existing_plan.hosts[0].host_id.clone(),
                        ),
                    },
                ],
            )
            .unwrap();

        let (mut create_saga, mut create_plan) = ssh_sync_restore_fixture();
        create_saga.plugin_id = owner.key.plugin_id.clone();
        create_saga.profile_id = owner.key.profile_id.clone();
        create_saga.bundle_sha256 = "2".repeat(64);
        create_saga.plan_sha256 = "3".repeat(64);
        create_plan.attempt_id = create_saga.attempt_id.clone();
        create_plan.plugin_id = create_saga.plugin_id.clone();
        create_plan.profile_id = create_saga.profile_id.clone();
        create_plan.bundle_sha256 = create_saga.bundle_sha256.clone();
        create_plan.plan_sha256 = create_saga.plan_sha256.clone();
        create_plan.credentials[0].idempotency_key = "mixed-create-credential".to_owned();
        create_plan.hosts[0].request.idempotency_key = "mixed-create-host".to_owned();
        create_plan.hosts[0].request.address = "mixed-create.example".to_owned();
        let portable_created_identity = uuid::Uuid::new_v4().to_string();
        let portable_created_credential = uuid::Uuid::new_v4().to_string();
        let portable_created_host = uuid::Uuid::new_v4().to_string();
        let portable_created_secret = uuid::Uuid::new_v4().to_string();
        let create_mappings = vec![
            SshSyncObjectMappingInput {
                portable_object_id: portable_created_identity,
                local_object_id: SshSyncLocalObjectId::Identity(
                    create_plan.identities[0].identity_id.clone(),
                ),
            },
            SshSyncObjectMappingInput {
                portable_object_id: portable_created_credential,
                local_object_id: SshSyncLocalObjectId::Credential(
                    create_plan.credentials[0].credential_ref_id.clone(),
                ),
            },
            SshSyncObjectMappingInput {
                portable_object_id: portable_created_host,
                local_object_id: SshSyncLocalObjectId::Host(create_plan.hosts[0].host_id.clone()),
            },
            SshSyncObjectMappingInput {
                portable_object_id: portable_created_secret,
                local_object_id: SshSyncLocalObjectId::Secret(
                    create_saga.secret_ref_ids[0].clone(),
                ),
            },
        ];
        repository
            .begin_ssh_sync_restore_saga(&create_saga)
            .unwrap();
        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: create_saga.attempt_id.clone(),
            delta_sha256: "4".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: create_plan.clone(),
                mappings: create_mappings,
            }),
            identity_updates: vec![SshSyncOwnedIdentityUpdate {
                portable_object_id: portable_existing_identity.clone(),
                identity_id: existing_plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
                label: "Mixed updated identity".to_owned(),
                username: Some("updated-user".to_owned()),
            }],
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: vec![SshSyncOwnedHostDelete {
                portable_object_id: portable_existing_host,
                host_id: existing_plan.hosts[0].host_id.clone(),
                expected: initial_ssh_sync_host_versions(),
            }],
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        };
        let applied = repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        assert_eq!(applied.created_count, 3);
        assert_eq!(applied.updated_count, 1);
        assert_eq!(applied.deleted_count, 1);
        assert_eq!(
            repository
                .get_identity(&existing_plan.identities[0].identity_id)
                .unwrap()
                .label,
            "Mixed updated identity"
        );
        assert!(matches!(
            repository.get_host(&existing_plan.hosts[0].host_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(
            repository
                .get_ready_credential_record(&create_plan.credentials[0].credential_ref_id)
                .is_ok()
        );
        assert!(repository.get_host(&create_plan.hosts[0].host_id).is_ok());
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&create_saga.attempt_id)
                .unwrap()
                .is_none()
        );
        let replay = repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.created_count, 3);
        assert_eq!(replay.updated_count, 1);
        assert_eq!(replay.deleted_count, 1);

        let (mut rollback_saga, mut rollback_plan) = ssh_sync_restore_fixture();
        rollback_saga.plugin_id = owner.key.plugin_id.clone();
        rollback_saga.profile_id = owner.key.profile_id.clone();
        rollback_saga.bundle_sha256 = "5".repeat(64);
        rollback_saga.plan_sha256 = "6".repeat(64);
        rollback_plan.attempt_id = rollback_saga.attempt_id.clone();
        rollback_plan.plugin_id = rollback_saga.plugin_id.clone();
        rollback_plan.profile_id = rollback_saga.profile_id.clone();
        rollback_plan.bundle_sha256 = rollback_saga.bundle_sha256.clone();
        rollback_plan.plan_sha256 = rollback_saga.plan_sha256.clone();
        rollback_plan.credentials[0].idempotency_key = "mixed-rollback-credential".to_owned();
        rollback_plan.hosts[0].request.idempotency_key = "mixed-rollback-host".to_owned();
        rollback_plan.hosts[0].request.address = "mixed-rollback.example".to_owned();
        let rollback_host_id = rollback_plan.hosts[0].host_id.clone();
        let rollback_identity_id = rollback_plan.identities[0].identity_id.clone();
        let rollback_credential_id = rollback_plan.credentials[0].credential_ref_id.clone();
        let rollback_delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: rollback_saga.attempt_id.clone(),
            delta_sha256: "7".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                mappings: vec![
                    SshSyncObjectMappingInput {
                        portable_object_id: uuid::Uuid::new_v4().to_string(),
                        local_object_id: SshSyncLocalObjectId::Identity(
                            rollback_identity_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: uuid::Uuid::new_v4().to_string(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            rollback_credential_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: uuid::Uuid::new_v4().to_string(),
                        local_object_id: SshSyncLocalObjectId::Host(rollback_host_id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: uuid::Uuid::new_v4().to_string(),
                        local_object_id: SshSyncLocalObjectId::Secret(
                            rollback_saga.secret_ref_ids[0].clone(),
                        ),
                    },
                ],
                plan: rollback_plan,
            }),
            identity_updates: vec![SshSyncOwnedIdentityUpdate {
                portable_object_id: portable_existing_identity,
                identity_id: existing_plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
                label: "Must not persist".to_owned(),
                username: None,
            }],
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: vec![SshSyncOwnedHostDelete {
                portable_object_id: delta.creates.as_ref().unwrap().mappings[2]
                    .portable_object_id
                    .clone(),
                host_id: create_plan.hosts[0].host_id.clone(),
                expected: initial_ssh_sync_host_versions(),
            }],
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        };
        repository
            .begin_ssh_sync_restore_saga(&rollback_saga)
            .unwrap();
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&rollback_delta),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .get_identity(&existing_plan.identities[0].identity_id)
                .unwrap()
                .label,
            "Mixed updated identity"
        );
        assert!(repository.get_host(&create_plan.hosts[0].host_id).is_ok());
        assert!(matches!(
            repository.get_host(&rollback_host_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(matches!(
            repository.get_identity(&rollback_identity_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(matches!(
            repository.get_ready_credential_record(&rollback_credential_id),
            Err(AppPersistenceError::NotFound)
        ));
        assert!(
            repository
                .get_pending_ssh_sync_restore_saga(&rollback_saga.attempt_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn sync_host_tags_create_reuse_update_and_rollback_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let reused = repository.create_host_tag("Production").unwrap();
        let (saga, mut plan) = ssh_sync_restore_fixture();
        plan.hosts[0].tag_labels = vec!["production".to_owned(), "Linux".to_owned()];
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let organization = repository
            .get_host_organization(&plan.hosts[0].host_id)
            .unwrap();
        assert_eq!(organization.tag_ids.len(), 2);
        assert!(organization.tag_ids.contains(&reused.tag_id));

        let owner = ssh_sync_profile_fixture("tag-update");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_host = uuid::Uuid::new_v4().to_string();
        let portable_identity = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_host.clone(),
                        local_object_id: SshSyncLocalObjectId::Host(plan.hosts[0].host_id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_identity.clone(),
                        local_object_id: SshSyncLocalObjectId::Identity(
                            plan.identities[0].identity_id.clone(),
                        ),
                    },
                ],
            )
            .unwrap();
        let mut desired = plan.hosts[0].request.clone();
        desired.label = "Updated tagged Host".to_owned();
        let update = SshSyncOwnedHostUpdate {
            portable_object_id: portable_host.clone(),
            host_id: plan.hosts[0].host_id.clone(),
            expected: SshSyncOwnedHostBaseVersions {
                host: WireSequence::new(1),
                route: WireSequence::new(1),
                authentication: WireSequence::new(1),
                algorithm: WireSequence::new(1),
                heartbeat: WireSequence::new(1),
                monitoring: WireSequence::new(1),
                login_automation: WireSequence::new(1),
            },
            desired,
            tag_labels: vec!["Updated".to_owned()],
            login_automation: SshSyncLoginAutomation::disabled(),
        };
        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: OperationId::new(),
            delta_sha256: "e".repeat(64),
            creates: None,
            identity_updates: Vec::new(),
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: vec![update.clone()],
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        };
        repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        assert_eq!(
            repository.get_host(&update.host_id).unwrap().label,
            "Updated tagged Host"
        );

        let mut rollback_update = update;
        rollback_update.expected = SshSyncOwnedHostBaseVersions {
            host: WireSequence::new(2),
            route: WireSequence::new(2),
            authentication: WireSequence::new(2),
            algorithm: WireSequence::new(2),
            heartbeat: WireSequence::new(2),
            monitoring: WireSequence::new(2),
            login_automation: WireSequence::new(2),
        };
        rollback_update.desired.label = "Must rollback".to_owned();
        rollback_update.tag_labels = vec!["MustRollbackTag".to_owned()];
        let rollback = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: OperationId::new(),
            delta_sha256: "f".repeat(64),
            creates: None,
            identity_updates: Vec::new(),
            identity_deletes: vec![SshSyncOwnedIdentityDelete {
                portable_object_id: portable_identity,
                identity_id: plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
            }],
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: vec![rollback_update],
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        };
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&rollback),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository.get_host(&plan.hosts[0].host_id).unwrap().label,
            "Updated tagged Host"
        );
        assert!(
            !repository
                .list_host_tags()
                .unwrap()
                .iter()
                .any(|tag| tag.label == "MustRollbackTag")
        );
    }

    #[test]
    fn v25_plugin_delete_is_durable_chunked_owner_scoped_and_atomic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let input = ssh_sync_profile_fixture("delete-saga");
        let sync_key = SecretRefId::new();
        let gc_ref = SecretRefId::new();
        let delete_operation = OperationId::new();
        {
            let mut repository = AppRepository::open(&database_path).unwrap();
            let profile = repository.ensure_ssh_sync_profile_state(&input).unwrap();
            repository
                .bind_ssh_sync_profile_key(
                    &input.key,
                    profile.state_version,
                    &SshSyncProfileKeyBinding {
                        sync_key_secret_ref_id: sync_key.clone(),
                        password_wrapped_sync_key_envelope: Some(vec![7; 32]),
                    },
                )
                .unwrap();
            repository
                .connection
                .execute(
                    "INSERT INTO ssh_sync_vault_gc_queue
                     (plugin_id, signer_fingerprint_sha256, profile_id, secret_ref_id,
                      reason, attempt_id, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, 'secret_deleted', ?5, 1)",
                    params![
                        input.key.plugin_id.as_str(),
                        input.key.signer_fingerprint_sha256,
                        input.key.profile_id,
                        gc_ref.as_str(),
                        OperationId::new().as_str()
                    ],
                )
                .unwrap();
            let begun = repository
                .begin_ssh_sync_plugin_delete(&input.key.plugin_id, &delete_operation)
                .unwrap();
            assert_eq!(begun.tasks.len(), 1);
            assert_eq!(begun.tasks[0].secret_ref_ids.len(), 2);
            assert!(
                repository
                    .get_ssh_sync_profile_state(&input.key)
                    .unwrap()
                    .is_none()
            );
            assert!(!format!("{:?}", begun.tasks[0]).contains(sync_key.as_str()));
        }

        let mut repository = AppRepository::open(&database_path).unwrap();
        let replay = repository
            .begin_ssh_sync_plugin_delete(&input.key.plugin_id, &delete_operation)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.tasks[0].secret_ref_ids.len(), 2);
        let wrong_owner = SshSyncProfileStateKey {
            signer_fingerprint_sha256: "2".repeat(64),
            ..input.key.clone()
        };
        assert!(matches!(
            repository.ack_ssh_sync_plugin_delete_refs(
                &wrong_owner,
                &delete_operation,
                std::slice::from_ref(&gc_ref)
            ),
            Err(AppPersistenceError::NotFound)
        ));
        let first_ack = repository
            .ack_ssh_sync_plugin_delete_refs(
                &input.key,
                &delete_operation,
                std::slice::from_ref(&gc_ref),
            )
            .unwrap();
        assert!(!first_ack.profile_finalized);
        repository
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_sync_profile_delete
                 BEFORE DELETE ON ssh_sync_profile_states
                 BEGIN SELECT RAISE(ABORT, 'injected profile delete failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            repository.ack_ssh_sync_plugin_delete_refs(
                &input.key,
                &delete_operation,
                std::slice::from_ref(&sync_key)
            ),
            Err(AppPersistenceError::Database(_))
        ));
        assert_eq!(
            repository.list_pending_ssh_sync_plugin_deletes().unwrap()[0]
                .secret_ref_ids
                .as_slice(),
            std::slice::from_ref(&sync_key)
        );
        repository
            .connection
            .execute_batch("DROP TRIGGER fail_sync_profile_delete")
            .unwrap();
        let final_ack = repository
            .ack_ssh_sync_plugin_delete_refs(
                &input.key,
                &delete_operation,
                std::slice::from_ref(&sync_key),
            )
            .unwrap();
        assert!(final_ack.profile_finalized);
        assert!(final_ack.operation_completed);
        assert!(
            repository
                .list_pending_ssh_sync_plugin_deletes()
                .unwrap()
                .is_empty()
        );
        let completed_replay = repository
            .begin_ssh_sync_plugin_delete(&input.key.plugin_id, &delete_operation)
            .unwrap();
        assert!(completed_replay.completed);
        assert!(completed_replay.replayed);
    }

    #[test]
    fn ssh_sync_login_automation_create_update_and_rollback_are_atomic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (mut saga, mut plan) = ssh_sync_restore_fixture();
        let first_automation_ref = SecretRefId::new();
        plan.hosts[0].login_automation = SshSyncLoginAutomation {
            enabled: true,
            confirmed: true,
            steps: vec![
                SshSyncLoginAutomationStep::Expect {
                    literal_text: "Password:".to_owned(),
                    timeout_seconds: 10,
                },
                SshSyncLoginAutomationStep::SendText {
                    text: "echo ready".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                },
                SshSyncLoginAutomationStep::SendSecret {
                    secret_ref_id: first_automation_ref.clone(),
                    secret_label: "Remote password".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                },
                SshSyncLoginAutomationStep::SendSecret {
                    secret_ref_id: first_automation_ref.clone(),
                    secret_label: "Remote password again".to_owned(),
                    append_enter: false,
                    timeout_seconds: 10,
                },
            ],
        };
        saga.secret_ref_ids.push(first_automation_ref.clone());
        saga.secret_ref_ids
            .sort_by(|left, right| left.as_str().cmp(right.as_str()));
        let preview = repository.preview_ssh_sync_restore_plan(&plan).unwrap();
        assert!(preview.new_secret_ref_ids.contains(&first_automation_ref));
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let created = repository
            .get_login_automation(&plan.hosts[0].host_id)
            .unwrap();
        assert!(created.enabled);
        assert_eq!(created.confirmed_revision, Some(WireSequence::new(1)));
        assert_eq!(created.steps.len(), 4);
        let stored_refs: Vec<String> = repository
            .connection
            .prepare(
                "SELECT secret_ref_id FROM host_login_automation_steps
                 WHERE host_id = ?1 AND kind = 'send_secret' ORDER BY ordinal",
            )
            .unwrap()
            .query_map([plan.hosts[0].host_id.as_str()], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(stored_refs, [first_automation_ref.as_str(); 2]);

        let owner = ssh_sync_profile_fixture("automation-update");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_host = uuid::Uuid::new_v4().to_string();
        let portable_secret = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_host.clone(),
                        local_object_id: SshSyncLocalObjectId::Host(plan.hosts[0].host_id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_secret.clone(),
                        local_object_id: SshSyncLocalObjectId::Secret(first_automation_ref.clone()),
                    },
                ],
            )
            .unwrap();
        let second_automation_ref = SecretRefId::new();
        let update_attempt = OperationId::new();
        let update_fence = SshSyncRestorePlan {
            attempt_id: update_attempt.clone(),
            plugin_id: owner.key.plugin_id.clone(),
            profile_id: owner.key.profile_id.clone(),
            bundle_sha256: "1".repeat(64),
            plan_sha256: "2".repeat(64),
            identities: Vec::new(),
            credentials: Vec::new(),
            hosts: Vec::new(),
            desktop_profiles: Vec::new(),
        };
        repository
            .begin_ssh_sync_restore_saga(&SshSyncRestoreSagaInput {
                attempt_id: update_attempt.clone(),
                plugin_id: owner.key.plugin_id.clone(),
                profile_id: owner.key.profile_id.clone(),
                bundle_sha256: update_fence.bundle_sha256.clone(),
                plan_sha256: update_fence.plan_sha256.clone(),
                secret_ref_ids: vec![second_automation_ref.clone()],
            })
            .unwrap();
        let mut desired = plan.hosts[0].request.clone();
        desired.label = "Automation updated Host".to_owned();
        let update_delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: update_attempt,
            delta_sha256: "3".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: update_fence,
                mappings: Vec::new(),
            }),
            identity_updates: Vec::new(),
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: vec![SshSyncOwnedHostUpdate {
                portable_object_id: portable_host.clone(),
                host_id: plan.hosts[0].host_id.clone(),
                expected: initial_ssh_sync_host_versions(),
                desired,
                tag_labels: Vec::new(),
                login_automation: SshSyncLoginAutomation {
                    enabled: true,
                    confirmed: true,
                    steps: vec![
                        SshSyncLoginAutomationStep::SendSecret {
                            secret_ref_id: second_automation_ref.clone(),
                            secret_label: "Updated secret".to_owned(),
                            append_enter: false,
                            timeout_seconds: 20,
                        },
                        SshSyncLoginAutomationStep::SendSecret {
                            secret_ref_id: second_automation_ref.clone(),
                            secret_label: "Updated secret again".to_owned(),
                            append_enter: true,
                            timeout_seconds: 20,
                        },
                    ],
                },
            }],
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: vec![SshSyncOwnedSecretReplacement {
                portable_object_id: portable_secret.clone(),
                expected_secret_ref_id: first_automation_ref.clone(),
                new_secret_ref_id: second_automation_ref.clone(),
            }],
            secret_deletes: Vec::new(),
        };
        repository
            .apply_ssh_sync_owned_metadata_delta(&update_delta)
            .unwrap();
        let updated = repository
            .get_login_automation(&plan.hosts[0].host_id)
            .unwrap();
        assert_eq!(updated.revision, WireSequence::new(2));
        assert_eq!(updated.confirmed_revision, Some(WireSequence::new(2)));
        assert_eq!(
            repository.list_ssh_sync_vault_gc(&owner.key).unwrap()[0].secret_ref_id,
            first_automation_ref
        );

        let third_automation_ref = SecretRefId::new();
        let rollback_attempt = OperationId::new();
        let rollback_fence = SshSyncRestorePlan {
            attempt_id: rollback_attempt.clone(),
            plugin_id: owner.key.plugin_id.clone(),
            profile_id: owner.key.profile_id.clone(),
            bundle_sha256: "4".repeat(64),
            plan_sha256: "5".repeat(64),
            identities: Vec::new(),
            credentials: Vec::new(),
            hosts: Vec::new(),
            desktop_profiles: Vec::new(),
        };
        repository
            .begin_ssh_sync_restore_saga(&SshSyncRestoreSagaInput {
                attempt_id: rollback_attempt.clone(),
                plugin_id: owner.key.plugin_id.clone(),
                profile_id: owner.key.profile_id.clone(),
                bundle_sha256: rollback_fence.bundle_sha256.clone(),
                plan_sha256: rollback_fence.plan_sha256.clone(),
                secret_ref_ids: vec![third_automation_ref.clone()],
            })
            .unwrap();
        let mut rollback_desired = plan.hosts[0].request.clone();
        rollback_desired.label = "Must rollback Host".to_owned();
        let rollback_delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: rollback_attempt,
            delta_sha256: "6".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: rollback_fence,
                mappings: Vec::new(),
            }),
            identity_updates: Vec::new(),
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: vec![SshSyncOwnedHostUpdate {
                portable_object_id: portable_host,
                host_id: plan.hosts[0].host_id.clone(),
                expected: SshSyncOwnedHostBaseVersions {
                    host: WireSequence::new(2),
                    route: WireSequence::new(2),
                    authentication: WireSequence::new(2),
                    algorithm: WireSequence::new(2),
                    heartbeat: WireSequence::new(2),
                    monitoring: WireSequence::new(2),
                    login_automation: WireSequence::new(2),
                },
                desired: rollback_desired,
                tag_labels: Vec::new(),
                login_automation: SshSyncLoginAutomation {
                    enabled: true,
                    confirmed: false,
                    steps: vec![SshSyncLoginAutomationStep::SendSecret {
                        secret_ref_id: third_automation_ref.clone(),
                        secret_label: "Must rollback".to_owned(),
                        append_enter: true,
                        timeout_seconds: 10,
                    }],
                },
            }],
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: vec![SshSyncOwnedSecretReplacement {
                portable_object_id: portable_secret,
                expected_secret_ref_id: second_automation_ref.clone(),
                new_secret_ref_id: third_automation_ref,
            }],
            secret_deletes: Vec::new(),
        };
        repository
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_sync_automation_insert
                 BEFORE INSERT ON host_login_automation_steps
                 BEGIN SELECT RAISE(ABORT, 'injected automation failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&rollback_delta),
            Err(AppPersistenceError::Database(_))
        ));
        assert_eq!(
            repository.get_host(&plan.hosts[0].host_id).unwrap().label,
            "Automation updated Host"
        );
        let still_updated = repository
            .get_login_automation(&plan.hosts[0].host_id)
            .unwrap();
        assert_eq!(still_updated.revision, WireSequence::new(2));
        let stored_ref: String = repository
            .connection
            .query_row(
                "SELECT secret_ref_id FROM host_login_automation_steps
                 WHERE host_id = ?1 AND kind = 'send_secret'",
                [plan.hosts[0].host_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_ref, second_automation_ref.as_str());
    }

    #[test]
    fn ssh_sync_shared_password_restore_and_replacement_migrate_all_owned_references() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, mut plan) = ssh_sync_restore_fixture();
        let shared_secret = saga.secret_ref_ids[0].clone();
        let second_credential_id = CredentialRefId::new();
        plan.credentials.push(SshSyncRestoreCredentialInput {
            credential_ref_id: second_credential_id.clone(),
            identity_id: plan.identities[0].identity_id.clone(),
            operation_id: OperationId::new(),
            idempotency_key: "shared-password-second-credential".to_owned(),
            priority: 1,
            label: "Shared restored password".to_owned(),
            material: SshSyncRestoreCredentialMaterial::Password {
                secret_ref_id: shared_secret.clone(),
            },
        });
        let preview = repository.preview_ssh_sync_restore_plan(&plan).unwrap();
        assert_eq!(
            preview.new_secret_ref_ids.as_slice(),
            std::slice::from_ref(&shared_secret)
        );
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        assert!(matches!(
            repository
                .get_ready_credential_record(&second_credential_id)
                .unwrap()
                .details,
            CredentialRecordDetails::Password { secret_ref_id } if secret_ref_id == shared_secret
        ));

        let owner = ssh_sync_profile_fixture("shared-password");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let first_portable = uuid::Uuid::new_v4().to_string();
        let second_portable = uuid::Uuid::new_v4().to_string();
        let secret_portable = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: first_portable.clone(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            plan.credentials[0].credential_ref_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: second_portable.clone(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            second_credential_id.clone(),
                        ),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: secret_portable.clone(),
                        local_object_id: SshSyncLocalObjectId::Secret(shared_secret.clone()),
                    },
                ],
            )
            .unwrap();

        let replacement = SecretRefId::new();
        let attempt_id = OperationId::new();
        let fence = SshSyncRestorePlan {
            attempt_id: attempt_id.clone(),
            plugin_id: owner.key.plugin_id.clone(),
            profile_id: owner.key.profile_id.clone(),
            bundle_sha256: "7".repeat(64),
            plan_sha256: "8".repeat(64),
            identities: Vec::new(),
            credentials: Vec::new(),
            hosts: Vec::new(),
            desktop_profiles: Vec::new(),
        };
        repository
            .begin_ssh_sync_restore_saga(&SshSyncRestoreSagaInput {
                attempt_id: attempt_id.clone(),
                plugin_id: owner.key.plugin_id.clone(),
                profile_id: owner.key.profile_id.clone(),
                bundle_sha256: fence.bundle_sha256.clone(),
                plan_sha256: fence.plan_sha256.clone(),
                secret_ref_ids: vec![replacement.clone(), replacement.clone()],
            })
            .unwrap();
        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id,
            delta_sha256: "9".repeat(64),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: fence,
                mappings: Vec::new(),
            }),
            identity_updates: Vec::new(),
            identity_deletes: Vec::new(),
            credential_updates: vec![
                SshSyncOwnedCredentialUpdate {
                    portable_object_id: first_portable,
                    credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                    expected_state_version: WireSequence::new(1),
                    identity_id: plan.identities[0].identity_id.clone(),
                    priority: 0,
                    label: "Shared password first".to_owned(),
                    material: SshSyncRestoreCredentialMaterial::Password {
                        secret_ref_id: replacement.clone(),
                    },
                },
                SshSyncOwnedCredentialUpdate {
                    portable_object_id: second_portable,
                    credential_ref_id: second_credential_id.clone(),
                    expected_state_version: WireSequence::new(1),
                    identity_id: plan.identities[0].identity_id.clone(),
                    priority: 1,
                    label: "Shared password second".to_owned(),
                    material: SshSyncRestoreCredentialMaterial::Password {
                        secret_ref_id: replacement.clone(),
                    },
                },
            ],
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: vec![SshSyncOwnedSecretReplacement {
                portable_object_id: secret_portable,
                expected_secret_ref_id: shared_secret.clone(),
                new_secret_ref_id: replacement.clone(),
            }],
            secret_deletes: Vec::new(),
        };
        let mut partial_delta = delta.clone();
        partial_delta.delta_sha256 = "a".repeat(64);
        partial_delta.credential_updates.pop();
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&partial_delta),
            Err(AppPersistenceError::Conflict)
        ));
        repository
            .apply_ssh_sync_owned_metadata_delta(&delta)
            .unwrap();
        for credential_id in [
            &plan.credentials[0].credential_ref_id,
            &second_credential_id,
        ] {
            assert!(matches!(
                repository
                    .get_ready_credential_record(credential_id)
                    .unwrap()
                    .details,
                CredentialRecordDetails::Password { secret_ref_id }
                    if secret_ref_id == replacement
            ));
        }
        assert_eq!(
            repository.list_ssh_sync_vault_gc(&owner.key).unwrap()[0].secret_ref_id,
            shared_secret
        );
    }

    #[test]
    fn v26_upload_attempt_binds_destination_body_and_completion_proof() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let first = ssh_sync_profile_fixture("upload-first");
        let mut second = ssh_sync_profile_fixture("upload-second");
        second.key.signer_fingerprint_sha256 = "2".repeat(64);
        let idempotency_key = uuid::Uuid::new_v4().to_string();
        let input = SshSyncHttpUploadAttemptInput {
            owner: first.key.clone(),
            canonical_url: "https://api.norixor.org/apps/norishell/sync/exchange".to_owned(),
            http_method: SshSyncHttpMethod::Put,
            use_oauth: true,
            authorization_revision: WireSequence::new(3),
            configuration_revision: WireSequence::new(7),
            base_revision: 0,
            base_etag: None,
            target_revision: 1,
            keyed_content_sha256: "a".repeat(64),
            body_sha256: "b".repeat(64),
            idempotency_key: idempotency_key.clone(),
        };
        {
            let mut repository = AppRepository::open(&database_path).unwrap();
            repository.ensure_ssh_sync_profile_state(&first).unwrap();
            repository.ensure_ssh_sync_profile_state(&second).unwrap();
            let created = repository
                .ensure_ssh_sync_http_upload_attempt(&input)
                .unwrap();
            assert_eq!(created.state, SshSyncHttpUploadAttemptState::Prepared);
            assert!(!format!("{created:?}").contains(&idempotency_key));
        }
        let mut repository = AppRepository::open(&database_path).unwrap();
        let mut restart_retry = input.clone();
        restart_retry.idempotency_key = uuid::Uuid::new_v4().to_string();
        let replay = repository
            .ensure_ssh_sync_http_upload_attempt(&restart_retry)
            .unwrap();
        assert_eq!(replay.input.idempotency_key, idempotency_key);
        let mut drift = input.clone();
        drift.body_sha256 = "c".repeat(64);
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut destination_drift = input.clone();
        destination_drift.canonical_url =
            "https://other.example/apps/norishell/sync/exchange".to_owned();
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&destination_drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut authorization_drift = input.clone();
        authorization_drift.authorization_revision = WireSequence::new(4);
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&authorization_drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut method_drift = input.clone();
        method_drift.http_method = SshSyncHttpMethod::Post;
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&method_drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut oauth_drift = input.clone();
        oauth_drift.use_oauth = false;
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&oauth_drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut configuration_drift = input.clone();
        configuration_drift.configuration_revision = WireSequence::new(8);
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&configuration_drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut target_drift = input.clone();
        target_drift.target_revision = 2;
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&target_drift),
            Err(AppPersistenceError::Conflict)
        ));
        let mut stolen = input.clone();
        stolen.owner = second.key.clone();
        assert!(matches!(
            repository.ensure_ssh_sync_http_upload_attempt(&stolen),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        assert!(matches!(
            repository.advance_ssh_sync_http_upload_attempt(
                &first.key,
                WireSequence::new(2),
                SshSyncHttpUploadAttemptState::Sent
            ),
            Err(AppPersistenceError::Conflict)
        ));
        let sent = repository
            .advance_ssh_sync_http_upload_attempt(
                &first.key,
                replay.state_version,
                SshSyncHttpUploadAttemptState::Sent,
            )
            .unwrap();
        assert_eq!(sent.state_version, WireSequence::new(2));
        let completion_fence = SshSyncHttpUploadCompletionFence {
            canonical_url: input.canonical_url.clone(),
            http_method: input.http_method,
            use_oauth: input.use_oauth,
            authorization_revision: input.authorization_revision,
            configuration_revision: input.configuration_revision,
        };
        let completion_fence_drifts = [
            SshSyncHttpUploadCompletionFence {
                canonical_url: "https://other.example/apps/norishell/sync/exchange".to_owned(),
                ..completion_fence.clone()
            },
            SshSyncHttpUploadCompletionFence {
                authorization_revision: WireSequence::new(4),
                ..completion_fence.clone()
            },
            SshSyncHttpUploadCompletionFence {
                configuration_revision: WireSequence::new(8),
                ..completion_fence.clone()
            },
        ];
        for drifted_fence in &completion_fence_drifts {
            for proof in [
                SshSyncHttpUploadCompletionProof::ExactBodyObserved {
                    observed_revision: input.target_revision,
                    observed_body_sha256: input.body_sha256.clone(),
                },
                SshSyncHttpUploadCompletionProof::SupersededByNewerRemote {
                    authenticated_remote_revision: input.target_revision + 1,
                },
            ] {
                assert!(matches!(
                    repository.complete_ssh_sync_http_upload_attempt(
                        &first.key,
                        &idempotency_key,
                        drifted_fence,
                        &proof,
                    ),
                    Err(AppPersistenceError::Conflict)
                ));
                assert!(
                    repository
                        .get_ssh_sync_http_upload_attempt(&first.key)
                        .unwrap()
                        .is_some()
                );
            }
        }
        assert!(matches!(
            repository.complete_ssh_sync_http_upload_attempt(
                &first.key,
                &idempotency_key,
                &completion_fence,
                &SshSyncHttpUploadCompletionProof::ExactBodyObserved {
                    observed_revision: input.target_revision,
                    observed_body_sha256: "c".repeat(64),
                }
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .complete_ssh_sync_http_upload_attempt(
                    &first.key,
                    &idempotency_key,
                    &completion_fence,
                    &SshSyncHttpUploadCompletionProof::ExactBodyObserved {
                        observed_revision: input.target_revision,
                        observed_body_sha256: input.body_sha256.clone(),
                    },
                )
                .unwrap(),
            1
        );
        assert_eq!(
            repository
                .complete_ssh_sync_http_upload_attempt(
                    &first.key,
                    &idempotency_key,
                    &completion_fence,
                    &SshSyncHttpUploadCompletionProof::ExactBodyObserved {
                        observed_revision: input.target_revision,
                        observed_body_sha256: input.body_sha256.clone(),
                    },
                )
                .unwrap(),
            0
        );

        let superseded = SshSyncHttpUploadAttemptInput {
            owner: second.key.clone(),
            canonical_url: input.canonical_url.clone(),
            http_method: SshSyncHttpMethod::Put,
            use_oauth: true,
            authorization_revision: WireSequence::new(3),
            configuration_revision: WireSequence::new(7),
            base_revision: 4,
            base_etag: Some("\"revision-4\"".to_owned()),
            target_revision: 5,
            keyed_content_sha256: "d".repeat(64),
            body_sha256: "e".repeat(64),
            idempotency_key: uuid::Uuid::new_v4().to_string(),
        };
        repository
            .ensure_ssh_sync_http_upload_attempt(&superseded)
            .unwrap();
        let superseded_fence = SshSyncHttpUploadCompletionFence {
            canonical_url: superseded.canonical_url.clone(),
            http_method: superseded.http_method,
            use_oauth: superseded.use_oauth,
            authorization_revision: superseded.authorization_revision,
            configuration_revision: superseded.configuration_revision,
        };
        assert!(matches!(
            repository.complete_ssh_sync_http_upload_attempt(
                &second.key,
                &superseded.idempotency_key,
                &superseded_fence,
                &SshSyncHttpUploadCompletionProof::SupersededByNewerRemote {
                    authenticated_remote_revision: superseded.target_revision,
                }
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .complete_ssh_sync_http_upload_attempt(
                    &second.key,
                    &superseded.idempotency_key,
                    &superseded_fence,
                    &SshSyncHttpUploadCompletionProof::SupersededByNewerRemote {
                        authenticated_remote_revision: superseded.target_revision + 1,
                    },
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn v26_migration_discards_unbound_upload_attempt_and_preserves_secret_slots() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let owner = ssh_sync_profile_fixture("v26-migration");
        let (saga, plan) = ssh_sync_restore_fixture();
        let secret_ref = saga.secret_ref_ids[0].clone();
        {
            let mut repository = AppRepository::open(&database_path).unwrap();
            repository.begin_ssh_sync_restore_saga(&saga).unwrap();
            repository.commit_ssh_sync_restore_plan(&plan).unwrap();
            repository.ensure_ssh_sync_profile_state(&owner).unwrap();
            repository
                .ensure_ssh_sync_http_upload_attempt(&SshSyncHttpUploadAttemptInput {
                    owner: owner.key.clone(),
                    canonical_url: "https://api.norixor.org/apps/norishell/sync/exchange"
                        .to_owned(),
                    http_method: SshSyncHttpMethod::Put,
                    use_oauth: true,
                    authorization_revision: WireSequence::new(1),
                    configuration_revision: WireSequence::new(1),
                    base_revision: 0,
                    base_etag: None,
                    target_revision: 1,
                    keyed_content_sha256: "a".repeat(64),
                    body_sha256: "b".repeat(64),
                    idempotency_key: uuid::Uuid::new_v4().to_string(),
                })
                .unwrap();
            super::remove_schema_added_after_fixture_version(&repository.connection, 25).unwrap();
            repository
                .connection
                .execute_batch("DROP TABLE desktop_profiles; ALTER TABLE plugin_catalog_entries DROP COLUMN release_details_json; DROP TABLE plugin_settings;")
                .unwrap();
            repository
                .connection
                .pragma_update(None, "user_version", 25)
                .unwrap();
        }
        let repository = AppRepository::open(&database_path).unwrap();
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);
        assert!(
            repository
                .get_ssh_sync_http_upload_attempt(&owner.key)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            repository
                .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
                .unwrap()
                .details,
            CredentialRecordDetails::Password { secret_ref_id: stored } if stored == secret_ref
        ));
        let slot_schema: String = repository
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'credential_secret_slots'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!slot_schema.contains("secret_ref_id TEXT NOT NULL UNIQUE"));
    }

    #[test]
    fn staged_scope_replace_rejects_unrelated_host_write_without_side_effects() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut first_connection = repository(&directory);
        let mut other_connection = repository(&directory);
        let owner = ssh_sync_profile_fixture("scope-change-fence");
        let profile = first_connection
            .ensure_ssh_sync_profile_state(&owner)
            .unwrap();
        let initial = vec![SshSyncScopeMembershipInput {
            object_kind: SshSyncObjectKind::Host,
            portable_object_id: uuid::Uuid::new_v4().to_string(),
            state: SshSyncScopeMembershipState::Included,
        }];
        let profile = first_connection
            .replace_ssh_sync_scope_memberships(&owner.key, profile.state_version, &initial)
            .unwrap();
        let staged_fence = first_connection.ssh_sync_change_fence().unwrap();
        other_connection
            .create_host_configured(&configured_host_request(OperationId::new()))
            .expect("concurrent unrelated Host write");

        assert!(matches!(
            first_connection.replace_ssh_sync_scope_memberships_with_fence(
                &owner.key,
                profile.state_version,
                &[],
                &staged_fence,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            first_connection
                .get_ssh_sync_profile_state(&owner.key)
                .unwrap()
                .unwrap()
                .state_version,
            profile.state_version
        );
        assert_eq!(
            first_connection
                .list_ssh_sync_scope_memberships(&owner.key)
                .unwrap()
                .into_iter()
                .map(|record| record.membership)
                .collect::<Vec<_>>(),
            initial
        );

        let refreshed_fence = first_connection.ssh_sync_change_fence().unwrap();
        let replaced = first_connection
            .replace_ssh_sync_scope_memberships_with_fence(
                &owner.key,
                profile.state_version,
                &[],
                &refreshed_fence,
            )
            .unwrap();
        assert_eq!(replaced.state_version, WireSequence::new(3));
        assert!(
            first_connection
                .list_ssh_sync_scope_memberships(&owner.key)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn v25_scope_omission_and_owned_delta_membership_replace_are_atomic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, plan) = ssh_sync_restore_fixture();
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        let owner = ssh_sync_profile_fixture("scope-membership");
        let profile = repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_identity = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[SshSyncObjectMappingInput {
                    portable_object_id: portable_identity.clone(),
                    local_object_id: SshSyncLocalObjectId::Identity(
                        plan.identities[0].identity_id.clone(),
                    ),
                }],
            )
            .unwrap();
        let initial = vec![SshSyncScopeMembershipInput {
            object_kind: SshSyncObjectKind::Identity,
            portable_object_id: portable_identity.clone(),
            state: SshSyncScopeMembershipState::Included,
        }];
        let profile = repository
            .replace_ssh_sync_scope_memberships(&owner.key, profile.state_version, &initial)
            .unwrap();
        assert_eq!(profile.state_version, WireSequence::new(2));

        let delta = SshSyncOwnedMetadataDelta {
            owner: owner.key.clone(),
            attempt_id: OperationId::new(),
            delta_sha256: "9".repeat(64),
            creates: None,
            identity_updates: vec![SshSyncOwnedIdentityUpdate {
                portable_object_id: portable_identity,
                identity_id: plan.identities[0].identity_id.clone(),
                expected_state_version: WireSequence::new(1),
                label: "Must commit with omission".to_owned(),
                username: Some("synced".to_owned()),
            }],
            identity_deletes: Vec::new(),
            credential_updates: Vec::new(),
            credential_deletes: Vec::new(),
            host_updates: Vec::new(),
            host_deletes: Vec::new(),
            desktop_profile_updates: Vec::new(),
            desktop_profile_deletes: Vec::new(),
            secret_replacements: Vec::new(),
            secret_deletes: Vec::new(),
        };
        repository
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_scope_membership_replace
                 BEFORE DELETE ON ssh_sync_scope_memberships
                 BEGIN SELECT RAISE(ABORT, 'injected membership failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            repository
                .apply_ssh_sync_owned_metadata_delta_with_scope_memberships(&delta, Some(&[])),
            Err(AppPersistenceError::Database(_))
        ));
        assert_eq!(
            repository
                .get_identity(&plan.identities[0].identity_id)
                .unwrap()
                .label,
            "Restored identity"
        );
        assert_eq!(
            repository
                .list_ssh_sync_scope_memberships(&owner.key)
                .unwrap()
                .len(),
            1
        );
        repository
            .connection
            .execute_batch("DROP TRIGGER fail_scope_membership_replace")
            .unwrap();
        repository
            .apply_ssh_sync_owned_metadata_delta_with_scope_memberships(&delta, Some(&[]))
            .unwrap();
        assert_eq!(
            repository
                .get_identity(&plan.identities[0].identity_id)
                .unwrap()
                .label,
            "Must commit with omission"
        );
        assert!(
            repository
                .list_ssh_sync_scope_memberships(&owner.key)
                .unwrap()
                .is_empty()
        );
        assert!(
            repository
                .get_identity(&plan.identities[0].identity_id)
                .is_ok()
        );
        assert!(
            repository
                .apply_ssh_sync_owned_metadata_delta_with_scope_memberships(&delta, Some(&[]))
                .unwrap()
                .replayed
        );
        assert!(matches!(
            repository
                .apply_ssh_sync_owned_metadata_delta_with_scope_memberships(&delta, Some(&initial)),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn ssh_sync_restore_desktop_profile_creates_after_ready_password_and_accepts_256_username() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, mut plan) = ssh_sync_restore_fixture();
        let identity_username = "u".repeat(256);
        plan.identities[0].username = Some(identity_username.clone());
        let profile = ssh_sync_desktop_profile(
            plan.hosts[0].host_id.clone(),
            plan.credentials[0].credential_ref_id.clone(),
        );
        plan.desktop_profiles
            .push(SshSyncRestoreDesktopProfileInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                profile: profile.clone(),
            });

        let preview = repository.preview_ssh_sync_restore_plan(&plan).unwrap();
        assert_eq!(preview.create_count, 4);
        assert_eq!(preview.conflict_count, 0);
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        let committed = repository.commit_ssh_sync_restore_plan(&plan).unwrap();
        assert_eq!(committed.created_count, 4);

        let mut expected = profile;
        expected.revision = WireSequence::new(1);
        assert_eq!(repository.list_desktop_profiles().unwrap(), vec![expected]);
        assert_eq!(
            repository
                .get_identity(&plan.identities[0].identity_id)
                .unwrap()
                .username,
            Some(identity_username.clone())
        );
        assert!(matches!(
            repository
                .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
                .unwrap()
                .details,
            CredentialRecordDetails::Password { .. }
        ));

        let owner = ssh_sync_profile_fixture("desktop-username-update");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_identity = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[SshSyncObjectMappingInput {
                    portable_object_id: portable_identity.clone(),
                    local_object_id: SshSyncLocalObjectId::Identity(
                        plan.identities[0].identity_id.clone(),
                    ),
                }],
            )
            .unwrap();
        let mut update = empty_ssh_sync_owned_delta(owner.key.clone(), 'a');
        update.identity_updates.push(SshSyncOwnedIdentityUpdate {
            portable_object_id: portable_identity,
            identity_id: plan.identities[0].identity_id.clone(),
            expected_state_version: WireSequence::new(1),
            label: "Updated restored identity".to_owned(),
            username: Some("v".repeat(256)),
        });
        repository
            .apply_ssh_sync_owned_metadata_delta(&update)
            .unwrap();
        assert_eq!(
            repository
                .get_identity(&plan.identities[0].identity_id)
                .unwrap()
                .username,
            Some("v".repeat(256))
        );
    }

    #[test]
    fn ssh_sync_owned_desktop_updates_and_deletes_use_strict_cas_and_rollback() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, mut plan) = ssh_sync_restore_fixture();
        let profile = ssh_sync_desktop_profile(
            plan.hosts[0].host_id.clone(),
            plan.credentials[0].credential_ref_id.clone(),
        );
        plan.desktop_profiles
            .push(SshSyncRestoreDesktopProfileInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                profile: profile.clone(),
            });
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();

        let owner = ssh_sync_profile_fixture("desktop-cas");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        let portable_desktop = uuid::Uuid::new_v4().to_string();
        let portable_host = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_desktop.clone(),
                        local_object_id: SshSyncLocalObjectId::DesktopProfile(profile.id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_host.clone(),
                        local_object_id: SshSyncLocalObjectId::Host(plan.hosts[0].host_id.clone()),
                    },
                ],
            )
            .unwrap();

        let mut desired = profile.clone();
        desired.label = "CAS updated desktop".to_owned();
        let mut update = empty_ssh_sync_owned_delta(owner.key.clone(), 'b');
        update
            .desktop_profile_updates
            .push(SshSyncOwnedDesktopProfileUpdate {
                portable_object_id: portable_desktop.clone(),
                profile: desired.clone(),
                expected_revision: WireSequence::new(1),
            });
        let applied = repository
            .apply_ssh_sync_owned_metadata_delta(&update)
            .unwrap();
        assert_eq!(applied.updated_count, 1);
        assert_eq!(
            repository.list_desktop_profiles().unwrap()[0].revision,
            WireSequence::new(2)
        );
        assert_eq!(
            repository.list_desktop_profiles().unwrap()[0].label,
            desired.label
        );

        let mut stale_update = empty_ssh_sync_owned_delta(owner.key.clone(), 'c');
        stale_update
            .desktop_profile_updates
            .push(SshSyncOwnedDesktopProfileUpdate {
                portable_object_id: portable_desktop.clone(),
                profile: desired.clone(),
                expected_revision: WireSequence::new(1),
            });
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&stale_update),
            Err(AppPersistenceError::Conflict)
        ));

        let mut rollback_profile = desired.clone();
        rollback_profile.label = "Must roll back desktop".to_owned();
        let mut rollback = empty_ssh_sync_owned_delta(owner.key.clone(), 'd');
        rollback
            .desktop_profile_updates
            .push(SshSyncOwnedDesktopProfileUpdate {
                portable_object_id: portable_desktop.clone(),
                profile: rollback_profile,
                expected_revision: WireSequence::new(2),
            });
        rollback.host_deletes.push(SshSyncOwnedHostDelete {
            portable_object_id: portable_host,
            host_id: plan.hosts[0].host_id.clone(),
            expected: initial_ssh_sync_host_versions(),
        });
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&rollback),
            Err(AppPersistenceError::Conflict)
        ));
        let after_rollback = repository.list_desktop_profiles().unwrap();
        assert_eq!(after_rollback[0].revision, WireSequence::new(2));
        assert_eq!(after_rollback[0].label, desired.label);
        assert!(repository.get_host(&plan.hosts[0].host_id).is_ok());

        let mut stale_delete = empty_ssh_sync_owned_delta(owner.key.clone(), 'e');
        stale_delete
            .desktop_profile_deletes
            .push(SshSyncOwnedDesktopProfileDelete {
                portable_object_id: portable_desktop.clone(),
                profile_id: profile.id.clone(),
                expected_revision: WireSequence::new(1),
            });
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&stale_delete),
            Err(AppPersistenceError::Conflict)
        ));

        let mut delete = empty_ssh_sync_owned_delta(owner.key.clone(), 'f');
        delete
            .desktop_profile_deletes
            .push(SshSyncOwnedDesktopProfileDelete {
                portable_object_id: portable_desktop,
                profile_id: profile.id,
                expected_revision: WireSequence::new(2),
            });
        assert_eq!(
            repository
                .apply_ssh_sync_owned_metadata_delta(&delete)
                .unwrap()
                .deleted_count,
            1
        );
        assert!(repository.list_desktop_profiles().unwrap().is_empty());
    }

    #[test]
    fn ssh_sync_desktop_references_protect_shared_credentials_and_other_profile_mappings() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let (saga, mut plan) = ssh_sync_restore_fixture();
        let profile = ssh_sync_desktop_profile(
            plan.hosts[0].host_id.clone(),
            plan.credentials[0].credential_ref_id.clone(),
        );
        plan.desktop_profiles
            .push(SshSyncRestoreDesktopProfileInput {
                portable_object_id: uuid::Uuid::new_v4().to_string(),
                profile: profile.clone(),
            });
        repository.begin_ssh_sync_restore_saga(&saga).unwrap();
        repository.commit_ssh_sync_restore_plan(&plan).unwrap();

        let owner = ssh_sync_profile_fixture("desktop-reference-owner");
        let other_owner = ssh_sync_profile_fixture("desktop-reference-other");
        repository.ensure_ssh_sync_profile_state(&owner).unwrap();
        repository
            .ensure_ssh_sync_profile_state(&other_owner)
            .unwrap();
        let portable_desktop = uuid::Uuid::new_v4().to_string();
        let portable_credential = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &owner.key,
                &[
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_desktop.clone(),
                        local_object_id: SshSyncLocalObjectId::DesktopProfile(profile.id.clone()),
                    },
                    SshSyncObjectMappingInput {
                        portable_object_id: portable_credential.clone(),
                        local_object_id: SshSyncLocalObjectId::Credential(
                            plan.credentials[0].credential_ref_id.clone(),
                        ),
                    },
                ],
            )
            .unwrap();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &other_owner.key,
                &[SshSyncObjectMappingInput {
                    portable_object_id: uuid::Uuid::new_v4().to_string(),
                    local_object_id: SshSyncLocalObjectId::DesktopProfile(profile.id.clone()),
                }],
            )
            .unwrap();

        let mut delete_profile = empty_ssh_sync_owned_delta(owner.key.clone(), '1');
        delete_profile
            .desktop_profile_deletes
            .push(SshSyncOwnedDesktopProfileDelete {
                portable_object_id: portable_desktop,
                profile_id: profile.id.clone(),
                expected_revision: WireSequence::new(1),
            });
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&delete_profile),
            Err(AppPersistenceError::Conflict)
        ));

        let mut delete_credential = empty_ssh_sync_owned_delta(owner.key.clone(), '2');
        delete_credential
            .credential_deletes
            .push(SshSyncOwnedCredentialDelete {
                portable_object_id: portable_credential,
                credential_ref_id: plan.credentials[0].credential_ref_id.clone(),
                expected_state_version: WireSequence::new(1),
            });
        assert!(matches!(
            repository.apply_ssh_sync_owned_metadata_delta(&delete_credential),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository.list_desktop_profiles().unwrap()[0].id,
            profile.id
        );
        assert!(
            repository
                .get_ready_credential_record(&plan.credentials[0].credential_ref_id)
                .is_ok()
        );
    }

    #[test]
    fn v42_ssh_sync_desktop_scope_and_mapping_persist_after_reopen() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        {
            let repository = AppRepository::open(&database_path).unwrap();
            super::remove_schema_added_after_fixture_version(&repository.connection, 41).unwrap();
            repository
                .connection
                .pragma_update(None, "user_version", 41)
                .unwrap();
        }
        let mut repository = AppRepository::open(&database_path).unwrap();
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let mut desktop = ssh_sync_desktop_profile(HostId::new(), CredentialRefId::new());
        desktop.host_id = None;
        desktop.credential_ref_id = None;
        let desktop = repository.save_desktop_profile(&desktop).unwrap();
        let mut input = ssh_sync_profile_fixture("desktop-scope-persistence");
        input.scope.custom_host_ids.clear();
        input.scope.custom_credential_ref_ids.clear();
        input.scope.custom_desktop_profile_ids = vec![desktop.id.clone()];
        let created = repository.ensure_ssh_sync_profile_state(&input).unwrap();
        let portable_desktop = uuid::Uuid::new_v4().to_string();
        repository
            .resolve_or_create_ssh_sync_object_mappings(
                &input.key,
                &[SshSyncObjectMappingInput {
                    portable_object_id: portable_desktop.clone(),
                    local_object_id: SshSyncLocalObjectId::DesktopProfile(desktop.id.clone()),
                }],
            )
            .unwrap();
        repository
            .replace_ssh_sync_scope_memberships(
                &input.key,
                created.state_version,
                &[SshSyncScopeMembershipInput {
                    object_kind: SshSyncObjectKind::DesktopProfile,
                    portable_object_id: portable_desktop.clone(),
                    state: SshSyncScopeMembershipState::Included,
                }],
            )
            .unwrap();
        drop(repository);

        let repository = AppRepository::open(&database_path).unwrap();
        let state = repository
            .get_ssh_sync_profile_state(&input.key)
            .unwrap()
            .unwrap();
        assert_eq!(
            state.scope.custom_desktop_profile_ids,
            vec![desktop.id.clone()]
        );
        let mappings = repository
            .list_ssh_sync_object_mappings(&input.key)
            .unwrap();
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].object_kind, SshSyncObjectKind::DesktopProfile);
        assert_eq!(
            mappings[0].local_object_id,
            SshSyncLocalObjectId::DesktopProfile(desktop.id)
        );
        let memberships = repository
            .list_ssh_sync_scope_memberships(&input.key)
            .unwrap();
        assert_eq!(memberships.len(), 1);
        assert_eq!(
            memberships[0].membership,
            SshSyncScopeMembershipInput {
                object_kind: SshSyncObjectKind::DesktopProfile,
                portable_object_id: portable_desktop,
                state: SshSyncScopeMembershipState::Included,
            }
        );
    }
}
