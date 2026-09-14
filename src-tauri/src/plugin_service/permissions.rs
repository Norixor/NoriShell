//! Candidate-package permission decisions remain in Core until atomic installation.

use norishell_app_persistence::PluginHostScopeGrantRecord;
use norishell_core_api::HostId;

use super::*;

type ValidatedSpecialDecisions = (Vec<PluginCapabilityGrant>, Vec<(HostId, PluginCapability)>);

#[derive(Clone)]
pub(super) struct PreparedPermissionBaseline {
    pub(super) installed: Option<PluginInstalledRecord>,
    pub(super) grants: Vec<PluginCapabilityGrantRecord>,
    pub(super) scope: Option<PluginHostScopeSetRecord>,
    scope_grants: Vec<PluginHostScopeGrantRecord>,
}

impl PreparedPermissionBaseline {
    pub(super) fn grant_state_version(&self) -> Option<WireSequence> {
        self.grants.first().map(|grant| grant.state_version)
    }

    pub(super) fn scope_state_version(&self) -> Option<WireSequence> {
        self.scope.as_ref().map(|scope| scope.state_version)
    }

    fn matches(&self, current: &Self) -> bool {
        // Stopping the old instance for an update changes its state revision,
        // but never its package identity or the permission decisions reviewed.
        let same_source = match (&self.installed, &current.installed) {
            (None, None) => true,
            (Some(previous), Some(current)) => {
                previous.plugin_id == current.plugin_id
                    && previous.signer_fingerprint_sha256 == current.signer_fingerprint_sha256
                    && previous.package_sha256 == current.package_sha256
                    && previous.active_version == current.active_version
                    && previous.capabilities == current.capabilities
                    && previous.installed_at_unix_ms == current.installed_at_unix_ms
            }
            _ => false,
        };
        same_source
            && self.grants == current.grants
            && self.scope == current.scope
            && self.scope_grants == current.scope_grants
    }
}

#[derive(Clone)]
pub(super) struct PreparedSpecialPermissionApproval {
    preparation_id: String,
    plugin_id: PluginId,
    version: String,
    protocol_major: u16,
    authority: PreparedPackageAuthority,
    binding: PluginPermissionBinding,
    baseline: PreparedPermissionBaseline,
    pub(super) grants: Vec<PluginCapabilityGrant>,
    pub(super) host_scopes: Vec<(HostId, PluginCapability)>,
    expires_at_unix_ms: i64,
    permission_revision: u64,
}

impl PreparedSpecialPermissionApproval {
    pub(super) fn valid_for(
        &self,
        prepared: &PreparedLocalPackage,
        current: &PreparedPermissionBaseline,
    ) -> bool {
        self.expires_at_unix_ms > unix_time_ms()
            && self.preparation_id == prepared.preview.preparation_id
            && self.plugin_id == prepared.inspected.manifest.plugin_id
            && self.version == prepared.inspected.manifest.version
            && self.protocol_major == prepared.inspected.manifest.protocol_major
            && self.authority == prepared.authority
            && self.binding
                == current_plugin_permission_binding(&hex::encode(
                    prepared.inspected.package_sha256,
                ))
            && self.permission_revision == prepared.permission_revision
            && self.baseline.matches(current)
    }

    #[cfg(test)]
    pub(super) fn expire_for_test(&mut self) {
        self.expires_at_unix_ms = 0;
    }
}

impl PreparedLocalPackage {
    pub(super) fn permission_preview(&self) -> PluginLocalPackagePreview {
        let mut preview = self.preview.clone();
        if let Some(approval) = &self.approved_special_permissions {
            if approval.expires_at_unix_ms > unix_time_ms() {
                preview.approved_special_grants = approval.grants.clone();
            } else {
                preview.approved_special_grants.clear();
            }
            // The expiration fact remains part of the projection even after
            // its grants are unusable, so a cancelled/reopened secure window
            // cannot make the main surface treat this preparation as installable.
            preview.special_permission_expires_at_unix_ms = Some(approval.expires_at_unix_ms);
        } else {
            preview.approved_special_grants.clear();
            preview.special_permission_expires_at_unix_ms = None;
        }
        preview
    }

    fn signer_fingerprint(&self) -> String {
        match &self.authority {
            PreparedPackageAuthority::Local => hex::encode(self.inspected.package_sha256),
            PreparedPackageAuthority::Marketplace {
                signer_fingerprint_sha256,
                ..
            } => signer_fingerprint_sha256.clone(),
        }
    }
}

pub(super) fn validate_special_decisions(
    capabilities: &[PluginCapability],
    request: &PluginSpecialPermissionDecisionRequest,
    hosts: &[PluginSpecialPermissionHost],
) -> CoreResult<ValidatedSpecialDecisions> {
    let mut declared = capabilities
        .iter()
        .copied()
        .filter(|capability| special_plugin_capability(*capability))
        .collect::<Vec<_>>();
    declared.sort();
    let mut requested = request.special_grants.clone();
    requested.sort_by_key(|grant| grant.capability);
    if requested.len() != declared.len()
        || requested
            .iter()
            .zip(&declared)
            .any(|(grant, capability)| grant.capability != *capability)
    {
        return Err(plugin_validation_error(request.meta.request_id.clone()));
    }
    let granted = |capability| {
        requested
            .iter()
            .any(|grant| grant.capability == capability && grant.granted)
    };
    if (granted(PluginCapability::UiHostDomMutate) || granted(PluginCapability::UiHostCss))
        && !granted(PluginCapability::UiHostDomObserve)
        || (granted(PluginCapability::HostMutationPropose)
            || granted(PluginCapability::HostSessionRequest))
            && !granted(PluginCapability::HostMetadataRead)
        || request.host_selections.len() > 256
    {
        return Err(plugin_validation_error(request.meta.request_id.clone()));
    }
    let mut flattened = Vec::new();
    let mut unique = BTreeSet::new();
    for selection in &request.host_selections {
        if selection.capabilities.is_empty()
            || selection.capabilities.len() > 3
            || !hosts.iter().any(|host| host.host_id == selection.host_id)
        {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        }
        for capability in &selection.capabilities {
            if !host_scoped_plugin_capability(*capability)
                || !granted(*capability)
                || !unique.insert((selection.host_id.as_str().to_owned(), *capability))
            {
                return Err(plugin_validation_error(request.meta.request_id.clone()));
            }
            if matches!(
                capability,
                PluginCapability::HostMutationPropose | PluginCapability::HostSessionRequest
            ) && !selection
                .capabilities
                .contains(&PluginCapability::HostMetadataRead)
            {
                return Err(plugin_validation_error(request.meta.request_id.clone()));
            }
            flattened.push((selection.host_id.clone(), *capability));
        }
    }
    flattened.sort_by(|left, right| {
        left.0
            .as_str()
            .cmp(right.0.as_str())
            .then(left.1.cmp(&right.1))
    });
    Ok((requested, flattened))
}

impl PluginService {
    pub(super) fn installed_publisher_is_verified(
        &self,
        installed: &PluginInstalledRecord,
        request_id: RequestId,
    ) -> CoreResult<bool> {
        let publisher = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.plugin_installed_version_publisher(
                    &installed.plugin_id,
                    &installed.active_version,
                )
            })
            .map_err(|error| map_persistence_error(request_id, error))?;
        Ok(publisher.is_some_and(|publisher| {
            !publisher.publisher_signature_base64.is_empty()
                && BASE64
                    .decode(&publisher.publisher_key_base64)
                    .ok()
                    .is_some_and(|key| {
                        hex::encode(Sha256::digest(key)) == installed.signer_fingerprint_sha256
                    })
        }))
    }

    pub(super) fn prepared_permission_baseline(
        &self,
        prepared: &PreparedLocalPackage,
        request_id: RequestId,
    ) -> CoreResult<PreparedPermissionBaseline> {
        let plugin_id = &prepared.inspected.manifest.plugin_id;
        let candidate_signer = prepared.signer_fingerprint();
        let baseline = self
            .hosts
            .with_plugin_repository(|repository| {
                let installed = match repository.get_plugin_installation(plugin_id) {
                    Ok(installed) => Some(installed),
                    Err(AppPersistenceError::NotFound) => None,
                    Err(error) => return Err(error),
                };
                let signer = installed
                    .as_ref()
                    .map_or(candidate_signer.as_str(), |record| {
                        &record.signer_fingerprint_sha256
                    });
                let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
                let grants = repository.list_plugin_capability_grants(plugin_id, signer, major)?;
                let scope = repository.plugin_host_scope_set(plugin_id, signer, major)?;
                let scope_grants =
                    repository.list_plugin_host_scope_grants(plugin_id, signer, major)?;
                Ok(PreparedPermissionBaseline {
                    installed,
                    grants,
                    scope,
                    scope_grants,
                })
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        if baseline
            .grants
            .iter()
            .any(|grant| Some(grant.state_version) != baseline.grant_state_version())
        {
            return Err(plugin_validation_error(request_id));
        }
        Ok(baseline)
    }

    pub(super) fn prepare_package_special_permission(
        &self,
        request: PluginSpecialPermissionOpenRequest,
    ) -> CoreResult<PluginSpecialPermissionSnapshot> {
        let PluginSpecialPermissionTarget::PreparedPackage {
            preparation_id,
            expected_package_sha256,
            expected_state_version,
        } = &request.target
        else {
            return Err(plugin_validation_error(request.meta.request_id));
        };
        if preparation_id.len() > 80 || expected_package_sha256.len() != 64 {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prepared = runtime
            .prepared_local_packages
            .get(preparation_id)
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        if hex::encode(prepared.inspected.package_sha256) != *expected_package_sha256 {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        // Repository access is short and synchronous. Keeping the preparation
        // locked prevents cancellation/installation from moving it mid-snapshot.
        let baseline =
            self.prepared_permission_baseline(prepared, request.meta.request_id.clone())?;
        if baseline
            .installed
            .as_ref()
            .map(|record| record.state_version)
            != *expected_state_version
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                baseline
                    .installed
                    .as_ref()
                    .map(|record| record.state_version),
            ));
        }
        let mut declared = prepared
            .inspected
            .manifest
            .capabilities
            .iter()
            .copied()
            .filter(|capability| special_plugin_capability(*capability))
            .collect::<Vec<_>>();
        declared.sort();
        if declared.is_empty()
            || request
                .requested_capability
                .is_some_and(|capability| !declared.contains(&capability))
        {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let prior_approval = prepared
            .approved_special_permissions
            .as_ref()
            .filter(|approval| approval.valid_for(prepared, &baseline));
        let retained = &prepared.preview.retained_capability_grants;
        let special_grants = declared
            .iter()
            .map(|capability| PluginCapabilityGrant {
                capability: *capability,
                granted: prior_approval
                    .and_then(|approval| {
                        approval
                            .grants
                            .iter()
                            .find(|grant| grant.capability == *capability)
                    })
                    .or_else(|| {
                        retained
                            .iter()
                            .find(|grant| grant.capability == *capability)
                    })
                    .is_some_and(|grant| grant.granted),
            })
            .collect::<Vec<_>>();
        let hosts = if declared
            .iter()
            .any(|capability| host_scoped_plugin_capability(*capability))
        {
            self.hosts
                .with_plugin_repository(|repository| repository.list_hosts())
                .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?
                .into_iter()
                .map(|host| {
                    let mut granted_capabilities = if let Some(approval) = prior_approval {
                        approval
                            .host_scopes
                            .iter()
                            .filter(|(host_id, _)| *host_id == host.host_id)
                            .map(|(_, capability)| *capability)
                            .collect::<Vec<_>>()
                    } else if baseline.installed.as_ref().is_some_and(|installed| {
                        baseline
                            .scope
                            .as_ref()
                            .is_some_and(|scope| effective_plugin_scope(installed, scope))
                    }) {
                        baseline
                            .scope_grants
                            .iter()
                            .filter(|grant| {
                                grant.host_id == host.host_id
                                    && special_grants.iter().any(|decision| {
                                        decision.capability == grant.capability && decision.granted
                                    })
                            })
                            .map(|grant| grant.capability)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    granted_capabilities.sort();
                    PluginSpecialPermissionHost {
                        host_id: host.host_id,
                        label: host.label,
                        endpoint: format!("{}:{}", host.address, host.port),
                        granted_capabilities,
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut hosts = hosts;
        hosts.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.host_id.as_str().cmp(right.host_id.as_str()))
        });
        let next_revision = prepared
            .permission_revision
            .checked_add(1)
            .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
        let snapshot = PluginSpecialPermissionSnapshot {
            target: request.target.clone(),
            requested_capability: request.requested_capability,
            publisher_verified: matches!(
                prepared.authority,
                PreparedPackageAuthority::Marketplace { .. }
            ),
            approval_id: PluginApprovalId::new(),
            approval_state_version: WireSequence::new(next_revision),
            expires_at_unix_ms: unix_time_ms().saturating_add(PLUGIN_HOST_APPROVAL_MILLIS),
            plugin_id: prepared.inspected.manifest.plugin_id.clone(),
            plugin_name: prepared.inspected.manifest.name.clone(),
            publisher: prepared.inspected.manifest.publisher.clone(),
            signer_fingerprint_sha256: prepared.signer_fingerprint(),
            version: prepared.inspected.manifest.version.clone(),
            package_sha256: expected_package_sha256.clone(),
            special_grants,
            hosts,
        };
        let now = unix_time_ms();
        runtime.pending_special_permissions.retain(|_, pending| pending.snapshot.expires_at_unix_ms > now
            && !matches!(&pending.snapshot.target, PluginSpecialPermissionTarget::PreparedPackage { preparation_id: candidate, .. } if candidate == preparation_id));
        if runtime.pending_special_permissions.len() >= PLUGIN_HOST_APPROVAL_LIMIT {
            return Err(plugin_error(
                request.meta.request_id,
                "plugin.permission_queue_full",
                ErrorCategory::Unavailable,
                RetryStrategy::AfterMilliseconds(1_000),
                "errors.plugin.permissionQueueFull",
                None,
            ));
        }
        let prepared = runtime
            .prepared_local_packages
            .get_mut(preparation_id)
            .expect("preparation remains locked");
        prepared.permission_revision = next_revision;
        // A previous explicit approval remains valid if this replacement
        // window is cancelled; only a new confirmed decision replaces it.
        if let Some(approval) = &mut prepared.approved_special_permissions {
            approval.permission_revision = next_revision;
        }
        runtime.pending_special_permissions.insert(
            snapshot.approval_id.as_str().to_owned(),
            PendingPluginSpecialPermission {
                snapshot: snapshot.clone(),
                installed_snapshot: None,
                expected_plugin_state_version: expected_state_version
                    .unwrap_or(WireSequence::new(0)),
                expected_grant_state_version: baseline.grant_state_version(),
                expected_scope_state_version: baseline.scope_state_version(),
                all_grants: baseline.grants.clone(),
                prepared_baseline: Some(baseline),
            },
        );
        Ok(snapshot)
    }

    pub(super) fn decide_package_special_permission(
        &self,
        pending: PendingPluginSpecialPermission,
        request: PluginSpecialPermissionDecisionRequest,
    ) -> CoreResult<PluginSpecialPermissionDecisionResponse> {
        let PluginSpecialPermissionTarget::PreparedPackage {
            preparation_id,
            expected_package_sha256,
            ..
        } = &pending.snapshot.target
        else {
            return Err(plugin_validation_error(request.meta.request_id));
        };
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prepared = runtime
            .prepared_local_packages
            .get_mut(preparation_id)
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        let baseline =
            self.prepared_permission_baseline(prepared, request.meta.request_id.clone())?;
        if prepared.permission_revision != pending.snapshot.approval_state_version.get()
            || hex::encode(prepared.inspected.package_sha256) != *expected_package_sha256
            || !pending
                .prepared_baseline
                .as_ref()
                .is_some_and(|previous| previous.matches(&baseline))
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        if request.decision == PluginApprovalDecision::Approve {
            let (grants, host_scopes) = validate_special_decisions(
                &prepared.inspected.manifest.capabilities,
                &request,
                &pending.snapshot.hosts,
            )?;
            prepared.approved_special_permissions = Some(PreparedSpecialPermissionApproval {
                preparation_id: preparation_id.clone(),
                plugin_id: prepared.inspected.manifest.plugin_id.clone(),
                version: prepared.inspected.manifest.version.clone(),
                protocol_major: prepared.inspected.manifest.protocol_major,
                authority: prepared.authority.clone(),
                binding: current_plugin_permission_binding(expected_package_sha256),
                baseline,
                grants,
                host_scopes,
                expires_at_unix_ms: pending.snapshot.expires_at_unix_ms,
                permission_revision: prepared.permission_revision,
            });
        }
        Ok(PluginSpecialPermissionDecisionResponse {
            approval_id: request.approval_id,
            decision: request.decision,
            target: PluginSpecialPermissionOutcome::PreparedPackage {
                preview: prepared.permission_preview(),
            },
        })
    }

    pub(super) fn cancel_special_permission(
        &self,
        approval_id: &PluginApprovalId,
    ) -> Option<PluginSpecialPermissionOutcome> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = runtime
            .pending_special_permissions
            .remove(approval_id.as_str())?;
        match &pending.snapshot.target {
            PluginSpecialPermissionTarget::PreparedPackage { preparation_id, .. } => runtime
                .prepared_local_packages
                .get(preparation_id)
                .map(|prepared| PluginSpecialPermissionOutcome::PreparedPackage {
                    preview: prepared.permission_preview(),
                }),
            PluginSpecialPermissionTarget::Installed { .. } => pending
                .installed_snapshot
                .map(|plugin| PluginSpecialPermissionOutcome::Installed { plugin }),
        }
    }

    pub(super) fn cancel_prepared_package(&self, preparation_id: &str) {
        let approval_ids = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime.prepared_local_packages.remove(preparation_id);
            let ids = runtime.pending_special_permissions.iter().filter(|(_, pending)|
                matches!(&pending.snapshot.target, PluginSpecialPermissionTarget::PreparedPackage { preparation_id: candidate, .. } if candidate == preparation_id)
            ).map(|(_, pending)| pending.snapshot.approval_id.clone()).collect::<Vec<_>>();
            for id in &ids {
                runtime.pending_special_permissions.remove(id.as_str());
            }
            ids
        };
        self.close_special_permission_windows(&approval_ids);
    }

    pub(super) fn close_special_permission_windows(&self, approval_ids: &[PluginApprovalId]) {
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(app) = app {
            for id in approval_ids {
                if let Some(window) =
                    app.get_webview_window(&secure_special_permission_window_label(id))
                {
                    let _ = window.close();
                }
            }
        }
    }

    pub(super) fn revalidate_prepared_marketplace(
        &self,
        prepared: &PreparedLocalPackage,
        request_id: RequestId,
    ) -> CoreResult<()> {
        let PreparedPackageAuthority::Marketplace {
            catalog_entry,
            signer_fingerprint_sha256,
            ..
        } = &prepared.authority
        else {
            return Ok(());
        };
        let (root, catalog, _) = self.fetch_verified_norixor_catalog(request_id.clone())?;
        let item = catalog
            .item(
                &prepared.inspected.manifest.plugin_id,
                &prepared.inspected.manifest.version,
            )
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        let fresh = norixor_catalog_entry_record(&root, &catalog, item)
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        let signer = root
            .publisher_key_base64(&item.publisher_key_id)
            .and_then(|key| BASE64.decode(key).ok())
            .map(|key| hex::encode(Sha256::digest(key)));
        if !catalog_install_candidate_is_unchanged(catalog_entry, &fresh)
            || signer.as_deref() != Some(signer_fingerprint_sha256)
            || hex::encode(prepared.inspected.package_sha256) != fresh.package_sha256
        {
            return Err(plugin_conflict_error(request_id, None));
        }
        Ok(())
    }
}
