//! Short-lived Core-only state for data exchange calls.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use norishell_core_api::{PluginApiErrorCode, PluginId};
use uuid::Uuid;

use crate::ssh_sync_exchange::data_exchange::{
    DataApplyReceipt, DataComposedSnapshot, DataExchangeBlob, DataExchangeInspection,
    DataLocalSnapshot,
};

use super::{ResourceFence, ResourceOwner};

const MAX_DATA_STATES_PER_OWNER: usize = 24;
const MAX_DATA_STATES_TOTAL: usize = 96;
const DATA_STATE_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Default)]
pub(crate) struct DataStateRegistry {
    entries: Arc<Mutex<BTreeMap<String, DataStateEntry>>>,
}

struct DataStateEntry {
    owner: ResourceOwner,
    profile_id: String,
    state: DataState,
    expires_at: Instant,
}

enum DataState {
    Snapshot(Arc<DataLocalSnapshot>),
    Inspection(Arc<DataExchangeInspection>),
    Composed(Arc<DataComposedSnapshot>),
    Export(Arc<DataExportState>),
    Apply(Arc<DataApplyReceipt>),
}

pub(crate) struct DataExportState {
    pub(crate) blob: Arc<DataExchangeBlob>,
    pub(crate) source_handle: String,
    pub(crate) base_receipt_handle: String,
}

impl DataStateRegistry {
    pub(crate) fn insert_snapshot(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        snapshot: Arc<DataLocalSnapshot>,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        self.insert(owner, profile_id, DataState::Snapshot(snapshot), fence)
    }

    pub(crate) fn insert_inspection(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        inspection: Arc<DataExchangeInspection>,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        self.insert(owner, profile_id, DataState::Inspection(inspection), fence)
    }

    pub(crate) fn insert_composed(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        composed: Arc<DataComposedSnapshot>,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        self.insert(owner, profile_id, DataState::Composed(composed), fence)
    }

    pub(crate) fn insert_export(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        export: Arc<DataExportState>,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        self.insert(owner, profile_id, DataState::Export(export), fence)
    }

    pub(crate) fn insert_apply(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        apply: Arc<DataApplyReceipt>,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        self.insert(owner, profile_id, DataState::Apply(apply), fence)
    }

    pub(crate) fn get_snapshot(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<Arc<DataLocalSnapshot>, PluginApiErrorCode> {
        match self.get(owner, profile_id, handle, fence)? {
            DataState::Snapshot(snapshot) => Ok(snapshot),
            DataState::Inspection(_)
            | DataState::Composed(_)
            | DataState::Export(_)
            | DataState::Apply(_) => Err(PluginApiErrorCode::Revoked),
        }
    }

    pub(crate) fn get_inspection(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<Arc<DataExchangeInspection>, PluginApiErrorCode> {
        match self.get(owner, profile_id, handle, fence)? {
            DataState::Inspection(inspection) => Ok(inspection),
            DataState::Snapshot(_)
            | DataState::Composed(_)
            | DataState::Export(_)
            | DataState::Apply(_) => Err(PluginApiErrorCode::Revoked),
        }
    }

    pub(crate) fn get_composed(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<Arc<DataComposedSnapshot>, PluginApiErrorCode> {
        match self.get(owner, profile_id, handle, fence)? {
            DataState::Composed(composed) => Ok(composed),
            DataState::Snapshot(_)
            | DataState::Inspection(_)
            | DataState::Export(_)
            | DataState::Apply(_) => Err(PluginApiErrorCode::Revoked),
        }
    }

    pub(crate) fn get_export(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<Arc<DataExportState>, PluginApiErrorCode> {
        match self.get(owner, profile_id, handle, fence)? {
            DataState::Export(export) => Ok(export),
            DataState::Snapshot(_)
            | DataState::Inspection(_)
            | DataState::Composed(_)
            | DataState::Apply(_) => Err(PluginApiErrorCode::Revoked),
        }
    }

    pub(crate) fn get_apply(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<Arc<DataApplyReceipt>, PluginApiErrorCode> {
        match self.get(owner, profile_id, handle, fence)? {
            DataState::Apply(apply) => Ok(apply),
            DataState::Snapshot(_)
            | DataState::Inspection(_)
            | DataState::Composed(_)
            | DataState::Export(_) => Err(PluginApiErrorCode::Revoked),
        }
    }

    pub(crate) fn release(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handles: &[String],
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        self.ensure_owned_handles(owner, profile_id, handles, fence)?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reap(&mut entries);
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        for handle in handles {
            entries.remove(handle);
        }
        Ok(())
    }

    pub(crate) fn ensure_owned_handles(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handles: &[String],
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reap(&mut entries);
        if handles.iter().any(|handle| {
            entries
                .get(handle)
                .is_some_and(|entry| &entry.owner != owner || entry.profile_id != profile_id)
        }) || !fence()
        {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(())
    }

    pub(crate) fn remove_plugin(&self, plugin_id: &PluginId) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, entry| &entry.owner.plugin_id != plugin_id);
    }

    fn insert(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        state: DataState,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if !valid_profile(profile_id) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reap(&mut entries);
        if entries.len() >= MAX_DATA_STATES_TOTAL
            || entries
                .values()
                .filter(|entry| &entry.owner == owner)
                .count()
                >= MAX_DATA_STATES_PER_OWNER
        {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let handle = Uuid::new_v4().to_string();
        entries.insert(
            handle.clone(),
            DataStateEntry {
                owner: owner.clone(),
                profile_id: profile_id.to_owned(),
                state,
                expires_at: Instant::now() + DATA_STATE_LIFETIME,
            },
        );
        Ok(handle)
    }

    fn get(
        &self,
        owner: &ResourceOwner,
        profile_id: &str,
        handle: &str,
        fence: &ResourceFence,
    ) -> Result<DataState, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reap(&mut entries);
        let entry = entries
            .get(handle)
            .filter(|entry| &entry.owner == owner && entry.profile_id == profile_id)
            .ok_or(PluginApiErrorCode::Revoked)?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(match &entry.state {
            DataState::Snapshot(snapshot) => DataState::Snapshot(snapshot.clone()),
            DataState::Inspection(inspection) => DataState::Inspection(inspection.clone()),
            DataState::Composed(composed) => DataState::Composed(composed.clone()),
            DataState::Export(export) => DataState::Export(export.clone()),
            DataState::Apply(apply) => DataState::Apply(apply.clone()),
        })
    }
}

fn reap(entries: &mut BTreeMap<String, DataStateEntry>) {
    let now = Instant::now();
    entries.retain(|_, entry| entry.expires_at > now);
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
    use norishell_ssh_profile_sync::PluginExchangeBinding;

    use super::{DataExportState, DataStateRegistry, ResourceFence, ResourceOwner};
    use crate::ssh_sync_exchange::data_exchange::DataExchangeBlob;

    fn owner(plugin_id: &str) -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse(plugin_id).expect("plugin id"),
            signer: "signed".to_owned(),
            package: "package".to_owned(),
            generation: WireSequence::new(1),
        }
    }

    #[test]
    fn repeated_state_release_keeps_other_plugin_states() {
        let registry = DataStateRegistry::default();
        let first = owner("com.norishell.first");
        let second = owner("com.norishell.second");
        let fence: ResourceFence = Arc::new(|| true);
        let exported = Arc::new(DataExportState {
            blob: Arc::new(DataExchangeBlob {
                bytes: Vec::new(),
                objects: Vec::new(),
                binding: PluginExchangeBinding {
                    plugin_id: first.plugin_id.as_str().to_owned(),
                    signer_fingerprint_sha256: "owner".to_owned(),
                    profile_id: "primary".to_owned(),
                    revision: 1,
                    base_revision: None,
                    base_etag: None,
                },
                exchange_sha256: String::new(),
                keyed_content_sha256: String::new(),
                idempotency_key: String::new(),
                source_remote_exchange_sha256: None,
                profile_state_version: None,
            }),
            source_handle: String::new(),
            base_receipt_handle: String::new(),
        });
        let retained = registry
            .insert_export(&second, "primary", exported.clone(), &fence)
            .expect("second plugin entry");
        let saturated: Vec<String> = (0..super::MAX_DATA_STATES_PER_OWNER)
            .map(|_| {
                registry
                    .insert_export(&first, "primary", exported.clone(), &fence)
                    .expect("first plugin capacity")
            })
            .collect();
        assert!(matches!(
            registry.insert_export(&first, "primary", exported.clone(), &fence),
            Err(PluginApiErrorCode::QuotaExceeded)
        ));
        assert!(
            registry
                .get_export(&second, "primary", &retained, &fence)
                .is_ok()
        );
        registry
            .release(&first, "primary", &saturated, &fence)
            .expect("release first plugin capacity");
        for _ in 0..64 {
            let handle = registry
                .insert_export(&first, "primary", exported.clone(), &fence)
                .expect("state capacity after release");
            assert!(matches!(
                registry.release(&first, "primary", std::slice::from_ref(&retained), &fence),
                Err(PluginApiErrorCode::Revoked)
            ));
            registry
                .release(&first, "primary", &[handle], &fence)
                .expect("release this exchange only");
        }
        assert!(
            registry
                .get_export(&second, "primary", &retained, &fence)
                .is_ok()
        );
    }
}
