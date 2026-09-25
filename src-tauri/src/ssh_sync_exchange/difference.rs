use std::collections::{BTreeMap, BTreeSet};

use norishell_core_api::{
    SshSyncSecureDifference, SshSyncSecureDifferenceChange, SshSyncSecureDifferenceKind,
};
use norishell_ssh_profile_sync::{
    LoginAutomationStep, PortableBundleV1, PortableCredential, PortableCredentialMaterial,
    PortableHost, PortableObjectId, PortableObjectKind, PortableTombstone,
};

const MAX_VISIBLE_DIFFERENCES: usize = 200;

pub(crate) struct SecureDifferenceReport {
    pub(crate) differences: Vec<SshSyncSecureDifference>,
    pub(crate) total_count: u32,
    pub(crate) omitted_count: u32,
    pub(crate) changed_count: u32,
}

pub(crate) fn compare_bundles(
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
) -> SecureDifferenceReport {
    let mut collector = DifferenceCollector::default();
    compare_values(
        &mut collector,
        &local.objects.desktop_profiles,
        &remote.objects.desktop_profiles,
        |value| value.id,
        SshSyncSecureDifferenceKind::DesktopProfile,
        |_, local, remote| {
            local
                .or(remote)
                .map(|v| v.label.clone())
                .unwrap_or_default()
        },
        |v| {
            Some(format!(
                "{:?} · {}@{}:{} · {}",
                v.protocol, v.username, v.address, v.port, v.domain
            ))
        },
    );

    compare_values(
        &mut collector,
        &local.objects.hosts,
        &remote.objects.hosts,
        |value| value.id,
        SshSyncSecureDifferenceKind::Host,
        |_, local, remote| {
            local
                .or(remote)
                .map(|value| value.label.clone())
                .unwrap_or_default()
        },
        |value| Some(host_summary(value)),
    );
    compare_values(
        &mut collector,
        &local.objects.identities,
        &remote.objects.identities,
        |value| value.id,
        SshSyncSecureDifferenceKind::Identity,
        |_, local, remote| {
            local
                .or(remote)
                .map(|value| value.label.clone())
                .unwrap_or_default()
        },
        |value| value.username.clone(),
    );
    compare_values(
        &mut collector,
        &local.objects.credentials,
        &remote.objects.credentials,
        |value| value.id,
        SshSyncSecureDifferenceKind::Credential,
        |_, local, remote| {
            local
                .or(remote)
                .map(|value| value.label.clone())
                .unwrap_or_default()
        },
        |_| None,
    );

    macro_rules! compare_host_settings {
        ($local:expr, $remote:expr, $id:expr, $kind:expr, $matches:expr) => {
            compare_values(
                &mut collector,
                $local,
                $remote,
                $id,
                $kind,
                |id, _, _| host_label_for_setting(local, remote, id, $matches),
                |_| None,
            )
        };
    }

    compare_host_settings!(
        &local.objects.routes,
        &remote.objects.routes,
        |value| value.id,
        SshSyncSecureDifferenceKind::ConnectionRoute,
        |host: &PortableHost, id| host.route_id == id
    );
    compare_host_settings!(
        &local.objects.authentication_plans,
        &remote.objects.authentication_plans,
        |value| value.id,
        SshSyncSecureDifferenceKind::Authentication,
        |host: &PortableHost, id| host.authentication_plan_id == id
    );
    compare_host_settings!(
        &local.objects.algorithm_policies,
        &remote.objects.algorithm_policies,
        |value| value.id,
        SshSyncSecureDifferenceKind::Algorithms,
        |host: &PortableHost, id| host.algorithm_policy_id == id
    );
    compare_host_settings!(
        &local.objects.heartbeat_policies,
        &remote.objects.heartbeat_policies,
        |value| value.id,
        SshSyncSecureDifferenceKind::Heartbeat,
        |host: &PortableHost, id| host.heartbeat_policy_id == id
    );
    compare_host_settings!(
        &local.objects.monitoring_policies,
        &remote.objects.monitoring_policies,
        |value| value.id,
        SshSyncSecureDifferenceKind::Monitoring,
        |host: &PortableHost, id| host.monitoring_policy_id == id
    );
    compare_host_settings!(
        &local.objects.login_automations,
        &remote.objects.login_automations,
        |value| value.id,
        SshSyncSecureDifferenceKind::LoginAutomation,
        |host: &PortableHost, id| host.login_automation_id == Some(id)
    );
    compare_values(
        &mut collector,
        &local.secrets,
        &remote.secrets,
        |value| value.id,
        SshSyncSecureDifferenceKind::EncryptedSecret,
        |id, _, _| secret_label(local, remote, id),
        |_| None,
    );

    // A pre-V4 bundle has no opinion about application preferences. Report
    // present groups without describing the absent side as a deletion.
    let local_preferences = local.preferences.as_ref().map(|value| &value.groups);
    let remote_preferences = remote.preferences.as_ref().map(|value| &value.groups);
    let preference_groups = local_preferences
        .into_iter()
        .flat_map(|groups| groups.keys())
        .chain(
            remote_preferences
                .into_iter()
                .flat_map(|groups| groups.keys()),
        )
        .collect::<BTreeSet<_>>();
    for group in preference_groups {
        let local_value = local
            .preferences
            .as_ref()
            .and_then(|value| value.groups.get(group.as_str()));
        let remote_value = remote
            .preferences
            .as_ref()
            .and_then(|value| value.groups.get(group.as_str()));
        let change = match (local_value, remote_value) {
            (Some(left), Some(right)) if left != right => {
                Some(SshSyncSecureDifferenceChange::Changed)
            }
            (Some(_), None) => Some(SshSyncSecureDifferenceChange::LocalOnly),
            (None, Some(_)) => Some(SshSyncSecureDifferenceChange::RemoteOnly),
            _ => None,
        };
        if let Some(change) = change {
            collector.add(SshSyncSecureDifference {
                kind: SshSyncSecureDifferenceKind::Preferences,
                change,
                label: group.to_owned(),
                local_summary: None,
                remote_summary: None,
            });
        }
    }

    add_tombstone_only_differences(&mut collector, local, remote);
    collector.finish()
}

/// Builds a local-apply representation of an exact remote snapshot. Objects
/// that exist only in the current local scoped snapshot become authenticated
/// tombstones so the owned restore transaction removes them. The original
/// remote bundle remains unchanged for baseline verification.
pub(crate) fn with_tombstones_for_missing(
    target: &PortableBundleV1,
    current: &PortableBundleV1,
) -> norishell_ssh_profile_sync::Result<PortableBundleV1> {
    let mut result = target.clone();
    result.schema = match target.schema {
        norishell_ssh_profile_sync::BundleSchema::V5 => {
            norishell_ssh_profile_sync::BundleSchema::V5
        }
        norishell_ssh_profile_sync::BundleSchema::V4 => {
            norishell_ssh_profile_sync::BundleSchema::V4
        }
        _ => norishell_ssh_profile_sync::BundleSchema::V3,
    };
    let target_ids = object_keys(target);
    let current_ids = object_keys(current);
    // Historical tombstones may remain in the cloud baseline, but must not delete local objects outside this operation's scope.
    result
        .tombstones
        .retain(|value| current_ids.contains(&(value.kind, value.id)));
    let existing_tombstones = result
        .tombstones
        .iter()
        .map(|value| (value.kind, value.id))
        .collect::<BTreeSet<_>>();
    result.tombstones.extend(
        current_ids
            .difference(&target_ids)
            .filter(|value| !existing_tombstones.contains(value))
            .map(|(kind, id)| PortableTombstone {
                kind: *kind,
                id: *id,
            }),
    );
    result
        .tombstones
        .sort_by_key(|value| (value.kind, value.id));
    result.tombstones.dedup();
    let staged_ids = object_keys(&result)
        .into_iter()
        .chain(result.tombstones.iter().map(|value| (value.kind, value.id)))
        .collect::<BTreeSet<_>>();
    result
        .update_times
        .retain(|value| staged_ids.contains(&(value.kind, value.id)));
    result.validate()?;
    Ok(result)
}

pub(crate) fn object_keys(
    bundle: &PortableBundleV1,
) -> BTreeSet<(PortableObjectKind, PortableObjectId)> {
    bundle
        .objects
        .hosts
        .iter()
        .map(|value| (PortableObjectKind::Host, value.id))
        .chain(
            bundle
                .objects
                .desktop_profiles
                .iter()
                .map(|value| (PortableObjectKind::DesktopProfile, value.id)),
        )
        .chain(
            bundle
                .objects
                .identities
                .iter()
                .map(|value| (PortableObjectKind::Identity, value.id)),
        )
        .chain(
            bundle
                .objects
                .credentials
                .iter()
                .map(|value| (PortableObjectKind::Credential, value.id)),
        )
        .chain(
            bundle
                .objects
                .routes
                .iter()
                .map(|value| (PortableObjectKind::Route, value.id)),
        )
        .chain(
            bundle
                .objects
                .authentication_plans
                .iter()
                .map(|value| (PortableObjectKind::AuthenticationPlan, value.id)),
        )
        .chain(
            bundle
                .objects
                .algorithm_policies
                .iter()
                .map(|value| (PortableObjectKind::AlgorithmPolicy, value.id)),
        )
        .chain(
            bundle
                .objects
                .heartbeat_policies
                .iter()
                .map(|value| (PortableObjectKind::HeartbeatPolicy, value.id)),
        )
        .chain(
            bundle
                .objects
                .monitoring_policies
                .iter()
                .map(|value| (PortableObjectKind::MonitoringPolicy, value.id)),
        )
        .chain(
            bundle
                .objects
                .login_automations
                .iter()
                .map(|value| (PortableObjectKind::LoginAutomation, value.id)),
        )
        .chain(
            bundle
                .secrets
                .iter()
                .map(|value| (PortableObjectKind::Secret, value.id)),
        )
        .collect()
}

#[derive(Default)]
struct DifferenceCollector {
    differences: Vec<SshSyncSecureDifference>,
    total_count: usize,
    changed_count: usize,
}

impl DifferenceCollector {
    fn add(&mut self, difference: SshSyncSecureDifference) {
        self.total_count = self.total_count.saturating_add(1);
        if difference.change == SshSyncSecureDifferenceChange::Changed {
            self.changed_count = self.changed_count.saturating_add(1);
        }
        if self.differences.len() < MAX_VISIBLE_DIFFERENCES {
            self.differences.push(difference);
        }
    }

    fn finish(self) -> SecureDifferenceReport {
        let visible = self.differences.len();
        SecureDifferenceReport {
            differences: self.differences,
            total_count: u32::try_from(self.total_count).unwrap_or(u32::MAX),
            omitted_count: u32::try_from(self.total_count.saturating_sub(visible))
                .unwrap_or(u32::MAX),
            changed_count: u32::try_from(self.changed_count).unwrap_or(u32::MAX),
        }
    }
}

fn compare_values<T: PartialEq>(
    collector: &mut DifferenceCollector,
    local: &[T],
    remote: &[T],
    id: impl Fn(&T) -> PortableObjectId,
    kind: SshSyncSecureDifferenceKind,
    label: impl Fn(PortableObjectId, Option<&T>, Option<&T>) -> String,
    summary: impl Fn(&T) -> Option<String>,
) {
    let local = local
        .iter()
        .map(|value| (id(value), value))
        .collect::<BTreeMap<_, _>>();
    let remote = remote
        .iter()
        .map(|value| (id(value), value))
        .collect::<BTreeMap<_, _>>();
    let ids = local
        .keys()
        .chain(remote.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    for value_id in ids {
        let local_value = local.get(&value_id).copied();
        let remote_value = remote.get(&value_id).copied();
        let change = match (local_value, remote_value) {
            (Some(local), Some(remote)) if local == remote => continue,
            (Some(_), Some(_)) => SshSyncSecureDifferenceChange::Changed,
            (Some(_), None) => SshSyncSecureDifferenceChange::LocalOnly,
            (None, Some(_)) => SshSyncSecureDifferenceChange::RemoteOnly,
            (None, None) => continue,
        };
        collector.add(SshSyncSecureDifference {
            kind,
            change,
            label: label(value_id, local_value, remote_value),
            local_summary: local_value.and_then(&summary),
            remote_summary: remote_value.and_then(&summary),
        });
    }
}

fn host_summary(host: &PortableHost) -> String {
    match host.username.as_deref() {
        Some(username) => format!("{username}@{}:{}", host.address, host.port),
        None => format!("{}:{}", host.address, host.port),
    }
}

fn host_label_for_setting(
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    id: PortableObjectId,
    matches: impl Fn(&PortableHost, PortableObjectId) -> bool,
) -> String {
    local
        .objects
        .hosts
        .iter()
        .chain(remote.objects.hosts.iter())
        .find(|host| matches(host, id))
        .map(|host| host.label.clone())
        .unwrap_or_default()
}

fn secret_label(
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    id: PortableObjectId,
) -> String {
    local
        .objects
        .credentials
        .iter()
        .chain(remote.objects.credentials.iter())
        .find(|credential| credential_secret_ids(credential).contains(&id))
        .map(|credential| credential.label.clone())
        .or_else(|| {
            local
                .objects
                .login_automations
                .iter()
                .chain(remote.objects.login_automations.iter())
                .find(|automation| {
                    automation.steps.iter().any(|step| {
                        matches!(step, LoginAutomationStep::SendSecret { secret_id, .. } if *secret_id == id)
                    })
                })
                .and_then(|automation| {
                    local
                        .objects
                        .hosts
                        .iter()
                        .chain(remote.objects.hosts.iter())
                        .find(|host| host.login_automation_id == Some(automation.id))
                        .map(|host| host.label.clone())
                })
        })
        .unwrap_or_default()
}

fn credential_secret_ids(credential: &PortableCredential) -> Vec<PortableObjectId> {
    match credential.material {
        PortableCredentialMaterial::Password { password_secret_id } => vec![password_secret_id],
        PortableCredentialMaterial::PrivateKey {
            private_key_secret_id,
            passphrase_secret_id,
            ..
        } => std::iter::once(private_key_secret_id)
            .chain(passphrase_secret_id)
            .collect(),
        PortableCredentialMaterial::Certificate {
            certificate_secret_id,
            private_key_secret_id,
            passphrase_secret_id,
            ..
        } => std::iter::once(certificate_secret_id)
            .chain(private_key_secret_id)
            .chain(passphrase_secret_id)
            .collect(),
        PortableCredentialMaterial::KeyboardInteractive { .. } => Vec::new(),
    }
}

fn add_tombstone_only_differences(
    collector: &mut DifferenceCollector,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
) {
    let present = local
        .objects
        .hosts
        .iter()
        .map(|value| (PortableObjectKind::Host, value.id))
        .chain(
            local
                .objects
                .identities
                .iter()
                .map(|value| (PortableObjectKind::Identity, value.id)),
        )
        .chain(
            local
                .objects
                .credentials
                .iter()
                .map(|value| (PortableObjectKind::Credential, value.id)),
        )
        .chain(
            remote
                .objects
                .hosts
                .iter()
                .map(|value| (PortableObjectKind::Host, value.id)),
        )
        .chain(
            remote
                .objects
                .identities
                .iter()
                .map(|value| (PortableObjectKind::Identity, value.id)),
        )
        .chain(
            remote
                .objects
                .credentials
                .iter()
                .map(|value| (PortableObjectKind::Credential, value.id)),
        )
        .chain(
            local
                .objects
                .desktop_profiles
                .iter()
                .chain(&remote.objects.desktop_profiles)
                .map(|value| (PortableObjectKind::DesktopProfile, value.id)),
        )
        .collect::<BTreeSet<_>>();
    let local_tombstones = local
        .tombstones
        .iter()
        .map(|value| (value.kind, value.id))
        .collect::<BTreeSet<_>>();
    let remote_tombstones = remote
        .tombstones
        .iter()
        .map(|value| (value.kind, value.id))
        .collect::<BTreeSet<_>>();
    for value in local_tombstones.symmetric_difference(&remote_tombstones) {
        if present.contains(value) {
            continue;
        }
        let change = if local_tombstones.contains(value) {
            SshSyncSecureDifferenceChange::LocalOnly
        } else {
            SshSyncSecureDifferenceChange::RemoteOnly
        };
        collector.add(SshSyncSecureDifference {
            kind: secure_kind(value.0),
            change,
            label: String::new(),
            local_summary: None,
            remote_summary: None,
        });
    }
}

fn secure_kind(kind: PortableObjectKind) -> SshSyncSecureDifferenceKind {
    match kind {
        PortableObjectKind::DesktopProfile => SshSyncSecureDifferenceKind::DesktopProfile,
        PortableObjectKind::Host => SshSyncSecureDifferenceKind::Host,
        PortableObjectKind::Identity => SshSyncSecureDifferenceKind::Identity,
        PortableObjectKind::Credential => SshSyncSecureDifferenceKind::Credential,
        PortableObjectKind::Route => SshSyncSecureDifferenceKind::ConnectionRoute,
        PortableObjectKind::AuthenticationPlan => SshSyncSecureDifferenceKind::Authentication,
        PortableObjectKind::AlgorithmPolicy => SshSyncSecureDifferenceKind::Algorithms,
        PortableObjectKind::HeartbeatPolicy => SshSyncSecureDifferenceKind::Heartbeat,
        PortableObjectKind::MonitoringPolicy => SshSyncSecureDifferenceKind::Monitoring,
        PortableObjectKind::LoginAutomation => SshSyncSecureDifferenceKind::LoginAutomation,
        PortableObjectKind::Secret => SshSyncSecureDifferenceKind::EncryptedSecret,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_ssh_profile_sync::{
        BundleSchema, PortableItemUpdateTime, PortableObjects, PortablePreferencesV1,
    };

    fn v5(mut value: PortableBundleV1) -> PortableBundleV1 {
        let groups = serde_json::json!({
            "application": {"themePreference":null,"locale":null,"uiZoom":null,"terminalStartupBehavior":null,"newTerminalBehavior":null,"singlePaneTabCloseBehavior":null},
            "appearance": {"terminalThemeMode":null,"terminalFontFamily":null,"terminalFontSize":null,"terminalFontWeight":null,"terminalBoldFontWeight":null,"terminalLineHeight":null,"terminalLetterSpacing":null,"terminalCursorStyle":null,"terminalCursorBlink":null,"customTerminalPalette":null,"customTerminalPaletteName":null},
            "interaction": {"interaction":null,"pasteWarning":null},
            "highlights": {"enabled":null,"rules":null},
            "shortcuts": {"version":null,"bindings":null},
            "files": {"browser":null,"rememberLastDirectory":null},
            "desktop": {"windowCloseBehavior":null,"trayShowStatus":null,"trayRecentLimit":null,"trayShowHostNames":null,"notificationBackgroundOnly":null,"notificationFailureOnly":null,"notifyTransferCompleted":null,"notifyTransferFailed":null,"notifyDisconnected":null},
            "commandNotifications": {"notificationsEnabled":null,"notificationThresholdSeconds":null}
        });
        value.schema = BundleSchema::V5;
        value.preferences = Some(PortablePreferencesV1 {
            product: "NoriShell".into(),
            version: 1,
            groups: serde_json::from_value(groups).expect("groups"),
        });
        value
    }

    fn bundle(hosts: Vec<PortableHost>) -> PortableBundleV1 {
        PortableBundleV1 {
            schema: BundleSchema::V2,
            revision: 1,
            objects: PortableObjects {
                hosts,
                ..PortableObjects::default()
            },
            preferences: None,
            secrets: Vec::new(),
            skipped_machine_bound: Vec::new(),
            tombstones: Vec::new(),
            update_times: Vec::new(),
            preference_update_times: BTreeMap::new(),
        }
    }

    fn host(id: PortableObjectId, label: &str, address: &str) -> PortableHost {
        PortableHost {
            id,
            label: label.to_owned(),
            address: address.to_owned(),
            port: 22,
            username: Some("deploy".to_owned()),
            favorite: false,
            tags: BTreeSet::new(),
            identity_id: None,
            route_id: PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("route"),
            authentication_plan_id: PortableObjectId::from_uuid(uuid::Uuid::new_v4())
                .expect("auth"),
            algorithm_policy_id: PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("algo"),
            heartbeat_policy_id: PortableObjectId::from_uuid(uuid::Uuid::new_v4())
                .expect("heartbeat"),
            monitoring_policy_id: PortableObjectId::from_uuid(uuid::Uuid::new_v4())
                .expect("monitor"),
            login_automation_id: None,
        }
    }

    #[test]
    fn preference_differences_expose_group_only_and_legacy_absence_is_not_deletion() {
        let mut local = bundle(Vec::new());
        local.preferences = Some(PortablePreferencesV1 {
            product: "NoriShell".to_owned(),
            version: 1,
            groups: BTreeMap::from([(
                "application".to_owned(),
                serde_json::json!({"themePreference":"dark"}),
            )]),
        });
        let remote = bundle(Vec::new());
        let report = compare_bundles(&local, &remote);
        assert_eq!(report.total_count, 1);
        assert_eq!(
            report.differences[0].kind,
            SshSyncSecureDifferenceKind::Preferences
        );
        assert_eq!(
            report.differences[0].change,
            SshSyncSecureDifferenceChange::LocalOnly
        );
        assert_eq!(report.differences[0].label, "application");
        assert!(report.differences[0].local_summary.is_none());
        assert!(report.differences[0].remote_summary.is_none());
    }

    #[test]
    fn mirror_deletes_only_reviewed_scope_and_preserves_remote_history() {
        let in_scope = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("id");
        let excluded = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("id");
        let mut current = bundle(Vec::new());
        current
            .objects
            .identities
            .push(norishell_ssh_profile_sync::PortableIdentity {
                id: in_scope,
                label: "in scope".into(),
                username: None,
                credential_ids: Vec::new(),
            });
        let mut remote = bundle(Vec::new());
        remote.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Identity,
            id: excluded,
        });
        let applied = with_tombstones_for_missing(&remote, &current).expect("mirror plan");
        assert_eq!(
            applied.tombstones,
            vec![PortableTombstone {
                kind: PortableObjectKind::Identity,
                id: in_scope
            }]
        );
        assert_eq!(remote.tombstones[0].id, excluded);
        assert!(
            current
                .objects
                .identities
                .iter()
                .any(|value| value.id == in_scope)
        );
    }

    #[test]
    fn v5_mirror_plan_keeps_schema_and_drops_out_of_scope_tombstone_clock() {
        let in_scope = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("id");
        let old_tombstone = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("id");
        let mut current = v5(bundle(Vec::new()));
        current
            .objects
            .identities
            .push(norishell_ssh_profile_sync::PortableIdentity {
                id: in_scope,
                label: "in scope".into(),
                username: None,
                credential_ids: Vec::new(),
            });
        let mut remote = v5(bundle(Vec::new()));
        remote.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Identity,
            id: old_tombstone,
        });
        remote.update_times.push(PortableItemUpdateTime {
            kind: PortableObjectKind::Identity,
            id: old_tombstone,
            update_time_unix_ms: 1_000,
        });
        let staged = with_tombstones_for_missing(&remote, &current).expect("staged V5");
        assert_eq!(staged.schema, BundleSchema::V5);
        assert_eq!(staged.tombstones.len(), 1);
        assert_eq!(staged.tombstones[0].id, in_scope);
        assert!(staged.update_times.is_empty());
    }

    #[test]
    fn changed_secret_payload_never_enters_comparison_dto() {
        use norishell_ssh_profile_sync::{PortableSecret, PortableSecretKind, SecretBytes};
        let id = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("id");
        let mut local = bundle(Vec::new());
        local.secrets.push(PortableSecret {
            id,
            kind: PortableSecretKind::Password,
            selected_by_user: true,
            payload: SecretBytes::new(b"secret-local-never-display".to_vec()).expect("secret"),
        });
        let mut remote = local.clone();
        remote.secrets[0].payload =
            SecretBytes::new(b"secret-remote-never-display".to_vec()).expect("secret");
        let report = compare_bundles(&local, &remote);
        assert_eq!(report.changed_count, 1);
        let json = serde_json::to_string(&report.differences).expect("dto");
        assert!(!json.contains("secret-local-never-display"));
        assert!(!json.contains("secret-remote-never-display"));
        assert!(report.differences[0].local_summary.is_none());
        assert!(report.differences[0].remote_summary.is_none());
    }

    #[test]
    fn reports_safe_concrete_host_differences() {
        let shared_id = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("shared");
        let local_only_id = PortableObjectId::from_uuid(uuid::Uuid::new_v4()).expect("local");
        let report = compare_bundles(
            &bundle(vec![
                host(shared_id, "prod", "10.0.0.1"),
                host(local_only_id, "local", "10.0.0.2"),
            ]),
            &bundle(vec![host(shared_id, "prod", "10.0.0.9")]),
        );
        assert_eq!(report.total_count, 2);
        assert_eq!(report.omitted_count, 0);
        let changed = report
            .differences
            .iter()
            .find(|value| value.label == "prod")
            .expect("changed host");
        assert_eq!(changed.change, SshSyncSecureDifferenceChange::Changed);
        assert_eq!(changed.local_summary.as_deref(), Some("deploy@10.0.0.1:22"));
        assert_eq!(
            changed.remote_summary.as_deref(),
            Some("deploy@10.0.0.9:22")
        );
        let local_only = report
            .differences
            .iter()
            .find(|value| value.label == "local")
            .expect("local-only host");
        assert_eq!(local_only.change, SshSyncSecureDifferenceChange::LocalOnly);
    }
}
