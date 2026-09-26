//! Opaque, owner-scoped encrypted exchange bytes and HTTP receipts.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use norishell_core_api::PluginApiErrorCode;
use norishell_core_api::PluginId;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{ResourceFence, ResourceOwner};

pub(crate) const MAX_EXCHANGE_BLOB_BYTES: usize = 96 * 1024 * 1024;
const MAX_TOTAL_BLOB_BYTES: usize = 512 * 1024 * 1024;
const MAX_OWNER_BLOB_BYTES: usize = 224 * 1024 * 1024;
const MAX_BLOB_ENTRIES_PER_OWNER: usize = 12;
const MAX_BLOB_ENTRIES_TOTAL: usize = 48;
const MAX_RECEIPTS_PER_OWNER: usize = 24;
const MAX_RECEIPTS_TOTAL: usize = 96;
const BLOB_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub(crate) struct ExchangeBlobStore {
    inner: Arc<Mutex<BlobLedger>>,
}

#[derive(Default)]
struct BlobLedger {
    blobs: BTreeMap<String, BlobEntry>,
    receipts: BTreeMap<String, ReceiptEntry>,
    bytes: usize,
}

struct BlobEntry {
    owner: ResourceOwner,
    profile_id: String,
    bytes: Arc<Zeroizing<Vec<u8>>>,
    sha256: String,
    expires_at: Instant,
}

struct ReceiptEntry {
    owner: ResourceOwner,
    profile_id: String,
    receipt: NetworkReceipt,
    expires_at: Instant,
}

#[derive(Clone)]
pub(crate) struct ExchangeBlob {
    pub handle: String,
    pub bytes: Arc<Zeroizing<Vec<u8>>>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkReceipt {
    pub endpoint_origin: String,
    pub resource_url: String,
    pub method: String,
    pub status: u16,
    pub request_if_match: Option<String>,
    pub request_expected_next_revision: Option<u64>,
    pub request_idempotency_key: Option<String>,
    pub etag: Option<String>,
    pub response_revision: Option<u64>,
    pub response_next_revision: Option<u64>,
    pub request_body_sha256: Option<String>,
    pub response_body_sha256: Option<String>,
    pub response_blob_handle: Option<String>,
}

impl Default for ExchangeBlobStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BlobLedger::default())),
        }
    }
}

impl ExchangeBlobStore {
    pub(crate) fn insert(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        bytes: Vec<u8>,
        fence: &ResourceFence,
    ) -> Result<ExchangeBlob, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if !valid_profile(profile_id) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        if bytes.len() > MAX_EXCHANGE_BLOB_BYTES {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let size = bytes.len();
        let bytes = Arc::new(Zeroizing::new(bytes));
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.reap();
        let owner_blobs = ledger.blobs.values().filter(|entry| &entry.owner == owner);
        let owner_count = owner_blobs.clone().count();
        let owner_bytes: usize = owner_blobs.map(|entry| entry.bytes.len()).sum();
        if ledger.blobs.len() >= MAX_BLOB_ENTRIES_TOTAL
            || owner_count >= MAX_BLOB_ENTRIES_PER_OWNER
            || ledger.bytes.saturating_add(size) > MAX_TOTAL_BLOB_BYTES
            || owner_bytes.saturating_add(size) > MAX_OWNER_BLOB_BYTES
        {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let handle = Uuid::new_v4().to_string();
        ledger.blobs.insert(
            handle.clone(),
            BlobEntry {
                owner: owner.clone(),
                profile_id: profile_id.to_owned(),
                bytes: bytes.clone(),
                sha256: sha256.clone(),
                expires_at: Instant::now() + BLOB_LIFETIME,
            },
        );
        ledger.bytes += size;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(ExchangeBlob {
            handle,
            bytes,
            sha256,
        })
    }

    pub(crate) fn get(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<ExchangeBlob, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.reap();
        let entry = ledger
            .blobs
            .get(handle)
            .filter(|entry| &entry.owner == owner && entry.profile_id == profile_id)
            .ok_or(PluginApiErrorCode::Revoked)?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(ExchangeBlob {
            handle: handle.to_owned(),
            bytes: entry.bytes.clone(),
            sha256: entry.sha256.clone(),
        })
    }

    pub(crate) fn insert_receipt(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        receipt: NetworkReceipt,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        if !valid_profile(profile_id) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.reap();
        if ledger.receipts.len() >= MAX_RECEIPTS_TOTAL
            || ledger
                .receipts
                .values()
                .filter(|entry| &entry.owner == owner)
                .count()
                >= MAX_RECEIPTS_PER_OWNER
        {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let handle = Uuid::new_v4().to_string();
        ledger.receipts.insert(
            handle.clone(),
            ReceiptEntry {
                owner: owner.clone(),
                profile_id: profile_id.to_owned(),
                receipt,
                expires_at: Instant::now() + BLOB_LIFETIME,
            },
        );
        Ok(handle)
    }

    pub(crate) fn get_receipt(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<NetworkReceipt, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.reap();
        ledger
            .receipts
            .get(handle)
            .filter(|entry| &entry.owner == owner && entry.profile_id == profile_id)
            .map(|entry| entry.receipt.clone())
            .ok_or(PluginApiErrorCode::Revoked)
    }

    pub(crate) fn release(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        blob_handles: &[String],
        receipt_handles: &[String],
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        self.ensure_owned_handles(owner, profile_id, blob_handles, receipt_handles, fence)?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.reap();
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        for handle in blob_handles {
            ledger.blobs.remove(handle);
        }
        for handle in receipt_handles {
            ledger.receipts.remove(handle);
        }
        ledger.bytes = ledger.blobs.values().map(|entry| entry.bytes.len()).sum();
        Ok(())
    }

    pub(crate) fn ensure_owned_handles(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        blob_handles: &[String],
        receipt_handles: &[String],
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.reap();
        if blob_handles.iter().any(|handle| {
            ledger
                .blobs
                .get(handle)
                .is_some_and(|entry| &entry.owner != owner || entry.profile_id != profile_id)
        }) || receipt_handles.iter().any(|handle| {
            ledger
                .receipts
                .get(handle)
                .is_some_and(|entry| &entry.owner != owner || entry.profile_id != profile_id)
        }) || !fence()
        {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(())
    }

    pub(crate) fn remove_plugin(&self, plugin_id: &PluginId) {
        let mut ledger = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger
            .blobs
            .retain(|_, entry| &entry.owner.plugin_id != plugin_id);
        ledger
            .receipts
            .retain(|_, entry| &entry.owner.plugin_id != plugin_id);
        ledger.bytes = ledger.blobs.values().map(|entry| entry.bytes.len()).sum();
    }
}

impl BlobLedger {
    fn reap(&mut self) {
        let now = Instant::now();
        self.blobs.retain(|_, entry| entry.expires_at > now);
        self.receipts.retain(|_, entry| entry.expires_at > now);
        self.bytes = self.blobs.values().map(|entry| entry.bytes.len()).sum();
    }
}

fn valid_profile(profile_id: &str) -> bool {
    !profile_id.is_empty()
        && profile_id.len() <= 80
        && profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use norishell_core_api::{PluginApiErrorCode, PluginId, WireSequence};

    use super::{ExchangeBlobStore, NetworkReceipt, ResourceOwner};

    fn owner(plugin_id: &str, generation: u64) -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse(plugin_id).expect("plugin id"),
            signer: "signed".to_owned(),
            package: "package".to_owned(),
            generation: WireSequence::new(generation),
        }
    }

    #[test]
    fn blobs_and_receipts_are_bound_to_owner_profile_and_lifetime() {
        let store = ExchangeBlobStore::default();
        let original = owner("com.norishell.one", 1);
        let other = owner("com.norishell.one", 2);
        let current: super::ResourceFence = Arc::new(|| true);
        let blob = store
            .insert(&original, "primary", b"encrypted".to_vec(), &current)
            .expect("blob");
        assert_eq!(
            store
                .get(&original, "primary", &blob.handle, &current)
                .expect("same owner")
                .sha256,
            blob.sha256
        );
        assert!(matches!(
            store.get(&other, "primary", &blob.handle, &current),
            Err(PluginApiErrorCode::Revoked)
        ));
        assert!(matches!(
            store.get(&original, "other", &blob.handle, &current),
            Err(PluginApiErrorCode::Revoked)
        ));
        let receipt = store
            .insert_receipt(
                &original,
                "primary",
                NetworkReceipt {
                    endpoint_origin: "https://example.test".to_owned(),
                    resource_url: "https://example.test/exchange".to_owned(),
                    method: "GET".to_owned(),
                    status: 200,
                    request_if_match: None,
                    request_expected_next_revision: None,
                    request_idempotency_key: None,
                    etag: Some("\"v1\"".to_owned()),
                    response_revision: Some(1),
                    response_next_revision: None,
                    request_body_sha256: None,
                    response_body_sha256: Some(blob.sha256.clone()),
                    response_blob_handle: Some(blob.handle.clone()),
                },
                &current,
            )
            .expect("receipt");
        assert!(
            store
                .get_receipt(&other, "primary", &receipt, &current)
                .is_err()
        );
        store.remove_plugin(&original.plugin_id);
        assert!(
            store
                .get(&original, "primary", &blob.handle, &current)
                .is_err()
        );
        assert!(
            store
                .get_receipt(&original, "primary", &receipt, &current)
                .is_err()
        );
    }

    #[test]
    fn release_preserves_other_owners_and_per_owner_capacity() {
        let store = ExchangeBlobStore::default();
        let first = owner("com.norishell.first", 1);
        let second = owner("com.norishell.second", 1);
        let current: super::ResourceFence = Arc::new(|| true);
        let first_handles: Vec<String> = (0..super::MAX_BLOB_ENTRIES_PER_OWNER)
            .map(|_| {
                store
                    .insert(&first, "primary", b"ciphertext".to_vec(), &current)
                    .expect("first owner capacity")
                    .handle
            })
            .collect();
        assert!(matches!(
            store.insert(&first, "primary", b"extra".to_vec(), &current),
            Err(PluginApiErrorCode::QuotaExceeded)
        ));
        let second_blob = store
            .insert(&second, "primary", b"other".to_vec(), &current)
            .expect("another owner retains capacity");
        assert!(matches!(
            store.release(
                &first,
                "primary",
                std::slice::from_ref(&second_blob.handle),
                &[],
                &current
            ),
            Err(PluginApiErrorCode::Revoked)
        ));
        assert!(
            store
                .get(&second, "primary", &second_blob.handle, &current)
                .is_ok()
        );
        store
            .release(&first, "primary", &first_handles, &[], &current)
            .expect("release first owner");
        store
            .release(&first, "primary", &first_handles, &[], &current)
            .expect("release remains idempotent after expiry or retry");
        store
            .insert(&first, "primary", b"reused".to_vec(), &current)
            .expect("capacity is restored");
        assert!(
            store
                .get(&second, "primary", &second_blob.handle, &current)
                .is_ok()
        );
    }

    #[test]
    fn repeated_exchange_releases_restore_blob_and_receipt_capacity() {
        let store = ExchangeBlobStore::default();
        let owner = owner("com.norishell.sync", 1);
        let current: super::ResourceFence = Arc::new(|| true);
        for revision in 0..64 {
            let blob = store
                .insert(&owner, "primary", b"encrypted".to_vec(), &current)
                .expect("blob capacity after previous release");
            let receipt = store
                .insert_receipt(
                    &owner,
                    "primary",
                    NetworkReceipt {
                        endpoint_origin: "https://sync.example".to_owned(),
                        resource_url: "https://sync.example/exchange".to_owned(),
                        method: "GET".to_owned(),
                        status: 200,
                        request_if_match: None,
                        request_expected_next_revision: None,
                        request_idempotency_key: None,
                        etag: None,
                        response_revision: Some(revision),
                        response_next_revision: None,
                        request_body_sha256: None,
                        response_body_sha256: Some(blob.sha256.clone()),
                        response_blob_handle: Some(blob.handle.clone()),
                    },
                    &current,
                )
                .expect("receipt capacity after previous release");
            store
                .release(&owner, "primary", &[blob.handle], &[receipt], &current)
                .expect("release completed exchange");
        }
    }
}
