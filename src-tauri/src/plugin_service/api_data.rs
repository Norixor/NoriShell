//! Category-scoped Core data operations. Network transport and item decisions
//! belong to the plugin; plaintext, key recovery, local CAS, and checkpoints do not.

use std::sync::Arc;

use super::{api_invocation::ApiInvocation, *};
use crate::{
    plugin_api::{
        ResourceFence, ResourceOwner, blobs::NetworkReceipt, data_states::DataExportState,
    },
    ssh_sync_exchange::{
        BrokerError,
        data_exchange::{
            DataExchangeBlob, DataObjectDecision, DataObjectDescriptor, DataObjectDisplay,
            DataObjectSource,
        },
    },
};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiOperation, PluginApiValue, PluginDataLocalCounts,
    PluginDataObjectDescriptor, PluginDataObjectDisplay, PluginDataObjectKind,
    PluginDataObjectSource, VaultState,
};
use norishell_ssh_profile_sync::{
    PluginExchangeBinding, PortableObjectKind, canonical_bundle_bytes,
};

const EXCHANGE_CONTENT_TYPE: &str = "application/vnd.norishell.ssh-sync-exchange+json;version=1";

impl PluginService {
    pub(super) async fn invoke_api_data(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
        operation: &PluginApiOperation,
        current: &ResourceFence,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::SshSync,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if !invocation.explicit_user_action()
            && matches!(
                operation,
                PluginApiOperation::DataSnapshot { .. }
                    | PluginApiOperation::DataInspect { .. }
                    | PluginApiOperation::DataReview { .. }
                    | PluginApiOperation::DataApply { .. }
                    | PluginApiOperation::DataExport { .. }
            )
        {
            match self.api_vault_service()?.status().state {
                VaultState::Missing => return Err(PluginApiErrorCode::VaultMissing),
                VaultState::Locked => return Err(PluginApiErrorCode::VaultLocked),
                VaultState::RequiresReload => return Err(PluginApiErrorCode::VaultRequiresReload),
                VaultState::Unlocked => {}
            }
        }
        let states = &self.api.data_states;
        let blobs = &self.api.blobs;
        if let PluginApiOperation::DataRelease { request } = operation {
            states.ensure_owned_handles(
                owner,
                &request.profile_id,
                &request.state_handles,
                current,
            )?;
            blobs.ensure_owned_handles(
                owner,
                &request.profile_id,
                &request.blob_handles,
                &request.receipt_handles,
                current,
            )?;
            states.release(owner, &request.profile_id, &request.state_handles, current)?;
            blobs.release(
                owner,
                &request.profile_id,
                &request.blob_handles,
                &request.receipt_handles,
                current,
            )?;
            return Ok(PluginApiValue::DataRelease {});
        }
        let broker = self
            .ssh_sync
            .as_ref()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        let broker = if invocation.explicit_user_action() {
            broker.clone()
        } else {
            broker.clone().for_background()
        };
        let plugin_id = owner.plugin_id.as_str();

        match operation {
            PluginApiOperation::DataSnapshot { request } => {
                let data_owner = broker
                    .data_resolve_local_owner(plugin_id, &owner.signer, &request.profile_id)
                    .await
                    .map_err(map_data_error)?;
                let snapshot = Arc::new(
                    broker
                        .data_snapshot_local(
                            plugin_id,
                            &data_owner,
                            &request.profile_id,
                            &request.categories,
                            current,
                        )
                        .await
                        .map_err(map_data_error)?,
                );
                let objects = describe(
                    broker
                        .data_describe_local(&snapshot)
                        .map_err(map_data_error)?,
                );
                let key_pending = snapshot.key_pending;
                let local_counts = PluginDataLocalCounts {
                    host_count: snapshot.selected_bundle.objects.hosts.len() as u32,
                    credential_count: snapshot.selected_bundle.objects.credentials.len() as u32,
                    desktop_profile_count: snapshot.selected_bundle.objects.desktop_profiles.len()
                        as u32,
                    tombstone_count: snapshot.selected_bundle.tombstones.len() as u32,
                };
                let handle =
                    states.insert_snapshot(owner, &request.profile_id, snapshot, current)?;
                Ok(PluginApiValue::DataSnapshot {
                    snapshot_handle: handle,
                    key_pending,
                    local_counts,
                    objects,
                })
            }
            PluginApiOperation::DataInspect { request } => {
                let receipt = blobs.get_receipt(
                    owner,
                    &request.profile_id,
                    &request.receipt_handle,
                    current,
                )?;
                let blob = blobs.get(
                    owner,
                    &request.profile_id,
                    &request.body_blob_handle,
                    current,
                )?;
                let data_owner = broker
                    .data_resolve_local_owner(plugin_id, &owner.signer, &request.profile_id)
                    .await
                    .map_err(map_data_error)?;
                let inspected = Arc::new(
                    broker
                        .data_inspect_blob(
                            plugin_id,
                            &data_owner,
                            &request.profile_id,
                            &blob,
                            &receipt,
                            &request.categories,
                            current,
                        )
                        .await
                        .map_err(map_data_error)?,
                );
                let objects = describe(
                    broker
                        .data_describe_inspection(&inspected)
                        .map_err(map_data_error)?,
                );
                let remote_revision = inspected.binding.revision;
                let etag = receipt.etag.ok_or(PluginApiErrorCode::RemoteDataInvalid)?;
                let migration_required = inspected.migration_required;
                let excluded_categories = inspected.excluded_categories.clone();
                let handle =
                    states.insert_inspection(owner, &request.profile_id, inspected, current)?;
                Ok(PluginApiValue::DataInspect {
                    inspection_handle: handle,
                    objects,
                    remote_revision,
                    etag,
                    migration_required,
                    excluded_categories,
                })
            }
            PluginApiOperation::DataCompose { request } => {
                let local = states.get_snapshot(
                    owner,
                    &request.profile_id,
                    &request.local_snapshot_handle,
                    current,
                )?;
                let remote = states.get_inspection(
                    owner,
                    &request.profile_id,
                    &request.remote_inspection_handle,
                    current,
                )?;
                if local.categories != request.categories || remote.categories != request.categories
                {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let decisions = request
                    .decisions
                    .iter()
                    .map(|decision| DataObjectDecision {
                        object_handle: decision.object_handle.clone(),
                        source: match decision.source {
                            PluginDataObjectSource::Local => DataObjectSource::Local,
                            PluginDataObjectSource::Remote => DataObjectSource::Remote,
                        },
                    })
                    .collect::<Vec<_>>();
                let composed = Arc::new(
                    broker
                        .data_compose(local, remote, &decisions)
                        .map_err(map_data_error)?,
                );
                let objects = describe(
                    broker
                        .data_describe_composed(&composed)
                        .map_err(map_data_error)?,
                );
                let handle =
                    states.insert_composed(owner, &request.profile_id, composed, current)?;
                Ok(PluginApiValue::DataCompose {
                    composed_handle: handle,
                    objects,
                })
            }
            PluginApiOperation::DataReview { request } => {
                let local = states.get_snapshot(
                    owner,
                    &request.profile_id,
                    &request.local_snapshot_handle,
                    current,
                )?;
                let remote = states.get_inspection(
                    owner,
                    &request.profile_id,
                    &request.remote_inspection_handle,
                    current,
                )?;
                let base = blobs.get_receipt(
                    owner,
                    &request.profile_id,
                    &request.base_receipt_handle,
                    current,
                )?;
                let local_choice = states.get_composed(
                    owner,
                    &request.profile_id,
                    &request.local_composed_handle,
                    current,
                )?;
                let remote_choice = states.get_composed(
                    owner,
                    &request.profile_id,
                    &request.remote_composed_handle,
                    current,
                )?;
                if local.categories != request.categories || remote.categories != request.categories
                {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let data_owner_sha256 = local.data_owner_sha256.clone();
                let (chosen, source) = broker
                    .data_review_choices(
                        plugin_id,
                        &data_owner_sha256,
                        &request.profile_id,
                        local,
                        remote,
                        &base,
                        local_choice,
                        remote_choice,
                        &request.categories,
                        current,
                    )
                    .await
                    .map_err(map_data_error)?;
                let chosen = Arc::new(chosen);
                let objects = describe(
                    broker
                        .data_describe_composed(&chosen)
                        .map_err(map_data_error)?,
                );
                let handle = states.insert_composed(owner, &request.profile_id, chosen, current)?;
                Ok(PluginApiValue::DataReview {
                    composed_handle: handle,
                    source,
                    objects,
                })
            }
            PluginApiOperation::DataApply { request } => {
                let expected = states.get_snapshot(
                    owner,
                    &request.profile_id,
                    &request.expected_local_snapshot_handle,
                    current,
                )?;
                let composed = states.get_composed(
                    owner,
                    &request.profile_id,
                    &request.composed_handle,
                    current,
                )?;
                let authoritative = blobs.get_receipt(
                    owner,
                    &request.profile_id,
                    &request.authoritative_receipt_handle,
                    current,
                )?;
                if !Arc::ptr_eq(&expected, &composed.local)
                    || expected.categories != request.categories
                {
                    return Err(PluginApiErrorCode::Revoked);
                }
                if let Some(export_handle) = request.export_handle.as_deref() {
                    let exported =
                        states.get_export(owner, &request.profile_id, export_handle, current)?;
                    let base = blobs.get_receipt(
                        owner,
                        &request.profile_id,
                        &exported.base_receipt_handle,
                        current,
                    )?;
                    verify_reviewed_apply_source(
                        composed
                            .reviewed
                            .as_ref()
                            .map(|reviewed| &reviewed.base_receipt),
                        Some(&base),
                        &authoritative,
                    )?;
                    if exported.source_handle != request.composed_handle
                        || exported.blob.source_remote_exchange_sha256.as_deref()
                            != Some(composed.remote.exchange_sha256.as_str())
                    {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    verify_upload_receipt(&base, &authoritative, &exported.blob)?;
                } else {
                    if authoritative.method != "GET"
                        || authoritative.status != 200
                        || authoritative.endpoint_origin != composed.remote.remote_origin
                        || authoritative.resource_url != composed.remote.remote_resource_url
                        || authoritative.response_body_sha256.as_deref()
                            != Some(composed.remote.exchange_sha256.as_str())
                        || authoritative.response_revision != Some(composed.remote.binding.revision)
                        || canonical_bundle_bytes(&composed.bundle)
                            .map_err(|_| PluginApiErrorCode::RemoteDataInvalid)?
                            != canonical_bundle_bytes(&composed.remote.bundle)
                                .map_err(|_| PluginApiErrorCode::RemoteDataInvalid)?
                    {
                        return Err(PluginApiErrorCode::RemoteDataInvalid);
                    }
                    verify_reviewed_apply_source(
                        composed
                            .reviewed
                            .as_ref()
                            .map(|reviewed| &reviewed.base_receipt),
                        None,
                        &authoritative,
                    )?;
                }
                let receipt = Arc::new(
                    broker
                        .data_apply_composed(
                            plugin_id,
                            &expected.data_owner_sha256,
                            &request.profile_id,
                            &composed,
                            request.export_handle.is_some(),
                            current,
                        )
                        .await
                        .map_err(map_data_error)?,
                );
                let handle = states.insert_apply(owner, &request.profile_id, receipt, current)?;
                Ok(PluginApiValue::DataApply {
                    apply_receipt_handle: handle,
                })
            }
            PluginApiOperation::DataExport { request } => {
                let base = blobs.get_receipt(
                    owner,
                    &request.profile_id,
                    &request.base_receipt_handle,
                    current,
                )?;
                let revision = match base.status {
                    200 => base
                        .response_revision
                        .and_then(|value| value.checked_add(1)),
                    404 => base.response_next_revision,
                    _ => None,
                }
                .ok_or(PluginApiErrorCode::RemoteDataInvalid)?;
                let data_owner = if base.status == 404 {
                    states
                        .get_snapshot(owner, &request.profile_id, &request.source_handle, current)?
                        .data_owner_sha256
                        .clone()
                } else {
                    states
                        .get_composed(owner, &request.profile_id, &request.source_handle, current)?
                        .local
                        .data_owner_sha256
                        .clone()
                };
                let binding = PluginExchangeBinding {
                    plugin_id: plugin_id.to_owned(),
                    signer_fingerprint_sha256: data_owner.clone(),
                    profile_id: request.profile_id.clone(),
                    revision,
                    base_revision: (base.status == 200).then_some(revision - 1),
                    base_etag: (base.status == 200).then(|| base.etag.clone()).flatten(),
                };
                let exported = if base.status == 404 {
                    let local = states.get_snapshot(
                        owner,
                        &request.profile_id,
                        &request.source_handle,
                        current,
                    )?;
                    broker
                        .data_export_snapshot(
                            plugin_id,
                            &data_owner,
                            &request.profile_id,
                            &local,
                            binding,
                            &base,
                            &request.categories,
                            current,
                        )
                        .await
                        .map_err(map_data_error)?
                } else {
                    let composed = states.get_composed(
                        owner,
                        &request.profile_id,
                        &request.source_handle,
                        current,
                    )?;
                    if composed.local.categories != request.categories {
                        return Err(PluginApiErrorCode::InvalidRequest);
                    }
                    broker
                        .data_export_composed(
                            plugin_id,
                            &data_owner,
                            &request.profile_id,
                            &composed,
                            binding,
                            &base,
                            current,
                        )
                        .await
                        .map_err(map_data_error)?
                };
                let objects = describe_ref(&exported.objects);
                let blob =
                    blobs.insert(owner, &request.profile_id, exported.bytes.clone(), current)?;
                if blob.sha256 != exported.exchange_sha256 {
                    let _ = blobs.release(
                        owner,
                        &request.profile_id,
                        std::slice::from_ref(&blob.handle),
                        &[],
                        current,
                    );
                    return Err(PluginApiErrorCode::RemoteDataInvalid);
                }
                let idempotency_key = exported.idempotency_key.clone();
                let export_handle = match states.insert_export(
                    owner,
                    &request.profile_id,
                    Arc::new(DataExportState {
                        blob: Arc::new(exported),
                        source_handle: request.source_handle.clone(),
                        base_receipt_handle: request.base_receipt_handle.clone(),
                    }),
                    current,
                ) {
                    Ok(handle) => handle,
                    Err(error) => {
                        let _ = blobs.release(
                            owner,
                            &request.profile_id,
                            std::slice::from_ref(&blob.handle),
                            &[],
                            current,
                        );
                        return Err(error);
                    }
                };
                Ok(PluginApiValue::DataExport {
                    export_handle,
                    blob_handle: blob.handle,
                    revision,
                    idempotency_key,
                    content_type: EXCHANGE_CONTENT_TYPE.to_owned(),
                    objects,
                })
            }
            PluginApiOperation::DataCheckpoint { request } => {
                let authoritative = blobs.get_receipt(
                    owner,
                    &request.profile_id,
                    &request.authoritative_receipt_handle,
                    current,
                )?;
                let applied = request
                    .apply_receipt_handle
                    .as_ref()
                    .map(|handle| states.get_apply(owner, &request.profile_id, handle, current))
                    .transpose()?;
                if let (Some(local_handle), Some(remote_handle)) = (
                    &request.expected_local_snapshot_handle,
                    &request.remote_inspection_handle,
                ) {
                    if &request.source_handle != local_handle {
                        return Err(PluginApiErrorCode::InvalidRequest);
                    }
                    let local =
                        states.get_snapshot(owner, &request.profile_id, local_handle, current)?;
                    let remote = states.get_inspection(
                        owner,
                        &request.profile_id,
                        remote_handle,
                        current,
                    )?;
                    let blob_handle = authoritative
                        .response_blob_handle
                        .as_deref()
                        .ok_or(PluginApiErrorCode::RemoteDataInvalid)?;
                    let blob = blobs.get(owner, &request.profile_id, blob_handle, current)?;
                    if local.categories != request.categories
                        || remote.categories != request.categories
                    {
                        return Err(PluginApiErrorCode::InvalidRequest);
                    }
                    broker
                        .data_checkpoint_equal(
                            plugin_id,
                            &local.data_owner_sha256,
                            &request.profile_id,
                            &local,
                            &remote,
                            &blob,
                            &authoritative,
                            current,
                        )
                        .await
                        .map_err(map_data_error)?;
                } else if let (Some(base_handle), Some(export_handle)) =
                    (&request.base_receipt_handle, &request.export_handle)
                {
                    let base =
                        blobs.get_receipt(owner, &request.profile_id, base_handle, current)?;
                    let exported =
                        states.get_export(owner, &request.profile_id, export_handle, current)?;
                    if exported.source_handle != request.source_handle
                        || exported.base_receipt_handle != *base_handle
                    {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    let local = if base.status == 404 {
                        states.get_snapshot(
                            owner,
                            &request.profile_id,
                            &request.source_handle,
                            current,
                        )?
                    } else {
                        states
                            .get_composed(
                                owner,
                                &request.profile_id,
                                &request.source_handle,
                                current,
                            )?
                            .local
                            .clone()
                    };
                    if local.categories != request.categories
                        || exported.blob.binding.signer_fingerprint_sha256
                            != local.data_owner_sha256
                    {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    broker
                        .data_checkpoint_upload(
                            plugin_id,
                            &local.data_owner_sha256,
                            &request.profile_id,
                            applied.as_ref().map_or_else(
                                || {
                                    exported
                                        .blob
                                        .profile_state_version
                                        .ok_or(PluginApiErrorCode::LocalStateChanged)
                                },
                                |receipt| Ok(receipt.profile_state_version),
                            )?,
                            &local,
                            &exported.blob,
                            &base,
                            &authoritative,
                            applied.as_deref(),
                            current,
                        )
                        .await
                        .map_err(map_data_error)?;
                } else {
                    let composed = states.get_composed(
                        owner,
                        &request.profile_id,
                        &request.source_handle,
                        current,
                    )?;
                    let applied = applied.ok_or(PluginApiErrorCode::InvalidRequest)?;
                    let blob_handle = authoritative
                        .response_blob_handle
                        .as_deref()
                        .ok_or(PluginApiErrorCode::RemoteDataInvalid)?;
                    let blob = blobs.get(owner, &request.profile_id, blob_handle, current)?;
                    broker
                        .data_checkpoint_download(
                            plugin_id,
                            &composed.local.data_owner_sha256,
                            &request.profile_id,
                            applied.profile_state_version,
                            &blob,
                            &authoritative,
                            &composed.remote,
                            &applied,
                            current,
                        )
                        .await
                        .map_err(map_data_error)?;
                }
                Ok(PluginApiValue::DataCheckpoint {
                    synced_at_unix_ms: unix_time_ms(),
                })
            }
            _ => Err(PluginApiErrorCode::InvalidRequest),
        }
    }
}

fn describe(items: Vec<DataObjectDescriptor>) -> Vec<PluginDataObjectDescriptor> {
    describe_ref(&items)
}

fn verify_upload_receipt(
    base: &NetworkReceipt,
    upload: &NetworkReceipt,
    exported: &DataExchangeBlob,
) -> Result<(), PluginApiErrorCode> {
    if base.method != "GET"
        || upload.method != "PUT"
        || !matches!(upload.status, 200 | 201)
        || upload.endpoint_origin != base.endpoint_origin
        || upload.resource_url != base.resource_url
        || upload.response_revision != Some(exported.binding.revision)
        || upload.request_body_sha256.as_deref() != Some(exported.exchange_sha256.as_str())
        || upload.request_idempotency_key.as_deref() != Some(exported.idempotency_key.as_str())
        || !upload
            .etag
            .as_deref()
            .is_some_and(|etag| etag.starts_with('"') && etag.ends_with('"') && etag.len() >= 2)
    {
        return Err(PluginApiErrorCode::RemoteDataInvalid);
    }
    match base.status {
        200 if upload.request_if_match == base.etag
            && upload.request_expected_next_revision.is_none() =>
        {
            Ok(())
        }
        404 if upload.request_if_match.is_none()
            && upload.request_expected_next_revision == Some(exported.binding.revision) =>
        {
            Ok(())
        }
        _ => Err(PluginApiErrorCode::Conflict),
    }
}

fn verify_reviewed_apply_source(
    reviewed_base: Option<&NetworkReceipt>,
    upload_base: Option<&NetworkReceipt>,
    authoritative: &NetworkReceipt,
) -> Result<(), PluginApiErrorCode> {
    if reviewed_base.is_some_and(|reviewed| reviewed != upload_base.unwrap_or(authoritative)) {
        return Err(PluginApiErrorCode::Revoked);
    }
    Ok(())
}

fn describe_ref(items: &[DataObjectDescriptor]) -> Vec<PluginDataObjectDescriptor> {
    items
        .iter()
        .map(|item| PluginDataObjectDescriptor {
            category: item.category,
            kind: match item.kind {
                PortableObjectKind::Host => PluginDataObjectKind::Host,
                PortableObjectKind::DesktopProfile => PluginDataObjectKind::DesktopProfile,
                PortableObjectKind::Identity => PluginDataObjectKind::Identity,
                PortableObjectKind::Credential => PluginDataObjectKind::Credential,
                PortableObjectKind::Route => PluginDataObjectKind::Route,
                PortableObjectKind::AuthenticationPlan => PluginDataObjectKind::AuthenticationPlan,
                PortableObjectKind::AlgorithmPolicy => PluginDataObjectKind::AlgorithmPolicy,
                PortableObjectKind::HeartbeatPolicy => PluginDataObjectKind::HeartbeatPolicy,
                PortableObjectKind::MonitoringPolicy => PluginDataObjectKind::MonitoringPolicy,
                PortableObjectKind::LoginAutomation => PluginDataObjectKind::LoginAutomation,
                PortableObjectKind::Secret => PluginDataObjectKind::Secret,
            },
            stable_id: item.stable_id.clone(),
            object_handle: item.object_handle.clone(),
            equality_tag: item.equality_tag.clone(),
            update_time_unix_ms: item.update_time_unix_ms,
            tombstone: item.tombstone,
            dependency: item.dependency,
            display: item.display.as_ref().map(|display| match display {
                DataObjectDisplay::Host {
                    label,
                    address,
                    port,
                } => PluginDataObjectDisplay::Host {
                    label: label.clone(),
                    address: address.clone(),
                    port: *port,
                },
                DataObjectDisplay::Credential {
                    label,
                    material_kind,
                } => PluginDataObjectDisplay::Credential {
                    label: label.clone(),
                    material_kind: (*material_kind).to_owned(),
                },
                DataObjectDisplay::DesktopProfile {
                    label,
                    protocol,
                    address,
                    port,
                } => PluginDataObjectDisplay::DesktopProfile {
                    label: label.clone(),
                    protocol: (*protocol).to_owned(),
                    address: address.clone(),
                    port: *port,
                },
            }),
        })
        .collect()
}

fn map_data_error(error: BrokerError) -> PluginApiErrorCode {
    match error {
        BrokerError::VaultMissing => PluginApiErrorCode::VaultMissing,
        BrokerError::VaultLocked => PluginApiErrorCode::VaultLocked,
        BrokerError::InteractionRequired => PluginApiErrorCode::InteractionRequired,
        BrokerError::Cancelled => PluginApiErrorCode::Cancelled,
        BrokerError::AuthorizationDenied => PluginApiErrorCode::PermissionDenied,
        BrokerError::AuthorizationExpired => PluginApiErrorCode::AuthorizationExpired,
        BrokerError::AccountNotConnected => PluginApiErrorCode::AccountNotConnected,
        BrokerError::NetworkUnavailable => PluginApiErrorCode::NetworkUnavailable,
        BrokerError::StateConflict | BrokerError::RetryLocalSnapshot => {
            PluginApiErrorCode::Conflict
        }
        BrokerError::LocalStateChanged => PluginApiErrorCode::LocalStateChanged,
        BrokerError::LocalStateChangedAt(code) => {
            eprintln!("plugin data operation rejected with Core diagnostic {code}");
            PluginApiErrorCode::LocalStateChanged
        }
        BrokerError::OwnerConflict => PluginApiErrorCode::OwnerConflict,
        BrokerError::KeyBindingConflict => PluginApiErrorCode::KeyBindingConflict,
        BrokerError::RevisionExhausted => PluginApiErrorCode::RevisionExhausted,
        BrokerError::RestoreConflict => PluginApiErrorCode::RestoreConflict,
        BrokerError::MergeInvalid(_) => PluginApiErrorCode::MergeInvalid,
        BrokerError::HttpFailure(_) => PluginApiErrorCode::RemoteRequestRejected,
        BrokerError::RemoteDataInvalid => PluginApiErrorCode::RemoteDataInvalid,
        BrokerError::RemoteFormatUnsupported => PluginApiErrorCode::RemoteFormatUnsupported,
        BrokerError::RecoveryRemoteKeyAuthenticationFailed => {
            PluginApiErrorCode::RecoveryAuthenticationFailed
        }
        BrokerError::RecoveryActionExpired => PluginApiErrorCode::RecoveryActionExpired,
        BrokerError::OperationRejected(_) => PluginApiErrorCode::InvalidRequest,
        BrokerError::LocalDataInvalid(_, _) => PluginApiErrorCode::LocalDataInvalid,
        BrokerError::LocalKeyUnavailable => PluginApiErrorCode::LocalKeyUnavailable,
        BrokerError::OperationBusy => PluginApiErrorCode::Busy,
        BrokerError::Internal(code) => {
            eprintln!("plugin data operation failed with Core diagnostic {code}");
            PluginApiErrorCode::Unavailable
        }
        BrokerError::Persistence(stage, kind, os_code) => {
            eprintln!("plugin data persistence failed at {stage}: {kind} ({os_code:?})");
            PluginApiErrorCode::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_receipt_must_target_exact_get_resource() {
        let exported = DataExchangeBlob {
            bytes: Vec::new(),
            objects: Vec::new(),
            binding: PluginExchangeBinding {
                plugin_id: "com.norishell.sync".to_owned(),
                signer_fingerprint_sha256: "owner".to_owned(),
                profile_id: "primary".to_owned(),
                revision: 2,
                base_revision: Some(1),
                base_etag: Some("\"v1\"".to_owned()),
            },
            exchange_sha256: "ciphertext-sha".to_owned(),
            keyed_content_sha256: "content-sha".to_owned(),
            idempotency_key: "idempotency".to_owned(),
            source_remote_exchange_sha256: None,
            profile_state_version: None,
        };
        let base = NetworkReceipt {
            endpoint_origin: "https://sync.example".to_owned(),
            resource_url: "https://sync.example/exchange?profile=primary".to_owned(),
            method: "GET".to_owned(),
            status: 200,
            request_if_match: None,
            request_expected_next_revision: None,
            request_idempotency_key: None,
            etag: Some("\"v1\"".to_owned()),
            response_revision: Some(1),
            response_next_revision: None,
            request_body_sha256: None,
            response_body_sha256: Some("old-sha".to_owned()),
            response_blob_handle: None,
        };
        let mut upload = NetworkReceipt {
            endpoint_origin: base.endpoint_origin.clone(),
            resource_url: base.resource_url.clone(),
            method: "PUT".to_owned(),
            status: 200,
            request_if_match: base.etag.clone(),
            request_expected_next_revision: None,
            request_idempotency_key: Some(exported.idempotency_key.clone()),
            etag: Some("\"v2\"".to_owned()),
            response_revision: Some(2),
            response_next_revision: None,
            request_body_sha256: Some(exported.exchange_sha256.clone()),
            response_body_sha256: None,
            response_blob_handle: None,
        };
        assert_eq!(verify_upload_receipt(&base, &upload, &exported), Ok(()));
        assert_eq!(
            verify_reviewed_apply_source(Some(&base), Some(&base), &upload),
            Ok(())
        );
        assert_eq!(
            verify_reviewed_apply_source(Some(&base), None, &base),
            Ok(())
        );
        assert_eq!(
            verify_reviewed_apply_source(Some(&base), None, &upload),
            Err(PluginApiErrorCode::Revoked)
        );
        upload.resource_url = "https://sync.example/other?profile=primary".to_owned();
        assert_eq!(
            verify_upload_receipt(&base, &upload, &exported),
            Err(PluginApiErrorCode::RemoteDataInvalid)
        );
        upload.resource_url = "https://sync.example/exchange?profile=other".to_owned();
        assert_eq!(
            verify_upload_receipt(&base, &upload, &exported),
            Err(PluginApiErrorCode::RemoteDataInvalid)
        );
    }
}
