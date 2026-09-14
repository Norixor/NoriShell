//! Core-owned lifecycle for package-declared plugin workflows.
//!
//! A task has no persisted payload or continuation interpreter. The resident
//! Wasm instance receives one event, emits at most one typed API call for a
//! fixed catalog step, and receives that reply as a later event. A restart
//! therefore reconciles unfinished rows to `Interrupted`; it never replays an
//! effect or resumes a task from SQLite.

use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use norishell_app_persistence::{
    AppPersistenceError, PluginWorkflowTaskCreate, PluginWorkflowTaskRecord,
    PluginWorkflowTaskStepRecord, PluginWorkflowTaskTransition,
};
use norishell_core_api::{
    ExitBlocker, MAX_PLUGIN_WORKFLOW_FILE_SCOPE_HANDLES, MAX_PLUGIN_WORKFLOW_INPUT_BYTES,
    MAX_PLUGIN_WORKFLOW_RESULT_BYTES, PluginApiCall, PluginApiErrorCode, PluginApiOperation,
    PluginApiOutcome, PluginApiReply, PluginId, PluginWorkflowApiMethod, PluginWorkflowDefinition,
    PluginWorkflowStepState, PluginWorkflowTaskId, PluginWorkflowTaskResult,
    PluginWorkflowTaskSnapshot, PluginWorkflowTaskStartRequest, PluginWorkflowTaskState,
    PluginWorkflowTaskStepSummary, PluginWorkflowTaskSummary, WireSequence, WorkflowEvent,
    WorkflowResponse, valid_plugin_workflow_id,
};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use uuid::Uuid;

use crate::{
    host_service::HostService,
    plugin_api::{ResourceConsumer, ResourceFence, ResourceOwner, ResourceRegistry, validate_call},
};

const MAX_TASK_STEPS: usize = 64;
const TASK_MAX_WALL_CLOCK: Duration = Duration::from_secs(10 * 60);
const MAX_START_DEDUPES: usize = 128;
const MAX_RESOURCE_EVENTS_PER_PUMP: u16 = 32;

pub(crate) type TaskRuntimeFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, PluginApiErrorCode>> + Send + 'static>>;

/// Trusted, non-serializable runtime context. Root builds it from an active
/// plugin instance and never from a guest JSON field. The API adapter should
/// turn it into `ApiInvocation::Task` before brokering a call.
#[derive(Clone)]
pub(crate) struct TaskRuntimeBinding {
    task_id: PluginWorkflowTaskId,
    owner: ResourceOwner,
    consumer: ResourceConsumer,
    workflow_id: String,
    step_id: Option<String>,
    active_fence: ResourceFence,
    transaction_authority: ResourceFence,
}

impl TaskRuntimeBinding {
    #[must_use]
    pub(crate) fn task_id(&self) -> &PluginWorkflowTaskId {
        &self.task_id
    }

    #[must_use]
    pub(crate) fn owner(&self) -> &ResourceOwner {
        &self.owner
    }

    #[must_use]
    pub(crate) fn consumer(&self) -> &ResourceConsumer {
        &self.consumer
    }

    #[must_use]
    pub(crate) fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    #[must_use]
    pub(crate) fn step_id(&self) -> Option<&str> {
        self.step_id.as_deref()
    }

    #[must_use]
    pub(crate) fn active(&self) -> bool {
        (self.active_fence)()
    }

    #[must_use]
    pub(crate) fn transaction_current(&self) -> bool {
        (self.transaction_authority)()
    }
}

/// The only runtime capability required by the task lifecycle. The adapter
/// owns Wasm invocation, active-installation validation, the independent
/// approval path and the central second allowlist for `ApiInvocation::Task`.
pub(crate) trait TaskRuntime: Send + Sync {
    /// Copies explicit parent file scopes into the fresh task consumer. The
    /// returned old-to-new map stays in memory and is passed only in Start.
    fn prepare_start(
        &self,
        binding: TaskRuntimeBinding,
        source_root_handles: Vec<String>,
    ) -> TaskRuntimeFuture<BTreeMap<String, String>>;

    fn event(
        &self,
        binding: TaskRuntimeBinding,
        event: WorkflowEvent,
    ) -> TaskRuntimeFuture<WorkflowResponse>;

    /// `allow_interaction` is false for Start, every automatic next step and
    /// resource events. It is true only for exactly one pending call selected
    /// by an explicit TaskResume action.
    fn invoke(
        &self,
        binding: TaskRuntimeBinding,
        call: PluginApiCall,
        allow_interaction: bool,
    ) -> TaskRuntimeFuture<PluginApiReply>;
}

/// Root creates this only after resolving the main-window or guest request to
/// an actual enabled plugin instance and immutable package workflow catalog.
#[derive(Clone)]
pub(crate) struct TaskStartContext {
    pub(crate) request: PluginWorkflowTaskStartRequest,
    pub(crate) workflow: PluginWorkflowDefinition,
    pub(crate) owner: ResourceOwner,
    pub(crate) task_authority: ResourceFence,
    /// Must be a non-reentrant atomic fence suitable for checking while the
    /// one repository mutex is held. It is never used to invoke a capability.
    pub(crate) transaction_authority: ResourceFence,
    pub(crate) explicit_user_action: bool,
}

/// A guest task operation carries no plugin identity. Root builds this from
/// `ApiInvocation`; all task access below compares the complete owner.
#[derive(Clone)]
pub(crate) struct TaskCaller {
    owner: ResourceOwner,
}

impl TaskCaller {
    #[must_use]
    pub(crate) fn new(owner: ResourceOwner) -> Self {
        Self { owner }
    }
}

#[derive(Clone)]
pub(crate) struct TaskService {
    hosts: HostService,
    resources: ResourceRegistry,
    runtime: Arc<dyn TaskRuntime>,
    controls: Arc<Mutex<TaskRegistry>>,
}

#[derive(Default)]
struct TaskRegistry {
    controls: BTreeMap<String, Arc<TaskControl>>,
    dedupes: BTreeMap<String, StartDedupe>,
}

#[derive(Clone)]
struct StartDedupe {
    fingerprint: [u8; 32],
    task_id: Option<PluginWorkflowTaskId>,
}

struct TaskControl {
    task_id: PluginWorkflowTaskId,
    workflow: PluginWorkflowDefinition,
    owner: ResourceOwner,
    consumer: ResourceConsumer,
    task_authority: ResourceFence,
    transaction_authority: ResourceFence,
    active: Arc<AtomicBool>,
    stopped: Arc<Notify>,
    serial: AsyncMutex<()>,
    record: Mutex<PluginWorkflowTaskRecord>,
    pending: Mutex<Option<PendingCall>>,
    result: Mutex<PluginWorkflowTaskResult>,
    result_bytes: AtomicUsize,
    dispatched_steps: AtomicUsize,
    started: Instant,
    terminal_at: Mutex<Option<Instant>>,
}

#[derive(Clone)]
struct PendingCall {
    step_id: String,
    step_revision: WireSequence,
    call: PluginApiCall,
}

enum DispatchResult {
    Advance(Box<WorkflowEvent>),
    WaitingForUser,
    Terminal,
}

impl TaskService {
    #[must_use]
    pub(crate) fn with_runtime(
        hosts: HostService,
        resources: ResourceRegistry,
        runtime: Arc<dyn TaskRuntime>,
    ) -> Self {
        Self {
            hosts,
            resources,
            runtime,
            controls: Arc::new(Mutex::new(TaskRegistry::default())),
        }
    }

    /// Reconciles durable rows only. In particular this does not instantiate
    /// Wasm or call `TaskRuntime`, so it cannot replay a dispatch.
    pub(crate) fn recover_startup(
        &self,
    ) -> Result<Vec<PluginWorkflowTaskSnapshot>, PluginApiErrorCode> {
        let recovered = self
            .hosts
            .with_plugin_repository(|repository| repository.recover_plugin_workflow_tasks())
            .map_err(map_persistence_error)?;
        recovered
            .into_iter()
            .map(|task| self.snapshot_from_record(task, None))
            .collect()
    }

    pub(crate) async fn start(
        &self,
        context: TaskStartContext,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        validate_start_context(&context)?;
        if !(context.task_authority)() || !(context.transaction_authority)() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let start_request_id = context.request.meta.request_id.as_str().to_owned();
        let start_fingerprint = start_fingerprint(&context);
        enum StartReservation {
            Fresh,
            Existing(PluginWorkflowTaskId),
        }
        let reservation = {
            let mut registry = self
                .controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match registry.dedupes.get(&start_request_id) {
                Some(existing) if existing.fingerprint != start_fingerprint => {
                    return Err(PluginApiErrorCode::Conflict);
                }
                // The first call has reserved the request identity but has
                // not committed a durable task yet. Do not admit a second
                // task while that reservation is in flight.
                Some(StartDedupe { task_id: None, .. }) => return Err(PluginApiErrorCode::Busy),
                Some(StartDedupe {
                    task_id: Some(task_id),
                    ..
                }) => StartReservation::Existing(task_id.clone()),
                None => {
                    Self::prune_terminal_retention(&mut registry);
                    if registry.dedupes.len() >= MAX_START_DEDUPES {
                        return Err(PluginApiErrorCode::Busy);
                    }
                    registry.dedupes.insert(
                        start_request_id.clone(),
                        StartDedupe {
                            fingerprint: start_fingerprint,
                            task_id: None,
                        },
                    );
                    StartReservation::Fresh
                }
            }
        };
        if let StartReservation::Existing(task_id) = reservation {
            return self.get(&task_id);
        }
        let task_id = PluginWorkflowTaskId::new();
        let create = PluginWorkflowTaskCreate {
            task_id: task_id.clone(),
            plugin_id: context.request.plugin_id.clone(),
            signer_fingerprint_sha256: context.request.signer_fingerprint_sha256.clone(),
            package_sha256: context.request.expected_package_sha256.clone(),
            expected_installed_state_version: context.request.expected_state_version,
            workflow_id: context.request.workflow_id.clone(),
        };
        let transaction_authority = context.transaction_authority.clone();
        let record = match self
            .hosts
            .with_plugin_repository(|repository| {
                if !transaction_authority() {
                    return Err(AppPersistenceError::Conflict);
                }
                repository.create_plugin_workflow_task(&create)
            })
            .map_err(map_persistence_error)
        {
            Ok(record) => record,
            Err(error) => {
                self.remove_start_dedupe(&start_request_id, start_fingerprint);
                return Err(error);
            }
        };
        let consumer = ResourceConsumer {
            connection: Uuid::new_v4(),
            generation: context.owner.generation,
            stream: Uuid::new_v4(),
        };
        let control = Arc::new(TaskControl {
            task_id: task_id.clone(),
            workflow: context.workflow,
            owner: context.owner,
            consumer,
            task_authority: context.task_authority,
            transaction_authority: context.transaction_authority,
            active: Arc::new(AtomicBool::new(true)),
            stopped: Arc::new(Notify::new()),
            serial: AsyncMutex::new(()),
            record: Mutex::new(record.clone()),
            pending: Mutex::new(None),
            result: Mutex::new(PluginWorkflowTaskResult {
                task_id,
                replies: Vec::new(),
            }),
            result_bytes: AtomicUsize::new(0),
            dispatched_steps: AtomicUsize::new(0),
            started: Instant::now(),
            terminal_at: Mutex::new(None),
        });
        {
            let mut registry = self
                .controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry
                .controls
                .insert(control.task_id.as_str().to_owned(), control.clone());
            if let Some(entry) = registry.dedupes.get_mut(&start_request_id)
                && entry.fingerprint == start_fingerprint
            {
                entry.task_id = Some(control.task_id.clone());
            }
        }

        let source_handles = context.request.file_scope_handles.clone();
        // Scope adoption can allocate owner-scoped resources. Keep it in the
        // task serial lane so a concurrent cancellation either waits for the
        // adoption and cleans it, or revokes the fence before the adapter can
        // use the adopted handles. We never hold the repository lock here.
        let scopes = {
            let _serial = control.serial.lock().await;
            let prepared = self
                .runtime
                .prepare_start(control.binding(None), source_handles.clone())
                .await
                .and_then(|scopes| validate_adopted_scopes(&source_handles, scopes));
            match prepared {
                Ok(scopes) if control.is_active() => scopes,
                Ok(_) => {
                    // `cancel` and lifecycle shutdown deliberately revoke
                    // before they await their serial turn. If an adapter
                    // adopted scopes in that narrow window, this second
                    // owner-scoped cleanup prevents a late-adoption leak.
                    let _ = self.cleanup_task_owners(&control).await;
                    if control.task_is_active() {
                        let _ = self.fail_locked(&control, false).await;
                    }
                    return Err(PluginApiErrorCode::Revoked);
                }
                Err(error) => {
                    if control.task_is_active() {
                        let _ = self.fail_locked(&control, false).await;
                    } else {
                        let _ = self.cleanup_task_owners(&control).await;
                    }
                    return Err(error);
                }
            }
        };

        self.spawn_background(control.clone());
        let service = self.clone();
        let input_json = context.request.input_json;
        tokio::spawn(async move {
            service
                .drive_event(
                    control,
                    WorkflowEvent::Start {
                        input_json,
                        file_scopes: scopes,
                    },
                )
                .await;
        });
        self.snapshot_from_record(record, None)
    }

    pub(crate) fn get(
        &self,
        task_id: &PluginWorkflowTaskId,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let record = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_workflow_task(task_id))
            .map_err(map_persistence_error)?;
        self.snapshot_from_record(record, self.control(task_id))
    }

    pub(crate) fn list(
        &self,
        plugin_id: Option<&norishell_core_api::PluginId>,
    ) -> Result<Vec<PluginWorkflowTaskSnapshot>, PluginApiErrorCode> {
        let records = self
            .hosts
            .with_plugin_repository(|repository| repository.list_plugin_workflow_tasks(plugin_id))
            .map_err(map_persistence_error)?;
        records
            .into_iter()
            .map(|record| {
                let control = self.control(&record.task_id);
                self.snapshot_from_record(record, control)
            })
            .collect()
    }

    pub(crate) fn get_for_owner(
        &self,
        caller: &TaskCaller,
        task_id: &PluginWorkflowTaskId,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let snapshot = self.get(task_id)?;
        self.assert_owner(caller, &snapshot.task, self.control(task_id).as_deref())?;
        Ok(snapshot)
    }

    pub(crate) fn list_for_owner(
        &self,
        caller: &TaskCaller,
    ) -> Result<Vec<PluginWorkflowTaskSnapshot>, PluginApiErrorCode> {
        let records = self.list(Some(&caller.owner.plugin_id))?;
        Ok(records
            .into_iter()
            .filter(|snapshot| {
                self.assert_owner(
                    caller,
                    &snapshot.task,
                    self.control(&snapshot.task.task_id).as_deref(),
                )
                .is_ok()
            })
            .collect())
    }

    pub(crate) async fn cancel(
        &self,
        task_id: &PluginWorkflowTaskId,
        expected_revision: WireSequence,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let control = match self.control(task_id) {
            Some(control) => control,
            None => {
                let snapshot = self.get(task_id)?;
                if snapshot.task.revision != expected_revision {
                    return Err(PluginApiErrorCode::Conflict);
                }
                return Ok(snapshot);
            }
        };
        if control.record().revision != expected_revision {
            return Err(PluginApiErrorCode::Conflict);
        }
        // This is intentionally before awaiting the actual resource owner.
        control.deactivate();
        let cleanup = self.cleanup_task_owners(&control).await;
        let _serial = control.serial.lock().await;
        let current = control.record();
        if current.state.is_terminal()
            && current.state != PluginWorkflowTaskState::CleanupIncomplete
        {
            return self.snapshot_from_record(current, Some(control.clone()));
        }
        let cancelling = if current.state == PluginWorkflowTaskState::Cancelling {
            current
        } else {
            self.transition(
                &control,
                current.revision,
                PluginWorkflowTaskTransition {
                    state: PluginWorkflowTaskState::Cancelling,
                    outcome_unknown: false,
                    cleanup_incomplete: false,
                },
            )?
        };
        let terminal = match cleanup {
            Ok(()) => PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::Cancelled,
                outcome_unknown: false,
                cleanup_incomplete: false,
            },
            Err(_) => PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::CleanupIncomplete,
                outcome_unknown: false,
                cleanup_incomplete: true,
            },
        };
        let record = self.transition(&control, cancelling.revision, terminal)?;
        self.snapshot_from_record(record, Some(control.clone()))
    }

    pub(crate) async fn cancel_for_owner(
        &self,
        caller: &TaskCaller,
        task_id: &PluginWorkflowTaskId,
        expected_revision: WireSequence,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let snapshot = self.get_for_owner(caller, task_id)?;
        if snapshot.task.revision != expected_revision {
            return Err(PluginApiErrorCode::Conflict);
        }
        self.cancel(task_id, expected_revision).await
    }

    pub(crate) async fn resume(
        &self,
        task_id: &PluginWorkflowTaskId,
        expected_revision: WireSequence,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let control = self.control(task_id).ok_or(PluginApiErrorCode::NotFound)?;
        let _serial = control.serial.lock().await;
        let record = control.record();
        if !control.is_active()
            || record.revision != expected_revision
            || record.state != PluginWorkflowTaskState::NeedsUserAction
        {
            return Err(PluginApiErrorCode::Conflict);
        }
        let pending = control
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or(PluginApiErrorCode::Conflict)?;
        let dispatch = match self.dispatch_call_locked(&control, pending, true).await {
            Ok(dispatch) => dispatch,
            Err(error) => {
                self.fail_locked(&control, control.dispatch_outcome_unknown())
                    .await?;
                return Err(error);
            }
        };
        match dispatch {
            DispatchResult::Advance(event) => {
                self.drive_event_or_fail_locked(&control, *event).await?;
            }
            DispatchResult::WaitingForUser | DispatchResult::Terminal => {}
        }
        self.snapshot_from_record(control.record(), Some(control.clone()))
    }

    pub(crate) async fn resume_for_owner(
        &self,
        caller: &TaskCaller,
        task_id: &PluginWorkflowTaskId,
        expected_revision: WireSequence,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let snapshot = self.get_for_owner(caller, task_id)?;
        if snapshot.task.revision != expected_revision {
            return Err(PluginApiErrorCode::Conflict);
        }
        self.resume(task_id, expected_revision).await
    }

    /// Stops every live task for an installation generation. This is used for
    /// disable, package replacement and Plugin Host shutdown; it has no guest
    /// supplied task identity and never invokes Wasm.
    pub(crate) async fn stop_plugin(
        &self,
        plugin_id: &PluginId,
        generation: Option<WireSequence>,
    ) -> Result<(), PluginApiErrorCode> {
        let controls = self
            .controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .controls
            .values()
            .filter(|control| {
                control.owner.plugin_id == *plugin_id
                    && generation.is_none_or(|generation| control.owner.generation == generation)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut failed = false;
        for control in controls {
            failed |= self.stop_control(control).await.is_err();
        }
        if failed {
            Err(PluginApiErrorCode::CleanupIncomplete)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn shutdown_all(&self) -> Result<(), PluginApiErrorCode> {
        let controls = self
            .controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .controls
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut failed = false;
        for control in controls {
            failed |= self.stop_control(control).await.is_err();
        }
        if failed {
            Err(PluginApiErrorCode::CleanupIncomplete)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub(crate) fn active_plugin_ids(&self) -> Vec<PluginId> {
        self.controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .controls
            .values()
            .filter(|control| {
                let state = control.record().state;
                !state.is_terminal() || state == PluginWorkflowTaskState::CleanupIncomplete
            })
            .map(|control| control.owner.plugin_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[must_use]
    pub(crate) fn exit_blockers(&self) -> Vec<ExitBlocker> {
        self.controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .controls
            .values()
            .filter_map(|control| {
                let task = control.record();
                (!task.state.is_terminal() || task.cleanup_incomplete).then(|| {
                    ExitBlocker::PluginResource {
                        plugin_id: task.plugin_id,
                        resource_kind: "plugin-workflow-task".to_owned(),
                        resource_id: task.task_id.as_str().to_owned(),
                    }
                })
            })
            .collect()
    }

    fn spawn_background(&self, control: Arc<TaskControl>) {
        let event_service = self.clone();
        let event_control = control.clone();
        tokio::spawn(async move { event_service.pump_resource_events(event_control).await });
        let timeout_service = self.clone();
        tokio::spawn(async move {
            // Register before inspecting `active`: a terminal transition can
            // happen between task creation and this watchdog's first poll.
            // Without the enabled waiter, `notify_waiters` could be missed
            // and retain terminal controls/results for the full timeout.
            let stopped = control.stopped.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if !control.task_is_active() {
                return;
            }
            tokio::select! {
                _ = &mut stopped => return,
                _ = tokio::time::sleep(TASK_MAX_WALL_CLOCK) => {}
            }
            if control.task_is_active() {
                // Revoke the execution fence before waiting for a Wasm/API
                // turn that is already holding `serial`. Every capability
                // creation/write fence sees this atomically and fails closed.
                control.deactivate();
                let outcome_unknown =
                    control.record().state == PluginWorkflowTaskState::Dispatching;
                let _ = timeout_service
                    .fail_control(control.clone(), outcome_unknown)
                    .await;
            }
        });
    }

    async fn pump_resource_events(&self, control: Arc<TaskControl>) {
        let registry = self.resources.for_consumer(control.consumer.clone());
        loop {
            // See the matching watchdog registration above. This pump also
            // owns an `Arc<TaskControl>` while it waits for the first event.
            let stopped = control.stopped.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if !control.is_active() {
                return;
            }
            tokio::select! {
                _ = registry.wait_events(&control.owner) => {},
                _ = &mut stopped => return,
            }
            if !control.is_active() {
                return;
            }
            let _serial = control.serial.lock().await;
            if !control.is_active() {
                return;
            }
            if control.waiting_for_user() {
                drop(_serial);
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            }
            for resource in registry.list(&control.owner) {
                if !control.is_active() || control.waiting_for_user() {
                    break;
                }
                let Ok((events, backpressured)) = registry.take_events(
                    &control.owner,
                    &resource.handle,
                    MAX_RESOURCE_EVENTS_PER_PUMP,
                ) else {
                    continue;
                };
                if events.is_empty() && !backpressured {
                    continue;
                }
                let event = WorkflowEvent::ResourceEvents {
                    handle: resource.handle,
                    events,
                    backpressured,
                };
                if self
                    .drive_event_or_fail_locked(&control, event)
                    .await
                    .is_err()
                    || !control.is_active()
                {
                    return;
                }
            }
        }
    }

    async fn drive_event(&self, control: Arc<TaskControl>, event: WorkflowEvent) {
        let _serial = control.serial.lock().await;
        let _ = self.drive_event_or_fail_locked(&control, event).await;
    }

    async fn drive_event_or_fail_locked(
        &self,
        control: &Arc<TaskControl>,
        event: WorkflowEvent,
    ) -> Result<(), PluginApiErrorCode> {
        match self.drive_event_locked(control, event).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // A persisted Dispatching row is the crash boundary for an
                // externally observable call. If any later validation or
                // persistence operation fails, retain that uncertainty rather
                // than leaving the task visibly Running.
                let outcome_unknown = control.dispatch_outcome_unknown();
                self.fail_locked(control, outcome_unknown).await?;
                Err(error)
            }
        }
    }

    async fn drive_event_locked(
        &self,
        control: &Arc<TaskControl>,
        event: WorkflowEvent,
    ) -> Result<(), PluginApiErrorCode> {
        if !control.is_active() {
            return Ok(());
        }
        if control.deadline_expired() {
            return self
                .fail_locked(control, control.dispatch_outcome_unknown())
                .await;
        }
        let mut next_event = Some(event);
        while let Some(event) = next_event.take() {
            if !control.is_active() {
                return Ok(());
            }
            if control.deadline_expired() {
                return self
                    .fail_locked(control, control.dispatch_outcome_unknown())
                    .await;
            }
            let response = self.runtime.event(control.binding(None), event).await?;
            // A resident guest turn can itself straddle the wall-clock
            // deadline. Do not let a late `Complete` response win the race
            // against the watchdog after it has revoked this task's fence.
            if !control.is_active() || control.deadline_expired() {
                self.fail_locked(control, control.dispatch_outcome_unknown())
                    .await?;
                return Ok(());
            }
            let call = match validate_workflow_response(
                &control.workflow,
                response,
                self.task_has_owned_resource(control),
            )? {
                WorkflowAction::Call(step_id, call) => PendingCall {
                    step_id,
                    step_revision: WireSequence::new(0),
                    call: *call,
                },
                WorkflowAction::Complete => {
                    return self.complete_locked(control).await;
                }
                WorkflowAction::Wait => return Ok(()),
            };
            match self.dispatch_call_locked(control, call, false).await? {
                DispatchResult::Advance(event) => next_event = Some(*event),
                DispatchResult::WaitingForUser | DispatchResult::Terminal => return Ok(()),
            }
        }
        Ok(())
    }

    fn task_has_owned_resource(&self, control: &TaskControl) -> bool {
        !self
            .resources
            .for_consumer(control.consumer.clone())
            .list(&control.owner)
            .is_empty()
    }

    async fn dispatch_call_locked(
        &self,
        control: &Arc<TaskControl>,
        mut pending: PendingCall,
        allow_interaction: bool,
    ) -> Result<DispatchResult, PluginApiErrorCode> {
        if !control.is_active() {
            return Ok(DispatchResult::Terminal);
        }
        if control.deadline_expired() {
            self.fail_locked(control, control.dispatch_outcome_unknown())
                .await?;
            return Ok(DispatchResult::Terminal);
        }
        let is_resume = pending.step_revision.get() != 0;
        if !is_resume && control.dispatched_steps.fetch_add(1, Ordering::AcqRel) >= MAX_TASK_STEPS {
            self.fail_locked(control, false).await?;
            return Ok(DispatchResult::Terminal);
        }
        let method = PluginWorkflowApiMethod::from_operation(&pending.call.operation)
            .filter(|_| task_call_allowed(&pending.call.operation))
            .ok_or(PluginApiErrorCode::PermissionDenied)?;
        validate_call(&pending.call)?;
        let current = control.record();
        let expected_step_revision = is_resume.then_some(pending.step_revision);
        let (task, step) = self.with_repository(control, true, |repository| {
            repository.begin_plugin_workflow_task_step(
                &control.task_id,
                current.revision,
                &pending.step_id,
                method,
                expected_step_revision,
            )
        })?;
        control.replace_record(task);
        pending.step_revision = step.revision;
        if !control.is_active() || control.deadline_expired() {
            self.settle_and_fail_locked(
                control,
                &pending,
                PluginWorkflowStepState::OutcomeUnknown,
                true,
            )
            .await?;
            return Ok(DispatchResult::Terminal);
        }
        let reply = self
            .runtime
            .invoke(
                control.binding(Some(pending.step_id.clone())),
                pending.call.clone(),
                allow_interaction,
            )
            .await;
        let reply = match reply {
            Ok(reply) if reply.call_id == pending.call.call_id => reply,
            Ok(_) | Err(_) => {
                self.settle_and_fail_locked(
                    control,
                    &pending,
                    PluginWorkflowStepState::OutcomeUnknown,
                    true,
                )
                .await?;
                return Ok(DispatchResult::Terminal);
            }
        };
        if !control.is_active() || control.deadline_expired() {
            self.settle_and_fail_locked(
                control,
                &pending,
                PluginWorkflowStepState::OutcomeUnknown,
                true,
            )
            .await?;
            return Ok(DispatchResult::Terminal);
        }
        if matches!(
            reply.outcome,
            PluginApiOutcome::Failed {
                code: PluginApiErrorCode::InteractionRequired
            }
        ) {
            let task = control.record();
            let (task, step) = self.with_repository(control, true, |repository| {
                repository.settle_plugin_workflow_task_step(
                    &control.task_id,
                    task.revision,
                    &pending.step_id,
                    pending.step_revision,
                    PluginWorkflowStepState::NeedsUserAction,
                    PluginWorkflowTaskState::NeedsUserAction,
                    false,
                )
            })?;
            control.replace_record(task);
            pending.step_revision = step.revision;
            *control
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pending);
            return Ok(DispatchResult::WaitingForUser);
        }
        if !control.append_reply(reply.clone()) {
            self.settle_and_fail_locked(control, &pending, PluginWorkflowStepState::Failed, false)
                .await?;
            return Ok(DispatchResult::Terminal);
        }
        match reply.outcome {
            PluginApiOutcome::Completed { .. } => {
                let task = control.record();
                let (task, _) = self.with_repository(control, true, |repository| {
                    repository.settle_plugin_workflow_task_step(
                        &control.task_id,
                        task.revision,
                        &pending.step_id,
                        pending.step_revision,
                        PluginWorkflowStepState::Succeeded,
                        PluginWorkflowTaskState::Running,
                        false,
                    )
                })?;
                control.replace_record(task);
                Ok(DispatchResult::Advance(Box::new(
                    WorkflowEvent::StepResult {
                        step_id: pending.step_id,
                        reply,
                    },
                )))
            }
            PluginApiOutcome::Failed { code } => {
                let outcome_unknown = code == PluginApiErrorCode::OutcomeUnknown;
                self.settle_and_fail_locked(
                    control,
                    &pending,
                    if outcome_unknown {
                        PluginWorkflowStepState::OutcomeUnknown
                    } else {
                        PluginWorkflowStepState::Failed
                    },
                    outcome_unknown,
                )
                .await?;
                Ok(DispatchResult::Terminal)
            }
        }
    }

    async fn settle_and_fail_locked(
        &self,
        control: &Arc<TaskControl>,
        pending: &PendingCall,
        step_state: PluginWorkflowStepState,
        outcome_unknown: bool,
    ) -> Result<(), PluginApiErrorCode> {
        let current = control.record();
        let (task, _) = self.with_repository(control, false, |repository| {
            repository.settle_plugin_workflow_task_step(
                &control.task_id,
                current.revision,
                &pending.step_id,
                pending.step_revision,
                step_state,
                // Persist the step fact first, then let the Core-owned
                // cleanup choose Failed or CleanupIncomplete. Going directly
                // to Failed would make a later cleanup failure impossible to
                // record under the durable transition matrix.
                PluginWorkflowTaskState::Running,
                outcome_unknown,
            )
        })?;
        control.replace_record(task);
        self.fail_locked(control, outcome_unknown).await
    }

    async fn complete_locked(&self, control: &Arc<TaskControl>) -> Result<(), PluginApiErrorCode> {
        control.deactivate();
        let cleanup = self.cleanup_task_owners(control).await;
        let current = control.record();
        if current.state.is_terminal() {
            return Ok(());
        }
        let transition = match cleanup {
            Ok(()) => PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::Completed,
                outcome_unknown: false,
                cleanup_incomplete: false,
            },
            Err(_) => PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::CleanupIncomplete,
                outcome_unknown: false,
                cleanup_incomplete: true,
            },
        };
        self.transition(control, current.revision, transition)?;
        Ok(())
    }

    async fn fail_control(
        &self,
        control: Arc<TaskControl>,
        outcome_unknown: bool,
    ) -> Result<(), PluginApiErrorCode> {
        let _serial = control.serial.lock().await;
        self.fail_locked(&control, outcome_unknown).await
    }

    async fn fail_locked(
        &self,
        control: &Arc<TaskControl>,
        outcome_unknown: bool,
    ) -> Result<(), PluginApiErrorCode> {
        control.deactivate();
        let cleanup = self.cleanup_task_owners(control).await;
        let current = control.record();
        if current.state.is_terminal() {
            return Ok(());
        }
        let transition = if cleanup.is_ok() {
            PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::Failed,
                outcome_unknown,
                cleanup_incomplete: false,
            }
        } else {
            PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::CleanupIncomplete,
                outcome_unknown,
                cleanup_incomplete: true,
            }
        };
        self.transition(control, current.revision, transition)?;
        Ok(())
    }

    async fn cleanup_task_owners(&self, control: &TaskControl) -> Result<(), PluginApiErrorCode> {
        self.resources
            .for_consumer(control.consumer.clone())
            .stop_plugin_generation(&control.owner.plugin_id, control.owner.generation)
            .await
    }

    async fn stop_control(&self, control: Arc<TaskControl>) -> Result<(), PluginApiErrorCode> {
        control.deactivate();
        let cleanup = self.cleanup_task_owners(&control).await;
        let _serial = control.serial.lock().await;
        let current = control.record();
        if current.state.is_terminal()
            && current.state != PluginWorkflowTaskState::CleanupIncomplete
        {
            return cleanup;
        }
        let cancelling = if current.state == PluginWorkflowTaskState::Cancelling {
            current
        } else if current.state == PluginWorkflowTaskState::CleanupIncomplete {
            self.transition(
                &control,
                current.revision,
                PluginWorkflowTaskTransition {
                    state: PluginWorkflowTaskState::Cancelling,
                    outcome_unknown: false,
                    cleanup_incomplete: false,
                },
            )?
        } else if current.state == PluginWorkflowTaskState::Dispatching {
            let terminal = if cleanup.is_ok() {
                PluginWorkflowTaskTransition {
                    state: PluginWorkflowTaskState::Failed,
                    outcome_unknown: true,
                    cleanup_incomplete: false,
                }
            } else {
                PluginWorkflowTaskTransition {
                    state: PluginWorkflowTaskState::CleanupIncomplete,
                    outcome_unknown: true,
                    cleanup_incomplete: true,
                }
            };
            self.transition(&control, current.revision, terminal)?;
            return cleanup;
        } else {
            self.transition(
                &control,
                current.revision,
                PluginWorkflowTaskTransition {
                    state: PluginWorkflowTaskState::Cancelling,
                    outcome_unknown: false,
                    cleanup_incomplete: false,
                },
            )?
        };
        let terminal = if cleanup.is_ok() {
            PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::Cancelled,
                outcome_unknown: false,
                cleanup_incomplete: false,
            }
        } else {
            PluginWorkflowTaskTransition {
                state: PluginWorkflowTaskState::CleanupIncomplete,
                outcome_unknown: false,
                cleanup_incomplete: true,
            }
        };
        self.transition(&control, cancelling.revision, terminal)?;
        cleanup
    }

    fn transition(
        &self,
        control: &TaskControl,
        expected_revision: WireSequence,
        transition: PluginWorkflowTaskTransition,
    ) -> Result<PluginWorkflowTaskRecord, PluginApiErrorCode> {
        let task = self.with_repository(control, false, |repository| {
            repository.transition_plugin_workflow_task(
                &control.task_id,
                expected_revision,
                transition,
            )
        })?;
        control.replace_record(task.clone());
        if control.is_retainable_terminal(&task) {
            control.mark_terminal();
        }
        Ok(task)
    }

    fn with_repository<T>(
        &self,
        control: &TaskControl,
        require_active: bool,
        operation: impl FnOnce(
            &mut norishell_app_persistence::AppRepository,
        ) -> Result<T, AppPersistenceError>,
    ) -> Result<T, PluginApiErrorCode> {
        if require_active && (!(control.transaction_authority)() || !control.is_active()) {
            return Err(PluginApiErrorCode::Revoked);
        }
        if !require_active {
            // Durable task cleanup remains Core-owned after disable, crash or
            // revocation. The repository operation still checks the exact
            // task id, state and expected revision; this only avoids asking a
            // revoked plugin execution fence to authorize its own obituary.
            return self
                .hosts
                .with_plugin_repository(operation)
                .map_err(map_persistence_error);
        }
        let authority = control.transaction_authority.clone();
        let active = control.active.clone();
        self.hosts
            .with_plugin_repository(|repository| {
                // The Root-supplied transaction authority is explicitly
                // atomic/non-reentrant, so checking it under this repository
                // lock cannot call back into the repository or broker.
                if !authority() || !active.load(Ordering::Acquire) {
                    return Err(AppPersistenceError::Conflict);
                }
                operation(repository)
            })
            .map_err(map_persistence_error)
    }

    fn snapshot_from_record(
        &self,
        record: PluginWorkflowTaskRecord,
        control: Option<Arc<TaskControl>>,
    ) -> Result<PluginWorkflowTaskSnapshot, PluginApiErrorCode> {
        let steps = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_workflow_task_steps(&record.task_id)
            })
            .map_err(map_persistence_error)?;
        Ok(PluginWorkflowTaskSnapshot {
            task: task_summary(record),
            steps: steps.into_iter().map(step_summary).collect(),
            result: control.map(|control| control.result()),
        })
    }

    fn control(&self, task_id: &PluginWorkflowTaskId) -> Option<Arc<TaskControl>> {
        self.controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .controls
            .get(task_id.as_str())
            .cloned()
    }

    fn assert_owner(
        &self,
        caller: &TaskCaller,
        task: &PluginWorkflowTaskSummary,
        control: Option<&TaskControl>,
    ) -> Result<(), PluginApiErrorCode> {
        let valid = task.plugin_id == caller.owner.plugin_id
            && control.is_none_or(|control| control.owner == caller.owner)
            && self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.get_plugin_workflow_task(&task.task_id)
                })
                .map(|record| {
                    record.signer_fingerprint_sha256 == caller.owner.signer
                        && record.package_sha256 == caller.owner.package
                })
                .unwrap_or(false);
        valid.then_some(()).ok_or(PluginApiErrorCode::NotFound)
    }

    fn remove_start_dedupe(&self, request_id: &str, fingerprint: [u8; 32]) {
        let mut registry = self
            .controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registry
            .dedupes
            .get(request_id)
            .is_some_and(|entry| entry.fingerprint == fingerprint && entry.task_id.is_none())
        {
            registry.dedupes.remove(request_id);
        }
    }

    fn prune_terminal_retention(registry: &mut TaskRegistry) {
        while registry.dedupes.len() >= MAX_START_DEDUPES {
            let Some(task_id) = registry
                .controls
                .iter()
                .filter_map(|(task_id, control)| {
                    control
                        .terminal_at()
                        .map(|finished| (finished, task_id.clone()))
                })
                .min_by_key(|(finished, _)| *finished)
                .map(|(_, task_id)| task_id)
            else {
                return;
            };
            registry.controls.remove(&task_id);
            registry.dedupes.retain(|_, entry| {
                entry
                    .task_id
                    .as_ref()
                    .is_none_or(|existing| existing.as_str() != task_id)
            });
        }
    }
}

impl TaskControl {
    fn task_is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn is_active(&self) -> bool {
        self.task_is_active() && (self.task_authority)()
    }

    fn deadline_expired(&self) -> bool {
        self.started.elapsed() >= TASK_MAX_WALL_CLOCK
    }

    fn waiting_for_user(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
            || self.record().state == PluginWorkflowTaskState::NeedsUserAction
    }

    fn dispatch_outcome_unknown(&self) -> bool {
        let record = self.record();
        record.outcome_unknown || record.state == PluginWorkflowTaskState::Dispatching
    }

    fn is_retainable_terminal(&self, record: &PluginWorkflowTaskRecord) -> bool {
        record.state.is_terminal()
            && record.state != PluginWorkflowTaskState::CleanupIncomplete
            && !record.cleanup_incomplete
    }

    fn mark_terminal(&self) {
        let mut terminal_at = self
            .terminal_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if terminal_at.is_none() {
            *terminal_at = Some(Instant::now());
        }
    }

    fn terminal_at(&self) -> Option<Instant> {
        *self
            .terminal_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn deactivate(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.stopped.notify_waiters();
        }
    }

    fn binding(&self, step_id: Option<String>) -> TaskRuntimeBinding {
        let active = self.active.clone();
        let task_authority = self.task_authority.clone();
        let transaction_active = self.active.clone();
        let transaction_authority = self.transaction_authority.clone();
        TaskRuntimeBinding {
            task_id: self.task_id.clone(),
            owner: self.owner.clone(),
            consumer: self.consumer.clone(),
            workflow_id: self.workflow.id.clone(),
            step_id,
            active_fence: Arc::new(move || active.load(Ordering::Acquire) && task_authority()),
            transaction_authority: Arc::new(move || {
                transaction_active.load(Ordering::Acquire) && transaction_authority()
            }),
        }
    }

    fn record(&self) -> PluginWorkflowTaskRecord {
        self.record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn replace_record(&self, record: PluginWorkflowTaskRecord) {
        *self
            .record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = record;
    }

    fn append_reply(&self, reply: PluginApiReply) -> bool {
        let Ok(bytes) = serde_json::to_vec(&reply) else {
            return false;
        };
        let previous = self.result_bytes.fetch_add(bytes.len(), Ordering::AcqRel);
        if previous.saturating_add(bytes.len()) > MAX_PLUGIN_WORKFLOW_RESULT_BYTES {
            self.result_bytes.fetch_sub(bytes.len(), Ordering::AcqRel);
            return false;
        }
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replies
            .push(reply);
        true
    }

    fn result(&self) -> PluginWorkflowTaskResult {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn validate_start_context(context: &TaskStartContext) -> Result<(), PluginApiErrorCode> {
    if !context.explicit_user_action
        || context.request.plugin_id != context.owner.plugin_id
        || context.request.signer_fingerprint_sha256 != context.owner.signer
        || context.request.expected_package_sha256 != context.owner.package
        || context.request.workflow_id != context.workflow.id
        || !valid_plugin_workflow_id(&context.request.workflow_id)
        || context.request.input_json.as_ref().is_some_and(|input| {
            input.len() > MAX_PLUGIN_WORKFLOW_INPUT_BYTES
                || serde_json::from_str::<serde_json::Value>(input).is_err()
        })
        || context.request.file_scope_handles.len() > MAX_PLUGIN_WORKFLOW_FILE_SCOPE_HANDLES
        || context
            .request
            .file_scope_handles
            .iter()
            .any(|handle| handle.is_empty() || handle.len() > 128)
        || context
            .request
            .file_scope_handles
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != context.request.file_scope_handles.len()
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(())
}

fn validate_adopted_scopes(
    source_handles: &[String],
    scopes: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, PluginApiErrorCode> {
    if source_handles.is_empty() {
        return scopes
            .is_empty()
            .then_some(scopes)
            .ok_or(PluginApiErrorCode::InvalidRequest);
    }
    let source = source_handles.iter().collect::<BTreeSet<_>>();
    let keys = scopes.keys().collect::<BTreeSet<_>>();
    let values = scopes.values().collect::<BTreeSet<_>>();
    if source != keys
        || values.len() != scopes.len()
        || scopes
            .values()
            .any(|handle| handle.is_empty() || handle.len() > 128)
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(scopes)
}

fn validate_workflow_response(
    workflow: &PluginWorkflowDefinition,
    response: WorkflowResponse,
    has_owned_resource: bool,
) -> Result<WorkflowAction, PluginApiErrorCode> {
    if response.complete {
        return if response.step_id.is_none() && response.call.is_none() {
            Ok(WorkflowAction::Complete)
        } else {
            Err(PluginApiErrorCode::InvalidRequest)
        };
    }
    if response.step_id.is_none() && response.call.is_none() {
        return has_owned_resource
            .then_some(WorkflowAction::Wait)
            .ok_or(PluginApiErrorCode::InvalidRequest);
    }
    let (Some(step_id), Some(call)) = (response.step_id, response.call) else {
        return Err(PluginApiErrorCode::InvalidRequest);
    };
    if !workflow.steps.iter().any(|step| step.id == step_id) {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(WorkflowAction::Call(step_id, Box::new(call)))
}

enum WorkflowAction {
    Complete,
    Wait,
    Call(String, Box<PluginApiCall>),
}

fn start_fingerprint(context: &TaskStartContext) -> [u8; 32] {
    let mut digest = Sha256::new();
    for field in [
        context.request.meta.request_id.as_str(),
        context.owner.plugin_id.as_str(),
        &context.owner.signer,
        &context.owner.package,
        context.request.workflow_id.as_str(),
    ] {
        digest.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(field.as_bytes());
    }
    digest.update(context.owner.generation.get().to_be_bytes());
    match &context.request.input_json {
        Some(input) => {
            digest.update([1]);
            digest.update(u64::try_from(input.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(input.as_bytes());
        }
        None => digest.update([0]),
    }
    for handle in &context.request.file_scope_handles {
        digest.update(
            u64::try_from(handle.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        digest.update(handle.as_bytes());
    }
    digest.finalize().into()
}

fn task_call_allowed(operation: &PluginApiOperation) -> bool {
    !matches!(
        operation,
        PluginApiOperation::PermissionRequest { .. }
            | PluginApiOperation::Credential { .. }
            | PluginApiOperation::FilePick { .. }
            | PluginApiOperation::TerminalRequestInput { .. }
            | PluginApiOperation::ProtocolOpen { .. }
    )
}

fn map_persistence_error(error: AppPersistenceError) -> PluginApiErrorCode {
    match error {
        AppPersistenceError::NotFound => PluginApiErrorCode::NotFound,
        AppPersistenceError::Conflict | AppPersistenceError::IdempotencyConflict => {
            PluginApiErrorCode::Conflict
        }
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::InvalidStoredData => {
            PluginApiErrorCode::InvalidRequest
        }
        AppPersistenceError::UnsupportedSchema(_) | AppPersistenceError::RequiresReload => {
            PluginApiErrorCode::Unavailable
        }
        AppPersistenceError::RestoreCommitUnknown => PluginApiErrorCode::OutcomeUnknown,
        AppPersistenceError::KnownHostMismatch { .. }
        | AppPersistenceError::KnownHostAlgorithmChanged { .. } => PluginApiErrorCode::Conflict,
        AppPersistenceError::Endpoint(_)
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => PluginApiErrorCode::Unavailable,
    }
}

fn task_summary(record: PluginWorkflowTaskRecord) -> PluginWorkflowTaskSummary {
    PluginWorkflowTaskSummary {
        task_id: record.task_id,
        plugin_id: record.plugin_id,
        workflow_id: record.workflow_id,
        state: record.state,
        revision: record.revision,
        current_step_id: record.current_step_id,
        outcome_unknown: record.outcome_unknown,
        cleanup_incomplete: record.cleanup_incomplete,
        created_at_unix_ms: record.created_at_unix_ms,
        updated_at_unix_ms: record.updated_at_unix_ms,
    }
}

fn step_summary(record: PluginWorkflowTaskStepRecord) -> PluginWorkflowTaskStepSummary {
    PluginWorkflowTaskStepSummary {
        step_id: record.step_id,
        method: record.method,
        state: record.state,
        revision: record.revision,
        outcome_unknown: record.outcome_unknown,
        created_at_unix_ms: record.created_at_unix_ms,
        updated_at_unix_ms: record.updated_at_unix_ms,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use norishell_app_persistence::PluginInstalledRecord;
    use norishell_core_api::{
        PluginApiResourceEventKind, PluginApiValue, PluginId, PluginInstallState,
        PluginSettingLabel,
    };
    use tokio::sync::Notify;

    use super::*;

    #[derive(Default)]
    struct TestRuntime {
        calls: AtomicUsize,
    }

    impl TaskRuntime for TestRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async move {
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), format!("task-{handle}")))
                    .collect())
            })
        }

        fn event(
            &self,
            _binding: TaskRuntimeBinding,
            event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            Box::pin(async move {
                match event {
                    WorkflowEvent::Start { .. } => Ok(WorkflowResponse {
                        step_id: Some("run".into()),
                        call: Some(PluginApiCall {
                            call_id: "call".into(),
                            operation: PluginApiOperation::Describe {},
                        }),
                        complete: false,
                    }),
                    WorkflowEvent::StepResult { .. } => Ok(WorkflowResponse {
                        step_id: None,
                        call: None,
                        complete: true,
                    }),
                    WorkflowEvent::ResourceEvents { .. } => Ok(WorkflowResponse {
                        step_id: None,
                        call: None,
                        complete: false,
                    }),
                }
            })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Box::pin(async move {
                Ok(PluginApiReply {
                    call_id: call.call_id,
                    outcome: PluginApiOutcome::Failed {
                        code: PluginApiErrorCode::InteractionRequired,
                    },
                })
            })
        }
    }

    fn service_with_runtime(
        runtime: Arc<dyn TaskRuntime>,
    ) -> (
        tempfile::TempDir,
        TaskService,
        TaskStartContext,
        ResourceRegistry,
    ) {
        service_with_resources(runtime, ResourceRegistry::default())
    }

    fn service_with_resources(
        runtime: Arc<dyn TaskRuntime>,
        resources: ResourceRegistry,
    ) -> (
        tempfile::TempDir,
        TaskService,
        TaskStartContext,
        ResourceRegistry,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let hosts = HostService::start(directory.path()).unwrap();
        let plugin_id = PluginId::parse("org.norishell.task-test").unwrap();
        hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &PluginInstalledRecord {
                        plugin_id: plugin_id.clone(),
                        name: "Task test".into(),
                        publisher: "NoriShell".into(),
                        signer_fingerprint_sha256: "a".repeat(64),
                        active_version: "1.0.0".into(),
                        package_sha256: "b".repeat(64),
                        capabilities: Vec::new(),
                        state: PluginInstallState::Enabled,
                        state_version: WireSequence::new(1),
                        installed_at_unix_ms: 1,
                        updated_at_unix_ms: 1,
                    },
                    1,
                    "",
                    "",
                    true,
                    None,
                )
            })
            .unwrap();
        let service = TaskService::with_runtime(hosts, resources.clone(), runtime);
        let owner = ResourceOwner {
            plugin_id: plugin_id.clone(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        };
        let context = TaskStartContext {
            request: PluginWorkflowTaskStartRequest {
                meta: norishell_core_api::RequestMeta {
                    request_id: norishell_core_api::RequestId::new(),
                },
                plugin_id,
                signer_fingerprint_sha256: "a".repeat(64),
                expected_package_sha256: "b".repeat(64),
                expected_state_version: WireSequence::new(1),
                instance_generation: WireSequence::new(1),
                workflow_id: "fixture".into(),
                input_json: Some(r#"{"private":"memory-only"}"#.into()),
                file_scope_handles: Vec::new(),
            },
            workflow: PluginWorkflowDefinition {
                id: "fixture".into(),
                label: PluginSettingLabel {
                    en: "Fixture".into(),
                    zh_cn: "测试".into(),
                },
                steps: vec![norishell_core_api::PluginWorkflowStepDefinition {
                    id: "run".into(),
                    label: PluginSettingLabel {
                        en: "Run".into(),
                        zh_cn: "运行".into(),
                    },
                }],
            },
            owner,
            task_authority: Arc::new(|| true),
            transaction_authority: Arc::new(|| true),
            explicit_user_action: true,
        };
        (directory, service, context, resources)
    }

    fn service() -> (tempfile::TempDir, TaskService, TaskStartContext) {
        let (directory, service, context, _) =
            service_with_runtime(Arc::new(TestRuntime::default()));
        (directory, service, context)
    }

    fn completed_reply(call_id: String) -> PluginApiReply {
        PluginApiReply {
            call_id,
            outcome: PluginApiOutcome::Completed {
                value: PluginApiValue::AppAccepted {},
            },
        }
    }

    fn describe_response(call_id: &str) -> WorkflowResponse {
        WorkflowResponse {
            step_id: Some("run".into()),
            call: Some(PluginApiCall {
                call_id: call_id.into(),
                operation: PluginApiOperation::Describe {},
            }),
            complete: false,
        }
    }

    fn wait_response() -> WorkflowResponse {
        WorkflowResponse {
            step_id: None,
            call: None,
            complete: false,
        }
    }

    fn complete_response() -> WorkflowResponse {
        WorkflowResponse {
            step_id: None,
            call: None,
            complete: true,
        }
    }

    async fn wait_for_state(
        service: &TaskService,
        task_id: &PluginWorkflowTaskId,
        expected: PluginWorkflowTaskState,
    ) -> PluginWorkflowTaskSnapshot {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = service.get(task_id).expect("task record remains readable");
                if snapshot.task.state == expected {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("task reached the expected lifecycle state")
    }

    async fn wait_for_condition(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("condition became true");
    }

    #[derive(Default)]
    struct CompletingRuntime {
        calls: AtomicUsize,
    }

    impl TaskRuntime for CompletingRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async move {
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), handle))
                    .collect())
            })
        }

        fn event(
            &self,
            _binding: TaskRuntimeBinding,
            event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            Box::pin(async move {
                Ok(match event {
                    WorkflowEvent::Start { .. } => describe_response("complete-call"),
                    WorkflowEvent::StepResult { .. } | WorkflowEvent::ResourceEvents { .. } => {
                        complete_response()
                    }
                })
            })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Box::pin(async move { Ok(completed_reply(call.call_id)) })
        }
    }

    struct NoResourceWaitRuntime;

    impl TaskRuntime for NoResourceWaitRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            _source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async { Ok(BTreeMap::new()) })
        }

        fn event(
            &self,
            _binding: TaskRuntimeBinding,
            _event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            Box::pin(async { Ok(wait_response()) })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            _call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            Box::pin(async { Err(PluginApiErrorCode::InvalidRequest) })
        }
    }

    struct RevokingCompleteRuntime {
        task_authority: Arc<AtomicBool>,
    }

    impl TaskRuntime for RevokingCompleteRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async move {
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), handle))
                    .collect())
            })
        }

        fn event(
            &self,
            _binding: TaskRuntimeBinding,
            _event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            self.task_authority.store(false, Ordering::Release);
            Box::pin(async { Ok(complete_response()) })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            _call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            Box::pin(async { Err(PluginApiErrorCode::InvalidRequest) })
        }
    }

    #[derive(Default)]
    struct DuplicateStepRuntime {
        calls: AtomicUsize,
    }

    impl TaskRuntime for DuplicateStepRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async move {
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), handle))
                    .collect())
            })
        }

        fn event(
            &self,
            _binding: TaskRuntimeBinding,
            event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            Box::pin(async move {
                Ok(match event {
                    WorkflowEvent::Start { .. } => describe_response("first"),
                    WorkflowEvent::StepResult { .. } => describe_response("duplicate"),
                    WorkflowEvent::ResourceEvents { .. } => complete_response(),
                })
            })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Box::pin(async move { Ok(completed_reply(call.call_id)) })
        }
    }

    fn spawn_emitting_resource(
        resources: &ResourceRegistry,
        binding: &TaskRuntimeBinding,
    ) -> Result<String, PluginApiErrorCode> {
        resources.for_consumer(binding.consumer().clone()).spawn(
            binding.owner().clone(),
            "workflow-test-resource",
            move |mut cancel, events| async move {
                events.emit(PluginApiResourceEventKind::TimerFired {})?;
                let _ = cancel.changed().await;
                Ok(())
            },
        )
    }

    struct ResourceEventCompletingRuntime {
        resources: ResourceRegistry,
        event_calls: AtomicUsize,
    }

    impl ResourceEventCompletingRuntime {
        fn new(resources: ResourceRegistry) -> Self {
            Self {
                resources,
                event_calls: AtomicUsize::new(0),
            }
        }
    }

    impl TaskRuntime for ResourceEventCompletingRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async move {
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), handle))
                    .collect())
            })
        }

        fn event(
            &self,
            binding: TaskRuntimeBinding,
            event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            let result = match event {
                WorkflowEvent::Start { .. } => {
                    spawn_emitting_resource(&self.resources, &binding).map(|_| wait_response())
                }
                WorkflowEvent::ResourceEvents { .. } => {
                    self.event_calls.fetch_add(1, Ordering::AcqRel);
                    Ok(complete_response())
                }
                WorkflowEvent::StepResult { .. } => Ok(complete_response()),
            };
            Box::pin(async move { result })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            _call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            Box::pin(async { Err(PluginApiErrorCode::InvalidRequest) })
        }
    }

    struct PendingResourceRuntime {
        resources: ResourceRegistry,
        resource_event_calls: AtomicUsize,
    }

    impl PendingResourceRuntime {
        fn new(resources: ResourceRegistry) -> Self {
            Self {
                resources,
                resource_event_calls: AtomicUsize::new(0),
            }
        }
    }

    impl TaskRuntime for PendingResourceRuntime {
        fn prepare_start(
            &self,
            _binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            Box::pin(async move {
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), handle))
                    .collect())
            })
        }

        fn event(
            &self,
            binding: TaskRuntimeBinding,
            event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            let result = match event {
                WorkflowEvent::Start { .. } => spawn_emitting_resource(&self.resources, &binding)
                    .map(|_| describe_response("needs-user-action")),
                WorkflowEvent::ResourceEvents { .. } => {
                    self.resource_event_calls.fetch_add(1, Ordering::AcqRel);
                    Ok(complete_response())
                }
                WorkflowEvent::StepResult { .. } => Ok(complete_response()),
            };
            Box::pin(async move { result })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            Box::pin(async move {
                Ok(PluginApiReply {
                    call_id: call.call_id,
                    outcome: PluginApiOutcome::Failed {
                        code: PluginApiErrorCode::InteractionRequired,
                    },
                })
            })
        }
    }

    struct PrepareGateRuntime {
        resources: ResourceRegistry,
        entered: AtomicBool,
        release: Arc<Notify>,
    }

    impl PrepareGateRuntime {
        fn new(resources: ResourceRegistry) -> Self {
            Self {
                resources,
                entered: AtomicBool::new(false),
                release: Arc::new(Notify::new()),
            }
        }
    }

    impl TaskRuntime for PrepareGateRuntime {
        fn prepare_start(
            &self,
            binding: TaskRuntimeBinding,
            source: Vec<String>,
        ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
            let resources = self.resources.clone();
            let entered = &self.entered;
            entered.store(true, Ordering::Release);
            let release = self.release.clone();
            Box::pin(async move {
                // The cancellation deliberately arrives before this simulated
                // adapter finishes its owner-scoped allocation. It models a
                // real allocation already in flight when the fence changes.
                release.notified().await;
                spawn_emitting_resource(&resources, &binding)?;
                Ok(source
                    .into_iter()
                    .map(|handle| (handle.clone(), format!("task-{handle}")))
                    .collect())
            })
        }

        fn event(
            &self,
            _binding: TaskRuntimeBinding,
            _event: WorkflowEvent,
        ) -> TaskRuntimeFuture<WorkflowResponse> {
            Box::pin(async { Err(PluginApiErrorCode::InvalidRequest) })
        }

        fn invoke(
            &self,
            _binding: TaskRuntimeBinding,
            _call: PluginApiCall,
            _allow_interaction: bool,
        ) -> TaskRuntimeFuture<PluginApiReply> {
            Box::pin(async { Err(PluginApiErrorCode::InvalidRequest) })
        }
    }

    #[tokio::test]
    async fn interaction_required_never_advances_without_resume_payload_replacement() {
        let (_directory, service, context) = service();
        let started = service.start(context).await.unwrap();
        let task = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::NeedsUserAction,
        )
        .await;
        assert!(
            service
                .resume(&task.task.task_id, WireSequence::new(1))
                .await
                .is_err()
        );
        let resumed = service
            .resume(&task.task.task_id, task.task.revision)
            .await
            .unwrap();
        assert_eq!(resumed.task.state, PluginWorkflowTaskState::NeedsUserAction);
        assert!(resumed.result.is_none() || resumed.result.unwrap().replies.is_empty());
    }

    #[tokio::test]
    async fn wait_without_a_task_owned_resource_fails_the_lifecycle() {
        let runtime = Arc::new(NoResourceWaitRuntime);
        let (_directory, service, context, _) = service_with_runtime(runtime);
        let started = service.start(context).await.unwrap();

        let terminal = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::Failed,
        )
        .await;
        assert!(!terminal.task.outcome_unknown);
        assert!(terminal.steps.is_empty());
    }

    #[tokio::test]
    async fn a_guest_turn_that_loses_its_execution_fence_cannot_complete_late() {
        let authority = Arc::new(AtomicBool::new(true));
        let runtime = Arc::new(RevokingCompleteRuntime {
            task_authority: authority.clone(),
        });
        let (_directory, service, mut context, _) = service_with_runtime(runtime);
        let task_authority = authority.clone();
        context.task_authority = Arc::new(move || task_authority.load(Ordering::Acquire));
        let started = service.start(context).await.unwrap();

        let terminal = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::Failed,
        )
        .await;
        assert!(!terminal.task.outcome_unknown);
    }

    #[tokio::test]
    async fn fixed_step_cannot_be_dispatched_twice_and_leak_a_running_task() {
        let runtime = Arc::new(DuplicateStepRuntime::default());
        let (_directory, service, context, _) = service_with_runtime(runtime.clone());
        let started = service.start(context).await.unwrap();

        let terminal = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::Failed,
        )
        .await;
        assert!(!terminal.task.outcome_unknown);
        assert_eq!(runtime.calls.load(Ordering::Acquire), 1);
        assert_eq!(terminal.steps.len(), 1);
        assert_eq!(terminal.steps[0].state, PluginWorkflowStepState::Succeeded);
        assert_eq!(terminal.result.unwrap().replies.len(), 1);
    }

    #[tokio::test]
    async fn revoked_transaction_authority_does_not_block_core_owned_stop_cleanup() {
        let authority = Arc::new(AtomicBool::new(true));
        let runtime = Arc::new(TestRuntime::default());
        let (_directory, service, mut context, _) = service_with_runtime(runtime);
        let transaction_authority = authority.clone();
        context.transaction_authority =
            Arc::new(move || transaction_authority.load(Ordering::Acquire));
        let owner = context.owner.clone();
        let started = service.start(context).await.unwrap();
        let waiting = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::NeedsUserAction,
        )
        .await;

        authority.store(false, Ordering::Release);
        service
            .stop_plugin(&owner.plugin_id, Some(owner.generation))
            .await
            .unwrap();

        let stopped = service.get(&waiting.task.task_id).unwrap();
        assert_eq!(stopped.task.state, PluginWorkflowTaskState::Cancelled);
        assert!(!stopped.task.cleanup_incomplete);
    }

    #[tokio::test]
    async fn queued_resource_events_cannot_overwrite_a_frozen_pending_call() {
        let resources = ResourceRegistry::default();
        let runtime = Arc::new(PendingResourceRuntime::new(resources.clone()));
        let (_directory, service, context, _) =
            service_with_resources(runtime.clone(), resources.clone());
        let owner = context.owner.clone();
        let started = service.start(context).await.unwrap();
        let pending = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::NeedsUserAction,
        )
        .await;
        let control = service.control(&started.task.task_id).unwrap();
        let scoped = resources.for_consumer(control.consumer.clone());

        tokio::time::sleep(Duration::from_millis(75)).await;
        assert_eq!(runtime.resource_event_calls.load(Ordering::Acquire), 0);
        let resource = scoped
            .list(&owner)
            .into_iter()
            .next()
            .expect("task resource remains queued until an explicit resume");
        let (events, backpressured) = scoped.take_events(&owner, &resource.handle, 1).unwrap();
        assert!(!backpressured);
        assert_eq!(events.len(), 1);
        assert_eq!(pending.task.state, PluginWorkflowTaskState::NeedsUserAction);

        let cancelled = service
            .cancel(&started.task.task_id, pending.task.revision)
            .await
            .unwrap();
        assert_eq!(cancelled.task.state, PluginWorkflowTaskState::Cancelled);
    }

    #[tokio::test]
    async fn task_resource_events_are_consumer_scoped_and_drive_a_waiting_workflow() {
        let resources = ResourceRegistry::default();
        let runtime = Arc::new(ResourceEventCompletingRuntime::new(resources.clone()));
        let (_directory, service, context, _) =
            service_with_resources(runtime.clone(), resources.clone());
        let owner = context.owner.clone();
        let sibling = ResourceConsumer {
            connection: Uuid::new_v4(),
            generation: owner.generation,
            stream: Uuid::new_v4(),
        };
        let sibling_resources = resources.for_consumer(sibling.clone());
        let sibling_handle = sibling_resources
            .spawn(
                owner.clone(),
                "sibling-resource",
                move |mut cancel, _events| async move {
                    let _ = cancel.changed().await;
                    Ok(())
                },
            )
            .unwrap();

        let started = service.start(context).await.unwrap();
        let completed = wait_for_state(
            &service,
            &started.task.task_id,
            PluginWorkflowTaskState::Completed,
        )
        .await;
        assert_eq!(runtime.event_calls.load(Ordering::Acquire), 1);
        let control = service.control(&started.task.task_id).unwrap();
        assert!(
            resources
                .for_consumer(control.consumer.clone())
                .list(&owner)
                .is_empty()
        );
        assert_eq!(sibling_resources.list(&owner).len(), 1);
        assert_eq!(completed.steps.len(), 0);

        sibling_resources
            .close(&owner, &sibling_handle)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancellation_after_late_scope_adoption_cleans_the_task_consumer() {
        let resources = ResourceRegistry::default();
        let runtime = Arc::new(PrepareGateRuntime::new(resources.clone()));
        let (_directory, service, mut context, _) =
            service_with_resources(runtime.clone(), resources.clone());
        context.request.file_scope_handles = vec!["source-root".into()];
        let owner = context.owner.clone();
        let start_service = service.clone();
        let start = tokio::spawn(async move { start_service.start(context).await });

        wait_for_condition(|| runtime.entered.load(Ordering::Acquire)).await;
        let control = service
            .controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .controls
            .values()
            .next()
            .cloned()
            .expect("durable task is registered before scope adoption");
        let task_id = control.task_id.clone();
        let task_revision = control.record().revision;
        let cancel_service = service.clone();
        let cancellation =
            tokio::spawn(async move { cancel_service.cancel(&task_id, task_revision).await });

        wait_for_condition(|| !control.task_is_active()).await;
        runtime.release.notify_one();

        assert_eq!(
            start.await.unwrap().unwrap_err(),
            PluginApiErrorCode::Revoked
        );
        let cancelled = cancellation.await.unwrap().unwrap();
        assert_eq!(cancelled.task.state, PluginWorkflowTaskState::Cancelled);
        assert!(
            resources
                .for_consumer(control.consumer.clone())
                .list(&owner)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn terminal_start_dedupes_and_results_are_pruned_without_replaying_recent_start() {
        let runtime = Arc::new(CompletingRuntime::default());
        let (_directory, service, context, _) = service_with_runtime(runtime.clone());
        let mut first_control = None;
        let mut newest = None;
        let mut newest_request = None;

        for index in 0..=MAX_START_DEDUPES {
            let mut request = context.clone();
            request.request.meta.request_id = norishell_core_api::RequestId::new();
            let started = service.start(request.clone()).await.unwrap();
            wait_for_state(
                &service,
                &started.task.task_id,
                PluginWorkflowTaskState::Completed,
            )
            .await;
            if index == 0 {
                first_control = service.control(&started.task.task_id);
            }
            newest = Some(started.task.task_id);
            newest_request = Some(request);
        }

        let newest = newest.unwrap();
        let first_control = first_control.unwrap();
        let invocation_count = runtime.calls.load(Ordering::Acquire);
        let duplicate = service.start(newest_request.unwrap()).await.unwrap();
        assert_eq!(duplicate.task.task_id, newest);
        assert_eq!(runtime.calls.load(Ordering::Acquire), invocation_count);
        {
            let registry = service
                .controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(registry.controls.len() <= MAX_START_DEDUPES);
            assert!(registry.dedupes.len() <= MAX_START_DEDUPES);
        }
        assert!(service.control(&first_control.task_id).is_none());
        wait_for_condition(|| Arc::strong_count(&first_control) == 1).await;
    }
}
