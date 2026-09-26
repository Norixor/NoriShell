//! Network-free, Core-owned portable data exchange building blocks.
//!
//! The plugin may transport a sealed blob, but plaintext and the sync key stay
//! inside Core. Category projection rejects cross-category references unless
//! the caller explicitly selected every referenced category.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use crate::plugin_api::blobs::{ExchangeBlob, NetworkReceipt};
use norishell_core_api::{
    PluginDataCategory, PluginDataLocalCounts, PluginDataObjectSource,
    SshSyncSecureDataReviewChoice, SshSyncSecureDataReviewProjection, WireSequence,
};
use norishell_ssh_profile_sync::{
    BundleSchema, LoginAutomationStep, PluginExchangeBinding, PortableBundleV1,
    PortableCredentialMaterial, PortableDataCategory, PortableObjectId, PortableObjectKind,
    PortableTombstone, RouteIngress, SyncKey, canonical_bundle_bytes,
    create_plugin_exchange_with_key, inspect_plugin_exchange_data_owner,
    inspect_plugin_exchange_owner, open_plugin_exchange_with_key,
};
use serde::Serialize;
use zeroize::Zeroizing;

use super::{
    ActionFence, BrokerError, DownloadedExchange, PendingRestoreHandle, PortableProfileState,
    PortableRemoteBaseline, PortableSnapshot, RestorePreview, SecureApplyApproval,
    SshSyncExchangeBroker, StagedRestoreCleanup, SyncDataReview, SyncDataReviewChoice,
    SyncDirectionReview, portable_content_sha256, sha256_hex,
};

pub(crate) struct DataLocalSnapshot {
    pub(crate) plugin_id: String,
    pub(crate) data_owner_sha256: String,
    pub(crate) profile_id: String,
    pub(crate) snapshot: PortableSnapshot,
    pub(crate) selected_bundle: PortableBundleV1,
    pub(crate) categories: Vec<PluginDataCategory>,
    pub(crate) profile_state_version: WireSequence,
    pub(crate) key_pending: bool,
    key: Option<SyncKey>,
    envelope: Option<Vec<u8>>,
}

pub(crate) struct DataObjectDescriptor {
    pub(crate) category: PluginDataCategory,
    pub(crate) kind: PortableObjectKind,
    pub(crate) stable_id: String,
    pub(crate) object_handle: String,
    pub(crate) equality_tag: String,
    pub(crate) update_time_unix_ms: Option<i64>,
    pub(crate) tombstone: bool,
    pub(crate) dependency: bool,
    pub(crate) display: Option<DataObjectDisplay>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataObjectSource {
    Local,
    Remote,
}

pub(crate) struct DataObjectDecision {
    pub(crate) object_handle: String,
    pub(crate) source: DataObjectSource,
}

pub(crate) struct DataComposedSnapshot {
    pub(crate) bundle: PortableBundleV1,
    pub(crate) local: Arc<DataLocalSnapshot>,
    pub(crate) remote: Arc<DataExchangeInspection>,
    pub(crate) reviewed: Option<Arc<ReviewedDataPlan>>,
}

pub(crate) struct ReviewedDataPlan {
    pub(crate) base_receipt: NetworkReceipt,
    restore_handle: PendingRestoreHandle,
    approval: Mutex<Option<SecureApplyApproval>>,
    _cleanup: StagedRestoreCleanup,
}

impl ReviewedDataPlan {
    fn approval(&self) -> Result<SecureApplyApproval, BrokerError> {
        self.approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(BrokerError::StateConflict)
    }

    fn consume_approval(&self) -> Result<SecureApplyApproval, BrokerError> {
        self.approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or(BrokerError::StateConflict)
    }
}

pub(crate) enum DataObjectDisplay {
    Host {
        label: String,
        address: String,
        port: u16,
    },
    Credential {
        label: String,
        material_kind: &'static str,
    },
    DesktopProfile {
        label: String,
        protocol: &'static str,
        address: String,
        port: u16,
    },
}

pub(crate) fn describe_objects(
    bundle: &PortableBundleV1,
    key: &SyncKey,
    selected_categories: &[PluginDataCategory],
) -> Result<Vec<DataObjectDescriptor>, BrokerError> {
    let mut result = Vec::new();
    macro_rules! describe {
        ($items:expr, $kind:expr) => {
            for item in &$items {
                result.push(describe_item(
                    bundle,
                    key,
                    selected_categories,
                    $kind,
                    item.id,
                    item,
                    false,
                )?);
            }
        };
    }
    describe!(bundle.objects.hosts, PortableObjectKind::Host);
    describe!(
        bundle.objects.desktop_profiles,
        PortableObjectKind::DesktopProfile
    );
    describe!(bundle.objects.identities, PortableObjectKind::Identity);
    describe!(bundle.objects.credentials, PortableObjectKind::Credential);
    describe!(bundle.objects.routes, PortableObjectKind::Route);
    describe!(
        bundle.objects.authentication_plans,
        PortableObjectKind::AuthenticationPlan
    );
    describe!(
        bundle.objects.algorithm_policies,
        PortableObjectKind::AlgorithmPolicy
    );
    describe!(
        bundle.objects.heartbeat_policies,
        PortableObjectKind::HeartbeatPolicy
    );
    describe!(
        bundle.objects.monitoring_policies,
        PortableObjectKind::MonitoringPolicy
    );
    describe!(
        bundle.objects.login_automations,
        PortableObjectKind::LoginAutomation
    );
    describe!(bundle.secrets, PortableObjectKind::Secret);
    for tombstone in &bundle.tombstones {
        result.push(describe_item(
            bundle,
            key,
            selected_categories,
            tombstone.kind,
            tombstone.id,
            tombstone,
            true,
        )?);
    }
    for item in &mut result {
        if item.tombstone {
            continue;
        }
        let id = item.stable_id.as_str();
        item.display = match item.kind {
            PortableObjectKind::Host => bundle
                .objects
                .hosts
                .iter()
                .find(|value| value.id.as_uuid().to_string() == id)
                .map(|value| DataObjectDisplay::Host {
                    label: value.label.clone(),
                    address: value.address.clone(),
                    port: value.port,
                }),
            PortableObjectKind::Credential => bundle
                .objects
                .credentials
                .iter()
                .find(|value| value.id.as_uuid().to_string() == id)
                .map(|value| DataObjectDisplay::Credential {
                    label: value.label.clone(),
                    material_kind: match value.material {
                        PortableCredentialMaterial::Password { .. } => "password",
                        PortableCredentialMaterial::PrivateKey { .. } => "privateKey",
                        PortableCredentialMaterial::Certificate { .. } => "certificate",
                        PortableCredentialMaterial::KeyboardInteractive { .. } => {
                            "keyboardInteractive"
                        }
                    },
                }),
            PortableObjectKind::DesktopProfile => bundle
                .objects
                .desktop_profiles
                .iter()
                .find(|value| value.id.as_uuid().to_string() == id)
                .map(|value| DataObjectDisplay::DesktopProfile {
                    label: value.label.clone(),
                    protocol: match value.protocol {
                        norishell_ssh_profile_sync::PortableDesktopProtocol::Rdp => "rdp",
                        norishell_ssh_profile_sync::PortableDesktopProtocol::Vnc => "vnc",
                    },
                    address: value.address.clone(),
                    port: value.port,
                }),
            _ => None,
        };
    }
    result.sort_by(|left, right| (left.kind, &left.stable_id).cmp(&(right.kind, &right.stable_id)));
    Ok(result)
}

fn describe_item<T: Serialize>(
    bundle: &PortableBundleV1,
    key: &SyncKey,
    selected_categories: &[PluginDataCategory],
    kind: PortableObjectKind,
    id: PortableObjectId,
    value: &T,
    tombstone: bool,
) -> Result<DataObjectDescriptor, BrokerError> {
    let category = category_for_kind(kind).ok_or(BrokerError::RemoteDataInvalid)?;
    let stable_id = id.as_uuid().to_string();
    let kind_name = serde_json::to_string(&kind).map_err(|_| BrokerError::RemoteDataInvalid)?;
    let mut content = Zeroizing::new(
        format!(
            "norishell:data-object:v1\0{kind_name}\0{stable_id}\0{}\0",
            u8::from(tombstone)
        )
        .into_bytes(),
    );
    let encoded =
        Zeroizing::new(serde_json::to_vec(value).map_err(|_| BrokerError::RemoteDataInvalid)?);
    content.extend_from_slice(&encoded);
    let equality_tag = hex::encode(key.baseline_digest(&content));
    let object_handle = object_handle(key, kind, id)?;
    Ok(DataObjectDescriptor {
        category,
        kind,
        stable_id,
        object_handle,
        equality_tag,
        update_time_unix_ms: bundle
            .update_times
            .iter()
            .find(|item| item.kind == kind && item.id == id)
            .map(|item| item.update_time_unix_ms),
        tombstone,
        dependency: !selected_categories.contains(&category),
        display: None,
    })
}

fn object_handle(
    key: &SyncKey,
    kind: PortableObjectKind,
    id: PortableObjectId,
) -> Result<String, BrokerError> {
    let kind_name = serde_json::to_string(&kind).map_err(|_| BrokerError::RemoteDataInvalid)?;
    let handle_material = format!(
        "norishell:data-object-handle:v1\0{kind_name}\0{}",
        id.as_uuid()
    );
    Ok(hex::encode(key.baseline_digest(handle_material.as_bytes())))
}

/// Builds one authenticated, Core-private object selection. Every object or
/// tombstone in the two snapshots needs a decision; missing on the chosen side
/// is an explicit deletion. Referential integrity is checked before use.
pub(crate) fn compose_selected_objects(
    local: Arc<DataLocalSnapshot>,
    remote: Arc<DataExchangeInspection>,
    decisions: &[DataObjectDecision],
) -> Result<DataComposedSnapshot, BrokerError> {
    let local_key = local.key.as_ref().ok_or(BrokerError::LocalKeyUnavailable)?;
    if local.plugin_id != remote.binding.plugin_id
        || local.data_owner_sha256 != remote.binding.signer_fingerprint_sha256
        || local.profile_id != remote.binding.profile_id
        || local.categories != remote.categories
        || local_key.baseline_digest(b"norishell:data-key-check:v1")
            != remote.key.baseline_digest(b"norishell:data-key-check:v1")
    {
        return Err(BrokerError::KeyBindingConflict);
    }
    let local_bundle = &local.selected_bundle;
    let remote_bundle = &remote.bundle;
    let local_keys = super::difference::object_keys(local_bundle)
        .into_iter()
        .chain(
            local_bundle
                .tombstones
                .iter()
                .map(|item| (item.kind, item.id)),
        )
        .collect::<BTreeSet<_>>();
    let remote_keys = super::difference::object_keys(remote_bundle)
        .into_iter()
        .chain(
            remote_bundle
                .tombstones
                .iter()
                .map(|item| (item.kind, item.id)),
        )
        .collect::<BTreeSet<_>>();
    let all_keys = local_keys
        .union(&remote_keys)
        .copied()
        .collect::<BTreeSet<_>>();
    let mut handles = BTreeMap::new();
    for (kind, id) in &all_keys {
        handles.insert(object_handle(local_key, *kind, *id)?, (*kind, *id));
    }
    if decisions.len() != all_keys.len() {
        return Err(BrokerError::OperationRejected(
            "broker.data_exchange.selection_incomplete",
        ));
    }
    let mut choices = BTreeMap::new();
    for decision in decisions {
        let key =
            handles
                .get(&decision.object_handle)
                .copied()
                .ok_or(BrokerError::OperationRejected(
                    "broker.data_exchange.selection_invalid",
                ))?;
        if choices.insert(key, decision.source).is_some() {
            return Err(BrokerError::OperationRejected(
                "broker.data_exchange.selection_duplicate",
            ));
        }
    }
    if local_keys.is_subset(&remote_keys)
        && choices
            .values()
            .all(|source| *source == DataObjectSource::Remote)
    {
        return Ok(DataComposedSnapshot {
            bundle: remote_bundle.clone(),
            local,
            remote,
            reviewed: None,
        });
    }
    let mut bundle = remote_bundle.clone();
    bundle.objects = Default::default();
    bundle.secrets.clear();
    bundle.tombstones.clear();
    bundle.update_times.clear();
    macro_rules! choose_items {
        ($field:ident, $kind:expr) => {
            for ((_, id), source) in choices.iter().filter(|((kind, _), _)| *kind == $kind) {
                let source_bundle = match source {
                    DataObjectSource::Local => local_bundle,
                    DataObjectSource::Remote => remote_bundle,
                };
                if let Some(item) = source_bundle
                    .objects
                    .$field
                    .iter()
                    .find(|item| item.id == *id)
                {
                    bundle.objects.$field.push(item.clone());
                }
            }
        };
    }
    choose_items!(hosts, PortableObjectKind::Host);
    choose_items!(desktop_profiles, PortableObjectKind::DesktopProfile);
    choose_items!(identities, PortableObjectKind::Identity);
    choose_items!(credentials, PortableObjectKind::Credential);
    choose_items!(routes, PortableObjectKind::Route);
    choose_items!(authentication_plans, PortableObjectKind::AuthenticationPlan);
    choose_items!(algorithm_policies, PortableObjectKind::AlgorithmPolicy);
    choose_items!(heartbeat_policies, PortableObjectKind::HeartbeatPolicy);
    choose_items!(monitoring_policies, PortableObjectKind::MonitoringPolicy);
    choose_items!(login_automations, PortableObjectKind::LoginAutomation);
    for ((kind, id), source) in choices
        .iter()
        .filter(|((kind, _), _)| *kind == PortableObjectKind::Secret)
    {
        let source_bundle = match source {
            DataObjectSource::Local => local_bundle,
            DataObjectSource::Remote => remote_bundle,
        };
        if let Some(item) = source_bundle.secrets.iter().find(|item| item.id == *id) {
            bundle.secrets.push(item.clone());
        }
        debug_assert_eq!(*kind, PortableObjectKind::Secret);
    }
    let present = super::difference::object_keys(&bundle);
    for ((kind, id), source) in &choices {
        let source_bundle = match source {
            DataObjectSource::Local => local_bundle,
            DataObjectSource::Remote => remote_bundle,
        };
        if !present.contains(&(*kind, *id)) {
            bundle.tombstones.push(PortableTombstone {
                kind: *kind,
                id: *id,
            });
        }
        if let Some(clock) = source_bundle
            .update_times
            .iter()
            .find(|item| item.kind == *kind && item.id == *id)
        {
            bundle.update_times.push(*clock);
        }
    }
    bundle
        .validate_current_business_exchange()
        .map_err(|_| BrokerError::MergeInvalid("broker.data_exchange.compose_references"))?;
    Ok(DataComposedSnapshot {
        bundle,
        local,
        remote,
        reviewed: None,
    })
}

/// Exact bytes for the plugin's network bridge; no decrypted object leaves Core.
pub(crate) struct DataExchangeBlob {
    pub(crate) bytes: Vec<u8>,
    pub(crate) objects: Vec<DataObjectDescriptor>,
    pub(crate) binding: PluginExchangeBinding,
    pub(crate) exchange_sha256: String,
    pub(crate) keyed_content_sha256: String,
    pub(crate) idempotency_key: String,
    pub(crate) source_remote_exchange_sha256: Option<String>,
    pub(crate) profile_state_version: Option<WireSequence>,
}

/// Authenticated source data and its selected business-data projection.
#[derive(Clone)]
pub(crate) struct DataExchangeInspection {
    pub(crate) migration_required: bool,
    pub(crate) excluded_categories: Vec<PluginDataCategory>,
    pub(crate) binding: PluginExchangeBinding,
    pub(crate) exchange_sha256: String,
    pub(crate) bundle: PortableBundleV1,
    pub(crate) categories: Vec<PluginDataCategory>,
    pub(crate) source_categories: Option<Vec<PortableDataCategory>>,
    source_receipt: Option<NetworkReceipt>,
    pub(crate) remote_origin: String,
    pub(crate) remote_resource_url: String,
    key: SyncKey,
}

pub(crate) struct DataApplyReceipt {
    pub(crate) source_exchange_sha256: String,
    pub(crate) keyed_content_sha256: String,
    pub(crate) current: PortableSnapshot,
    pub(crate) profile_state_version: WireSequence,
}

fn validate_categories(categories: &[PluginDataCategory]) -> Result<(), BrokerError> {
    if categories.is_empty()
        || categories.len() > 3
        || categories.iter().any(|item| {
            !matches!(
                item,
                PluginDataCategory::Hosts
                    | PluginDataCategory::Credentials
                    | PluginDataCategory::DesktopProfiles
            )
        })
        || categories
            .iter()
            .enumerate()
            .any(|(index, item)| categories[index + 1..].contains(item))
    {
        return Err(BrokerError::OperationRejected(
            "broker.data_exchange.categories_invalid",
        ));
    }
    Ok(())
}

/// Authenticated category projection. Unselected categories never disclose
/// objects or secrets; a cross-category reference requires explicit selection.
pub(crate) fn project_selected_bundle(
    source: &PortableBundleV1,
    categories: &[PluginDataCategory],
) -> Result<PortableBundleV1, BrokerError> {
    validate_categories(categories)?;
    source
        .validate()
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    if source.selected_categories.as_ref().is_some_and(|selected| {
        categories
            .iter()
            .any(|category| !selected.contains(&portable_category(*category)))
    }) {
        return Err(BrokerError::OperationRejected(
            "broker.data_exchange.category_not_in_source",
        ));
    }
    let required = required_dependency_categories(source, categories)?;
    if !required.is_empty() {
        return Err(BrokerError::OperationRejected(match required.as_slice() {
            [PluginDataCategory::Hosts] => "broker.data_exchange.requires_hosts",
            [PluginDataCategory::Credentials] => "broker.data_exchange.requires_credentials",
            _ => "broker.data_exchange.requires_hosts_and_credentials",
        }));
    }
    let mut result = source.clone();
    result.schema = BundleSchema::V6;
    let mut selected = categories
        .iter()
        .copied()
        .map(portable_category)
        .collect::<Vec<_>>();
    selected.sort();
    result.selected_categories = (categories.len() != 3).then_some(selected);
    result.preferences = None;
    result.preference_update_times.clear();
    if categories.len() == 3 {
        result
            .validate_current_business_exchange()
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        return Ok(result);
    }
    let mut included = BTreeSet::<(PortableObjectKind, PortableObjectId)>::new();
    let mut pending = Vec::new();
    let mut add = |kind, id| pending.push((kind, id));
    if categories.contains(&PluginDataCategory::Hosts) {
        for host in &source.objects.hosts {
            add(PortableObjectKind::Host, host.id);
        }
    }
    if categories.contains(&PluginDataCategory::Credentials) {
        for identity in &source.objects.identities {
            add(PortableObjectKind::Identity, identity.id);
        }
        for credential in &source.objects.credentials {
            add(PortableObjectKind::Credential, credential.id);
        }
        for secret in &source.secrets {
            add(PortableObjectKind::Secret, secret.id);
        }
    }
    if categories.contains(&PluginDataCategory::DesktopProfiles) {
        for desktop in &source.objects.desktop_profiles {
            add(PortableObjectKind::DesktopProfile, desktop.id);
        }
    }
    while let Some((kind, id)) = pending.pop() {
        if !included.insert((kind, id)) {
            continue;
        }
        match kind {
            PortableObjectKind::Host => {
                let host = source
                    .objects
                    .hosts
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                pending.extend([
                    (PortableObjectKind::Route, host.route_id),
                    (
                        PortableObjectKind::AuthenticationPlan,
                        host.authentication_plan_id,
                    ),
                    (
                        PortableObjectKind::AlgorithmPolicy,
                        host.algorithm_policy_id,
                    ),
                    (
                        PortableObjectKind::HeartbeatPolicy,
                        host.heartbeat_policy_id,
                    ),
                    (
                        PortableObjectKind::MonitoringPolicy,
                        host.monitoring_policy_id,
                    ),
                ]);
                if let Some(value) = host.identity_id {
                    pending.push((PortableObjectKind::Identity, value));
                }
                if let Some(value) = host.login_automation_id {
                    pending.push((PortableObjectKind::LoginAutomation, value));
                }
            }
            PortableObjectKind::DesktopProfile => {
                let desktop = source
                    .objects
                    .desktop_profiles
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                if let Some(value) = desktop.host_id {
                    pending.push((PortableObjectKind::Host, value));
                }
                if let Some(value) = desktop.gateway_host_id {
                    pending.push((PortableObjectKind::Host, value));
                }
                if let Some(value) = desktop.credential_id {
                    pending.push((PortableObjectKind::Credential, value));
                }
            }
            PortableObjectKind::Identity => {
                let identity = source
                    .objects
                    .identities
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                pending.extend(
                    identity
                        .credential_ids
                        .iter()
                        .copied()
                        .map(|value| (PortableObjectKind::Credential, value)),
                );
            }
            PortableObjectKind::Credential => {
                let credential = source
                    .objects
                    .credentials
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                pending.push((PortableObjectKind::Identity, credential.identity_id));
                match credential.material {
                    PortableCredentialMaterial::Password { password_secret_id } => {
                        pending.push((PortableObjectKind::Secret, password_secret_id))
                    }
                    PortableCredentialMaterial::PrivateKey {
                        private_key_secret_id,
                        passphrase_secret_id,
                        ..
                    } => {
                        pending.push((PortableObjectKind::Secret, private_key_secret_id));
                        if let Some(value) = passphrase_secret_id {
                            pending.push((PortableObjectKind::Secret, value));
                        }
                    }
                    PortableCredentialMaterial::Certificate {
                        certificate_secret_id,
                        private_key_secret_id,
                        passphrase_secret_id,
                        ..
                    } => {
                        pending.push((PortableObjectKind::Secret, certificate_secret_id));
                        if let Some(value) = private_key_secret_id {
                            pending.push((PortableObjectKind::Secret, value));
                        }
                        if let Some(value) = passphrase_secret_id {
                            pending.push((PortableObjectKind::Secret, value));
                        }
                    }
                    PortableCredentialMaterial::KeyboardInteractive { .. } => {}
                }
            }
            PortableObjectKind::Route => {
                let route = source
                    .objects
                    .routes
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                pending.extend(
                    route
                        .jump_hops
                        .iter()
                        .map(|hop| (PortableObjectKind::Host, hop.host_id)),
                );
                let credential = match route.ingress {
                    RouteIngress::Direct => None,
                    RouteIngress::HttpConnect { credential_id, .. }
                    | RouteIngress::Socks5 { credential_id, .. } => credential_id,
                };
                if let Some(value) = credential {
                    pending.push((PortableObjectKind::Credential, value));
                }
            }
            PortableObjectKind::AuthenticationPlan => {
                let plan = source
                    .objects
                    .authentication_plans
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                pending.extend(
                    plan.credential_ids
                        .iter()
                        .copied()
                        .map(|value| (PortableObjectKind::Credential, value)),
                );
            }
            PortableObjectKind::LoginAutomation => {
                let automation = source
                    .objects
                    .login_automations
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                for step in &automation.steps {
                    if let LoginAutomationStep::SendSecret { secret_id, .. } = step {
                        pending.push((PortableObjectKind::Secret, *secret_id));
                    }
                }
            }
            PortableObjectKind::AlgorithmPolicy
            | PortableObjectKind::HeartbeatPolicy
            | PortableObjectKind::MonitoringPolicy
            | PortableObjectKind::Secret => {}
        }
    }
    macro_rules! retain_included {
        ($items:expr, $kind:expr) => {
            $items.retain(|item| included.contains(&($kind, item.id)));
        };
    }
    retain_included!(result.objects.hosts, PortableObjectKind::Host);
    retain_included!(
        result.objects.desktop_profiles,
        PortableObjectKind::DesktopProfile
    );
    retain_included!(result.objects.identities, PortableObjectKind::Identity);
    retain_included!(result.objects.credentials, PortableObjectKind::Credential);
    retain_included!(result.objects.routes, PortableObjectKind::Route);
    retain_included!(
        result.objects.authentication_plans,
        PortableObjectKind::AuthenticationPlan
    );
    retain_included!(
        result.objects.algorithm_policies,
        PortableObjectKind::AlgorithmPolicy
    );
    retain_included!(
        result.objects.heartbeat_policies,
        PortableObjectKind::HeartbeatPolicy
    );
    retain_included!(
        result.objects.monitoring_policies,
        PortableObjectKind::MonitoringPolicy
    );
    retain_included!(
        result.objects.login_automations,
        PortableObjectKind::LoginAutomation
    );
    retain_included!(result.secrets, PortableObjectKind::Secret);
    if !categories.contains(&PluginDataCategory::Credentials) {
        result.skipped_machine_bound.clear();
    }
    result.tombstones.retain(|item| {
        category_for_kind(item.kind).is_some_and(|category| categories.contains(&category))
    });
    included.extend(result.tombstones.iter().map(|item| (item.kind, item.id)));
    result
        .update_times
        .retain(|item| included.contains(&(item.kind, item.id)));
    result
        .validate_current_business_exchange()
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    Ok(result)
}

fn portable_category(category: PluginDataCategory) -> PortableDataCategory {
    match category {
        PluginDataCategory::Hosts => PortableDataCategory::Hosts,
        PluginDataCategory::Credentials => PortableDataCategory::Credentials,
        PluginDataCategory::DesktopProfiles => PortableDataCategory::DesktopProfiles,
        _ => unreachable!("validated business category"),
    }
}

pub(crate) fn required_dependency_categories(
    source: &PortableBundleV1,
    categories: &[PluginDataCategory],
) -> Result<Vec<PluginDataCategory>, BrokerError> {
    validate_categories(categories)?;
    let mut required = Vec::new();
    if categories.contains(&PluginDataCategory::DesktopProfiles) {
        if !categories.contains(&PluginDataCategory::Hosts)
            && source
                .objects
                .desktop_profiles
                .iter()
                .any(|desktop| desktop.host_id.is_some() || desktop.gateway_host_id.is_some())
        {
            required.push(PluginDataCategory::Hosts);
        }
        if !categories.contains(&PluginDataCategory::Credentials)
            && source
                .objects
                .desktop_profiles
                .iter()
                .any(|desktop| desktop.credential_id.is_some())
        {
            required.push(PluginDataCategory::Credentials);
        }
    }
    if !categories.contains(&PluginDataCategory::Credentials) {
        let host_needs_credentials = if categories.contains(&PluginDataCategory::Hosts) {
            source
                .objects
                .hosts
                .iter()
                .any(|host| host_references_credentials(source, host))
        } else if categories.contains(&PluginDataCategory::DesktopProfiles) {
            source.objects.desktop_profiles.iter().any(|desktop| {
                source.objects.hosts.iter().any(|host| {
                    (desktop.host_id == Some(host.id) || desktop.gateway_host_id == Some(host.id))
                        && host_references_credentials(source, host)
                })
            })
        } else {
            false
        };
        if host_needs_credentials && !required.contains(&PluginDataCategory::Credentials) {
            required.push(PluginDataCategory::Credentials);
        }
    }
    Ok(required)
}

fn host_references_credentials(
    source: &PortableBundleV1,
    host: &norishell_ssh_profile_sync::PortableHost,
) -> bool {
    host.identity_id.is_some()
        || source
            .objects
            .authentication_plans
            .iter()
            .any(|plan| plan.id == host.authentication_plan_id && !plan.credential_ids.is_empty())
        || source.objects.routes.iter().any(|route| {
            route.id == host.route_id
                && matches!(
                    route.ingress,
                    RouteIngress::HttpConnect {
                        credential_id: Some(_),
                        ..
                    } | RouteIngress::Socks5 {
                        credential_id: Some(_),
                        ..
                    }
                )
        })
        || host.login_automation_id.is_some_and(|id| {
            source.objects.login_automations.iter().any(|automation| {
                automation.id == id
                    && automation
                        .steps
                        .iter()
                        .any(|step| matches!(step, LoginAutomationStep::SendSecret { .. }))
            })
        })
}

fn category_for_kind(kind: PortableObjectKind) -> Option<PluginDataCategory> {
    Some(match kind {
        PortableObjectKind::Host
        | PortableObjectKind::Route
        | PortableObjectKind::AuthenticationPlan
        | PortableObjectKind::AlgorithmPolicy
        | PortableObjectKind::HeartbeatPolicy
        | PortableObjectKind::MonitoringPolicy
        | PortableObjectKind::LoginAutomation => PluginDataCategory::Hosts,
        PortableObjectKind::Identity
        | PortableObjectKind::Credential
        | PortableObjectKind::Secret => PluginDataCategory::Credentials,
        PortableObjectKind::DesktopProfile => PluginDataCategory::DesktopProfiles,
    })
}

/// Project only after the source has passed its original schema validation.
/// Decrypts and validates the exact downloaded bytes before removing any
/// legacy preference fields from the in-memory projection.
pub(crate) fn inspect_three_category_blob(
    bytes: &[u8],
    key: &SyncKey,
    plugin_id: &str,
    data_owner_sha256: &str,
    profile_id: &str,
    categories: &[PluginDataCategory],
) -> Result<DataExchangeInspection, BrokerError> {
    validate_categories(categories)?;
    let (binding, _) =
        inspect_plugin_exchange_owner(bytes, plugin_id, data_owner_sha256, profile_id)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
    let source = open_plugin_exchange_with_key(bytes, key, &binding)
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    let source_categories = source.selected_categories.clone();
    let migration_required = source.schema != BundleSchema::V6
        || source.preferences.is_some()
        || !source.preference_update_times.is_empty();
    let excluded_categories = if source.preferences.is_some() {
        vec![PluginDataCategory::AppPreferences]
    } else {
        Vec::new()
    };
    let bundle = project_selected_bundle(&source, categories)?;
    Ok(DataExchangeInspection {
        migration_required,
        excluded_categories,
        binding,
        exchange_sha256: sha256_hex(bytes),
        bundle,
        categories: categories.to_vec(),
        source_categories,
        source_receipt: None,
        remote_origin: String::new(),
        remote_resource_url: String::new(),
        key: key.clone(),
    })
}

/// Seals an already selected Core snapshot. The caller must bind `binding` to
/// a verified base receipt and recheck the local snapshot before publication.
pub(crate) fn export_three_category_blob(
    snapshot: &PortableBundleV1,
    key: &SyncKey,
    password_wrapped_envelope: &[u8],
    binding: PluginExchangeBinding,
    categories: &[PluginDataCategory],
) -> Result<DataExchangeBlob, BrokerError> {
    let bundle = project_selected_bundle(snapshot, categories)?;
    bundle
        .validate_current_business_exchange()
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    if binding.revision != bundle.revision {
        return Err(BrokerError::StateConflict);
    }
    let keyed_content_sha256 = portable_content_sha256(&bundle, key)?;
    let objects = describe_objects(&bundle, key, categories)?;
    // Canonicalize before sealing so malformed snapshots fail before a network
    // bridge can acquire any bytes.
    canonical_bundle_bytes(&bundle).map_err(|_| BrokerError::RemoteDataInvalid)?;
    let bytes = create_plugin_exchange_with_key(&bundle, key, password_wrapped_envelope, &binding)
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    let exchange_sha256 = sha256_hex(&bytes);
    Ok(DataExchangeBlob {
        bytes,
        objects,
        binding,
        exchange_sha256,
        keyed_content_sha256,
        idempotency_key: uuid::Uuid::new_v4().to_string(),
        source_remote_exchange_sha256: None,
        profile_state_version: None,
    })
}

impl SshSyncExchangeBroker {
    /// Resolve the existing local namespace before a snapshot can create any
    /// profile state. A package signer change must not hide legacy data.
    pub(crate) async fn data_resolve_local_owner(
        &self,
        plugin_id: &str,
        current_signer: &str,
        profile_id: &str,
    ) -> Result<String, BrokerError> {
        if !super::valid_sha256(current_signer) || profile_id.is_empty() {
            return Err(BrokerError::OperationRejected(
                "broker.data_exchange.owner_identity_invalid",
            ));
        }
        let owners = self
            .profiles
            .existing_data_owners(plugin_id.to_owned(), profile_id.to_owned())
            .await
            .map_err(super::map_store_error)?;
        if owners.len() <= 1 {
            return super::select_local_data_owner(plugin_id, &owners);
        }
        // Older packages can leave an unbound provisional namespace next to
        // the established profile. Only an existing remote baseline can
        // identify that profile before the plugin downloads the exchange.
        if owners.iter().any(|owner| !super::valid_sha256(owner)) {
            return Err(BrokerError::OwnerConflict);
        }
        let mut established = None;
        for owner in owners {
            let profile = self
                .profiles
                .existing_profile_state(plugin_id.to_owned(), owner.clone(), profile_id.to_owned())
                .await
                .map_err(super::map_store_error)?;
            if profile.is_some_and(|profile| profile.remote_baseline.is_some())
                && established.replace(owner).is_some()
            {
                return Err(BrokerError::OwnerConflict);
            }
        }
        established.ok_or(BrokerError::OwnerConflict)
    }

    pub(crate) fn data_compose(
        &self,
        local: Arc<DataLocalSnapshot>,
        remote: Arc<DataExchangeInspection>,
        decisions: &[DataObjectDecision],
    ) -> Result<DataComposedSnapshot, BrokerError> {
        compose_selected_objects(local, remote, decisions)
    }

    pub(crate) fn data_describe_local(
        &self,
        snapshot: &DataLocalSnapshot,
    ) -> Result<Vec<DataObjectDescriptor>, BrokerError> {
        let Some(key) = snapshot.key.as_ref() else {
            return Ok(Vec::new());
        };
        describe_objects(&snapshot.selected_bundle, key, &snapshot.categories)
    }

    pub(crate) fn data_describe_inspection(
        &self,
        inspection: &DataExchangeInspection,
    ) -> Result<Vec<DataObjectDescriptor>, BrokerError> {
        describe_objects(&inspection.bundle, &inspection.key, &inspection.categories)
    }

    pub(crate) fn data_describe_composed(
        &self,
        composed: &DataComposedSnapshot,
    ) -> Result<Vec<DataObjectDescriptor>, BrokerError> {
        describe_objects(
            &composed.bundle,
            &composed.remote.key,
            &composed.local.categories,
        )
    }

    /// Presents both fully staged outcomes in one Core-owned protected review.
    /// The chosen restore and approval remain private to the returned handle.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the two candidate identities and authenticated GET explicit"
    )]
    pub(crate) async fn data_review_choices(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        local: Arc<DataLocalSnapshot>,
        remote: Arc<DataExchangeInspection>,
        base_receipt: &NetworkReceipt,
        local_choice: Arc<DataComposedSnapshot>,
        remote_choice: Arc<DataComposedSnapshot>,
        categories: &[PluginDataCategory],
        fence: &ActionFence,
    ) -> Result<(DataComposedSnapshot, PluginDataObjectSource), BrokerError> {
        validate_categories(categories)?;
        if local.plugin_id != plugin_id
            || local.data_owner_sha256 != data_owner_sha256
            || local.profile_id != profile_id
            || remote.binding.plugin_id != plugin_id
            || remote.binding.signer_fingerprint_sha256 != data_owner_sha256
            || remote.binding.profile_id != profile_id
            || local.categories != categories
            || remote.categories != categories
            || !Arc::ptr_eq(&local, &local_choice.local)
            || !Arc::ptr_eq(&local, &remote_choice.local)
            || !Arc::ptr_eq(&remote, &local_choice.remote)
            || !Arc::ptr_eq(&remote, &remote_choice.remote)
            || local_choice.reviewed.is_some()
            || remote_choice.reviewed.is_some()
        {
            return Err(BrokerError::OwnerConflict);
        }
        if !valid_review_base(&remote, base_receipt) {
            return Err(BrokerError::RemoteDataInvalid);
        }
        if !self.vault.is_unlocked() || !fence() {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, &local)
            .await
            .map_err(|error| local_state_error_at(error, "data_review.before_stage"))?;
        let local_bundle = with_selected_tombstones_for_missing(
            &local_choice.bundle,
            &local.snapshot.bundle,
            categories,
        )?;
        let remote_bundle = with_selected_tombstones_for_missing(
            &remote_choice.bundle,
            &local.snapshot.bundle,
            categories,
        )?;
        let local_restore = self
            .profiles
            .stage_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                local_bundle,
                remote.key.clone(),
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_review.stage_local")
            })?;
        let mut local_cleanup = Some(StagedRestoreCleanup {
            profiles: self.profiles.clone(),
            handle: local_restore.handle.clone(),
        });
        if local_restore.conflict_count != 0 {
            return Err(BrokerError::RestoreConflict);
        }
        let remote_restore = self
            .profiles
            .stage_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                remote_bundle,
                remote.key.clone(),
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_review.stage_remote")
            })?;
        let mut remote_cleanup = Some(StagedRestoreCleanup {
            profiles: self.profiles.clone(),
            handle: remote_restore.handle.clone(),
        });
        if remote_restore.conflict_count != 0 {
            return Err(BrokerError::RestoreConflict);
        }
        let review = SyncDataReview {
            local_before: data_counts(&local.selected_bundle),
            remote_before: data_counts(&remote.bundle),
            local_choice: SyncDataReviewChoice {
                restore_handle: local_restore.handle.clone(),
                view: data_review_choice(
                    PluginDataObjectSource::Local,
                    &local,
                    &remote,
                    &local_choice.bundle,
                )?,
            },
            remote_choice: SyncDataReviewChoice {
                restore_handle: remote_restore.handle.clone(),
                view: data_review_choice(
                    PluginDataObjectSource::Remote,
                    &local,
                    &remote,
                    &remote_choice.bundle,
                )?,
            },
        };
        let (source, approval) = self
            .secure_ui
            .review_data_choices(
                self.secure_context(
                    plugin_id,
                    data_owner_sha256,
                    profile_id,
                    Some(remote.remote_origin.clone()),
                    fence,
                ),
                review,
            )
            .await
            .map_err(super::map_selection_error)?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, &local)
            .await
            .map_err(|error| local_state_error_at(error, "data_review.after_approval"))?;
        let (chosen, restore_handle, cleanup) = match source {
            PluginDataObjectSource::Local => (
                &local_choice,
                local_restore.handle,
                local_cleanup.take().ok_or(BrokerError::StateConflict)?,
            ),
            PluginDataObjectSource::Remote => (
                &remote_choice,
                remote_restore.handle,
                remote_cleanup.take().ok_or(BrokerError::StateConflict)?,
            ),
        };
        self.profiles
            .validate_staged_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                restore_handle.clone(),
                approval.clone(),
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_review.plan_expired")
            })?;
        Ok((
            DataComposedSnapshot {
                bundle: chosen.bundle.clone(),
                local,
                remote,
                reviewed: Some(Arc::new(ReviewedDataPlan {
                    base_receipt: base_receipt.clone(),
                    restore_handle,
                    approval: Mutex::new(Some(approval)),
                    _cleanup: cleanup,
                })),
            },
            source,
        ))
    }

    /// Captures Core data with stable portable IDs. The returned plaintext is
    /// held only by the host-side snapshot registry, never by plugin code.
    pub(crate) async fn data_snapshot_local(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        categories: &[PluginDataCategory],
        fence: &ActionFence,
    ) -> Result<DataLocalSnapshot, BrokerError> {
        validate_categories(categories)?;
        self.ensure_vault_unlocked(plugin_id, data_owner_sha256, profile_id, fence)
            .await?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let snapshot = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(super::map_store_error)?;
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let (key, envelope) = if let Some(binding) = &profile.key_binding {
            let value = self
                .vault
                .read_secret(
                    &binding.secret_ref_id,
                    norishell_secret_vault::SecretKind::SshSyncKey,
                )
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            let bytes: [u8; 32] = value
                .expose()
                .try_into()
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            (
                Some(SyncKey::from_bytes(bytes)),
                Some(binding.password_wrapped_envelope.clone()),
            )
        } else {
            (None, None)
        };
        let mut selected_bundle = project_selected_bundle(&snapshot.bundle, categories)?;
        if let Some(key) = key.as_ref()
            && let Some(base) = self
                .open_profile_baseline(plugin_id, data_owner_sha256, profile_id, &profile, key)
                .await?
        {
            let base = project_selected_bundle(&base, categories)?;
            selected_bundle = self
                .profiles
                .prepare_local_merge(
                    plugin_id.to_owned(),
                    data_owner_sha256.to_owned(),
                    profile_id.to_owned(),
                    base,
                    selected_bundle,
                )
                .await
                .map_err(super::map_store_error)?;
        }
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        Ok(DataLocalSnapshot {
            plugin_id: plugin_id.to_owned(),
            data_owner_sha256: data_owner_sha256.to_owned(),
            profile_id: profile_id.to_owned(),
            snapshot,
            selected_bundle,
            categories: categories.to_vec(),
            profile_state_version: profile.state_version,
            key_pending: key.is_none(),
            key,
            envelope,
        })
    }

    /// Opens a plugin-transported blob without performing any network action.
    /// Existing key recovery remains Core-owned, including its secure prompt.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_inspect_blob(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        blob: &ExchangeBlob,
        receipt: &NetworkReceipt,
        categories: &[PluginDataCategory],
        fence: &ActionFence,
    ) -> Result<DataExchangeInspection, BrokerError> {
        validate_categories(categories)?;
        // The caller must obtain both handles from Core's owner/profile/fence
        // checked registry. A plugin supplied origin is never trusted here.
        if receipt.method != "GET"
            || receipt.status != 200
            || reqwest::Url::parse(&receipt.resource_url)
                .ok()
                .is_none_or(|url| url.origin().ascii_serialization() != receipt.endpoint_origin)
            || receipt.response_blob_handle.as_deref() != Some(blob.handle.as_str())
            || receipt.response_body_sha256.as_deref() != Some(blob.sha256.as_str())
            || sha256_hex(blob.bytes.as_ref().as_slice()) != blob.sha256
            || super::normalized_resource_origin(&receipt.endpoint_origin)
                .ok()
                .as_deref()
                != Some(receipt.endpoint_origin.as_str())
        {
            return Err(BrokerError::RemoteDataInvalid);
        }
        let bytes = blob.bytes.as_ref().as_slice().to_vec();
        self.ensure_vault_unlocked(plugin_id, data_owner_sha256, profile_id, fence)
            .await?;
        let (binding, _) = inspect_plugin_exchange_data_owner(&bytes, plugin_id, profile_id)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        if receipt.response_revision != Some(binding.revision)
            || !receipt
                .etag
                .as_deref()
                .is_some_and(super::valid_strong_etag)
        {
            return Err(BrokerError::RemoteDataInvalid);
        }
        // Envelope owner metadata is untrusted until the ciphertext opens.
        // A changed package signer may leave local scope under an older owner.
        let authenticated_remote_key = if binding.signer_fingerprint_sha256 != data_owner_sha256 {
            let remote = DownloadedExchange {
                http_status: 200,
                remote_origin: receipt.endpoint_origin.clone(),
                etag: String::new(),
                remote_updated_at_unix_ms: None,
                binding: binding.clone(),
                bytes: bytes.clone(),
            };
            let key = self
                .authenticate_data_owner(plugin_id, profile_id, &remote, fence)
                .await?;
            self.reconcile_local_data_owner(
                plugin_id,
                profile_id,
                &remote.binding.signer_fingerprint_sha256,
            )
            .await
            .map_err(|_| BrokerError::OwnerConflict)?;
            Some((sha256_hex(&bytes), key))
        } else {
            None
        };
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                binding.signer_fingerprint_sha256.clone(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        let remote = DownloadedExchange {
            http_status: 200,
            remote_origin: receipt.endpoint_origin.clone(),
            etag: String::new(),
            remote_updated_at_unix_ms: None,
            binding,
            bytes,
        };
        let mut broker = self.clone();
        broker.authenticated_remote_key = authenticated_remote_key;
        let (_, _, key) = broker
            .open_downloaded_exchange(
                plugin_id,
                &remote.binding.signer_fingerprint_sha256,
                profile_id,
                &remote,
                profile,
                fence,
            )
            .await?;
        self.data_reconcile_previous_upload(
            plugin_id,
            &remote.binding.signer_fingerprint_sha256,
            profile_id,
            receipt,
            Some((&remote, &key)),
        )
        .await?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let mut inspected = inspect_three_category_blob(
            &remote.bytes,
            &key,
            plugin_id,
            &remote.binding.signer_fingerprint_sha256,
            profile_id,
            categories,
        )?;
        inspected.remote_origin = remote.remote_origin;
        inspected.remote_resource_url = receipt.resource_url.clone();
        inspected.source_receipt = Some(receipt.clone());
        Ok(inspected)
    }

    /// Seals a captured snapshot only if its exact profile revision and local
    /// content remain current. The caller supplies a binding derived from its
    /// verified network receipt; this method never sends the blob.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_export_snapshot(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        captured: &DataLocalSnapshot,
        binding: PluginExchangeBinding,
        base_receipt: &NetworkReceipt,
        categories: &[PluginDataCategory],
        fence: &ActionFence,
    ) -> Result<DataExchangeBlob, BrokerError> {
        validate_categories(categories)?;
        if captured.plugin_id != plugin_id
            || captured.data_owner_sha256 != data_owner_sha256
            || captured.profile_id != profile_id
            || binding.plugin_id != plugin_id
            || binding.signer_fingerprint_sha256 != data_owner_sha256
            || binding.profile_id != profile_id
            || captured.categories != categories
        {
            return Err(BrokerError::OwnerConflict);
        }
        validate_export_base(&binding, base_receipt)?;
        if base_receipt.status != 404 {
            return Err(BrokerError::OperationRejected(
                "broker.data_exchange.local_export_requires_absent_remote",
            ));
        }
        if !fence() || !self.vault.is_unlocked() {
            return Err(BrokerError::StateConflict);
        }
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        if profile.state_version != captured.profile_state_version {
            return Err(BrokerError::StateConflict);
        }
        self.data_reconcile_previous_upload(
            plugin_id,
            data_owner_sha256,
            profile_id,
            base_receipt,
            None,
        )
        .await?;
        let current = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(super::map_store_error)?;
        if canonical_bundle_bytes(&current.bundle).map_err(|_| BrokerError::StateConflict)?
            != canonical_bundle_bytes(&captured.snapshot.bundle)
                .map_err(|_| BrokerError::StateConflict)?
        {
            return Err(BrokerError::StateConflict);
        }
        let (key, envelope, bound_profile) = self
            .sync_key_for_upload(plugin_id, data_owner_sha256, profile_id, profile)
            .await?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let fresh = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(super::map_store_error)?;
        if canonical_bundle_bytes(&fresh.bundle).map_err(|_| BrokerError::StateConflict)?
            != canonical_bundle_bytes(&captured.snapshot.bundle)
                .map_err(|_| BrokerError::StateConflict)?
        {
            return Err(BrokerError::LocalStateChanged);
        }
        let mut bundle = captured.selected_bundle.clone();
        bundle.revision = binding.revision;
        let mut exported =
            export_three_category_blob(&bundle, &key, &envelope, binding, categories)?;
        exported.profile_state_version = Some(bound_profile.state_version);
        Ok(exported)
    }

    /// Seals the exact Core-composed item selection against a verified GET
    /// baseline. The plugin selects objects, while Core checks their graph.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_export_composed(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        composed: &DataComposedSnapshot,
        binding: PluginExchangeBinding,
        base_receipt: &NetworkReceipt,
        fence: &ActionFence,
    ) -> Result<DataExchangeBlob, BrokerError> {
        let local = &composed.local;
        let remote = &composed.remote;
        if local.plugin_id != plugin_id
            || local.data_owner_sha256 != data_owner_sha256
            || local.profile_id != profile_id
            || remote.binding.plugin_id != plugin_id
            || remote.binding.signer_fingerprint_sha256 != data_owner_sha256
            || remote.binding.profile_id != profile_id
            || binding.plugin_id != plugin_id
            || binding.signer_fingerprint_sha256 != data_owner_sha256
            || binding.profile_id != profile_id
        {
            return Err(BrokerError::OwnerConflict);
        }
        validate_export_base(&binding, base_receipt)?;
        if base_receipt.status != 200
            || base_receipt.response_body_sha256.as_deref() != Some(remote.exchange_sha256.as_str())
            || base_receipt.response_revision != Some(remote.binding.revision)
            || base_receipt.endpoint_origin != remote.remote_origin
            || base_receipt.resource_url != remote.remote_resource_url
            || !fence()
            || !self.vault.is_unlocked()
        {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, local)
            .await
            .map_err(|error| local_state_error_at(error, "data_export.local_snapshot"))?;
        if let Some(reviewed) = &composed.reviewed {
            if &reviewed.base_receipt != base_receipt {
                return Err(BrokerError::StateConflict);
            }
            let approval = reviewed.approval()?;
            self.profiles
                .validate_staged_restore(
                    plugin_id.to_owned(),
                    data_owner_sha256.to_owned(),
                    profile_id.to_owned(),
                    reviewed.restore_handle.clone(),
                    approval,
                )
                .await
                .map_err(|error| {
                    local_state_error_at(
                        super::map_store_error(error),
                        "data_export.review_expired",
                    )
                })?;
        }
        let mut bundle = composed.bundle.clone();
        bundle.revision = binding.revision;
        let key = local.key.as_ref().ok_or(BrokerError::LocalKeyUnavailable)?;
        let envelope = local
            .envelope
            .as_ref()
            .ok_or(BrokerError::LocalKeyUnavailable)?;
        let mut exported =
            export_three_category_blob(&bundle, key, envelope, binding, &local.categories)?;
        exported.source_remote_exchange_sha256 = Some(remote.exchange_sha256.clone());
        exported.profile_state_version = Some(local.profile_state_version);
        Ok(exported)
    }

    pub(crate) async fn data_apply_composed(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        composed: &DataComposedSnapshot,
        uploaded: bool,
        fence: &ActionFence,
    ) -> Result<DataApplyReceipt, BrokerError> {
        let mut selected = (*composed.remote).clone();
        selected.bundle = composed.bundle.clone();
        if let Some(reviewed) = &composed.reviewed {
            return self
                .data_apply_reviewed(
                    plugin_id,
                    data_owner_sha256,
                    profile_id,
                    &composed.local,
                    &selected,
                    reviewed,
                    fence,
                )
                .await;
        }
        self.data_apply_inspected(
            plugin_id,
            data_owner_sha256,
            profile_id,
            &composed.local,
            &selected,
            &composed.local.categories,
            uploaded,
            fence,
        )
        .await
    }

    /// Restores only selected categories through staged local CAS and secure
    /// approval. Missing objects outside the selection never become tombstones.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_apply_inspected(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        expected_local: &DataLocalSnapshot,
        inspected: &DataExchangeInspection,
        categories: &[PluginDataCategory],
        uploaded: bool,
        fence: &ActionFence,
    ) -> Result<DataApplyReceipt, BrokerError> {
        validate_categories(categories)?;
        if expected_local.plugin_id != plugin_id
            || expected_local.data_owner_sha256 != data_owner_sha256
            || expected_local.profile_id != profile_id
            || inspected.binding.plugin_id != plugin_id
            || inspected.binding.signer_fingerprint_sha256 != data_owner_sha256
            || inspected.binding.profile_id != profile_id
            || inspected.remote_origin.is_empty()
            || expected_local.categories != categories
            || inspected.categories != categories
        {
            return Err(BrokerError::OwnerConflict);
        }
        if !self.vault.is_unlocked() || !fence() {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, expected_local)
            .await
            .map_err(|error| local_state_error_at(error, "data_apply.before_stage"))?;
        let staged_bundle = with_selected_tombstones_for_missing(
            &inspected.bundle,
            &expected_local.snapshot.bundle,
            categories,
        )?;
        let restore = self
            .profiles
            .stage_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                staged_bundle,
                inspected.key.clone(),
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_apply.stage_restore")
            })?;
        let _cleanup = StagedRestoreCleanup {
            profiles: self.profiles.clone(),
            handle: restore.handle.clone(),
        };
        if restore.conflict_count > 0 {
            return Err(BrokerError::RestoreConflict);
        }
        let approval = self
            .secure_ui
            .approve_data_apply(
                self.secure_context(
                    plugin_id,
                    data_owner_sha256,
                    profile_id,
                    Some(inspected.remote_origin.clone()),
                    fence,
                ),
                data_apply_review(expected_local, inspected, &restore),
                uploaded,
            )
            .await
            .map_err(super::map_selection_error)?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, expected_local)
            .await
            .map_err(|error| local_state_error_at(error, "data_apply.after_approval"))?;
        self.profiles
            .apply_staged_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                restore.handle,
                Some(approval),
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_apply.commit_restore")
            })?;
        self.finish_data_apply(
            plugin_id,
            data_owner_sha256,
            profile_id,
            inspected,
            categories,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the approved owner and staged plan identity explicit"
    )]
    async fn data_apply_reviewed(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        expected_local: &DataLocalSnapshot,
        inspected: &DataExchangeInspection,
        reviewed: &ReviewedDataPlan,
        fence: &ActionFence,
    ) -> Result<DataApplyReceipt, BrokerError> {
        if expected_local.plugin_id != plugin_id
            || expected_local.data_owner_sha256 != data_owner_sha256
            || expected_local.profile_id != profile_id
            || inspected.binding.plugin_id != plugin_id
            || inspected.binding.signer_fingerprint_sha256 != data_owner_sha256
            || inspected.binding.profile_id != profile_id
            || expected_local.categories != inspected.categories
        {
            return Err(BrokerError::OwnerConflict);
        }
        if !valid_review_base(inspected, &reviewed.base_receipt) {
            return Err(BrokerError::StateConflict);
        }
        if !self.vault.is_unlocked() || !fence() {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, expected_local)
            .await
            .map_err(|error| local_state_error_at(error, "data_apply.reviewed_snapshot"))?;
        let approval = reviewed.approval()?;
        self.profiles
            .validate_staged_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                reviewed.restore_handle.clone(),
                approval,
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_apply.review_expired")
            })?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let approval = reviewed.consume_approval()?;
        self.profiles
            .apply_staged_restore(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                reviewed.restore_handle.clone(),
                Some(approval),
            )
            .await
            .map_err(|error| {
                local_state_error_at(super::map_store_error(error), "data_apply.commit_restore")
            })?;
        self.finish_data_apply(
            plugin_id,
            data_owner_sha256,
            profile_id,
            inspected,
            &expected_local.categories,
        )
        .await
    }

    async fn finish_data_apply(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        inspected: &DataExchangeInspection,
        categories: &[PluginDataCategory],
    ) -> Result<DataApplyReceipt, BrokerError> {
        let mut current = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                inspected.binding.revision,
            )
            .await
            .map_err(super::map_store_error)?;
        current.bundle = project_selected_bundle(&current.bundle, categories)?;
        current.bundle = self
            .profiles
            .prepare_local_merge(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                inspected.bundle.clone(),
                current.bundle,
            )
            .await
            .map_err(super::map_store_error)?;
        let keyed_content_sha256 = portable_content_sha256(&current.bundle, &inspected.key)?;
        if keyed_content_sha256 != portable_content_sha256(&inspected.bundle, &inspected.key)? {
            return Err(BrokerError::LocalStateChangedAt(
                "data_apply.applied_content",
            ));
        }
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        Ok(DataApplyReceipt {
            source_exchange_sha256: inspected.exchange_sha256.clone(),
            keyed_content_sha256,
            current,
            profile_state_version: profile.state_version,
        })
    }

    async fn check_local_snapshot(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        expected: &DataLocalSnapshot,
    ) -> Result<PortableProfileState, BrokerError> {
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        if profile.state_version != expected.profile_state_version {
            return Err(BrokerError::StateConflict);
        }
        let current = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(super::map_store_error)?;
        if canonical_bundle_bytes(&current.bundle).map_err(|_| BrokerError::LocalStateChanged)?
            != canonical_bundle_bytes(&expected.snapshot.bundle)
                .map_err(|_| BrokerError::LocalStateChanged)?
        {
            return Err(BrokerError::LocalStateChanged);
        }
        Ok(profile)
    }

    /// Commits a V6 upload baseline only after a Core-issued HTTP receipt
    /// proves the exact sealed body was accepted under the expected CAS fence.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_checkpoint_upload(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        expected_profile_revision: WireSequence,
        expected_local: &DataLocalSnapshot,
        exported: &DataExchangeBlob,
        base_receipt: &NetworkReceipt,
        upload_receipt: &NetworkReceipt,
        applied: Option<&DataApplyReceipt>,
        fence: &ActionFence,
    ) -> Result<(), BrokerError> {
        if expected_local.plugin_id != plugin_id
            || expected_local.data_owner_sha256 != data_owner_sha256
            || expected_local.profile_id != profile_id
            || exported.binding.plugin_id != plugin_id
            || exported.binding.signer_fingerprint_sha256 != data_owner_sha256
            || exported.binding.profile_id != profile_id
        {
            return Err(BrokerError::OwnerConflict);
        }
        validate_export_base(&exported.binding, base_receipt)?;
        if !fence() || !self.vault.is_unlocked() {
            return Err(BrokerError::StateConflict);
        }
        if upload_receipt.method != "PUT"
            || !matches!(upload_receipt.status, 200 | 201)
            || upload_receipt.endpoint_origin != base_receipt.endpoint_origin
            || upload_receipt.resource_url != base_receipt.resource_url
            || upload_receipt.response_revision != Some(exported.binding.revision)
            || !upload_receipt
                .etag
                .as_deref()
                .is_some_and(super::valid_strong_etag)
            || upload_receipt.request_body_sha256.as_deref()
                != Some(exported.exchange_sha256.as_str())
            || upload_receipt.request_idempotency_key.as_deref()
                != Some(exported.idempotency_key.as_str())
            || sha256_hex(&exported.bytes) != exported.exchange_sha256
        {
            return Err(BrokerError::RemoteDataInvalid);
        }
        match base_receipt.status {
            200 if upload_receipt.request_if_match == base_receipt.etag
                && upload_receipt.request_expected_next_revision.is_none() => {}
            404 if upload_receipt.request_if_match.is_none()
                && upload_receipt.request_expected_next_revision
                    == Some(exported.binding.revision) => {}
            _ => return Err(BrokerError::StateConflict),
        }
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        if profile.state_version != expected_profile_revision {
            return Err(BrokerError::StateConflict);
        }
        if applied.map_or(
            exported.profile_state_version != Some(expected_profile_revision),
            |receipt| receipt.profile_state_version != expected_profile_revision,
        ) {
            return Err(BrokerError::StateConflict);
        }
        if self
            .profiles
            .pending_upload_attempt(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?
            .is_some()
        {
            return Err(BrokerError::StateConflict);
        }
        let binding = profile
            .key_binding
            .as_ref()
            .ok_or(BrokerError::LocalKeyUnavailable)?;
        let value = self
            .vault
            .read_secret(
                &binding.secret_ref_id,
                norishell_secret_vault::SecretKind::SshSyncKey,
            )
            .map_err(|_| BrokerError::LocalKeyUnavailable)?;
        let key_bytes: [u8; 32] = value
            .expose()
            .try_into()
            .map_err(|_| BrokerError::LocalKeyUnavailable)?;
        let key = SyncKey::from_bytes(key_bytes);
        let bundle = open_plugin_exchange_with_key(&exported.bytes, &key, &exported.binding)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        if bundle.validate_current_business_exchange().is_err()
            || portable_content_sha256(&bundle, &key)? != exported.keyed_content_sha256
        {
            return Err(BrokerError::RemoteDataInvalid);
        }
        let current = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(super::map_store_error)?;
        if let Some(applied) = applied {
            if exported.source_remote_exchange_sha256.as_deref()
                != Some(applied.source_exchange_sha256.as_str())
                || applied.keyed_content_sha256 != exported.keyed_content_sha256
            {
                return Err(BrokerError::StateConflict);
            }
        } else if canonical_bundle_bytes(&current.bundle)
            .map_err(|_| BrokerError::LocalStateChanged)?
            != canonical_bundle_bytes(&expected_local.snapshot.bundle)
                .map_err(|_| BrokerError::LocalStateChanged)?
        {
            return Err(BrokerError::LocalStateChanged);
        }
        let mut selected_current =
            project_selected_bundle(&current.bundle, &expected_local.categories)?;
        selected_current.revision = exported.binding.revision;
        selected_current = self
            .profiles
            .prepare_local_merge(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                bundle.clone(),
                selected_current,
            )
            .await
            .map_err(super::map_store_error)?;
        // Apply and checkpoint compare the same portable business content.
        // Machine-bound skip notices describe the exporting device, not data
        // that a receiving device can restore.
        if portable_content_sha256(&selected_current, &key)? != exported.keyed_content_sha256 {
            return Err(BrokerError::LocalStateChangedAt(
                "data_checkpoint.upload_content",
            ));
        }
        let exchange_sha256 = self
            .persist_baseline_exchange(plugin_id, data_owner_sha256, profile_id, &exported.bytes)
            .await?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let etag = upload_receipt
            .etag
            .clone()
            .ok_or(BrokerError::RemoteDataInvalid)?;
        self.profiles
            .update_remote_baseline(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                expected_profile_revision,
                PortableRemoteBaseline {
                    revision: exported.binding.revision,
                    etag: etag.clone(),
                    content_sha256: exported.keyed_content_sha256.clone(),
                    exchange_sha256: exchange_sha256.clone(),
                },
            )
            .await
            .map_err(super::map_store_error)?;
        Ok(())
    }

    /// Records a downloaded V6 baseline only after the exact authenticated
    /// blob has been applied locally and that applied snapshot is still current.
    /// Legacy V5 downloads must first be exported and committed as V6.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_checkpoint_download(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        expected_profile_revision: WireSequence,
        blob: &ExchangeBlob,
        receipt: &NetworkReceipt,
        inspected: &DataExchangeInspection,
        applied: &DataApplyReceipt,
        fence: &ActionFence,
    ) -> Result<(), BrokerError> {
        if inspected.binding.plugin_id != plugin_id
            || inspected.binding.signer_fingerprint_sha256 != data_owner_sha256
            || inspected.binding.profile_id != profile_id
        {
            return Err(BrokerError::OwnerConflict);
        }
        if inspected.migration_required
            || inspected.source_categories.as_ref().map_or(
                inspected.categories.len() != 3,
                |source| {
                    let mut selected = inspected
                        .categories
                        .iter()
                        .copied()
                        .map(portable_category)
                        .collect::<Vec<_>>();
                    selected.sort();
                    source != &selected
                },
            )
            || receipt.method != "GET"
            || receipt.status != 200
            || receipt.endpoint_origin != inspected.remote_origin
            || receipt.resource_url != inspected.remote_resource_url
            || receipt.response_blob_handle.as_deref() != Some(blob.handle.as_str())
            || receipt.response_body_sha256.as_deref() != Some(blob.sha256.as_str())
            || receipt.response_revision != Some(inspected.binding.revision)
            || !receipt
                .etag
                .as_deref()
                .is_some_and(super::valid_strong_etag)
            || sha256_hex(blob.bytes.as_ref().as_slice()) != blob.sha256
            || inspected.exchange_sha256 != blob.sha256
            || applied.source_exchange_sha256 != blob.sha256
            || applied.keyed_content_sha256
                != portable_content_sha256(&inspected.bundle, &inspected.key)?
            || !fence()
            || !self.vault.is_unlocked()
        {
            return Err(BrokerError::StateConflict);
        }
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?;
        if profile.state_version != expected_profile_revision {
            return Err(BrokerError::StateConflict);
        }
        if applied.profile_state_version != expected_profile_revision {
            return Err(BrokerError::StateConflict);
        }
        let mut current = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                inspected.binding.revision,
            )
            .await
            .map_err(super::map_store_error)?;
        current.bundle = project_selected_bundle(&current.bundle, &inspected.categories)?;
        current.bundle = self
            .profiles
            .prepare_local_merge(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                inspected.bundle.clone(),
                current.bundle,
            )
            .await
            .map_err(super::map_store_error)?;
        if portable_content_sha256(&current.bundle, &inspected.key)? != applied.keyed_content_sha256
            || canonical_bundle_bytes(&current.bundle)
                .map_err(|_| BrokerError::LocalStateChanged)?
                != canonical_bundle_bytes(&applied.current.bundle)
                    .map_err(|_| BrokerError::LocalStateChanged)?
        {
            return Err(BrokerError::LocalStateChanged);
        }
        let exchange_sha256 = self
            .persist_baseline_exchange(
                plugin_id,
                data_owner_sha256,
                profile_id,
                blob.bytes.as_ref().as_slice(),
            )
            .await?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let etag = receipt.etag.clone().ok_or(BrokerError::RemoteDataInvalid)?;
        self.profiles
            .update_remote_baseline(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                expected_profile_revision,
                PortableRemoteBaseline {
                    revision: inspected.binding.revision,
                    etag: etag.clone(),
                    content_sha256: applied.keyed_content_sha256.clone(),
                    exchange_sha256: exchange_sha256.clone(),
                },
            )
            .await
            .map_err(super::map_store_error)?;
        Ok(())
    }

    /// Establishes an authenticated remote baseline when both sides already
    /// contain the same selected data. No local restore is needed in this case.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep identity, signed state and action fence explicit at this boundary"
    )]
    pub(crate) async fn data_checkpoint_equal(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        local: &DataLocalSnapshot,
        inspected: &DataExchangeInspection,
        blob: &ExchangeBlob,
        receipt: &NetworkReceipt,
        fence: &ActionFence,
    ) -> Result<(), BrokerError> {
        if local.plugin_id != plugin_id
            || local.data_owner_sha256 != data_owner_sha256
            || local.profile_id != profile_id
            || local.categories != inspected.categories
            || inspected.binding.plugin_id != plugin_id
            || inspected.binding.signer_fingerprint_sha256 != data_owner_sha256
            || inspected.binding.profile_id != profile_id
            || inspected.migration_required
            || inspected
                .source_categories
                .as_ref()
                .is_some_and(|source| source.len() != 3)
            || receipt.method != "GET"
            || receipt.status != 200
            || receipt.endpoint_origin != inspected.remote_origin
            || receipt.resource_url != inspected.remote_resource_url
            || receipt.response_blob_handle.as_deref() != Some(blob.handle.as_str())
            || receipt.response_body_sha256.as_deref() != Some(blob.sha256.as_str())
            || receipt.response_revision != Some(inspected.binding.revision)
            || !receipt
                .etag
                .as_deref()
                .is_some_and(super::valid_strong_etag)
            || blob.sha256 != inspected.exchange_sha256
            || sha256_hex(blob.bytes.as_ref().as_slice()) != blob.sha256
            || !fence()
            || !self.vault.is_unlocked()
        {
            return Err(BrokerError::StateConflict);
        }
        self.check_local_snapshot(plugin_id, data_owner_sha256, profile_id, local)
            .await
            .map_err(|error| local_state_error_at(error, "data_checkpoint.local_snapshot"))?;
        let key = local.key.as_ref().ok_or(BrokerError::LocalKeyUnavailable)?;
        let local_content = portable_content_sha256(&local.selected_bundle, key)?;
        let remote_content = portable_content_sha256(&inspected.bundle, key)?;
        if local_content != remote_content {
            return Err(BrokerError::LocalStateChangedAt(
                "data_checkpoint.equal_content",
            ));
        }
        let exchange_sha256 = self
            .persist_baseline_exchange(
                plugin_id,
                data_owner_sha256,
                profile_id,
                blob.bytes.as_ref().as_slice(),
            )
            .await?;
        if !fence() {
            return Err(BrokerError::StateConflict);
        }
        let etag = receipt.etag.clone().ok_or(BrokerError::RemoteDataInvalid)?;
        self.profiles
            .update_remote_baseline(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
                local.profile_state_version,
                PortableRemoteBaseline {
                    revision: inspected.binding.revision,
                    etag: etag.clone(),
                    content_sha256: remote_content,
                    exchange_sha256: exchange_sha256.clone(),
                },
            )
            .await
            .map_err(super::map_store_error)?;
        Ok(())
    }

    /// Retires a pre-upgrade upload attempt only when a Core-issued GET proves
    /// the exact attempted body landed, was superseded, or its base still owns
    /// the slot. Ambiguous outcomes remain pending rather than guessed away.
    async fn data_reconcile_previous_upload(
        &self,
        plugin_id: &str,
        data_owner_sha256: &str,
        profile_id: &str,
        receipt: &NetworkReceipt,
        authenticated_remote: Option<(&DownloadedExchange, &SyncKey)>,
    ) -> Result<(), BrokerError> {
        let Some(attempt) = self
            .profiles
            .pending_upload_attempt(
                plugin_id.to_owned(),
                data_owner_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(super::map_store_error)?
        else {
            return Ok(());
        };
        if attempt.canonical_url != receipt.resource_url
            || attempt.method != norishell_core_api::PluginSshSyncHttpMethod::Put
            || receipt.method != "GET"
        {
            return Ok(());
        }
        let fence = super::DurableUploadFence {
            canonical_url: attempt.canonical_url.clone(),
            method: attempt.method,
            use_oauth: attempt.use_oauth,
            action_revision: super::SshSyncActionRevision {
                authorization: attempt.authorization_revision,
                configuration: attempt.configuration_revision,
            },
        };
        let remote_fact = match (receipt.status, authenticated_remote) {
            (200, Some((remote, key))) => {
                if receipt.response_body_sha256.as_deref()
                    != Some(sha256_hex(&remote.bytes).as_str())
                    || receipt.response_revision != Some(remote.binding.revision)
                    || !receipt
                        .etag
                        .as_deref()
                        .is_some_and(super::valid_strong_etag)
                {
                    return Err(BrokerError::RemoteDataInvalid);
                }
                open_plugin_exchange_with_key(&remote.bytes, key, &remote.binding)
                    .map_err(|_| BrokerError::RemoteDataInvalid)?;
                Some((
                    remote.binding.revision,
                    receipt.etag.clone(),
                    Some(sha256_hex(&remote.bytes)),
                ))
            }
            (404, None) => Some((0, None, None)),
            _ => None,
        };
        let Some((revision, etag, body_sha256)) = remote_fact else {
            return Ok(());
        };
        if let Some(proof) = body_sha256.as_deref().and_then(|sha| {
            super::authenticated_upload_completion_proof(&attempt, revision, sha, etag.as_deref())
        }) {
            self.profiles
                .complete_upload_attempt(
                    plugin_id.to_owned(),
                    data_owner_sha256.to_owned(),
                    profile_id.to_owned(),
                    attempt.idempotency_key,
                    attempt.body_sha256,
                    fence,
                    proof,
                )
                .await
                .map_err(super::map_store_error)?;
            return Ok(());
        }
        let proof = if attempt.state == super::PortableUploadAttemptState::Prepared {
            Some(super::PortableUploadAbandonProof::PreparedNotSent)
        } else if revision == attempt.base_revision
            && etag == attempt.base_etag
            && body_sha256.as_deref() != Some(attempt.body_sha256.as_str())
        {
            Some(
                super::PortableUploadAbandonProof::AuthenticatedRemoteAtBase {
                    remote_revision: revision,
                    remote_etag: etag,
                    remote_body_sha256: body_sha256,
                },
            )
        } else {
            None
        };
        if let Some(proof) = proof {
            self.profiles
                .abandon_legacy_upload_attempt(
                    plugin_id.to_owned(),
                    data_owner_sha256.to_owned(),
                    profile_id.to_owned(),
                    attempt.idempotency_key,
                    attempt.body_sha256,
                    attempt.state_version,
                    fence,
                    proof,
                )
                .await
                .map_err(super::map_store_error)?;
        }
        Ok(())
    }
}

fn with_selected_tombstones_for_missing(
    target: &PortableBundleV1,
    current: &PortableBundleV1,
    categories: &[PluginDataCategory],
) -> Result<PortableBundleV1, BrokerError> {
    let mut result = target.clone();
    let target_ids = super::difference::object_keys(target);
    let current_ids = super::difference::object_keys(current);
    result.tombstones.retain(|item| {
        current_ids.contains(&(item.kind, item.id))
            && category_for_kind(item.kind).is_some_and(|category| categories.contains(&category))
    });
    let existing = result
        .tombstones
        .iter()
        .map(|item| (item.kind, item.id))
        .collect::<BTreeSet<_>>();
    result.tombstones.extend(
        current_ids
            .difference(&target_ids)
            .filter(|(kind, id)| {
                !existing.contains(&(*kind, *id))
                    && category_for_kind(*kind)
                        .is_some_and(|category| categories.contains(&category))
            })
            .map(|(kind, id)| PortableTombstone {
                kind: *kind,
                id: *id,
            }),
    );
    result.tombstones.sort_by_key(|item| (item.kind, item.id));
    let present = target_ids
        .into_iter()
        .chain(result.tombstones.iter().map(|item| (item.kind, item.id)))
        .collect::<BTreeSet<_>>();
    result
        .update_times
        .retain(|item| present.contains(&(item.kind, item.id)));
    result
        .validate_current_business_exchange()
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    Ok(result)
}

fn data_apply_review(
    expected: &DataLocalSnapshot,
    inspected: &DataExchangeInspection,
    restore: &RestorePreview,
) -> SyncDirectionReview {
    let report = super::difference::compare_bundles(&expected.selected_bundle, &inspected.bundle);
    SyncDirectionReview {
        merge_plan: false,
        local_desktop_profile_count: expected.selected_bundle.objects.desktop_profiles.len() as u32,
        remote_desktop_profile_count: inspected.bundle.objects.desktop_profiles.len() as u32,
        restore_handle: restore.handle.clone(),
        local_host_count: expected.selected_bundle.objects.hosts.len() as u32,
        local_credential_count: expected.selected_bundle.objects.credentials.len() as u32,
        remote_host_count: inspected.bundle.objects.hosts.len() as u32,
        remote_credential_count: inspected.bundle.objects.credentials.len() as u32,
        conflict_count: report.changed_count,
        delete_count: restore.delete_count,
        local_compared_at_unix_ms: crate::time::unix_time_ms(),
        remote_updated_at_unix_ms: None,
        differences: report.differences,
        difference_total_count: report.total_count,
        difference_omitted_count: report.omitted_count,
    }
}

fn data_counts(bundle: &PortableBundleV1) -> PluginDataLocalCounts {
    PluginDataLocalCounts {
        host_count: bundle.objects.hosts.len().try_into().unwrap_or(u32::MAX),
        credential_count: bundle
            .objects
            .credentials
            .len()
            .try_into()
            .unwrap_or(u32::MAX),
        desktop_profile_count: bundle
            .objects
            .desktop_profiles
            .len()
            .try_into()
            .unwrap_or(u32::MAX),
        tombstone_count: bundle.tombstones.len().try_into().unwrap_or(u32::MAX),
    }
}

fn data_review_projection(
    before: &PortableBundleV1,
    after: &PortableBundleV1,
) -> SshSyncSecureDataReviewProjection {
    let report = super::difference::compare_bundles(before, after);
    let removed = super::difference::object_keys(before)
        .difference(&super::difference::object_keys(after))
        .count();
    SshSyncSecureDataReviewProjection {
        host_count: after.objects.hosts.len().try_into().unwrap_or(u32::MAX),
        credential_count: after
            .objects
            .credentials
            .len()
            .try_into()
            .unwrap_or(u32::MAX),
        desktop_profile_count: after
            .objects
            .desktop_profiles
            .len()
            .try_into()
            .unwrap_or(u32::MAX),
        delete_count: removed.try_into().unwrap_or(u32::MAX),
        differences: report.differences,
        difference_total_count: report.total_count,
        difference_omitted_count: report.omitted_count,
        related_forward_rule_labels: Vec::new(),
    }
}

fn data_review_choice(
    source: PluginDataObjectSource,
    local: &DataLocalSnapshot,
    remote: &DataExchangeInspection,
    choice: &PortableBundleV1,
) -> Result<SshSyncSecureDataReviewChoice, BrokerError> {
    Ok(SshSyncSecureDataReviewChoice {
        source,
        upload_required: remote.migration_required
            || canonical_bundle_bytes(choice).map_err(|_| BrokerError::RemoteDataInvalid)?
                != canonical_bundle_bytes(&remote.bundle)
                    .map_err(|_| BrokerError::RemoteDataInvalid)?,
        local: data_review_projection(&local.selected_bundle, choice),
        remote: data_review_projection(&remote.bundle, choice),
    })
}

fn valid_review_base(remote: &DataExchangeInspection, receipt: &NetworkReceipt) -> bool {
    remote.source_receipt.as_ref() == Some(receipt)
        && receipt.method == "GET"
        && receipt.status == 200
        && receipt
            .etag
            .as_deref()
            .is_some_and(super::valid_strong_etag)
        && receipt.endpoint_origin == remote.remote_origin
        && receipt.resource_url == remote.remote_resource_url
        && receipt.response_body_sha256.as_deref() == Some(remote.exchange_sha256.as_str())
        && receipt.response_revision == Some(remote.binding.revision)
        && receipt.response_blob_handle.is_some()
        && reqwest::Url::parse(&receipt.resource_url)
            .ok()
            .is_some_and(|url| url.origin().ascii_serialization() == receipt.endpoint_origin)
}

fn local_state_error_at(error: BrokerError, diagnostic: &'static str) -> BrokerError {
    match error {
        BrokerError::LocalStateChanged => BrokerError::LocalStateChangedAt(diagnostic),
        error => error,
    }
}

fn validate_export_base(
    binding: &PluginExchangeBinding,
    receipt: &NetworkReceipt,
) -> Result<(), BrokerError> {
    if receipt.method != "GET"
        || reqwest::Url::parse(&receipt.resource_url)
            .ok()
            .is_none_or(|url| url.origin().ascii_serialization() != receipt.endpoint_origin)
        || super::normalized_resource_origin(&receipt.endpoint_origin)
            .ok()
            .as_deref()
            != Some(receipt.endpoint_origin.as_str())
    {
        return Err(BrokerError::RemoteDataInvalid);
    }
    match receipt.status {
        200 if receipt
            .etag
            .as_deref()
            .is_some_and(super::valid_strong_etag)
            && binding.base_revision == receipt.response_revision
            && binding.base_etag == receipt.etag
            && receipt
                .response_revision
                .is_some_and(|base| binding.revision == base + 1)
            && receipt.response_body_sha256.is_some()
            && receipt.response_blob_handle.is_some() =>
        {
            Ok(())
        }
        404 if binding.base_revision.is_none()
            && binding.base_etag.is_none()
            && receipt.response_next_revision == Some(binding.revision) =>
        {
            Ok(())
        }
        _ => Err(BrokerError::StateConflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_business_digest_ignores_device_skip_notices_but_preserves_clocks() {
        let key = SyncKey::from_bytes([19; 32]);
        let mut remote = super::super::empty_portable_bundle(2);
        let id = PortableObjectId::new();
        remote.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Host,
            id,
        });
        remote
            .update_times
            .push(norishell_ssh_profile_sync::PortableItemUpdateTime {
                kind: PortableObjectKind::Host,
                id,
                update_time_unix_ms: 100,
            });
        remote
            .skipped_machine_bound
            .push(norishell_ssh_profile_sync::SkippedMachineBoundObject {
                id: PortableObjectId::new(),
                kind: norishell_ssh_profile_sync::MachineBoundKind::SshAgent,
                reason: norishell_ssh_profile_sync::MachineBoundSkipReason::MachineBound,
            });
        let mut restored = remote.clone();
        restored.skipped_machine_bound.clear();
        let expected = portable_content_sha256(&remote, &key).unwrap();
        assert_eq!(portable_content_sha256(&restored, &key).unwrap(), expected);
        restored.update_times[0].update_time_unix_ms += 1;
        assert_ne!(portable_content_sha256(&restored, &key).unwrap(), expected);
        restored.update_times.clear();
        restored.tombstones.clear();
        assert_ne!(portable_content_sha256(&restored, &key).unwrap(), expected);
    }

    #[test]
    fn credential_only_projection_omits_unselected_deletions() {
        let mut source = super::super::empty_portable_bundle(7);
        let host_id = PortableObjectId::new();
        source.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Host,
            id: host_id,
        });
        source.validate().unwrap();
        let projected =
            project_selected_bundle(&source, &[PluginDataCategory::Credentials]).unwrap();
        assert_eq!(
            projected.selected_categories,
            Some(vec![PortableDataCategory::Credentials])
        );
        assert!(projected.tombstones.is_empty());
        assert!(projected.preferences.is_none());
        projected.validate().unwrap();
    }

    #[test]
    fn applying_credential_subset_never_tombstones_unselected_desktop() {
        let target = project_selected_bundle(
            &super::super::empty_portable_bundle(1),
            &[PluginDataCategory::Credentials],
        )
        .unwrap();
        let mut current = super::super::empty_portable_bundle(1);
        let desktop_id = PortableObjectId::new();
        current
            .objects
            .desktop_profiles
            .push(norishell_ssh_profile_sync::PortableDesktopProfile {
                id: desktop_id,
                label: "Remote desktop".to_owned(),
                protocol: norishell_ssh_profile_sync::PortableDesktopProtocol::Rdp,
                address: "example.test".to_owned(),
                port: 3389,
                username: String::new(),
                domain: String::new(),
                host_id: None,
                gateway_host_id: None,
                credential_id: None,
                width: 1024,
                height: 768,
                clipboard_enabled: false,
                audio_playback_enabled: false,
                vnc_protocol_version: Default::default(),
                vnc_resolution_mode: Default::default(),
                rdp_transport_mode: Default::default(),
                rdp_graphics_mode: Default::default(),
                rdp_resolution_mode: Default::default(),
            });
        current.validate().unwrap();
        let projected_choice =
            data_review_projection(&current, &super::super::empty_portable_bundle(1));
        assert_eq!(projected_choice.delete_count, 1);
        assert_eq!(projected_choice.desktop_profile_count, 0);
        let staged = with_selected_tombstones_for_missing(
            &target,
            &current,
            &[PluginDataCategory::Credentials],
        )
        .unwrap();
        assert!(staged.tombstones.is_empty());
        staged.validate().unwrap();
    }

    #[test]
    fn all_remote_compose_preserves_skip_metadata() {
        let plugin_id = "com.norishell.self-host-sync".to_owned();
        let owner = "a".repeat(64);
        let profile_id = "default".to_owned();
        let key = SyncKey::from_bytes([7; 32]);
        let categories = vec![
            PluginDataCategory::Hosts,
            PluginDataCategory::Credentials,
            PluginDataCategory::DesktopProfiles,
        ];
        let local_bundle = super::super::empty_portable_bundle(1);
        let local = Arc::new(DataLocalSnapshot {
            plugin_id: plugin_id.clone(),
            data_owner_sha256: owner.clone(),
            profile_id: profile_id.clone(),
            snapshot: PortableSnapshot {
                desktop_profile_count: 0,
                bundle: local_bundle.clone(),
                host_count: 0,
                credential_count: 0,
            },
            selected_bundle: local_bundle,
            categories: categories.clone(),
            profile_state_version: WireSequence::new(1),
            key_pending: false,
            key: Some(key.clone()),
            envelope: Some(vec![1]),
        });
        let mut remote_bundle = super::super::empty_portable_bundle(2);
        remote_bundle.skipped_machine_bound.push(
            norishell_ssh_profile_sync::SkippedMachineBoundObject {
                id: PortableObjectId::new(),
                kind: norishell_ssh_profile_sync::MachineBoundKind::SshAgent,
                reason: norishell_ssh_profile_sync::MachineBoundSkipReason::MachineBound,
            },
        );
        remote_bundle.validate().unwrap();
        let source_receipt = NetworkReceipt {
            endpoint_origin: "https://example.test".to_owned(),
            resource_url: "https://example.test/exchange".to_owned(),
            method: "GET".to_owned(),
            status: 200,
            etag: Some("\"current\"".to_owned()),
            response_revision: Some(2),
            response_body_sha256: Some("b".repeat(64)),
            response_blob_handle: Some("blob-handle".to_owned()),
            request_if_match: None,
            request_expected_next_revision: None,
            request_idempotency_key: None,
            response_next_revision: None,
            request_body_sha256: None,
        };
        let remote = Arc::new(DataExchangeInspection {
            migration_required: false,
            excluded_categories: Vec::new(),
            binding: PluginExchangeBinding {
                plugin_id,
                signer_fingerprint_sha256: owner,
                profile_id,
                revision: 2,
                base_revision: Some(1),
                base_etag: Some("\"previous\"".to_owned()),
            },
            exchange_sha256: "b".repeat(64),
            bundle: remote_bundle.clone(),
            categories,
            source_categories: None,
            source_receipt: Some(source_receipt.clone()),
            remote_origin: "https://example.test".to_owned(),
            remote_resource_url: "https://example.test/exchange".to_owned(),
            key,
        });
        assert!(valid_review_base(&remote, &source_receipt));
        let mut different_get = source_receipt.clone();
        different_get.etag = Some("\"other\"".to_owned());
        assert!(!valid_review_base(&remote, &different_get));
        let composed = compose_selected_objects(local, remote, &[]).unwrap();
        assert_eq!(composed.bundle, remote_bundle);
    }
}
