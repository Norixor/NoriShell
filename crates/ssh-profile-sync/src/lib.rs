//! Strict, portable SSH profile bundle and encryption codecs.
//!
//! The crate deliberately has no SQLite, Vault, filesystem, UI, plugin, OAuth,
//! or network integration. Its portable schema cannot express local paths,
//! Known Hosts, Vault envelopes/keys/passwords, auto-unlock state, runtime
//! sessions, UI state, or plugin state. Unknown JSON fields fail closed.

mod crypto;
mod error;
mod exchange;
mod merge;
mod offline_backup;
mod schema;
mod secret;

pub use crypto::{
    BundleDigest, EncryptedSyncObject, RecoveryEnvelopeUpload, RecoveryOwnerBinding,
    SyncObjectBinding, SyncObjectKind, canonical_bundle_bytes, canonical_bundle_digest,
    create_recovery_envelope, create_recovery_envelope_for_service,
    create_recovery_envelope_random, decode_bundle, decrypt_bundle, decrypt_bundle_from_service,
    encrypt_bundle, encrypt_bundle_for_service, encrypt_bundle_random, open_recovery_envelope,
    open_recovery_envelope_from_service,
};
pub use error::{Result, SyncCodecError};
pub use exchange::{
    PluginExchangeBinding, PluginExchangeSummary, create_plugin_exchange,
    create_plugin_exchange_with_key, inspect_plugin_exchange, inspect_plugin_exchange_data_owner,
    inspect_plugin_exchange_owner, open_plugin_exchange, open_plugin_exchange_with_key,
    plugin_exchange_vault_key_envelope, stable_plugin_data_owner,
};
pub use merge::{
    BundleConflictResolution, BundleMergeOutcome, merge_bundles_three_way,
    merge_bundles_three_way_with_policies, merge_bundles_three_way_with_resolution,
};
pub use offline_backup::{
    MAX_OFFLINE_BACKUP_FILE_BYTES, MAX_OFFLINE_BACKUP_PLAINTEXT_BYTES, decrypt_offline_backup,
    encrypt_offline_backup,
};
pub use schema::*;
pub use secret::{RecoveryPassword, SecretBytes, SyncKey};
