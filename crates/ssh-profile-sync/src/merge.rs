use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    BundleSchema, PortableBundleV1, PortableDataCategory, PortableItemUpdateTime, PortableObjectId,
    PortableObjectKind, PortableObjects, PortablePreferencesV1, PortableTombstone, Result,
    SyncCodecError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleMergeOutcome {
    Merged(Box<PortableBundleV1>),
    Conflicts { count: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleConflictResolution {
    KeepLocal,
    UseRemote,
    Newest,
}

const MAX_FUTURE_CLOCK_SKEW_MS: i64 = 5 * 60 * 1000;
type ObjectNode = (PortableObjectKind, PortableObjectId);
type ResolvedSides = BTreeMap<ObjectNode, BundleConflictResolution>;

#[derive(Clone)]
enum Edit<T> {
    NoOpinion,
    Value(T),
    Omit,
    Delete,
}

#[derive(Clone, Copy)]
struct Applied<'a, T> {
    value: Option<&'a T>,
    tombstone: bool,
}

pub fn merge_bundles_three_way(
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    revision: u64,
) -> Result<BundleMergeOutcome> {
    merge_bundles_three_way_with_resolution(base, local, remote, revision, None)
}

pub fn merge_bundles_three_way_with_resolution(
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    revision: u64,
    resolution: Option<BundleConflictResolution>,
) -> Result<BundleMergeOutcome> {
    merge_bundles_three_way_with_policies(base, local, remote, revision, resolution, resolution)
}

pub fn merge_bundles_three_way_with_policies(
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    revision: u64,
    conflict_resolution: Option<BundleConflictResolution>,
    deletion_resolution: Option<BundleConflictResolution>,
) -> Result<BundleMergeOutcome> {
    base.validate()?;
    local.validate()?;
    remote.validate()?;
    let all_v6 = [base, local, remote]
        .iter()
        .all(|bundle| bundle.schema == BundleSchema::V6);
    if !all_v6
        && [base, local, remote]
            .iter()
            .any(|bundle| bundle.schema == BundleSchema::V6)
    {
        return Err(SyncCodecError::InvalidBundle(
            "mixed bundle v6 merge requires projection",
        ));
    }
    let normalized_scope = |bundle: &PortableBundleV1| {
        bundle.selected_categories.clone().unwrap_or_else(|| {
            vec![
                PortableDataCategory::Hosts,
                PortableDataCategory::Credentials,
                PortableDataCategory::DesktopProfiles,
            ]
        })
    };
    let scope = normalized_scope(base);
    if all_v6 && (normalized_scope(local) != scope || normalized_scope(remote) != scope) {
        return Err(SyncCodecError::InvalidBundle(
            "bundle category scopes differ",
        ));
    }
    let base_tombstones = tombstone_set(base);
    let local_tombstones = tombstone_set(local);
    let remote_tombstones = tombstone_set(remote);
    let conflict_nodes = collect_conflict_nodes(
        base,
        local,
        remote,
        &base_tombstones,
        &local_tombstones,
        &remote_tombstones,
    );
    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    let (resolved_sides, object_conflicts) = resolve_conflicts(
        [base, local, remote],
        &conflict_nodes,
        local,
        remote,
        conflict_resolution,
        deletion_resolution,
        now_unix_ms,
    );
    let (preferences, preference_conflicts) = if all_v6 {
        (None, 0)
    } else {
        merge_preferences(base, local, remote, conflict_resolution, now_unix_ms)
    };
    if object_conflicts > 0 || preference_conflicts > 0 {
        return Ok(BundleMergeOutcome::Conflicts {
            count: u32::try_from(object_conflicts.saturating_add(preference_conflicts))
                .unwrap_or(u32::MAX),
        });
    }
    let mut objects = PortableObjects::default();
    let mut secrets = Vec::new();
    let mut output_tombstones = base_tombstones.clone();

    macro_rules! merge_kind {
        ($kind:expr, $base:expr, $local:expr, $remote:expr, $output:expr, $id:expr) => {{
            let (values, deleted) = merge_values(
                $kind,
                $base,
                $local,
                $remote,
                &base_tombstones,
                &local_tombstones,
                &remote_tombstones,
                $id,
                &resolved_sides,
            );
            $output.extend(values);
            for id in deleted {
                output_tombstones.insert(($kind, id));
            }
            for value in $output.iter() {
                output_tombstones.remove(&($kind, $id(value)));
            }
        }};
    }

    merge_kind!(
        PortableObjectKind::Host,
        &base.objects.hosts,
        &local.objects.hosts,
        &remote.objects.hosts,
        objects.hosts,
        |value: &crate::PortableHost| value.id
    );
    merge_kind!(
        PortableObjectKind::DesktopProfile,
        &base.objects.desktop_profiles,
        &local.objects.desktop_profiles,
        &remote.objects.desktop_profiles,
        objects.desktop_profiles,
        |value: &crate::PortableDesktopProfile| value.id
    );
    merge_kind!(
        PortableObjectKind::Identity,
        &base.objects.identities,
        &local.objects.identities,
        &remote.objects.identities,
        objects.identities,
        |value: &crate::PortableIdentity| value.id
    );
    merge_kind!(
        PortableObjectKind::Credential,
        &base.objects.credentials,
        &local.objects.credentials,
        &remote.objects.credentials,
        objects.credentials,
        |value: &crate::PortableCredential| value.id
    );
    merge_kind!(
        PortableObjectKind::Route,
        &base.objects.routes,
        &local.objects.routes,
        &remote.objects.routes,
        objects.routes,
        |value: &crate::PortableRoute| value.id
    );
    merge_kind!(
        PortableObjectKind::AuthenticationPlan,
        &base.objects.authentication_plans,
        &local.objects.authentication_plans,
        &remote.objects.authentication_plans,
        objects.authentication_plans,
        |value: &crate::PortableAuthenticationPlan| value.id
    );
    merge_kind!(
        PortableObjectKind::AlgorithmPolicy,
        &base.objects.algorithm_policies,
        &local.objects.algorithm_policies,
        &remote.objects.algorithm_policies,
        objects.algorithm_policies,
        |value: &crate::PortableAlgorithmPolicy| value.id
    );
    merge_kind!(
        PortableObjectKind::HeartbeatPolicy,
        &base.objects.heartbeat_policies,
        &local.objects.heartbeat_policies,
        &remote.objects.heartbeat_policies,
        objects.heartbeat_policies,
        |value: &crate::PortableHeartbeatPolicy| value.id
    );
    merge_kind!(
        PortableObjectKind::MonitoringPolicy,
        &base.objects.monitoring_policies,
        &local.objects.monitoring_policies,
        &remote.objects.monitoring_policies,
        objects.monitoring_policies,
        |value: &crate::PortableMonitoringPolicy| value.id
    );
    merge_kind!(
        PortableObjectKind::LoginAutomation,
        &base.objects.login_automations,
        &local.objects.login_automations,
        &remote.objects.login_automations,
        objects.login_automations,
        |value: &crate::PortableLoginAutomation| value.id
    );
    merge_kind!(
        PortableObjectKind::Secret,
        &base.secrets,
        &local.secrets,
        &remote.secrets,
        secrets,
        |value: &crate::PortableSecret| value.id
    );

    let tombstones: Vec<_> = output_tombstones
        .into_iter()
        .map(|(kind, id)| PortableTombstone { kind, id })
        .collect();
    let update_times = merge_update_times(&objects, &secrets, &tombstones, base, local, remote);
    let preference_update_times = merge_preference_times(preferences.as_ref(), base, local, remote);
    let merged = PortableBundleV1 {
        schema: if all_v6 {
            BundleSchema::V6
        } else if preferences.is_some() {
            BundleSchema::V5
        } else {
            BundleSchema::V3
        },
        selected_categories: if all_v6 && scope.len() != 3 {
            Some(scope)
        } else {
            None
        },
        revision,
        objects,
        preferences,
        secrets,
        skipped_machine_bound: local.skipped_machine_bound.clone(),
        tombstones,
        update_times,
        preference_update_times,
    };
    if (conflict_resolution.is_some() || deletion_resolution.is_some())
        && (merged.validate().is_err() || validate_reverse_reachability(&merged).is_err())
    {
        return Ok(BundleMergeOutcome::Conflicts { count: 1 });
    }
    merged.validate()?;
    validate_reverse_reachability(&merged)?;
    Ok(BundleMergeOutcome::Merged(Box::new(merged)))
}

fn merge_preferences(
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    resolution: Option<BundleConflictResolution>,
    now_unix_ms: i64,
) -> (Option<PortablePreferencesV1>, usize) {
    let (Some(local_preferences), Some(remote_preferences)) =
        (&local.preferences, &remote.preferences)
    else {
        // A legacy bundle has no opinion about preferences; it cannot delete
        // preferences written by a newer client.
        return (
            local
                .preferences
                .as_ref()
                .or(remote.preferences.as_ref())
                .cloned(),
            0,
        );
    };
    let mut merged = local_preferences.clone();
    let mut conflicts = 0;
    for (group, local_value) in &local_preferences.groups {
        let remote_value = &remote_preferences.groups[group];
        let base_value = base
            .preferences
            .as_ref()
            .and_then(|value| value.groups.get(group));
        let chosen = if local_value == remote_value || base_value == Some(remote_value) {
            local_value
        } else if base_value == Some(local_value) {
            remote_value
        } else {
            match resolution {
                Some(BundleConflictResolution::UseRemote) => remote_value,
                Some(BundleConflictResolution::Newest) => match newest_side(
                    local.preference_update_times.get(group).copied(),
                    remote.preference_update_times.get(group).copied(),
                    now_unix_ms,
                ) {
                    Some(BundleConflictResolution::UseRemote) => remote_value,
                    Some(BundleConflictResolution::KeepLocal) => local_value,
                    _ => {
                        conflicts += 1;
                        local_value
                    }
                },
                Some(BundleConflictResolution::KeepLocal) => local_value,
                None => {
                    conflicts += 1;
                    local_value
                }
            }
        };
        merged.groups.insert(group.clone(), chosen.clone());
    }
    (Some(merged), conflicts)
}

fn newest_side(
    local: Option<i64>,
    remote: Option<i64>,
    now_unix_ms: i64,
) -> Option<BundleConflictResolution> {
    let (Some(local), Some(remote)) = (local, remote) else {
        return None;
    };
    if local <= 0
        || remote <= 0
        || local > now_unix_ms.saturating_add(MAX_FUTURE_CLOCK_SKEW_MS)
        || remote > now_unix_ms.saturating_add(MAX_FUTURE_CLOCK_SKEW_MS)
    {
        return None;
    }
    match local.cmp(&remote) {
        std::cmp::Ordering::Greater => Some(BundleConflictResolution::KeepLocal),
        std::cmp::Ordering::Less => Some(BundleConflictResolution::UseRemote),
        std::cmp::Ordering::Equal => None,
    }
}

fn item_time(bundle: &PortableBundleV1, node: ObjectNode) -> Option<i64> {
    bundle
        .update_times
        .iter()
        .find(|item| (item.kind, item.id) == node)
        .map(|item| item.update_time_unix_ms)
}

fn resolve_conflicts(
    bundles: [&PortableBundleV1; 3],
    conflicts: &BTreeSet<ObjectNode>,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    conflict_resolution: Option<BundleConflictResolution>,
    deletion_resolution: Option<BundleConflictResolution>,
    now_unix_ms: i64,
) -> (ResolvedSides, usize) {
    let mut sides = ResolvedSides::new();
    if conflicts.is_empty() {
        return (sides, 0);
    }
    match (conflict_resolution, deletion_resolution) {
        (None, None) => (sides, conflicts.len()),
        (
            Some(BundleConflictResolution::KeepLocal | BundleConflictResolution::UseRemote),
            resolution,
        ) if conflict_resolution == resolution => {
            let side = conflict_resolution.expect("explicit side");
            let selected = if bundles
                .iter()
                .all(|bundle| matches!(bundle.schema, BundleSchema::V5 | BundleSchema::V6))
            {
                conflicts.clone()
            } else {
                // Legacy explicit direction selected the full dependency closure.
                expand_conflict_closure(bundles, conflicts)
            };
            sides.extend(selected.into_iter().map(|node| (node, side)));
            (sides, 0)
        }
        _ => {
            let mut remaining = conflicts.clone();
            let mut unresolved = 0;
            while let Some(node) = remaining.iter().next().copied() {
                let closure = expand_conflict_closure(bundles, &BTreeSet::from([node]));
                let component = closure.intersection(conflicts).copied().collect::<Vec<_>>();
                for member in &component {
                    remaining.remove(member);
                }
                let mut chosen = None;
                let mut consistent = true;
                for member in &component {
                    let newest = newest_side(
                        item_time(local, *member),
                        item_time(remote, *member),
                        now_unix_ms,
                    );
                    let chosen_is_deletion = newest.is_some_and(|winner| match winner {
                        BundleConflictResolution::KeepLocal => local
                            .tombstones
                            .iter()
                            .any(|item| (item.kind, item.id) == *member),
                        BundleConflictResolution::UseRemote => remote
                            .tombstones
                            .iter()
                            .any(|item| (item.kind, item.id) == *member),
                        BundleConflictResolution::Newest => false,
                    });
                    let policy = if chosen_is_deletion {
                        deletion_resolution
                    } else {
                        conflict_resolution
                    };
                    let side = match policy {
                        Some(BundleConflictResolution::Newest) => newest,
                        Some(
                            BundleConflictResolution::KeepLocal
                            | BundleConflictResolution::UseRemote,
                        ) => policy,
                        None => None,
                    };
                    if side.is_none() || chosen.is_some_and(|previous| Some(previous) != side) {
                        consistent = false;
                    }
                    chosen = side.or(chosen);
                }
                if consistent {
                    let side = chosen.expect("nonempty component has a decision");
                    // The closure groups interdependent conflicts. One-sided
                    // changes in that closure still merge on their own merits.
                    sides.extend(component.into_iter().map(|member| (member, side)));
                } else {
                    unresolved += component.len();
                }
            }
            (sides, unresolved)
        }
    }
}

fn merge_update_times(
    objects: &PortableObjects,
    secrets: &[crate::PortableSecret],
    tombstones: &[PortableTombstone],
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
) -> Vec<PortableItemUpdateTime> {
    let mut times = BTreeMap::<ObjectNode, i64>::new();
    macro_rules! copy_kind {
        ($kind:expr, $field:ident) => {
            for value in &objects.$field {
                let node = ($kind, value.id);
                for bundle in [base, local, remote] {
                    if bundle
                        .objects
                        .$field
                        .iter()
                        .any(|candidate| candidate == value)
                    {
                        if let Some(time) = item_time(bundle, node) {
                            times
                                .entry(node)
                                .and_modify(|old| *old = (*old).max(time))
                                .or_insert(time);
                        }
                    }
                }
            }
        };
    }
    copy_kind!(PortableObjectKind::Host, hosts);
    copy_kind!(PortableObjectKind::DesktopProfile, desktop_profiles);
    copy_kind!(PortableObjectKind::Identity, identities);
    copy_kind!(PortableObjectKind::Credential, credentials);
    copy_kind!(PortableObjectKind::Route, routes);
    copy_kind!(PortableObjectKind::AuthenticationPlan, authentication_plans);
    copy_kind!(PortableObjectKind::AlgorithmPolicy, algorithm_policies);
    copy_kind!(PortableObjectKind::HeartbeatPolicy, heartbeat_policies);
    copy_kind!(PortableObjectKind::MonitoringPolicy, monitoring_policies);
    copy_kind!(PortableObjectKind::LoginAutomation, login_automations);
    for value in secrets {
        let node = (PortableObjectKind::Secret, value.id);
        for bundle in [base, local, remote] {
            if bundle.secrets.iter().any(|candidate| candidate == value)
                && let Some(time) = item_time(bundle, node)
            {
                times
                    .entry(node)
                    .and_modify(|old| *old = (*old).max(time))
                    .or_insert(time);
            }
        }
    }
    for value in tombstones {
        let node = (value.kind, value.id);
        for bundle in [base, local, remote] {
            if bundle.tombstones.contains(value)
                && let Some(time) = item_time(bundle, node)
            {
                times
                    .entry(node)
                    .and_modify(|old| *old = (*old).max(time))
                    .or_insert(time);
            }
        }
    }
    times
        .into_iter()
        .map(|((kind, id), update_time_unix_ms)| PortableItemUpdateTime {
            kind,
            id,
            update_time_unix_ms,
        })
        .collect()
}

fn merge_preference_times(
    preferences: Option<&PortablePreferencesV1>,
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
) -> BTreeMap<String, i64> {
    let mut times = BTreeMap::new();
    let Some(preferences) = preferences else {
        return times;
    };
    for (group, selected) in &preferences.groups {
        for bundle in [base, local, remote] {
            if bundle
                .preferences
                .as_ref()
                .and_then(|value| value.groups.get(group))
                == Some(selected)
                && let Some(time) = bundle.preference_update_times.get(group)
            {
                times
                    .entry(group.clone())
                    .and_modify(|old: &mut i64| *old = (*old).max(*time))
                    .or_insert(*time);
            }
        }
    }
    times
}

#[allow(clippy::too_many_arguments)]
fn merge_values<T: Clone + PartialEq>(
    kind: PortableObjectKind,
    base: &[T],
    local: &[T],
    remote: &[T],
    base_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    local_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    remote_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    id: impl Fn(&T) -> PortableObjectId + Copy,
    resolved_sides: &ResolvedSides,
) -> (Vec<T>, Vec<PortableObjectId>) {
    let base = value_map(base, id);
    let local = value_map(local, id);
    let remote = value_map(remote, id);
    let ids = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .chain(
            base_tombstones
                .iter()
                .filter_map(|(value_kind, value_id)| (*value_kind == kind).then_some(value_id)),
        )
        .chain(
            local_tombstones
                .iter()
                .filter_map(|(value_kind, value_id)| (*value_kind == kind).then_some(value_id)),
        )
        .chain(
            remote_tombstones
                .iter()
                .filter_map(|(value_kind, value_id)| (*value_kind == kind).then_some(value_id)),
        )
        .copied()
        .collect::<BTreeSet<_>>();
    let mut values = Vec::new();
    let mut deleted = Vec::new();
    for value_id in ids {
        let base_value = base.get(&value_id).copied();
        let local_edit = edit_for(
            base_value.is_some(),
            local.get(&value_id).copied(),
            local_tombstones.contains(&(kind, value_id)),
        );
        let remote_edit = edit_for(
            base_value.is_some(),
            remote.get(&value_id).copied(),
            remote_tombstones.contains(&(kind, value_id)),
        );
        let local_result = apply_edit(base_value, &local_edit);
        let remote_result = apply_edit(base_value, &remote_edit);
        let base_deleted = base_tombstones.contains(&(kind, value_id));
        let local_changed = changed(base_value, base_deleted, &local_edit, &local_result);
        let remote_changed = changed(base_value, base_deleted, &remote_edit, &remote_result);
        let selected = if let Some(side) = resolved_sides.get(&(kind, value_id)) {
            match side {
                BundleConflictResolution::KeepLocal => local_result,
                BundleConflictResolution::UseRemote => remote_result,
                BundleConflictResolution::Newest => unreachable!("newest is reduced to a side"),
            }
        } else {
            match (local_changed, remote_changed) {
                (false, false) => Applied {
                    value: base_value,
                    tombstone: base_deleted,
                },
                (true, false) => local_result,
                (false, true) => remote_result,
                (true, true) if local_result.value == remote_result.value => Applied {
                    value: local_result.value,
                    tombstone: local_result.tombstone || remote_result.tombstone,
                },
                (true, true) => unreachable!("unresolved conflicts return before merging"),
            }
        };
        match selected.value {
            Some(value) => values.push(value.clone()),
            None if selected.tombstone => deleted.push(value_id),
            None => {}
        }
    }
    (values, deleted)
}

#[allow(clippy::too_many_arguments)]
fn conflict_ids<T: PartialEq>(
    kind: PortableObjectKind,
    base: &[T],
    local: &[T],
    remote: &[T],
    base_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    local_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    remote_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    id: impl Fn(&T) -> PortableObjectId + Copy,
) -> BTreeSet<(PortableObjectKind, PortableObjectId)> {
    let base = value_map(base, id);
    let local = value_map(local, id);
    let remote = value_map(remote, id);
    base.keys()
        .chain(local.keys())
        .chain(remote.keys())
        .chain(
            base_tombstones
                .iter()
                .filter_map(|(value_kind, value_id)| (*value_kind == kind).then_some(value_id)),
        )
        .chain(
            local_tombstones
                .iter()
                .filter_map(|(value_kind, value_id)| (*value_kind == kind).then_some(value_id)),
        )
        .chain(
            remote_tombstones
                .iter()
                .filter_map(|(value_kind, value_id)| (*value_kind == kind).then_some(value_id)),
        )
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|value_id| {
            let base_value = base.get(&value_id).copied();
            let local_edit = edit_for(
                base_value.is_some(),
                local.get(&value_id).copied(),
                local_tombstones.contains(&(kind, value_id)),
            );
            let remote_edit = edit_for(
                base_value.is_some(),
                remote.get(&value_id).copied(),
                remote_tombstones.contains(&(kind, value_id)),
            );
            let local_result = apply_edit(base_value, &local_edit);
            let remote_result = apply_edit(base_value, &remote_edit);
            let base_deleted = base_tombstones.contains(&(kind, value_id));
            (changed(base_value, base_deleted, &local_edit, &local_result)
                && changed(base_value, base_deleted, &remote_edit, &remote_result)
                && (local_result.value != remote_result.value
                    || local_result.tombstone != remote_result.tombstone))
                .then_some((kind, value_id))
        })
        .collect()
}

fn collect_conflict_nodes(
    base: &PortableBundleV1,
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    base_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    local_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
    remote_tombstones: &BTreeSet<(PortableObjectKind, PortableObjectId)>,
) -> BTreeSet<(PortableObjectKind, PortableObjectId)> {
    let mut conflicts = BTreeSet::new();
    macro_rules! collect_kind {
        ($kind:expr, $field:ident, $id:expr) => {
            conflicts.extend(conflict_ids(
                $kind,
                &base.objects.$field,
                &local.objects.$field,
                &remote.objects.$field,
                base_tombstones,
                local_tombstones,
                remote_tombstones,
                $id,
            ));
        };
    }
    collect_kind!(
        PortableObjectKind::Host,
        hosts,
        |value: &crate::PortableHost| value.id
    );
    collect_kind!(
        PortableObjectKind::DesktopProfile,
        desktop_profiles,
        |value: &crate::PortableDesktopProfile| value.id
    );
    collect_kind!(
        PortableObjectKind::Identity,
        identities,
        |value: &crate::PortableIdentity| value.id
    );
    collect_kind!(
        PortableObjectKind::Credential,
        credentials,
        |value: &crate::PortableCredential| value.id
    );
    collect_kind!(
        PortableObjectKind::Route,
        routes,
        |value: &crate::PortableRoute| value.id
    );
    collect_kind!(
        PortableObjectKind::AuthenticationPlan,
        authentication_plans,
        |value: &crate::PortableAuthenticationPlan| value.id
    );
    collect_kind!(
        PortableObjectKind::AlgorithmPolicy,
        algorithm_policies,
        |value: &crate::PortableAlgorithmPolicy| value.id
    );
    collect_kind!(
        PortableObjectKind::HeartbeatPolicy,
        heartbeat_policies,
        |value: &crate::PortableHeartbeatPolicy| value.id
    );
    collect_kind!(
        PortableObjectKind::MonitoringPolicy,
        monitoring_policies,
        |value: &crate::PortableMonitoringPolicy| value.id
    );
    collect_kind!(
        PortableObjectKind::LoginAutomation,
        login_automations,
        |value: &crate::PortableLoginAutomation| value.id
    );
    conflicts.extend(conflict_ids(
        PortableObjectKind::Secret,
        &base.secrets,
        &local.secrets,
        &remote.secrets,
        base_tombstones,
        local_tombstones,
        remote_tombstones,
        |value: &crate::PortableSecret| value.id,
    ));
    conflicts
}

fn expand_conflict_closure<'a>(
    bundles: impl IntoIterator<Item = &'a PortableBundleV1>,
    conflicts: &BTreeSet<ObjectNode>,
) -> BTreeSet<ObjectNode> {
    let mut graph = BTreeMap::<ObjectNode, BTreeSet<ObjectNode>>::new();
    for bundle in bundles {
        add_bundle_edges(bundle, &mut graph);
    }
    let mut closure = conflicts.clone();
    let mut queue = conflicts.iter().copied().collect::<VecDeque<_>>();
    while let Some(node) = queue.pop_front() {
        for neighbour in graph.get(&node).into_iter().flatten() {
            if closure.insert(*neighbour) {
                queue.push_back(*neighbour);
            }
        }
    }
    closure
}

fn add_edge(
    graph: &mut BTreeMap<ObjectNode, BTreeSet<ObjectNode>>,
    left: ObjectNode,
    right: ObjectNode,
) {
    graph.entry(left).or_default().insert(right);
    graph.entry(right).or_default().insert(left);
}

fn add_bundle_edges(
    bundle: &PortableBundleV1,
    graph: &mut BTreeMap<ObjectNode, BTreeSet<ObjectNode>>,
) {
    use crate::{LoginAutomationStep, PortableCredentialMaterial, RouteIngress};

    for host in &bundle.objects.hosts {
        let host_node = (PortableObjectKind::Host, host.id);
        for child in [
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
        ] {
            add_edge(graph, host_node, child);
        }
        if let Some(id) = host.identity_id {
            add_edge(graph, host_node, (PortableObjectKind::Identity, id));
        }
        if let Some(id) = host.login_automation_id {
            add_edge(graph, host_node, (PortableObjectKind::LoginAutomation, id));
        }
    }
    for desktop in &bundle.objects.desktop_profiles {
        let desktop_node = (PortableObjectKind::DesktopProfile, desktop.id);
        if let Some(host_id) = desktop.host_id {
            add_edge(graph, desktop_node, (PortableObjectKind::Host, host_id));
        }
        if let Some(gateway_host_id) = desktop.gateway_host_id {
            add_edge(
                graph,
                desktop_node,
                (PortableObjectKind::Host, gateway_host_id),
            );
        }
        if let Some(credential_id) = desktop.credential_id {
            add_edge(
                graph,
                desktop_node,
                (PortableObjectKind::Credential, credential_id),
            );
        }
    }
    for identity in &bundle.objects.identities {
        for credential_id in &identity.credential_ids {
            add_edge(
                graph,
                (PortableObjectKind::Identity, identity.id),
                (PortableObjectKind::Credential, *credential_id),
            );
        }
    }
    for plan in &bundle.objects.authentication_plans {
        for credential_id in &plan.credential_ids {
            add_edge(
                graph,
                (PortableObjectKind::AuthenticationPlan, plan.id),
                (PortableObjectKind::Credential, *credential_id),
            );
        }
    }
    for credential in &bundle.objects.credentials {
        add_edge(
            graph,
            (PortableObjectKind::Credential, credential.id),
            (PortableObjectKind::Identity, credential.identity_id),
        );
        let mut secret_ids = Vec::new();
        match &credential.material {
            PortableCredentialMaterial::Password { password_secret_id } => {
                secret_ids.push(*password_secret_id);
            }
            PortableCredentialMaterial::PrivateKey {
                private_key_secret_id,
                passphrase_secret_id,
                ..
            } => {
                secret_ids.push(*private_key_secret_id);
                secret_ids.extend(*passphrase_secret_id);
            }
            PortableCredentialMaterial::Certificate {
                certificate_secret_id,
                private_key_secret_id,
                passphrase_secret_id,
                ..
            } => {
                secret_ids.push(*certificate_secret_id);
                secret_ids.extend(*private_key_secret_id);
                secret_ids.extend(*passphrase_secret_id);
            }
            PortableCredentialMaterial::KeyboardInteractive { .. } => {}
        }
        for secret_id in secret_ids {
            add_edge(
                graph,
                (PortableObjectKind::Credential, credential.id),
                (PortableObjectKind::Secret, secret_id),
            );
        }
    }
    for route in &bundle.objects.routes {
        let route_node = (PortableObjectKind::Route, route.id);
        for hop in &route.jump_hops {
            add_edge(graph, route_node, (PortableObjectKind::Host, hop.host_id));
        }
        let credential_id = match route.ingress {
            RouteIngress::Direct => None,
            RouteIngress::HttpConnect { credential_id, .. }
            | RouteIngress::Socks5 { credential_id, .. } => credential_id,
        };
        if let Some(credential_id) = credential_id {
            add_edge(
                graph,
                route_node,
                (PortableObjectKind::Credential, credential_id),
            );
        }
    }
    for automation in &bundle.objects.login_automations {
        for step in &automation.steps {
            if let LoginAutomationStep::SendSecret { secret_id, .. } = step {
                add_edge(
                    graph,
                    (PortableObjectKind::LoginAutomation, automation.id),
                    (PortableObjectKind::Secret, *secret_id),
                );
            }
        }
    }
}

fn validate_reverse_reachability(bundle: &PortableBundleV1) -> Result<()> {
    use crate::{LoginAutomationStep, PortableCredentialMaterial, RouteIngress};

    let mut host_children = BTreeSet::new();
    for host in &bundle.objects.hosts {
        host_children.extend([
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
        if let Some(id) = host.login_automation_id {
            host_children.insert((PortableObjectKind::LoginAutomation, id));
        }
    }
    for (kind, ids) in [
        (
            PortableObjectKind::Route,
            bundle
                .objects
                .routes
                .iter()
                .map(|value| value.id)
                .collect::<Vec<_>>(),
        ),
        (
            PortableObjectKind::AuthenticationPlan,
            bundle
                .objects
                .authentication_plans
                .iter()
                .map(|value| value.id)
                .collect(),
        ),
        (
            PortableObjectKind::AlgorithmPolicy,
            bundle
                .objects
                .algorithm_policies
                .iter()
                .map(|value| value.id)
                .collect(),
        ),
        (
            PortableObjectKind::HeartbeatPolicy,
            bundle
                .objects
                .heartbeat_policies
                .iter()
                .map(|value| value.id)
                .collect(),
        ),
        (
            PortableObjectKind::MonitoringPolicy,
            bundle
                .objects
                .monitoring_policies
                .iter()
                .map(|value| value.id)
                .collect(),
        ),
        (
            PortableObjectKind::LoginAutomation,
            bundle
                .objects
                .login_automations
                .iter()
                .map(|value| value.id)
                .collect(),
        ),
    ] {
        if ids
            .into_iter()
            .any(|id| !host_children.contains(&(kind, id)))
        {
            return Err(SyncCodecError::DanglingObjectReference);
        }
    }

    let mut referenced_credentials = BTreeSet::new();
    for identity in &bundle.objects.identities {
        referenced_credentials.extend(identity.credential_ids.iter().copied());
    }
    for plan in &bundle.objects.authentication_plans {
        referenced_credentials.extend(plan.credential_ids.iter().copied());
    }
    for route in &bundle.objects.routes {
        match route.ingress {
            RouteIngress::Direct => {}
            RouteIngress::HttpConnect { credential_id, .. }
            | RouteIngress::Socks5 { credential_id, .. } => {
                referenced_credentials.extend(credential_id);
            }
        }
    }
    for desktop in &bundle.objects.desktop_profiles {
        referenced_credentials.extend(desktop.credential_id);
    }
    if bundle
        .objects
        .credentials
        .iter()
        .any(|credential| !referenced_credentials.contains(&credential.id))
    {
        return Err(SyncCodecError::DanglingObjectReference);
    }

    let mut referenced_secrets = BTreeSet::new();
    for credential in &bundle.objects.credentials {
        match credential.material {
            PortableCredentialMaterial::Password { password_secret_id } => {
                referenced_secrets.insert(password_secret_id);
            }
            PortableCredentialMaterial::PrivateKey {
                private_key_secret_id,
                passphrase_secret_id,
                ..
            } => {
                referenced_secrets.insert(private_key_secret_id);
                referenced_secrets.extend(passphrase_secret_id);
            }
            PortableCredentialMaterial::Certificate {
                certificate_secret_id,
                private_key_secret_id,
                passphrase_secret_id,
                ..
            } => {
                referenced_secrets.insert(certificate_secret_id);
                referenced_secrets.extend(private_key_secret_id);
                referenced_secrets.extend(passphrase_secret_id);
            }
            PortableCredentialMaterial::KeyboardInteractive { .. } => {}
        }
    }
    for automation in &bundle.objects.login_automations {
        for step in &automation.steps {
            if let LoginAutomationStep::SendSecret { secret_id, .. } = step {
                referenced_secrets.insert(*secret_id);
            }
        }
    }
    if bundle
        .secrets
        .iter()
        .any(|secret| !referenced_secrets.contains(&secret.id))
    {
        return Err(SyncCodecError::DanglingObjectReference);
    }
    Ok(())
}

fn value_map<T>(
    values: &[T],
    id: impl Fn(&T) -> PortableObjectId,
) -> BTreeMap<PortableObjectId, &T> {
    values.iter().map(|value| (id(value), value)).collect()
}

fn edit_for<T>(base_present: bool, value: Option<&T>, deleted: bool) -> Edit<&T> {
    match (value, deleted, base_present) {
        (Some(value), false, _) => Edit::Value(value),
        (None, true, _) => Edit::Delete,
        (None, false, true) => Edit::Omit,
        _ => Edit::NoOpinion,
    }
}

fn apply_edit<'a, T>(base: Option<&'a T>, edit: &Edit<&'a T>) -> Applied<'a, T> {
    match edit {
        Edit::NoOpinion => Applied {
            value: base,
            tombstone: false,
        },
        Edit::Value(value) => Applied {
            value: Some(*value),
            tombstone: false,
        },
        Edit::Omit => Applied {
            value: None,
            tombstone: false,
        },
        Edit::Delete => Applied {
            value: None,
            tombstone: true,
        },
    }
}

fn changed<T: PartialEq>(
    base: Option<&T>,
    base_deleted: bool,
    edit: &Edit<&T>,
    result: &Applied<'_, T>,
) -> bool {
    !matches!(edit, Edit::NoOpinion) && (result.value != base || result.tombstone != base_deleted)
}

fn tombstone_set(bundle: &PortableBundleV1) -> BTreeSet<(PortableObjectKind, PortableObjectId)> {
    bundle
        .tombstones
        .iter()
        .map(|value| (value.kind, value.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::{
        BundleSchema, HeartbeatMode, PortableAlgorithmPolicy, PortableAuthenticationPlan,
        PortableBundleV1, PortableCredential, PortableCredentialMaterial, PortableDataCategory,
        PortableDesktopProfile, PortableDesktopProtocol, PortableHeartbeatPolicy, PortableHost,
        PortableIdentity, PortableItemUpdateTime, PortableMonitoringPolicy, PortableObjectId,
        PortableObjectKind, PortableObjects, PortablePreferencesV1, PortableRoute, PortableSecret,
        PortableSecretKind, PortableTombstone, RouteIngress, SecretBytes,
    };

    use super::{
        BundleConflictResolution, BundleMergeOutcome, merge_bundles_three_way,
        merge_bundles_three_way_with_policies, merge_bundles_three_way_with_resolution,
    };

    fn id(value: u128) -> PortableObjectId {
        PortableObjectId::from_uuid(Uuid::from_u128(value)).expect("portable ID")
    }

    fn v5(mut bundle: PortableBundleV1) -> PortableBundleV1 {
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
        bundle.schema = BundleSchema::V5;
        bundle.preferences = Some(PortablePreferencesV1 {
            product: "NoriShell".to_owned(),
            version: 1,
            groups: serde_json::from_value(groups).expect("groups"),
        });
        bundle
    }

    fn timed(
        bundle: &mut PortableBundleV1,
        kind: PortableObjectKind,
        id: PortableObjectId,
        time: i64,
    ) {
        bundle.update_times.push(PortableItemUpdateTime {
            kind,
            id,
            update_time_unix_ms: time,
        });
    }

    fn bundle(value: Option<&[u8]>, deleted: bool) -> PortableBundleV1 {
        let credential_id = id(2);
        let identity_id = id(3);
        PortableBundleV1 {
            schema: BundleSchema::V2,
            selected_categories: None,
            revision: 1,
            preferences: None,
            objects: PortableObjects {
                identities: vec![PortableIdentity {
                    id: identity_id,
                    label: "identity".to_owned(),
                    username: None,
                    credential_ids: value.map(|_| vec![credential_id]).unwrap_or_default(),
                }],
                credentials: value
                    .map(|_| PortableCredential {
                        id: credential_id,
                        identity_id,
                        label: "credential".to_owned(),
                        material: PortableCredentialMaterial::Password {
                            password_secret_id: id(1),
                        },
                    })
                    .into_iter()
                    .collect(),
                ..PortableObjects::default()
            },
            secrets: value
                .map(|value| PortableSecret {
                    id: id(1),
                    kind: PortableSecretKind::Password,
                    selected_by_user: true,
                    payload: SecretBytes::new(value.to_vec()).expect("secret"),
                })
                .into_iter()
                .collect(),
            skipped_machine_bound: Vec::new(),
            tombstones: if deleted {
                vec![
                    PortableTombstone {
                        kind: PortableObjectKind::Credential,
                        id: credential_id,
                    },
                    PortableTombstone {
                        kind: PortableObjectKind::Secret,
                        id: id(1),
                    },
                ]
            } else {
                Vec::new()
            },
            update_times: Vec::new(),
            preference_update_times: Default::default(),
        }
    }

    fn host_bundle(deleted: bool, remote_route_edit: bool) -> PortableBundleV1 {
        let host_id = id(10);
        let route_id = id(11);
        let auth_id = id(12);
        let algorithm_id = id(13);
        let heartbeat_id = id(14);
        let monitoring_id = id(15);
        let objects = if deleted {
            PortableObjects::default()
        } else {
            PortableObjects {
                hosts: vec![PortableHost {
                    id: host_id,
                    label: "host".to_owned(),
                    address: "host.example".to_owned(),
                    port: 22,
                    username: None,
                    favorite: false,
                    tags: Default::default(),
                    identity_id: None,
                    route_id,
                    authentication_plan_id: auth_id,
                    algorithm_policy_id: algorithm_id,
                    heartbeat_policy_id: heartbeat_id,
                    monitoring_policy_id: monitoring_id,
                    login_automation_id: None,
                }],
                routes: vec![PortableRoute {
                    id: route_id,
                    ingress: if remote_route_edit {
                        RouteIngress::Socks5 {
                            host: "proxy.example".to_owned(),
                            port: 1080,
                            username: None,
                            credential_id: None,
                            resolve_dns_remotely: true,
                        }
                    } else {
                        RouteIngress::Direct
                    },
                    jump_hops: Vec::new(),
                }],
                authentication_plans: vec![PortableAuthenticationPlan {
                    id: auth_id,
                    credential_ids: Vec::new(),
                }],
                algorithm_policies: vec![PortableAlgorithmPolicy {
                    id: algorithm_id,
                    policy_id: "secure-default".to_owned(),
                    compatibility_exceptions: Vec::new(),
                }],
                heartbeat_policies: vec![PortableHeartbeatPolicy {
                    id: heartbeat_id,
                    mode: HeartbeatMode::Disabled,
                }],
                monitoring_policies: vec![PortableMonitoringPolicy {
                    id: monitoring_id,
                    enabled: false,
                    interval_seconds: 30,
                    timeout_seconds: 5,
                    interval_millis: None,
                    timeout_millis: None,
                    resources: Default::default(),
                }],
                ..PortableObjects::default()
            }
        };
        PortableBundleV1 {
            schema: BundleSchema::V2,
            selected_categories: None,
            revision: 1,
            preferences: None,
            objects,
            secrets: Vec::new(),
            skipped_machine_bound: Vec::new(),
            tombstones: if deleted {
                [
                    (PortableObjectKind::Host, host_id),
                    (PortableObjectKind::Route, route_id),
                    (PortableObjectKind::AuthenticationPlan, auth_id),
                    (PortableObjectKind::AlgorithmPolicy, algorithm_id),
                    (PortableObjectKind::HeartbeatPolicy, heartbeat_id),
                    (PortableObjectKind::MonitoringPolicy, monitoring_id),
                ]
                .into_iter()
                .map(|(kind, id)| PortableTombstone { kind, id })
                .collect()
            } else {
                Vec::new()
            },
            update_times: Vec::new(),
            preference_update_times: Default::default(),
        }
    }

    #[test]
    fn v6_merge_preserves_authenticated_category_scope() {
        let mut base = host_bundle(false, false);
        base.schema = BundleSchema::V6;
        base.selected_categories = Some(vec![PortableDataCategory::Hosts]);
        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &base, &base, 2).unwrap()
        else {
            panic!("unchanged subset must merge");
        };
        assert_eq!(merged.selected_categories, base.selected_categories);

        let mut expanded = base.clone();
        expanded.selected_categories = None;
        assert!(merge_bundles_three_way(&base, &base, &expanded, 2).is_err());
    }

    fn shared_secret_bundle(secret: &[u8], include_first_credential: bool) -> PortableBundleV1 {
        let identity_id = id(20);
        let first_id = id(21);
        let second_id = id(22);
        let secret_id = id(23);
        let mut credential_ids = vec![second_id];
        if include_first_credential {
            credential_ids.insert(0, first_id);
        }
        PortableBundleV1 {
            schema: BundleSchema::V2,
            selected_categories: None,
            revision: 1,
            preferences: None,
            objects: PortableObjects {
                identities: vec![PortableIdentity {
                    id: identity_id,
                    label: "shared".to_owned(),
                    username: None,
                    credential_ids,
                }],
                credentials: [
                    include_first_credential.then_some(PortableCredential {
                        id: first_id,
                        identity_id,
                        label: "first".to_owned(),
                        material: PortableCredentialMaterial::Password {
                            password_secret_id: secret_id,
                        },
                    }),
                    Some(PortableCredential {
                        id: second_id,
                        identity_id,
                        label: "second".to_owned(),
                        material: PortableCredentialMaterial::Password {
                            password_secret_id: secret_id,
                        },
                    }),
                ]
                .into_iter()
                .flatten()
                .collect(),
                ..PortableObjects::default()
            },
            secrets: vec![PortableSecret {
                id: secret_id,
                kind: PortableSecretKind::Password,
                selected_by_user: true,
                payload: SecretBytes::new(secret.to_vec()).expect("secret"),
            }],
            skipped_machine_bound: Vec::new(),
            tombstones: (!include_first_credential)
                .then_some(PortableTombstone {
                    kind: PortableObjectKind::Credential,
                    id: first_id,
                })
                .into_iter()
                .collect(),
            update_times: Vec::new(),
            preference_update_times: Default::default(),
        }
    }

    fn proxy_bundle(credential_id: Option<PortableObjectId>) -> PortableBundleV1 {
        let mut bundle = host_bundle(false, false);
        let credentials = shared_secret_bundle(b"shared", true);
        bundle.objects.identities = credentials.objects.identities;
        bundle.objects.credentials = credentials.objects.credentials;
        bundle.secrets = credentials.secrets;
        bundle.objects.routes[0].ingress = RouteIngress::HttpConnect {
            host: "proxy.example".to_owned(),
            port: 8080,
            username: Some("proxy account".to_owned()),
            credential_id,
        };
        bundle
    }

    fn desktop_bundle(label: &str) -> PortableBundleV1 {
        let mut bundle = host_bundle(false, false);
        let identity_id = id(20);
        let credential_id = id(21);
        let secret_id = id(22);
        let gateway_host_id = id(30);
        let gateway_route_id = id(31);
        let gateway_auth_id = id(32);
        let gateway_algorithm_id = id(33);
        let gateway_heartbeat_id = id(34);
        let gateway_monitoring_id = id(35);
        bundle.schema = BundleSchema::V3;
        bundle.objects.identities.push(PortableIdentity {
            id: identity_id,
            label: "desktop identity".to_owned(),
            username: Some("desktop-user".to_owned()),
            credential_ids: vec![credential_id],
        });
        bundle.objects.credentials.push(PortableCredential {
            id: credential_id,
            identity_id,
            label: "desktop password".to_owned(),
            material: PortableCredentialMaterial::Password {
                password_secret_id: secret_id,
            },
        });
        bundle.secrets.push(PortableSecret {
            id: secret_id,
            kind: PortableSecretKind::Password,
            selected_by_user: true,
            payload: SecretBytes::new(b"desktop secret".to_vec()).expect("secret"),
        });
        bundle.objects.hosts.push(PortableHost {
            id: gateway_host_id,
            label: "gateway".to_owned(),
            address: "gateway.example".to_owned(),
            port: 22,
            username: None,
            favorite: false,
            tags: Default::default(),
            identity_id: None,
            route_id: gateway_route_id,
            authentication_plan_id: gateway_auth_id,
            algorithm_policy_id: gateway_algorithm_id,
            heartbeat_policy_id: gateway_heartbeat_id,
            monitoring_policy_id: gateway_monitoring_id,
            login_automation_id: None,
        });
        bundle.objects.routes.push(PortableRoute {
            id: gateway_route_id,
            ingress: RouteIngress::Direct,
            jump_hops: Vec::new(),
        });
        bundle
            .objects
            .authentication_plans
            .push(PortableAuthenticationPlan {
                id: gateway_auth_id,
                credential_ids: Vec::new(),
            });
        bundle
            .objects
            .algorithm_policies
            .push(PortableAlgorithmPolicy {
                id: gateway_algorithm_id,
                policy_id: "secure-default".to_owned(),
                compatibility_exceptions: Vec::new(),
            });
        bundle
            .objects
            .heartbeat_policies
            .push(PortableHeartbeatPolicy {
                id: gateway_heartbeat_id,
                mode: HeartbeatMode::Disabled,
            });
        bundle
            .objects
            .monitoring_policies
            .push(PortableMonitoringPolicy {
                id: gateway_monitoring_id,
                enabled: false,
                interval_seconds: 30,
                timeout_seconds: 5,
                interval_millis: None,
                timeout_millis: None,
                resources: Default::default(),
            });
        bundle
            .objects
            .desktop_profiles
            .push(PortableDesktopProfile {
                id: id(40),
                label: label.to_owned(),
                protocol: PortableDesktopProtocol::Rdp,
                address: "desktop.example".to_owned(),
                port: 3389,
                username: "desktop-user".to_owned(),
                domain: String::new(),
                host_id: Some(id(10)),
                gateway_host_id: Some(gateway_host_id),
                credential_id: Some(credential_id),
                width: 1_920,
                height: 1_080,
                clipboard_enabled: true,
                audio_playback_enabled: true,
                vnc_protocol_version: crate::PortableVncProtocolVersion::Auto,
                vnc_resolution_mode: crate::PortableVncResolutionMode::Server,
                rdp_transport_mode: crate::PortableRdpTransportMode::Auto,
                rdp_graphics_mode: crate::PortableRdpGraphicsMode::Auto,
                rdp_resolution_mode: crate::PortableRdpResolutionMode::Fixed,
            });
        bundle
    }

    #[test]
    fn three_way_merge_fast_forwards_one_side_and_preserves_deletes() {
        let base = bundle(Some(b"base"), false);
        let local = bundle(Some(b"local"), false);
        let remote = bundle(Some(b"base"), false);
        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &local, &remote, 2).expect("merge")
        else {
            panic!("one-sided edit must merge");
        };
        assert_eq!(merged.secrets, local.secrets);

        let BundleMergeOutcome::Merged(deleted) =
            merge_bundles_three_way(&base, &bundle(None, true), &remote, 2).expect("delete")
        else {
            panic!("one-sided delete must merge");
        };
        assert!(deleted.secrets.is_empty());
        assert_eq!(deleted.tombstones, bundle(None, true).tombstones);
    }

    #[test]
    fn remote_historical_tombstone_and_time_survive_merge() {
        let base = v5(bundle(None, false));
        let local = base.clone();
        let mut remote = base.clone();
        let tombstone = PortableTombstone {
            kind: PortableObjectKind::Host,
            id: id(99),
        };
        remote.tombstones.push(tombstone);
        timed(&mut remote, tombstone.kind, tombstone.id, 200);

        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &local, &remote, 2).expect("historical deletion")
        else {
            panic!("historical deletion must merge");
        };
        assert_eq!(merged.tombstones, remote.tombstones);
        assert_eq!(merged.update_times, remote.update_times);
    }

    #[test]
    fn unchanged_historical_tombstone_survives_the_next_merge() {
        let mut base = v5(bundle(None, false));
        let tombstone = PortableTombstone {
            kind: PortableObjectKind::Host,
            id: id(99),
        };
        base.tombstones.push(tombstone);
        timed(&mut base, tombstone.kind, tombstone.id, 200);

        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &base, &base, 2).expect("unchanged history")
        else {
            panic!("unchanged history must merge");
        };
        assert_eq!(merged.tombstones, base.tombstones);
        assert_eq!(merged.update_times, base.update_times);
    }

    #[test]
    fn new_live_value_conflicts_with_remote_historical_tombstone() {
        let base = v5(bundle(None, false));
        let local = v5(bundle(Some(b"new"), false));
        let mut remote = base.clone();
        for (kind, value_id) in [
            (PortableObjectKind::Credential, id(2)),
            (PortableObjectKind::Secret, id(1)),
        ] {
            remote
                .tombstones
                .push(PortableTombstone { kind, id: value_id });
            timed(&mut remote, kind, value_id, 200);
        }

        assert!(matches!(
            merge_bundles_three_way(&base, &local, &remote, 2).expect("conflict result"),
            BundleMergeOutcome::Conflicts { count } if count > 0
        ));
    }

    #[test]
    fn inherited_tombstone_is_not_a_new_local_delete_against_remote_revive() {
        let mut base = v5(bundle(None, true));
        timed(&mut base, PortableObjectKind::Credential, id(2), 100);
        timed(&mut base, PortableObjectKind::Secret, id(1), 100);
        let local = base.clone();
        let remote = v5(bundle(Some(b"restored"), false));

        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &local, &remote, 2).expect("remote revive")
        else {
            panic!("remote revive must merge without a false conflict");
        };
        assert_eq!(merged.objects.credentials, remote.objects.credentials);
        assert_eq!(merged.secrets, remote.secrets);
        assert!(merged.tombstones.is_empty());
    }

    #[test]
    fn three_way_merge_reports_edit_edit_and_delete_edit_conflicts() {
        let base = bundle(Some(b"base"), false);
        assert_eq!(
            merge_bundles_three_way(
                &base,
                &bundle(Some(b"local"), false),
                &bundle(Some(b"remote"), false),
                2,
            )
            .expect("conflict result"),
            BundleMergeOutcome::Conflicts { count: 1 }
        );
        assert_eq!(
            merge_bundles_three_way(
                &base,
                &bundle(None, true),
                &bundle(Some(b"remote"), false),
                2,
            )
            .expect("delete-edit conflict"),
            BundleMergeOutcome::Conflicts { count: 1 }
        );
    }

    #[test]
    fn explicit_resolution_selects_one_side_for_every_real_conflict() {
        let base = bundle(Some(b"base"), false);
        let local = bundle(Some(b"local"), false);
        let remote = bundle(Some(b"remote"), false);
        let BundleMergeOutcome::Merged(kept_local) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::KeepLocal),
        )
        .expect("keep local") else {
            panic!("explicit local resolution must merge");
        };
        assert_eq!(kept_local.secrets, local.secrets);

        let BundleMergeOutcome::Merged(used_remote) = merge_bundles_three_way_with_resolution(
            &base,
            &bundle(None, true),
            &remote,
            2,
            Some(BundleConflictResolution::UseRemote),
        )
        .expect("use remote") else {
            panic!("explicit remote resolution must merge");
        };
        assert_eq!(used_remote.secrets, remote.secrets);
        assert!(used_remote.tombstones.is_empty());
    }

    #[test]
    fn scope_omission_removes_remote_record_without_creating_a_delete_tombstone() {
        let base = bundle(Some(b"base"), false);
        let local = bundle(None, false);
        let remote = base.clone();
        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &local, &remote, 2).expect("scope omission")
        else {
            panic!("scope omission must merge");
        };
        assert!(merged.secrets.is_empty());
        assert!(merged.tombstones.is_empty());
    }

    #[test]
    fn resolving_child_delete_edit_conflict_selects_the_entire_host_closure() {
        let base = host_bundle(false, false);
        let local = host_bundle(true, false);
        let remote = host_bundle(false, true);
        assert_eq!(
            merge_bundles_three_way(&base, &local, &remote, 2).expect("conflict"),
            BundleMergeOutcome::Conflicts { count: 1 }
        );

        let BundleMergeOutcome::Merged(cloud) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::UseRemote),
        )
        .expect("use remote closure") else {
            panic!("resolution must merge");
        };
        assert_eq!(cloud.objects, remote.objects);
        assert!(cloud.tombstones.is_empty());

        let BundleMergeOutcome::Merged(local_result) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::KeepLocal),
        )
        .expect("keep local closure") else {
            panic!("resolution must merge");
        };
        assert!(local_result.objects.hosts.is_empty());
        assert_eq!(local_result.tombstones.len(), 6);
    }

    #[test]
    fn shared_secret_closure_preserves_all_credentials_from_the_selected_side() {
        let base = shared_secret_bundle(b"base", true);
        let local = shared_secret_bundle(b"local", false);
        let remote = shared_secret_bundle(b"remote", true);

        let BundleMergeOutcome::Merged(cloud) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::UseRemote),
        )
        .expect("use remote shared closure") else {
            panic!("resolution must merge");
        };
        assert_eq!(cloud.objects.credentials.len(), 2);
        assert_eq!(cloud.secrets, remote.secrets);

        let BundleMergeOutcome::Merged(local_result) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::KeepLocal),
        )
        .expect("keep local shared closure") else {
            panic!("resolution must merge");
        };
        assert_eq!(local_result.objects.credentials.len(), 1);
        assert_eq!(local_result.secrets, local.secrets);
    }

    #[test]
    fn merged_result_rejects_orphan_child_configuration_and_secret() {
        let base = host_bundle(false, false);
        let mut local = base.clone();
        local.objects.hosts.clear();
        local.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Host,
            id: id(10),
        });
        assert!(merge_bundles_three_way(&base, &local, &base, 2).is_err());

        let base = shared_secret_bundle(b"base", true);
        let mut local = base.clone();
        local.objects.identities[0].credential_ids.clear();
        local.objects.credentials.clear();
        for credential_id in [id(21), id(22)] {
            local.tombstones.push(PortableTombstone {
                kind: PortableObjectKind::Credential,
                id: credential_id,
            });
        }
        assert!(merge_bundles_three_way(&base, &local, &base, 2).is_err());
    }

    #[test]
    fn proxy_route_credential_conflict_selects_the_exact_account() {
        let base = proxy_bundle(None);
        let local = proxy_bundle(Some(id(21)));
        let remote = proxy_bundle(Some(id(22)));
        assert_eq!(
            merge_bundles_three_way(&base, &local, &remote, 2).expect("conflict"),
            BundleMergeOutcome::Conflicts { count: 1 }
        );

        let BundleMergeOutcome::Merged(cloud) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::UseRemote),
        )
        .expect("use remote account") else {
            panic!("resolution must merge");
        };
        assert!(matches!(
            cloud.objects.routes[0].ingress,
            RouteIngress::HttpConnect {
                credential_id: Some(value),
                ..
            } if value == id(22)
        ));
        assert_eq!(cloud.objects.credentials.len(), 2);
        assert_eq!(cloud.secrets.len(), 1);

        let BundleMergeOutcome::Merged(local_result) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::KeepLocal),
        )
        .expect("keep local account") else {
            panic!("resolution must merge");
        };
        assert!(matches!(
            local_result.objects.routes[0].ingress,
            RouteIngress::HttpConnect {
                credential_id: Some(value),
                ..
            } if value == id(21)
        ));
    }

    #[test]
    fn desktop_conflicts_select_the_complete_host_gateway_and_credential_closure() {
        let base = desktop_bundle("base desktop");
        let mut local = base.clone();
        local.objects.desktop_profiles[0].label = "local desktop".to_owned();
        local.objects.hosts[1].label = "local gateway".to_owned();

        let mut remote = base.clone();
        remote.objects.desktop_profiles[0].label = "remote desktop".to_owned();
        remote.objects.hosts[0].label = "remote target".to_owned();
        remote.objects.credentials[0].label = "remote desktop password".to_owned();

        assert_eq!(
            merge_bundles_three_way(&base, &local, &remote, 2).expect("desktop conflict"),
            BundleMergeOutcome::Conflicts { count: 1 }
        );

        let BundleMergeOutcome::Merged(keep_local) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::KeepLocal),
        )
        .expect("local desktop closure") else {
            panic!("resolution must merge");
        };
        assert_eq!(keep_local.schema, BundleSchema::V3);
        assert_eq!(
            keep_local.objects.desktop_profiles[0].label,
            "local desktop"
        );
        assert_eq!(keep_local.objects.hosts[0].label, "host");
        assert_eq!(keep_local.objects.hosts[1].label, "local gateway");
        assert_eq!(keep_local.objects.credentials[0].label, "desktop password");

        let BundleMergeOutcome::Merged(use_remote) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::UseRemote),
        )
        .expect("remote desktop closure") else {
            panic!("resolution must merge");
        };
        assert_eq!(use_remote.schema, BundleSchema::V3);
        assert_eq!(
            use_remote.objects.desktop_profiles[0].label,
            "remote desktop"
        );
        assert_eq!(use_remote.objects.hosts[0].label, "remote target");
        assert_eq!(use_remote.objects.hosts[1].label, "gateway");
        assert_eq!(
            use_remote.objects.credentials[0].label,
            "remote desktop password"
        );
    }

    #[test]
    fn desktop_deletion_merges_as_a_v3_tombstone_and_conflicts_with_edits() {
        let base = desktop_bundle("base desktop");
        let mut deleted = base.clone();
        deleted.objects.desktop_profiles.clear();
        deleted.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::DesktopProfile,
            id: id(40),
        });

        let BundleMergeOutcome::Merged(merged) =
            merge_bundles_three_way(&base, &deleted, &base, 2).expect("desktop delete")
        else {
            panic!("deletion must merge");
        };
        assert_eq!(merged.schema, BundleSchema::V3);
        assert!(merged.objects.desktop_profiles.is_empty());
        assert_eq!(merged.tombstones, deleted.tombstones);

        let mut edited = base.clone();
        edited.objects.desktop_profiles[0].address = "other-desktop.example".to_owned();
        assert_eq!(
            merge_bundles_three_way(&base, &deleted, &edited, 2).expect("delete edit conflict"),
            BundleMergeOutcome::Conflicts { count: 1 }
        );

        let BundleMergeOutcome::Merged(use_remote) = merge_bundles_three_way_with_resolution(
            &base,
            &deleted,
            &edited,
            2,
            Some(BundleConflictResolution::UseRemote),
        )
        .expect("use remote desktop") else {
            panic!("resolution must merge");
        };
        assert_eq!(
            use_remote.objects.desktop_profiles[0].address,
            "other-desktop.example"
        );
        assert!(use_remote.tombstones.is_empty());
    }

    #[test]
    fn newest_resolves_independent_objects_separately_and_retains_their_times() {
        let mut base = v5(desktop_bundle("first"));
        let first = &mut base.objects.desktop_profiles[0];
        first.host_id = None;
        first.gateway_host_id = None;
        first.credential_id = None;
        let mut second = first.clone();
        second.id = id(41);
        second.label = "second".to_owned();
        base.objects.desktop_profiles.push(second);
        let mut local = base.clone();
        local.objects.desktop_profiles[0].label = "local first".to_owned();
        local.objects.desktop_profiles[1].label = "local second".to_owned();
        timed(&mut local, PortableObjectKind::DesktopProfile, id(40), 200);
        timed(&mut local, PortableObjectKind::DesktopProfile, id(41), 100);
        let mut remote = base.clone();
        remote.objects.desktop_profiles[0].label = "remote first".to_owned();
        remote.objects.desktop_profiles[1].label = "remote second".to_owned();
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(40), 100);
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(41), 200);
        let BundleMergeOutcome::Merged(merged) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::Newest),
        )
        .unwrap() else {
            panic!("independent nodes resolve");
        };
        assert_eq!(merged.objects.desktop_profiles[0].label, "local first");
        assert_eq!(merged.objects.desktop_profiles[1].label, "remote second");
        assert_eq!(merged.update_times.len(), 2);
    }

    #[test]
    fn newest_keeps_one_sided_addition_in_conflict_dependency_closure() {
        let base = v5(desktop_bundle("base"));
        let mut local = base.clone();
        local.objects.desktop_profiles[0].label = "local edit".to_owned();
        timed(&mut local, PortableObjectKind::DesktopProfile, id(40), 200);
        let mut remote = base.clone();
        remote.objects.desktop_profiles[0].label = "remote edit".to_owned();
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(40), 100);
        let mut added = remote.objects.desktop_profiles[0].clone();
        added.id = id(41);
        added.label = "remote new".to_owned();
        remote.objects.desktop_profiles.push(added);
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(41), 150);

        let BundleMergeOutcome::Merged(merged) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::Newest),
        )
        .unwrap() else {
            panic!("independent addition must survive");
        };
        assert_eq!(merged.objects.desktop_profiles.len(), 2);
        assert_eq!(merged.objects.desktop_profiles[0].label, "local edit");
        assert_eq!(merged.objects.desktop_profiles[1].label, "remote new");
    }

    #[test]
    fn newest_tombstone_wins_over_older_edit() {
        let base = v5(desktop_bundle("base"));
        let mut local = base.clone();
        local.objects.desktop_profiles.clear();
        local.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::DesktopProfile,
            id: id(40),
        });
        timed(&mut local, PortableObjectKind::DesktopProfile, id(40), 200);
        let mut remote = base.clone();
        remote.objects.desktop_profiles[0].label = "older edit".to_owned();
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(40), 100);
        let BundleMergeOutcome::Merged(merged) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::Newest),
        )
        .unwrap() else {
            panic!("newer deletion resolves");
        };
        assert!(merged.objects.desktop_profiles.is_empty());
        assert_eq!(merged.tombstones, local.tombstones);
        assert_eq!(merged.update_times, local.update_times);
    }

    #[test]
    fn deletion_policy_is_independent_from_edit_conflict_policy() {
        let base = v5(desktop_bundle("base"));
        let mut local = base.clone();
        local.objects.desktop_profiles.clear();
        local.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::DesktopProfile,
            id: id(40),
        });
        timed(&mut local, PortableObjectKind::DesktopProfile, id(40), 200);
        let mut remote = base.clone();
        remote.objects.desktop_profiles[0].label = "older edit".to_owned();
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(40), 100);

        let BundleMergeOutcome::Merged(auto_deleted) = merge_bundles_three_way_with_policies(
            &base,
            &local,
            &remote,
            2,
            None,
            Some(BundleConflictResolution::Newest),
        )
        .unwrap() else {
            panic!("deletion policy should resolve the newer tombstone");
        };
        assert!(auto_deleted.objects.desktop_profiles.is_empty());
        assert_eq!(
            merge_bundles_three_way_with_policies(
                &base,
                &local,
                &remote,
                2,
                Some(BundleConflictResolution::Newest),
                None,
            )
            .unwrap(),
            BundleMergeOutcome::Conflicts { count: 1 }
        );
    }

    #[test]
    fn newest_requires_distinct_plausible_times_for_each_double_edit() {
        let base = v5(bundle(Some(b"base"), false));
        let mut local = v5(bundle(Some(b"local"), false));
        let mut remote = v5(bundle(Some(b"remote"), false));
        let merge = |local: &PortableBundleV1, remote: &PortableBundleV1| {
            merge_bundles_three_way_with_resolution(
                &base,
                local,
                remote,
                2,
                Some(BundleConflictResolution::Newest),
            )
            .unwrap()
        };
        assert_eq!(
            merge(&local, &remote),
            BundleMergeOutcome::Conflicts { count: 1 }
        );
        timed(&mut local, PortableObjectKind::Secret, id(1), 100);
        timed(&mut remote, PortableObjectKind::Secret, id(1), 100);
        assert_eq!(
            merge(&local, &remote),
            BundleMergeOutcome::Conflicts { count: 1 }
        );
        remote.update_times[0].update_time_unix_ms = i64::MAX;
        assert_eq!(
            merge(&local, &remote),
            BundleMergeOutcome::Conflicts { count: 1 }
        );
    }

    #[test]
    fn newest_opposite_choices_within_shared_dependency_stay_manual() {
        let base = v5(desktop_bundle("base"));
        let mut local = base.clone();
        local.objects.desktop_profiles[0].label = "local desktop".to_owned();
        local.objects.hosts[0].label = "local host".to_owned();
        timed(&mut local, PortableObjectKind::DesktopProfile, id(40), 200);
        timed(&mut local, PortableObjectKind::Host, id(10), 100);
        let mut remote = base.clone();
        remote.objects.desktop_profiles[0].label = "remote desktop".to_owned();
        remote.objects.hosts[0].label = "remote host".to_owned();
        timed(&mut remote, PortableObjectKind::DesktopProfile, id(40), 100);
        timed(&mut remote, PortableObjectKind::Host, id(10), 200);
        assert_eq!(
            merge_bundles_three_way_with_resolution(
                &base,
                &local,
                &remote,
                2,
                Some(BundleConflictResolution::Newest),
            )
            .unwrap(),
            BundleMergeOutcome::Conflicts { count: 2 }
        );
    }

    #[test]
    fn newest_selects_preference_groups_independently() {
        let base = v5(bundle(Some(b"base"), false));
        let mut local = base.clone();
        let mut remote = base.clone();
        local
            .preference_update_times
            .insert("application".to_owned(), 200);
        remote
            .preference_update_times
            .insert("application".to_owned(), 100);
        local
            .preference_update_times
            .insert("files".to_owned(), 100);
        remote
            .preference_update_times
            .insert("files".to_owned(), 200);
        local
            .preferences
            .as_mut()
            .unwrap()
            .groups
            .get_mut("application")
            .unwrap()["locale"] = serde_json::json!("local");
        remote
            .preferences
            .as_mut()
            .unwrap()
            .groups
            .get_mut("application")
            .unwrap()["locale"] = serde_json::json!("remote");
        local
            .preferences
            .as_mut()
            .unwrap()
            .groups
            .get_mut("files")
            .unwrap()["browser"] = serde_json::json!("local");
        remote
            .preferences
            .as_mut()
            .unwrap()
            .groups
            .get_mut("files")
            .unwrap()["browser"] = serde_json::json!("remote");
        let BundleMergeOutcome::Merged(merged) = merge_bundles_three_way_with_resolution(
            &base,
            &local,
            &remote,
            2,
            Some(BundleConflictResolution::Newest),
        )
        .unwrap() else {
            panic!("preference groups resolve");
        };
        let groups = &merged.preferences.as_ref().unwrap().groups;
        assert_eq!(groups["application"]["locale"], "local");
        assert_eq!(groups["files"]["browser"], "remote");
        assert_eq!(merged.preference_update_times["application"], 200);
        assert_eq!(merged.preference_update_times["files"], 200);
    }
}
