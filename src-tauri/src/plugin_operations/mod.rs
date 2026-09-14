//! Core-owned, one-shot SSH operations and independent protected prompts.
//! Plugin code receives bounded results, never SSH or authentication handles.

pub(crate) mod prompts;
#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use norishell_core_api::{
    PluginApprovalId, PluginId, PluginRemoteApprovalContent, PluginRemoteOperationData,
    PluginRemoteOperationResult, PluginRemoteOperationState, SshSessionId, WireSequence,
};
use norishell_plugin_platform::operations::{
    OperationClass, OperationFailureKind, OperationOutcome, ParsedOperationOutput,
    PreconditionFailure, RawOperationResult, parse_result,
};
use norishell_ssh_transport::{RemoteExecOutput, TransportError};
use tauri::AppHandle;
use tokio::sync::{Notify, watch};

use crate::{
    host_service::HostService, lifecycle::LifecycleState, ssh_session_service::SessionChannelLease,
};

pub(crate) type OperationFence = Arc<dyn Fn() -> bool + Send + Sync>;

const GLOBAL_LIMIT: usize = 4;
const SESSION_LIMIT: usize = 2;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);

/// Created only after the public catalog has validated every operation field.
pub(crate) use norishell_plugin_platform::operations::OperationPlan as RemoteExecutionPlan;

#[derive(Clone)]
pub(crate) struct RemoteInvocation {
    pub(crate) plugin_id: PluginId,
    pub(crate) instance_generation: WireSequence,
    pub(crate) plugin_name: String,
    pub(crate) locale: norishell_core_api::PluginLocale,
    pub(crate) lease: SessionChannelLease,
    pub(crate) host_label: String,
    pub(crate) endpoint: String,
    pub(crate) reason: String,
    pub(crate) fence: OperationFence,
    pub(crate) remembered_policy: Option<crate::plugin_operation_policy::PreparedOperationPolicy>,
}

#[derive(Clone)]
pub(crate) struct PluginOperationsService {
    hosts: HostService,
    lifecycle: LifecycleState,
    app: Option<AppHandle>,
    state: Arc<Mutex<OperationState>>,
    changed: Arc<Notify>,
}

#[derive(Default)]
struct OperationState {
    active: BTreeMap<String, ActiveOperation>,
    prompts: BTreeMap<String, prompts::PendingPrompt>,
}

struct ActiveOperation {
    plugin_id: PluginId,
    instance_generation: WireSequence,
    session_id: SshSessionId,
    stop: watch::Sender<bool>,
    mutation: bool,
}

struct OperationGuard {
    service: PluginOperationsService,
    id: PluginApprovalId,
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.service
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .remove(self.id.as_str());
        self.service.changed.notify_waiters();
    }
}

impl PluginOperationsService {
    pub(crate) fn new(hosts: HostService, lifecycle: LifecycleState, app: AppHandle) -> Self {
        Self {
            hosts,
            lifecycle,
            app: Some(app),
            state: Arc::new(Mutex::new(OperationState::default())),
            changed: Arc::new(Notify::new()),
        }
    }

    /// Test-only construction keeps remembered-policy integration tests headless.
    /// The production constructor always receives the native app handle used for prompts.
    #[cfg(test)]
    pub(crate) fn new_test(hosts: HostService, lifecycle: LifecycleState) -> Self {
        Self {
            hosts,
            lifecycle,
            app: None,
            state: Arc::new(Mutex::new(OperationState::default())),
            changed: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn cancel_plugin(&self, plugin_id: &PluginId) {
        self.cancel_matching(plugin_id, None);
    }

    pub(crate) fn cancel_plugin_generation(&self, plugin_id: &PluginId, generation: WireSequence) {
        self.cancel_matching(plugin_id, Some(generation));
    }

    fn cancel_matching(&self, plugin_id: &PluginId, generation: Option<WireSequence>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for operation in state.active.values().filter(|operation| {
            operation.plugin_id == *plugin_id
                && generation.is_none_or(|generation| operation.instance_generation == generation)
        }) {
            let _ = operation.stop.send(true);
        }
        let prompts = state
            .prompts
            .values()
            .filter(|pending| {
                pending.prompt.plugin_id == *plugin_id
                    && generation.is_none_or(|generation| pending.instance_generation == generation)
            })
            .map(|pending| pending.prompt.approval_id.clone())
            .collect::<Vec<_>>();
        for id in &prompts {
            state.prompts.remove(id.as_str());
        }
        drop(state);
        if let Some(app) = &self.app {
            use tauri::Manager;
            for id in prompts {
                if let Some(window) = app.get_webview_window(&prompts::window_label(&id)) {
                    let _ = window.close();
                }
            }
        }
    }

    pub(crate) async fn stop_plugin(&self, plugin_id: &PluginId) -> bool {
        self.cancel_plugin(plugin_id);
        let wait = async {
            loop {
                let changed = self.changed.notified();
                if !self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .active
                    .values()
                    .any(|operation| operation.plugin_id == *plugin_id)
                {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(CLEANUP_TIMEOUT, wait).await.is_ok()
    }

    pub(crate) fn active_plugin_ids(&self) -> Vec<PluginId> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .values()
            .map(|operation| operation.plugin_id.clone())
            .collect()
    }

    fn register(
        &self,
        id: &PluginApprovalId,
        plugin_id: &PluginId,
        instance_generation: WireSequence,
        session_id: &SshSessionId,
        mutation: bool,
    ) -> Result<(OperationGuard, watch::Receiver<bool>), &'static str> {
        let _permit = self
            .lifecycle
            .acquire_resource_creation(norishell_core_api::RequestId::new())
            .map_err(|_| "applicationExiting")?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.active.len() >= GLOBAL_LIMIT
            || state
                .active
                .values()
                .any(|operation| operation.plugin_id == *plugin_id)
            || state
                .active
                .values()
                .filter(|operation| operation.session_id == *session_id)
                .count()
                >= SESSION_LIMIT
            || (mutation
                && state
                    .active
                    .values()
                    .any(|operation| operation.session_id == *session_id && operation.mutation))
        {
            return Err("busy");
        }
        let (stop, receiver) = watch::channel(false);
        state.active.insert(
            id.as_str().to_owned(),
            ActiveOperation {
                plugin_id: plugin_id.clone(),
                instance_generation,
                session_id: session_id.clone(),
                stop,
                mutation,
            },
        );
        Ok((
            OperationGuard {
                service: self.clone(),
                id: id.clone(),
            },
            receiver,
        ))
    }

    pub(crate) async fn invoke(
        &self,
        invocation: RemoteInvocation,
        plan: RemoteExecutionPlan,
    ) -> PluginRemoteOperationResult {
        let started = tokio::time::Instant::now();
        let id = PluginApprovalId::new();
        let mut result = failure(&id, PluginRemoteOperationState::Failed, "unavailable");
        let (guard, mut stop) = match self.register(
            &id,
            &invocation.plugin_id,
            invocation.instance_generation,
            &invocation.lease.session_id,
            plan.class == OperationClass::Mutation,
        ) {
            Ok(value) => value,
            Err(code) => return failure(&id, PluginRemoteOperationState::Failed, code),
        };
        let execution = self.run(&id, &invocation, &plan, &mut stop).await;
        match execution {
            Ok(output) => {
                result.state = if output.exit_status == Some(0) {
                    PluginRemoteOperationState::Succeeded
                } else if output.exit_status.is_some() {
                    PluginRemoteOperationState::Failed
                } else {
                    PluginRemoteOperationState::OutcomeUnknown
                };
                result.stable_error = if output.exit_status == Some(0) {
                    None
                } else if output.exit_status.is_some() {
                    Some("commandFailed".to_owned())
                } else {
                    Some("exitStatusMissing".to_owned())
                };
                result.stdout = safe_output(&output.stdout);
                result.stderr = safe_output(&output.stderr);
                result.exit_status = output.exit_status;
                match parse_result(
                    &plan,
                    RawOperationResult {
                        exit_status: output.exit_status,
                        stdout: output.stdout,
                        stderr: output.stderr,
                        timed_out: false,
                        output_limit_exceeded: false,
                    },
                ) {
                    Ok(OperationOutcome::Succeeded(success)) => {
                        result.data = project_success_data(success.output, &mut result);
                    }
                    Ok(OperationOutcome::Failed(failure)) => {
                        result.state =
                            if matches!(failure.kind, OperationFailureKind::ConnectionClosed) {
                                PluginRemoteOperationState::OutcomeUnknown
                            } else {
                                PluginRemoteOperationState::Failed
                            };
                        result.stable_error = Some(parsed_failure_code(&failure.kind).to_owned());
                    }
                    Err(_) => {
                        result.state = PluginRemoteOperationState::Failed;
                        result.stable_error = Some("invalidStructuredOutput".to_owned());
                    }
                }
            }
            Err((state, code)) => {
                result.state = state;
                result.stable_error = Some(code.to_owned());
                result.truncated = code == "outputLimit";
            }
        }
        result.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = self.hosts.with_plugin_repository(|repository| {
            repository.append_plugin_audit(
                Some(&invocation.plugin_id),
                None,
                "remote.operation",
                if result.state == PluginRemoteOperationState::Succeeded {
                    "succeeded"
                } else {
                    "notCompleted"
                },
                result.stable_error.as_deref(),
            )
        });
        drop(guard);
        result
    }

    pub(crate) async fn approve_resource(
        &self,
        invocation: &RemoteInvocation,
        content: PluginRemoteApprovalContent,
    ) -> Result<Option<crate::plugin_operation_policy::ApprovedOperationPolicy>, &'static str> {
        let (_guard, mut stop) = self.register(
            &PluginApprovalId::new(),
            &invocation.plugin_id,
            invocation.instance_generation,
            &invocation.lease.session_id,
            true,
        )?;
        self.prompt(invocation, content, &mut stop)
            .await
            .map_err(|_| "approvalRejected")
    }

    async fn run(
        &self,
        _id: &PluginApprovalId,
        invocation: &RemoteInvocation,
        plan: &RemoteExecutionPlan,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<RemoteExecOutput, (PluginRemoteOperationState, &'static str)> {
        use PluginRemoteOperationState::{Cancelled, Failed, OutcomeUnknown, Rejected};
        if !(invocation.fence)() || !invocation.lease.is_current() {
            return Err((Cancelled, "sessionChanged"));
        }
        let approved = if plan.class == OperationClass::Mutation {
            self.prompt(
                invocation,
                PluginRemoteApprovalContent::Execute {
                    command: review_text(&plan.command),
                    stdin: plan
                        .stdin
                        .as_ref()
                        .map(|bytes| review_text(&String::from_utf8_lossy(bytes))),
                    reason: review_text(&invocation.reason),
                },
                stop,
            )
            .await
            .map_err(|_| (Rejected, "approvalRejected"))?
        } else {
            None
        };
        let original_fence = invocation.fence.clone();
        let policy = approved.clone();
        let fence: OperationFence = Arc::new(move || {
            original_fence() && policy.as_ref().is_none_or(|policy| policy.current())
        });
        let (cancel, cancelled) = watch::channel(false);
        let mut stop = stop.clone();
        let monitor_fence = fence.clone();
        let lease = invocation.lease.clone();
        let monitor = tokio::spawn(async move {
            tokio::select! { () = wait_cancelled(&mut stop, &monitor_fence) => {}, () = lease.wait_cancelled() => {} }
            cancel.send_replace(true);
        });
        let _monitor = OperationMonitor(monitor);
        let channels = &invocation.lease.channels;
        let platform = channels
            .execute_capture_request(
                b"uname -s",
                None,
                Duration::from_secs(5),
                256,
                || fence() && invocation.lease.is_current(),
                cancelled.clone(),
            )
            .await;
        if !platform.as_ref().is_ok_and(|output| {
            output.exit_status == Some(0) && output.stdout.trim_ascii() == b"Linux"
        }) {
            return Err(if *cancelled.borrow() {
                (Cancelled, "sessionChanged")
            } else {
                (Failed, "unsupportedPlatform")
            });
        }
        let dispatched = AtomicBool::new(false);
        channels
            .execute_capture_request(
                plan.command.as_bytes(),
                plan.stdin.as_deref(),
                plan.timeout,
                plan.max_output_bytes,
                || {
                    if !fence()
                        || !invocation.lease.is_current()
                        || !approved
                            .as_ref()
                            .is_none_or(|policy| policy.admit_dispatch())
                    {
                        return false;
                    }
                    dispatched.store(true, Ordering::Release);
                    true
                },
                cancelled.clone(),
            )
            .await
            .map_err(|error| {
                let state = if *cancelled.borrow() {
                    if dispatched.load(Ordering::Acquire) && plan.class == OperationClass::Mutation
                    {
                        OutcomeUnknown
                    } else {
                        Cancelled
                    }
                } else if matches!(error, TransportError::RemoteExecRejected) {
                    Failed
                } else if dispatched.load(Ordering::Acquire)
                    && plan.class == OperationClass::Mutation
                {
                    OutcomeUnknown
                } else {
                    Failed
                };
                (
                    state,
                    if *cancelled.borrow() {
                        "sessionChanged"
                    } else {
                        exec_error(&error)
                    },
                )
            })
    }
}

struct OperationMonitor(tokio::task::JoinHandle<()>);
impl Drop for OperationMonitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn wait_cancelled(stop: &mut watch::Receiver<bool>, fence: &OperationFence) {
    loop {
        if *stop.borrow() || !fence() {
            return;
        }
        tokio::select! { result = stop.changed() => if result.is_err() { return; }, () = tokio::time::sleep(Duration::from_millis(100)) => {} }
    }
}

fn failure(
    id: &PluginApprovalId,
    state: PluginRemoteOperationState,
    code: &str,
) -> PluginRemoteOperationResult {
    PluginRemoteOperationResult {
        operation_id: id.clone(),
        state,
        stdout: String::new(),
        stderr: String::new(),
        exit_status: None,
        duration_ms: 0,
        truncated: false,
        stable_error: Some(code.to_owned()),
        data: None,
    }
}

fn project_success_data(
    output: ParsedOperationOutput,
    result: &mut PluginRemoteOperationResult,
) -> Option<PluginRemoteOperationData> {
    match output {
        ParsedOperationOutput::Text(_) => None,
        ParsedOperationOutput::CrontabSnapshot { sha256, content } => {
            match String::from_utf8(content) {
                Ok(content) => Some(PluginRemoteOperationData::CrontabSnapshot { sha256, content }),
                Err(_) => {
                    result.state = PluginRemoteOperationState::Failed;
                    result.stable_error = Some("invalidStructuredOutput".to_owned());
                    None
                }
            }
        }
        ParsedOperationOutput::ProcessSnapshot { identity, details } => {
            Some(PluginRemoteOperationData::ProcessSnapshot {
                pid: identity.pid,
                start_time_ticks: identity.start_time_ticks.to_string(),
                details: safe_output(&details),
            })
        }
        ParsedOperationOutput::CpuUsage {
            basis_points,
            sample_duration_ms,
        } => {
            result.stdout.clear();
            Some(PluginRemoteOperationData::CpuUsage {
                basis_points,
                sample_duration_ms,
            })
        }
    }
}

fn safe_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .collect()
}

fn review_text(text: &str) -> String {
    text.chars().flat_map(|character| {
        if (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            character.escape_unicode().collect::<Vec<_>>()
        } else { vec![character] }
    }).collect()
}

fn exec_error(error: &TransportError) -> &'static str {
    match error {
        TransportError::RemoteExecTimeout => "timedOut",
        TransportError::RemoteExecOutputTooLarge => "outputLimit",
        TransportError::RemoteExecRejected => "execRejected",
        _ => "connectionLost",
    }
}

fn parsed_failure_code(error: &OperationFailureKind) -> &'static str {
    match error {
        OperationFailureKind::MissingTool { .. } => "missingTool",
        OperationFailureKind::PreconditionFailed(PreconditionFailure::ProcessIdentityChanged) => {
            "processIdentityChanged"
        }
        OperationFailureKind::PreconditionFailed(PreconditionFailure::CrontabChanged) => {
            "crontabChanged"
        }
        OperationFailureKind::PreconditionFailed(PreconditionFailure::NginxConfigTestFailed) => {
            "nginxConfigTestFailed"
        }
        OperationFailureKind::TimedOut => "timedOut",
        OperationFailureKind::OutputLimitExceeded => "outputLimit",
        OperationFailureKind::ConnectionClosed => "exitStatusMissing",
        OperationFailureKind::NonZeroExit { .. } => "commandFailed",
        OperationFailureKind::InvalidStructuredOutput => "invalidStructuredOutput",
    }
}
