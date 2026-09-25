//! Core-owned checkpoint and acknowledgement for portable, non-secret preferences.
//! The frontend alone knows how to validate and CAS-apply each registered group.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use norishell_ssh_profile_sync::PortablePreferencesV1;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter as _, State};
use tokio::sync::oneshot;
use uuid::Uuid;

const COLLECT_EVENT: &str = "norishell:ssh-sync-preferences:collect";
const PENDING_EVENT: &str = "norishell:ssh-sync-preferences:pending";
const COLLECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PendingPreferenceApply {
    pub id: String,
    #[serde(default = "ready_by_default")]
    pub ready: bool,
    pub expected: PortablePreferencesV1,
    pub desired: PortablePreferencesV1,
    #[serde(default)]
    pub desired_update_times: BTreeMap<String, i64>,
    pub results: BTreeMap<String, PreferenceApplyResult>,
}

fn ready_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreferenceApplyResult {
    Applied,
    Unchanged,
    Conflict,
    Failed,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferences {
    current: Option<PortablePreferencesV1>,
    #[serde(default)]
    group_update_times: BTreeMap<String, i64>,
    pending: Option<PendingPreferenceApply>,
}

#[derive(Default)]
struct PreferenceState {
    stored: StoredPreferences,
    collectors: BTreeMap<String, oneshot::Sender<PortablePreferencesV1>>,
    poisoned: bool,
    apply_waiter: Option<(String, oneshot::Sender<bool>)>,
}

#[derive(Clone)]
pub(crate) struct SshSyncPreferencesService {
    path: PathBuf,
    state: Arc<Mutex<PreferenceState>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectRequest<'a> {
    request_id: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublishRequest {
    request_id: Option<String>,
    transfer: PortablePreferencesV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApplyAckRequest {
    id: String,
    results: BTreeMap<String, PreferenceApplyResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResolvePendingRequest {
    id: String,
    resolution: PendingResolution,
    current: Option<PortablePreferencesV1>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum PendingResolution {
    Retry,
    UseRemote,
    KeepLocal,
}

impl SshSyncPreferencesService {
    pub(crate) fn new(app_data_directory: &Path) -> Result<Self, String> {
        let directory = app_data_directory.join("ssh-sync-preferences");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        let path = directory.join("state.json");
        let stored: StoredPreferences = if path.exists() {
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("preferences checkpoint too large".into());
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| "invalid preferences checkpoint".to_owned())?
        } else {
            StoredPreferences::default()
        };
        if let Some(value) = &stored.current {
            value
                .validate()
                .map_err(|_| "invalid preferences checkpoint")?;
        }
        if stored.group_update_times.iter().any(|(group, time)| {
            *time <= 0
                || !stored
                    .current
                    .as_ref()
                    .is_some_and(|value| value.groups.contains_key(group))
        }) {
            return Err("invalid preferences update times".into());
        }
        if let Some(value) = &stored.pending {
            value
                .expected
                .validate()
                .map_err(|_| "invalid pending preferences")?;
            value
                .desired
                .validate()
                .map_err(|_| "invalid pending preferences")?;
            if value
                .results
                .keys()
                .any(|key| !value.desired.groups.contains_key(key))
            {
                return Err("invalid pending results".into());
            }
            if value
                .desired_update_times
                .iter()
                .any(|(group, time)| *time <= 0 || !value.desired.groups.contains_key(group))
            {
                return Err("invalid pending preferences update times".into());
            }
        }
        Ok(Self {
            path,
            state: Arc::new(Mutex::new(PreferenceState {
                stored,
                collectors: BTreeMap::new(),
                poisoned: false,
                apply_waiter: None,
            })),
        })
    }

    fn persist(&self, stored: &StoredPreferences) -> Result<(), (String, bool)> {
        let directory = self
            .path
            .parent()
            .ok_or(("invalid preferences path".to_owned(), false))?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .map_err(|error| (error.to_string(), false))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| (error.to_string(), false))?;
        }
        serde_json::to_writer(&mut temporary, stored)
            .map_err(|error| (error.to_string(), false))?;
        temporary
            .flush()
            .map_err(|error| (error.to_string(), false))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| (error.to_string(), false))?;
        temporary
            .persist(&self.path)
            .map_err(|error| (error.to_string(), false))?;
        #[cfg(unix)]
        fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| (error.to_string(), true))?;
        Ok(())
    }

    pub(crate) fn has_pending_preferences(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.poisoned || state.stored.pending.is_some()
    }

    pub(crate) fn current_update_times(&self) -> BTreeMap<String, i64> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stored
            .group_update_times
            .clone()
    }

    pub(crate) async fn request_current_snapshot(
        &self,
        app: &AppHandle,
    ) -> Result<PortablePreferencesV1, String> {
        let request_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.poisoned {
                return Err("preferences checkpoint requires reload".into());
            }
            state.collectors.insert(request_id.clone(), sender);
        }
        if app
            .emit(
                COLLECT_EVENT,
                CollectRequest {
                    request_id: &request_id,
                },
            )
            .is_err()
        {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .collectors
                .remove(&request_id);
            return Err("preferences frontend unavailable".into());
        }
        let outcome = tokio::time::timeout(COLLECT_TIMEOUT, receiver).await;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .collectors
            .remove(&request_id);
        outcome
            .map_err(|_| "preferences collection timed out".to_owned())?
            .map_err(|_| "preferences collection cancelled".to_owned())
    }

    fn publish(&self, request: PublishRequest) -> Result<(), String> {
        request
            .transfer
            .validate()
            .map_err(|_| "invalid preferences transfer")?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.poisoned {
            return Err("preferences checkpoint requires reload".into());
        }
        if let Some(id) = &request.request_id
            && !state.collectors.contains_key(id)
        {
            return Err("stale preferences collection".into());
        }
        let mut group_update_times = state.stored.group_update_times.clone();
        if let Some(previous) = &state.stored.current {
            for (group, value) in &request.transfer.groups {
                if previous.groups.get(group).is_some_and(|old| old != value) {
                    // Collection observes the current value, possibly long after it changed.
                    // A collection timestamp would falsely make an old edit win a merge.
                    group_update_times.remove(group);
                }
            }
        }
        group_update_times.retain(|group, _| request.transfer.groups.contains_key(group));
        let mut stored = StoredPreferences {
            current: Some(request.transfer.clone()),
            group_update_times,
            pending: state.stored.pending.clone(),
        };
        if let Err((error, uncertain)) = self.persist(&stored) {
            state.poisoned |= uncertain;
            return Err(error);
        }
        std::mem::swap(&mut state.stored, &mut stored);
        if let Some(id) = request.request_id
            && let Some(sender) = state.collectors.remove(&id)
        {
            let _ = sender.send(request.transfer);
        }
        Ok(())
    }

    /// Persist the preference intent before the SSH restore can commit. A crash
    /// leaves an inert review item, never an automatically applied preference.
    pub(crate) fn prepare_remote(
        &self,
        expected: PortablePreferencesV1,
        desired: PortablePreferencesV1,
        desired_update_times: BTreeMap<String, i64>,
    ) -> Result<Option<String>, String> {
        expected
            .validate()
            .map_err(|_| "invalid local preferences")?;
        desired
            .validate()
            .map_err(|_| "invalid remote preferences")?;
        if desired_update_times
            .iter()
            .any(|(group, time)| *time <= 0 || !desired.groups.contains_key(group))
        {
            return Err("invalid remote preferences update times".into());
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.poisoned {
            return Err("preferences checkpoint requires reload".into());
        }
        if state.stored.pending.is_some() {
            return Err("pending preferences need review".into());
        }
        if expected == desired
            && desired_update_times
                .iter()
                .all(|(group, time)| state.stored.group_update_times.get(group) == Some(time))
        {
            return Ok(None);
        }
        let pending = PendingPreferenceApply {
            id: Uuid::new_v4().to_string(),
            ready: false,
            expected,
            desired,
            desired_update_times,
            results: BTreeMap::new(),
        };
        let id = pending.id.clone();
        let replacement = StoredPreferences {
            current: state.stored.current.clone(),
            group_update_times: state.stored.group_update_times.clone(),
            pending: Some(pending),
        };
        if let Err((error, uncertain)) = self.persist(&replacement) {
            state.poisoned |= uncertain;
            return Err(error);
        }
        state.stored = replacement;
        Ok(Some(id))
    }

    /// Only a confirmed SSH restore may activate automatic group CAS writes.
    pub(crate) async fn activate_prepared(
        &self,
        app: &AppHandle,
        id: &str,
    ) -> Result<(), &'static str> {
        let receiver = self
            .start_prepared(id)
            .map_err(|_| "local.preferences.activate_failed")?;
        let Some(receiver) = receiver else {
            return Ok(());
        };
        // Register before emitting so even an immediate frontend acknowledgement is observed.
        let _ = app.emit(PENDING_EVENT, ());
        self.wait_for_apply(id, receiver).await
    }

    async fn wait_for_apply(
        &self,
        id: &str,
        receiver: oneshot::Receiver<bool>,
    ) -> Result<(), &'static str> {
        let result = tokio::time::timeout(COLLECT_TIMEOUT, receiver).await;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .apply_waiter
            .as_ref()
            .is_some_and(|(pending_id, _)| pending_id == id)
        {
            state.apply_waiter = None;
        }
        match result {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err("local.preferences.apply_needs_review"),
            Ok(Err(_)) => Err("local.preferences.apply_cancelled"),
            Err(_) => Err("local.preferences.apply_timeout"),
        }
    }

    fn start_prepared(&self, id: &str) -> Result<Option<oneshot::Receiver<bool>>, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.poisoned {
            return Err("preferences checkpoint requires reload".into());
        }
        let mut pending = state
            .stored
            .pending
            .clone()
            .ok_or("no prepared preferences")?;
        if pending.id != id || pending.ready {
            return Err("stale prepared preferences".into());
        }
        if pending.expected == pending.desired {
            if state.stored.current.as_ref() != Some(&pending.expected) {
                return Err("preferences changed before clock confirmation".into());
            }
            let mut group_update_times = state.stored.group_update_times.clone();
            group_update_times.extend(pending.desired_update_times);
            let replacement = StoredPreferences {
                current: state.stored.current.clone(),
                group_update_times,
                pending: None,
            };
            if let Err((error, uncertain)) = self.persist(&replacement) {
                state.poisoned |= uncertain;
                return Err(error);
            }
            state.stored = replacement;
            return Ok(None);
        }
        pending.ready = true;
        let replacement = StoredPreferences {
            current: state.stored.current.clone(),
            group_update_times: state.stored.group_update_times.clone(),
            pending: Some(pending),
        };
        if let Err((error, uncertain)) = self.persist(&replacement) {
            state.poisoned |= uncertain;
            return Err(error);
        }
        state.stored = replacement;
        let (sender, receiver) = oneshot::channel();
        state.apply_waiter = Some((id.to_owned(), sender));
        Ok(Some(receiver))
    }

    pub(crate) fn notify_pending(&self, app: &AppHandle) {
        let _ = app.emit(PENDING_EVENT, ());
    }

    fn pending(&self) -> Option<PendingPreferenceApply> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stored
            .pending
            .clone()
    }

    fn acknowledge(&self, request: ApplyAckRequest) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.poisoned {
            return Err("preferences checkpoint requires reload".into());
        }
        let mut pending = state
            .stored
            .pending
            .clone()
            .ok_or("no pending preferences")?;
        if pending.id != request.id
            || !pending.ready
            || request
                .results
                .keys()
                .any(|key| !pending.desired.groups.contains_key(key))
        {
            return Err("stale preferences acknowledgement".into());
        }
        for (key, result) in request.results {
            if pending.results.contains_key(&key) {
                return Err("duplicate preferences acknowledgement".into());
            }
            pending.results.insert(key, result);
        }
        let finished = pending.desired.groups.keys().all(|key| {
            matches!(
                pending.results.get(key),
                Some(PreferenceApplyResult::Applied | PreferenceApplyResult::Unchanged)
            )
        });
        let all_attempted = pending
            .desired
            .groups
            .keys()
            .all(|key| pending.results.contains_key(key));
        let mut group_update_times = state.stored.group_update_times.clone();
        if finished {
            for (group, desired) in &pending.desired.groups {
                if let Some(time) = pending.desired_update_times.get(group) {
                    group_update_times.insert(group.clone(), *time);
                } else if pending.expected.groups.get(group) != Some(desired) {
                    group_update_times.remove(group);
                }
            }
        }
        let replacement = StoredPreferences {
            current: if finished {
                Some(pending.desired.clone())
            } else {
                state.stored.current.clone()
            },
            group_update_times,
            pending: (!finished).then_some(pending),
        };
        if let Err((error, uncertain)) = self.persist(&replacement) {
            state.poisoned |= uncertain;
            return Err(error);
        }
        state.stored = replacement;
        if all_attempted
            && state
                .apply_waiter
                .as_ref()
                .is_some_and(|(id, _)| id == &request.id)
            && let Some((_, sender)) = state.apply_waiter.take()
        {
            let _ = sender.send(finished);
        }
        Ok(())
    }

    fn resolve_pending(&self, request: ResolvePendingRequest) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.poisoned {
            return Err("preferences checkpoint requires reload".into());
        }
        let mut pending = state
            .stored
            .pending
            .clone()
            .ok_or("no pending preferences")?;
        if pending.id != request.id {
            return Err("stale preferences review".into());
        }
        // A prepared intent does not prove that SSH and desktop restore committed.
        // Only clearing it is safe; the user must start a new protected sync.
        if !pending.ready && !matches!(request.resolution, PendingResolution::KeepLocal) {
            return Err("prepared preferences require a new sync".into());
        }
        let next = match request.resolution {
            PendingResolution::KeepLocal => {
                if request.current.is_some() {
                    return Err("unexpected preferences snapshot".into());
                }
                None
            }
            PendingResolution::Retry => {
                if request.current.is_some() {
                    return Err("unexpected preferences snapshot".into());
                }
                pending.results.clear();
                pending.id = Uuid::new_v4().to_string();
                pending.ready = true;
                Some(pending)
            }
            PendingResolution::UseRemote => {
                let current = request.current.ok_or("missing current preferences")?;
                current
                    .validate()
                    .map_err(|_| "invalid current preferences")?;
                pending.expected = current;
                pending.results.clear();
                pending.id = Uuid::new_v4().to_string();
                pending.ready = true;
                Some(pending)
            }
        };
        let replacement = StoredPreferences {
            current: state.stored.current.clone(),
            group_update_times: state.stored.group_update_times.clone(),
            pending: next,
        };
        if let Err((error, uncertain)) = self.persist(&replacement) {
            state.poisoned |= uncertain;
            return Err(error);
        }
        state.stored = replacement;
        if let Some((_, sender)) = state.apply_waiter.take() {
            let _ = sender.send(false);
        }
        Ok(())
    }
}

#[tauri::command]
pub(crate) fn ssh_sync_preferences_publish(
    request: PublishRequest,
    service: State<'_, SshSyncPreferencesService>,
) -> Result<(), String> {
    service.publish(request)
}

#[tauri::command]
pub(crate) fn ssh_sync_preferences_pending_get(
    service: State<'_, SshSyncPreferencesService>,
) -> Option<PendingPreferenceApply> {
    service.pending()
}

#[tauri::command]
pub(crate) fn ssh_sync_preferences_apply_ack(
    request: ApplyAckRequest,
    service: State<'_, SshSyncPreferencesService>,
) -> Result<(), String> {
    service.acknowledge(request)
}

#[tauri::command]
pub(crate) fn ssh_sync_preferences_retry_pending(
    request: ResolvePendingRequest,
    service: State<'_, SshSyncPreferencesService>,
) -> Result<(), String> {
    service.resolve_pending(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn sample_preferences() -> PortablePreferencesV1 {
        let shapes: [(&str, &[&str]); 8] = [
            (
                "application",
                &[
                    "themePreference",
                    "locale",
                    "uiZoom",
                    "terminalStartupBehavior",
                    "newTerminalBehavior",
                    "singlePaneTabCloseBehavior",
                ],
            ),
            (
                "appearance",
                &[
                    "terminalThemeMode",
                    "terminalFontFamily",
                    "terminalFontSize",
                    "terminalFontWeight",
                    "terminalBoldFontWeight",
                    "terminalLineHeight",
                    "terminalLetterSpacing",
                    "terminalCursorStyle",
                    "terminalCursorBlink",
                    "customTerminalPalette",
                    "customTerminalPaletteName",
                ],
            ),
            ("interaction", &["interaction", "pasteWarning"]),
            ("highlights", &["enabled", "rules"]),
            ("shortcuts", &["version", "bindings"]),
            ("files", &["browser", "rememberLastDirectory"]),
            (
                "desktop",
                &[
                    "windowCloseBehavior",
                    "trayShowStatus",
                    "trayRecentLimit",
                    "trayShowHostNames",
                    "notificationBackgroundOnly",
                    "notificationFailureOnly",
                    "notifyTransferCompleted",
                    "notifyTransferFailed",
                    "notifyDisconnected",
                ],
            ),
            (
                "commandNotifications",
                &["notificationsEnabled", "notificationThresholdSeconds"],
            ),
        ];
        let groups = shapes
            .into_iter()
            .map(|(name, keys)| {
                let fields = keys
                    .iter()
                    .map(|key| ((*key).to_owned(), Value::Null))
                    .collect();
                (name.to_owned(), Value::Object(fields))
            })
            .collect();
        let transfer = PortablePreferencesV1 {
            product: "NoriShell".into(),
            version: 1,
            groups,
        };
        transfer.validate().unwrap();
        transfer
    }

    #[tokio::test]
    async fn prepared_apply_waits_for_all_groups_before_restore_continues() {
        for fail_last in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let service = SshSyncPreferencesService::new(directory.path()).unwrap();
            let expected = sample_preferences();
            let mut desired = expected.clone();
            desired
                .groups
                .get_mut("appearance")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("terminalFontSize".into(), json!(20));
            let id = service
                .prepare_remote(expected, desired.clone(), BTreeMap::new())
                .unwrap()
                .unwrap();
            let mut receiver = service.start_prepared(&id).unwrap().unwrap();
            assert!(service.has_pending_preferences());
            let keys = desired.groups.keys().cloned().collect::<Vec<_>>();
            for (index, key) in keys.iter().enumerate() {
                assert!(matches!(
                    receiver.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                service
                    .acknowledge(ApplyAckRequest {
                        id: id.clone(),
                        results: BTreeMap::from([(
                            key.clone(),
                            if fail_last && index + 1 == keys.len() {
                                PreferenceApplyResult::Conflict
                            } else {
                                PreferenceApplyResult::Applied
                            },
                        )]),
                    })
                    .unwrap();
            }
            let result = service.wait_for_apply(&id, receiver).await;
            if fail_last {
                assert_eq!(result, Err("local.preferences.apply_needs_review"));
                assert!(service.has_pending_preferences());
            } else {
                assert_eq!(result, Ok(()));
                assert!(!service.has_pending_preferences());
                assert_eq!(service.state.lock().unwrap().stored.current, Some(desired));
            }
            assert!(service.state.lock().unwrap().apply_waiter.is_none());
        }
    }

    #[test]
    fn collection_does_not_invent_a_change_time() {
        let directory = tempfile::tempdir().unwrap();
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        let original = sample_preferences();
        service
            .publish(PublishRequest {
                request_id: None,
                transfer: original.clone(),
            })
            .unwrap();
        assert!(service.current_update_times().is_empty());
        let mut changed = original;
        changed
            .groups
            .get_mut("appearance")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("terminalFontSize".into(), json!(19));
        service
            .publish(PublishRequest {
                request_id: None,
                transfer: changed.clone(),
            })
            .unwrap();
        let recorded = service.current_update_times();
        assert!(recorded.is_empty());
        service
            .publish(PublishRequest {
                request_id: None,
                transfer: changed,
            })
            .unwrap();
        assert_eq!(service.current_update_times(), recorded);
        drop(service);
        let reopened = SshSyncPreferencesService::new(directory.path()).unwrap();
        assert_eq!(reopened.current_update_times(), recorded);
    }

    #[test]
    fn acknowledged_remote_preference_keeps_authenticated_time() {
        let directory = tempfile::tempdir().unwrap();
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        let expected = sample_preferences();
        service
            .publish(PublishRequest {
                request_id: None,
                transfer: expected.clone(),
            })
            .unwrap();
        let mut desired = expected.clone();
        desired
            .groups
            .get_mut("appearance")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("terminalFontSize".into(), json!(20));
        let remote_time = 1_700_000_000_000;
        let pending = PendingPreferenceApply {
            id: "remote-clock".into(),
            ready: true,
            expected,
            desired: desired.clone(),
            desired_update_times: BTreeMap::from([("appearance".into(), remote_time)]),
            results: BTreeMap::new(),
        };
        let stored = StoredPreferences {
            current: service.state.lock().unwrap().stored.current.clone(),
            group_update_times: BTreeMap::new(),
            pending: Some(pending),
        };
        service.persist(&stored).unwrap();
        service.state.lock().unwrap().stored = stored;
        let results = desired
            .groups
            .keys()
            .map(|key| (key.clone(), PreferenceApplyResult::Applied))
            .collect();
        service
            .acknowledge(ApplyAckRequest {
                id: "remote-clock".into(),
                results,
            })
            .unwrap();
        service
            .publish(PublishRequest {
                request_id: None,
                transfer: desired,
            })
            .unwrap();
        assert_eq!(service.current_update_times()["appearance"], remote_time);
    }

    #[test]
    fn failed_group_survives_restart_and_explicit_retry() {
        let directory = tempfile::tempdir().unwrap();
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        let expected = sample_preferences();
        let mut desired = expected.clone();
        desired
            .groups
            .get_mut("appearance")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("terminalFontSize".into(), json!(17));
        let pending = PendingPreferenceApply {
            id: "review-1".into(),
            ready: true,
            expected,
            desired,
            desired_update_times: BTreeMap::new(),
            results: BTreeMap::new(),
        };
        let stored = StoredPreferences {
            current: None,
            group_update_times: BTreeMap::new(),
            pending: Some(pending),
        };
        service.persist(&stored).unwrap();
        drop(service);
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        assert!(service.has_pending_preferences());
        service
            .acknowledge(ApplyAckRequest {
                id: "review-1".into(),
                results: BTreeMap::from([
                    ("appearance".into(), PreferenceApplyResult::Failed),
                    ("desktop".into(), PreferenceApplyResult::Applied),
                ]),
            })
            .unwrap();
        drop(service);
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        assert_eq!(
            service.pending().unwrap().results.get("appearance"),
            Some(&PreferenceApplyResult::Failed)
        );
        assert_eq!(
            service.pending().unwrap().results.get("desktop"),
            Some(&PreferenceApplyResult::Applied)
        );
        service
            .resolve_pending(ResolvePendingRequest {
                id: "review-1".into(),
                resolution: PendingResolution::Retry,
                current: None,
            })
            .unwrap();
        assert!(service.pending().unwrap().results.is_empty());
        let retry = service.pending().unwrap();
        assert_ne!(retry.id, "review-1");
        assert!(
            service
                .acknowledge(ApplyAckRequest {
                    id: "review-1".into(),
                    results: BTreeMap::from([(
                        "appearance".into(),
                        PreferenceApplyResult::Applied
                    )])
                })
                .is_err()
        );
        service
            .acknowledge(ApplyAckRequest {
                id: retry.id,
                results: retry
                    .desired
                    .groups
                    .keys()
                    .map(|key| (key.clone(), PreferenceApplyResult::Applied))
                    .collect(),
            })
            .unwrap();
        assert!(!service.has_pending_preferences());
    }

    #[test]
    fn keeping_local_requires_explicit_review_and_clears_pending() {
        let directory = tempfile::tempdir().unwrap();
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        let transfer = sample_preferences();
        let stored = StoredPreferences {
            current: None,
            group_update_times: BTreeMap::new(),
            pending: Some(PendingPreferenceApply {
                id: "review-2".into(),
                ready: true,
                expected: transfer.clone(),
                desired: transfer,
                desired_update_times: BTreeMap::new(),
                results: BTreeMap::new(),
            }),
        };
        service.persist(&stored).unwrap();
        drop(service);
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        service
            .resolve_pending(ResolvePendingRequest {
                id: "review-2".into(),
                resolution: PendingResolution::KeepLocal,
                current: None,
            })
            .unwrap();
        assert!(
            !SshSyncPreferencesService::new(directory.path())
                .unwrap()
                .has_pending_preferences()
        );
    }

    #[test]
    fn prepared_intent_after_restart_requires_explicit_review() {
        let directory = tempfile::tempdir().unwrap();
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        let expected = sample_preferences();
        let mut desired = expected.clone();
        desired
            .groups
            .get_mut("appearance")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("terminalFontSize".into(), json!(18));
        let id = service
            .prepare_remote(expected, desired, BTreeMap::new())
            .unwrap()
            .unwrap();
        drop(service);
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        assert!(service.has_pending_preferences());
        assert!(!service.pending().unwrap().ready);
        assert!(
            service
                .acknowledge(ApplyAckRequest {
                    id: id.clone(),
                    results: BTreeMap::from([(
                        "appearance".into(),
                        PreferenceApplyResult::Applied
                    )])
                })
                .is_err()
        );
        assert!(
            service
                .resolve_pending(ResolvePendingRequest {
                    id: id.clone(),
                    resolution: PendingResolution::Retry,
                    current: None,
                })
                .is_err()
        );
        assert!(
            service
                .resolve_pending(ResolvePendingRequest {
                    id: id.clone(),
                    resolution: PendingResolution::UseRemote,
                    current: Some(sample_preferences()),
                })
                .is_err()
        );
        assert!(!service.pending().unwrap().ready);
        service
            .resolve_pending(ResolvePendingRequest {
                id,
                resolution: PendingResolution::KeepLocal,
                current: None,
            })
            .unwrap();
        assert!(!service.has_pending_preferences());
    }

    #[test]
    fn use_remote_rechecks_previously_applied_groups() {
        let directory = tempfile::tempdir().unwrap();
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        let initial = sample_preferences();
        let stored = StoredPreferences {
            current: None,
            group_update_times: BTreeMap::new(),
            pending: Some(PendingPreferenceApply {
                id: "review-3".into(),
                ready: true,
                expected: initial.clone(),
                desired: initial.clone(),
                desired_update_times: BTreeMap::new(),
                results: BTreeMap::from([("desktop".into(), PreferenceApplyResult::Applied)]),
            }),
        };
        service.persist(&stored).unwrap();
        drop(service);
        let service = SshSyncPreferencesService::new(directory.path()).unwrap();
        service
            .resolve_pending(ResolvePendingRequest {
                id: "review-3".into(),
                resolution: PendingResolution::UseRemote,
                current: Some(initial),
            })
            .unwrap();
        let pending = service.pending().unwrap();
        assert_ne!(pending.id, "review-3");
        assert!(pending.results.is_empty());
    }
}
