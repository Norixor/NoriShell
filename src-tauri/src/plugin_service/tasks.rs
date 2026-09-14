//! Package workflow admission and the resident Wasm adapter.
use super::{api_invocation::ApiInvocation, *};
use crate::plugin_task_service::{
    TaskCaller, TaskRuntime, TaskRuntimeBinding, TaskRuntimeFuture, TaskService, TaskStartContext,
};
use norishell_core_api::{
    PluginApiCall, PluginApiErrorCode, PluginApiOperation, PluginApiReply, PluginApiValue,
    PluginWorkflowTaskStartRequest, WorkflowEvent, WorkflowResponse,
};

struct WorkflowRuntime(PluginService);

impl TaskRuntime for WorkflowRuntime {
    fn prepare_start(
        &self,
        binding: TaskRuntimeBinding,
        handles: Vec<String>,
    ) -> TaskRuntimeFuture<BTreeMap<String, String>> {
        let files = self.0.api.files.clone();
        Box::pin(async move {
            if !binding.active() {
                return Err(PluginApiErrorCode::Revoked);
            }
            files
                .adopt_scopes(binding.owner(), &handles, binding.consumer().clone())
                .await
        })
    }

    fn event(
        &self,
        binding: TaskRuntimeBinding,
        event: WorkflowEvent,
    ) -> TaskRuntimeFuture<WorkflowResponse> {
        let service = self.0.clone();
        Box::pin(async move {
            if !binding.active() {
                return Err(PluginApiErrorCode::Revoked);
            }
            let instance = service
                .active_instance(
                    RequestId::new(),
                    &binding.owner().plugin_id,
                    &binding.owner().signer,
                    binding.owner().generation,
                )
                .map_err(|_| PluginApiErrorCode::Revoked)?;
            let request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: Uuid::new_v4().to_string(),
                kind: PluginHostMessageKind::WorkflowEvent,
                payload_json: serde_json::to_string(&norishell_core_api::PluginWorkflowEvent {
                    task_id: binding.task_id().clone(),
                    workflow_id: binding.workflow_id().to_owned(),
                    event,
                })
                .map_err(|_| PluginApiErrorCode::InvalidRequest)?,
            };
            let outputs = service
                .execute_instance(RequestId::new(), instance, request)
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?;
            if !binding.active() {
                return Err(PluginApiErrorCode::Revoked);
            }
            if outputs.len() != 1
                || outputs[0].kind != "workflow.response"
                || outputs[0].payload_json.len() > 64 * 1024
            {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            serde_json::from_str(&outputs[0].payload_json)
                .map_err(|_| PluginApiErrorCode::InvalidRequest)
        })
    }

    fn invoke(
        &self,
        binding: TaskRuntimeBinding,
        call: PluginApiCall,
        allow_interaction: bool,
    ) -> TaskRuntimeFuture<PluginApiReply> {
        let service = self.0.clone();
        Box::pin(async move {
            if !binding.active() {
                return Err(PluginApiErrorCode::Revoked);
            }
            let owner = binding.owner().clone();
            let installed = service
                .hosts
                .with_plugin_repository(|repository| {
                    repository.get_plugin_installation(&owner.plugin_id)
                })
                .map_err(|_| PluginApiErrorCode::Revoked)?;
            let active = binding.clone();
            let transaction = binding.clone();
            service
                .invoke_api(
                    &installed,
                    ApiInvocation::Task {
                        request_id: RequestId::new(),
                        owner,
                        operation_id: format!(
                            "workflow:{}:{}",
                            binding.workflow_id(),
                            binding
                                .step_id()
                                .ok_or(PluginApiErrorCode::InvalidRequest)?
                        ),
                        authority: Arc::new(move || active.active()),
                        transaction_authority: Arc::new(move || transaction.transaction_current()),
                        consumer: binding.consumer().clone(),
                        explicit_user_action: allow_interaction,
                    },
                    &call,
                )
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)
        })
    }
}

impl PluginService {
    pub(crate) fn workflow_service(&self) -> TaskService {
        TaskService::with_runtime(
            self.hosts.clone(),
            self.api.resources.clone(),
            Arc::new(WorkflowRuntime(self.clone())),
        )
    }

    pub(super) fn task_service(&self) -> Result<TaskService, PluginApiErrorCode> {
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        app.try_state::<TaskService>()
            .map(|state| state.inner().clone())
            .ok_or(PluginApiErrorCode::Unavailable)
    }

    pub(super) async fn invoke_task_api(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        call: &PluginApiCall,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        let tasks = self.task_service()?;
        let owner = invocation.owner(installed);
        let caller = TaskCaller::new(owner.clone());
        let snapshot = match &call.operation {
            PluginApiOperation::TaskList {} => {
                return tasks
                    .list_for_owner(&caller)
                    .map(|snapshots| PluginApiValue::Tasks { snapshots });
            }
            PluginApiOperation::TaskGet { task_id } => tasks.get_for_owner(&caller, task_id)?,
            PluginApiOperation::TaskCancel {
                task_id,
                expected_revision,
            } => {
                tasks
                    .cancel_for_owner(&caller, task_id, *expected_revision)
                    .await?
            }
            PluginApiOperation::TaskResume {
                task_id,
                expected_revision,
            } => {
                if !invocation.explicit_user_action() {
                    return Err(PluginApiErrorCode::InteractionRequired);
                }
                tasks
                    .resume_for_owner(&caller, task_id, *expected_revision)
                    .await?
            }
            PluginApiOperation::TaskStart {
                workflow_id,
                input_json,
                file_scope_handles,
            } => {
                if !invocation.explicit_user_action() {
                    return Err(PluginApiErrorCode::InteractionRequired);
                }
                let catalog = self
                    .installer
                    .read_workflow_catalog(
                        &owner.plugin_id,
                        &installed.active_version,
                        &owner.package,
                    )
                    .map_err(|_| PluginApiErrorCode::InvalidRequest)?
                    .ok_or(PluginApiErrorCode::NotFound)?;
                let workflow = catalog
                    .workflow(workflow_id)
                    .cloned()
                    .ok_or(PluginApiErrorCode::NotFound)?;
                // Request identity stays Core-owned and includes the call within one UI action.
                use sha2::Digest as _;
                let digest = sha2::Sha256::digest(
                    format!("{}:{}", invocation.request_id(), call.call_id).as_bytes(),
                );
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(&digest[..16]);
                let request_id = RequestId::parse(Uuid::from_bytes(bytes).to_string())
                    .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
                tasks
                    .start(TaskStartContext {
                        request: PluginWorkflowTaskStartRequest {
                            meta: norishell_core_api::RequestMeta { request_id },
                            plugin_id: owner.plugin_id.clone(),
                            signer_fingerprint_sha256: owner.signer.clone(),
                            expected_package_sha256: owner.package.clone(),
                            expected_state_version: installed.state_version,
                            instance_generation: owner.generation,
                            workflow_id: workflow_id.clone(),
                            input_json: input_json.clone(),
                            file_scope_handles: file_scope_handles.clone(),
                        },
                        workflow,
                        owner: owner.clone(),
                        task_authority: self.api_resource_fence(owner.clone(), None),
                        transaction_authority: self.api_runtime_fence(owner),
                        explicit_user_action: true,
                    })
                    .await?
            }
            _ => return Err(PluginApiErrorCode::InvalidRequest),
        };
        Ok(PluginApiValue::Task { snapshot })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use norishell_core_api::{
        PluginApiOutcome, PluginCapability, PluginCapabilityGrant, PluginHostMessageKind,
        PluginInstallState, PluginLocalInstallRequest, PluginWorkflowApiMethod,
        PluginWorkflowStepState, PluginWorkflowTaskSnapshot, PluginWorkflowTaskState, RequestMeta,
    };
    use sha2::{Digest as _, Sha256};

    use super::*;
    use crate::{
        plugin_host_process::PluginHostProcess,
        transient_credential_service::TransientCredentialService, vault_service::VaultService,
    };

    const WORKFLOW_DEMO_PACKAGE: &str = "/tmp/NoriShell-Workflow-Demo-1.0.0.zip";
    const WORKFLOW_DEMO_PACKAGE_SHA256: &str =
        "53cbc4a30b4f94034e6491651072b9de76e590c4a69dc0b6b081784864519ec4";

    struct WorkflowDemoFixture {
        _directory: tempfile::TempDir,
        service: PluginService,
        tasks: TaskService,
        owner: crate::plugin_api::ResourceOwner,
        workflow: norishell_core_api::PluginWorkflowDefinition,
    }

    #[tokio::test]
    #[ignore = "requires the SDK-built workflow-demo ZIP and target/debug/norishell plugin host"]
    async fn workflow_demo_real_core_timer_chain_completes_and_releases_its_resource() {
        let fixture = workflow_demo_fixture().await;
        let start = explicit_ui_task_start(&fixture).await;
        let started = fixture.tasks.start(start).await.expect("Core starts task");
        let completed = wait_for_snapshot(&fixture.tasks, &started.task.task_id, |snapshot| {
            snapshot.task.state == PluginWorkflowTaskState::Completed
        })
        .await;

        assert_step(
            &completed,
            "inspect-first",
            PluginWorkflowApiMethod::Describe,
        );
        assert_step(
            &completed,
            "start-timer",
            PluginWorkflowApiMethod::TimerStart,
        );
        assert_step(
            &completed,
            "inspect-last",
            PluginWorkflowApiMethod::Describe,
        );
        assert!(
            fixture.service.api.resources.exit_blockers().is_empty(),
            "the completed task must close the Core-owned timer resource"
        );

        shutdown_fixture(fixture).await;
    }

    #[tokio::test]
    #[ignore = "requires the SDK-built workflow-demo ZIP and target/debug/norishell plugin host"]
    async fn workflow_demo_cancel_from_the_real_ui_control_releases_the_waiting_timer() {
        let fixture = workflow_demo_fixture().await;
        let start_call = explicit_ui_task_start_call(&fixture).await;
        let started = fixture
            .tasks
            .start(task_start_context(&fixture, start_call.clone()))
            .await
            .expect("Core starts task");
        let waiting = wait_for_snapshot(&fixture.tasks, &started.task.task_id, |snapshot| {
            snapshot.task.state == PluginWorkflowTaskState::Running
                && snapshot.steps.iter().any(|step| {
                    step.step_id == "start-timer"
                        && step.state == PluginWorkflowStepState::Succeeded
                })
        })
        .await;

        // This is the normal Core reply for the explicit Start action. It only updates
        // the guest's visible task snapshot so its real Cancel button can issue the
        // revision-fenced TaskCancel call; it does not synthesize a workflow or resource event.
        deliver_task_control_reply(&fixture, &start_call, waiting.clone()).await;
        let cancel = explicit_ui_task_cancel_call(&fixture).await;
        let (task_id, expected_revision) = match cancel.operation {
            PluginApiOperation::TaskCancel {
                task_id,
                expected_revision,
            } => (task_id, expected_revision),
            operation => panic!("workflow-demo Cancel button returned {operation:?}"),
        };
        assert_eq!(task_id, started.task.task_id);
        assert_eq!(expected_revision, waiting.task.revision);

        let cancelled = fixture
            .tasks
            .cancel_for_owner(
                &TaskCaller::new(fixture.owner.clone()),
                &task_id,
                expected_revision,
            )
            .await
            .expect("explicit UI cancellation reaches the Core task service");
        assert_eq!(cancelled.task.state, PluginWorkflowTaskState::Cancelled);
        assert!(
            cancelled
                .steps
                .iter()
                .all(|step| step.step_id != "inspect-last"),
            "cancelling the wait must prevent the post-timer Describe step"
        );
        assert!(
            fixture.service.api.resources.exit_blockers().is_empty(),
            "cancellation must await ResourceRegistry timer cleanup"
        );

        shutdown_fixture(fixture).await;
    }

    async fn workflow_demo_fixture() -> WorkflowDemoFixture {
        let package_path = Path::new(WORKFLOW_DEMO_PACKAGE);
        let package = std::fs::read(package_path).expect("workflow-demo package is available");
        assert_eq!(
            hex::encode(Sha256::digest(&package)),
            WORKFLOW_DEMO_PACKAGE_SHA256,
            "fixture must run the reviewed workflow-demo package"
        );
        let directory = tempfile::tempdir().expect("fixture directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service =
            PluginService::start(directory.path(), hosts, sessions).expect("plugin service");
        service.attach_test_lifecycle(crate::lifecycle::LifecycleState::default());
        let preview = service
            .prepare_local_package(package_path, RequestId::new())
            .expect("production local package preparation");
        assert_eq!(preview.package_sha256, WORKFLOW_DEMO_PACKAGE_SHA256);
        let installed = service
            .install_local_plugin(PluginLocalInstallRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: norishell_core_api::PluginOperationId::new(),
                idempotency_key: Uuid::new_v4().to_string(),
                preparation_id: preview.preparation_id,
                expected_package_sha256: preview.package_sha256,
                expected_state_version: preview.current_state_version,
                capability_grants: vec![PluginCapabilityGrant {
                    capability: PluginCapability::UiPanel,
                    granted: true,
                }],
            })
            .expect("production local package installation");
        assert_eq!(
            installed.state,
            norishell_core_api::PluginOperationState::Succeeded
        );
        let installed = service
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&preview.plugin_id)
            })
            .expect("installed workflow-demo record");
        let installed = service
            .set_plugin_state_convergent(RequestId::new(), &installed, PluginInstallState::Enabled)
            .expect("activate workflow-demo installation for the fixture");
        let module = service
            .installer
            .read_active_module(
                &installed.plugin_id,
                &installed.active_version,
                &installed.package_sha256,
                16 * 1024 * 1024,
            )
            .expect("installed workflow-demo Wasm");
        let workflow = service
            .installer
            .read_workflow_catalog(
                &installed.plugin_id,
                &installed.active_version,
                &installed.package_sha256,
            )
            .expect("read installed workflow catalog")
            .and_then(|catalog| catalog.workflow("inspect-delay-inspect").cloned())
            .expect("workflow-demo declares the three-step workflow");
        let executable = workspace_root()
            .join("target")
            .join("debug")
            .join(format!("norishell{}", std::env::consts::EXE_SUFFIX));
        assert!(
            executable.is_file(),
            "build target/debug/norishell before running the real Core workflow fixture"
        );
        let mut process = PluginHostProcess::spawn_with_executable_for_tests(
            &executable,
            &module,
            installed.state_version.get(),
        )
        .expect("production isolated plugin host");
        let locale = PluginLocale::parse("en").expect("fixture locale");
        let initialized = process
            .execute(PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: Uuid::new_v4().to_string(),
                kind: PluginHostMessageKind::Initialize,
                payload_json: plugin_host_payload(
                    &locale,
                    serde_json::json!({
                        "pluginId": installed.plugin_id.as_str(),
                        "version": installed.active_version,
                        "storage": null,
                    }),
                ),
            })
            .expect("workflow-demo initialization");
        assert!(
            initialized
                .iter()
                .any(|output| output.kind == "ui.document"),
            "the real package must initialize its task controls"
        );

        let owner = crate::plugin_api::ResourceOwner {
            plugin_id: installed.plugin_id.clone(),
            signer: installed.signer_fingerprint_sha256.clone(),
            package: installed.package_sha256.clone(),
            generation: installed.state_version,
        };
        service
            .runtime
            .lock()
            .expect("plugin runtime")
            .active_instances
            .insert(
                installed.plugin_id.as_str().to_owned(),
                ActivePluginInstance {
                    plugin_name: installed.name.clone(),
                    signer_fingerprint_sha256: installed.signer_fingerprint_sha256,
                    package_sha256: installed.package_sha256,
                    instance_generation: installed.state_version,
                    state_version: installed.state_version,
                    previously_crashed: false,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    locale,
                    contributions: BTreeMap::new(),
                    ui_templates: BTreeMap::new(),
                    scoped_templates: BTreeMap::new(),
                    scoped_states: BTreeMap::new(),
                    operation_authority_revoked: false,
                    api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    navigation: BTreeMap::new(),
                    pages: BTreeMap::new(),
                    contribution_revision: WireSequence::new(1),
                    settings_revision: None,
                    contribution_action_in_flight: false,
                    ui_state_json: "{}".to_owned(),
                    process: Arc::new(Mutex::new(Some(process))),
                },
            );
        let tasks = service.workflow_service();
        WorkflowDemoFixture {
            _directory: directory,
            service,
            tasks,
            owner,
            workflow,
        }
    }

    async fn explicit_ui_task_start(fixture: &WorkflowDemoFixture) -> TaskStartContext {
        task_start_context(fixture, explicit_ui_task_start_call(fixture).await)
    }

    async fn explicit_ui_task_start_call(fixture: &WorkflowDemoFixture) -> PluginApiCall {
        let call = explicit_ui_task_call(fixture, "workflow.start").await;
        assert!(matches!(
            call.operation,
            PluginApiOperation::TaskStart { .. }
        ));
        call
    }

    async fn explicit_ui_task_cancel_call(fixture: &WorkflowDemoFixture) -> PluginApiCall {
        explicit_ui_task_call(fixture, "workflow.cancel").await
    }

    async fn explicit_ui_task_call(
        fixture: &WorkflowDemoFixture,
        action_id: &str,
    ) -> PluginApiCall {
        let instance = fixture
            .service
            .active_instance(
                RequestId::new(),
                &fixture.owner.plugin_id,
                &fixture.owner.signer,
                fixture.owner.generation,
            )
            .expect("active workflow-demo instance");
        let outputs = fixture
            .service
            .execute_instance(
                RequestId::new(),
                instance,
                PluginHostRequest {
                    protocol_major: PLUGIN_PROTOCOL_MAJOR,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    request_id: Uuid::new_v4().to_string(),
                    kind: PluginHostMessageKind::UiAction,
                    payload_json: plugin_host_payload(
                        &fixture.service.locale(),
                        serde_json::json!({"actionId": action_id}),
                    ),
                },
            )
            .await
            .expect("real workflow-demo UI action");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].kind, "api.request");
        serde_json::from_str(&outputs[0].payload_json).expect("typed task control request")
    }

    fn task_start_context(fixture: &WorkflowDemoFixture, call: PluginApiCall) -> TaskStartContext {
        let PluginApiOperation::TaskStart {
            workflow_id,
            input_json,
            file_scope_handles,
        } = call.operation
        else {
            panic!("workflow-demo Start button did not return TaskStart");
        };
        TaskStartContext {
            request: PluginWorkflowTaskStartRequest {
                meta: norishell_core_api::RequestMeta {
                    request_id: RequestId::new(),
                },
                plugin_id: fixture.owner.plugin_id.clone(),
                signer_fingerprint_sha256: fixture.owner.signer.clone(),
                expected_package_sha256: fixture.owner.package.clone(),
                expected_state_version: fixture.owner.generation,
                instance_generation: fixture.owner.generation,
                workflow_id,
                input_json,
                file_scope_handles,
            },
            workflow: fixture.workflow.clone(),
            owner: fixture.owner.clone(),
            task_authority: Arc::new(|| true),
            transaction_authority: Arc::new(|| true),
            explicit_user_action: true,
        }
    }

    async fn deliver_task_control_reply(
        fixture: &WorkflowDemoFixture,
        call: &PluginApiCall,
        snapshot: PluginWorkflowTaskSnapshot,
    ) {
        let instance = fixture
            .service
            .active_instance(
                RequestId::new(),
                &fixture.owner.plugin_id,
                &fixture.owner.signer,
                fixture.owner.generation,
            )
            .expect("active workflow-demo instance");
        let reply = PluginApiReply {
            call_id: call.call_id.clone(),
            outcome: PluginApiOutcome::Completed {
                value: PluginApiValue::Task { snapshot },
            },
        };
        fixture
            .service
            .execute_instance(
                RequestId::new(),
                instance,
                PluginHostRequest {
                    protocol_major: PLUGIN_PROTOCOL_MAJOR,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    request_id: Uuid::new_v4().to_string(),
                    kind: PluginHostMessageKind::BrokerResult,
                    payload_json: plugin_host_payload(
                        &fixture.service.locale(),
                        serde_json::json!({"result": {"reply": reply}}),
                    ),
                },
            )
            .await
            .expect("deliver actual Core task snapshot to the UI control");
    }

    async fn wait_for_snapshot(
        tasks: &TaskService,
        task_id: &norishell_core_api::PluginWorkflowTaskId,
        predicate: impl Fn(&PluginWorkflowTaskSnapshot) -> bool,
    ) -> PluginWorkflowTaskSnapshot {
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let snapshot = tasks.get(task_id).expect("read task snapshot");
                if predicate(&snapshot) {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        match result {
            Ok(snapshot) => snapshot,
            Err(_) => panic!(
                "workflow-demo did not reach the expected real Core state; last snapshot: {:?}",
                tasks.get(task_id)
            ),
        }
    }

    fn assert_step(
        snapshot: &PluginWorkflowTaskSnapshot,
        id: &str,
        method: PluginWorkflowApiMethod,
    ) {
        let step = snapshot
            .steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap_or_else(|| panic!("missing workflow step {id}"));
        assert_eq!(step.method, method, "wrong method for {id}");
        assert_eq!(
            step.state,
            PluginWorkflowStepState::Succeeded,
            "{id} did not succeed"
        );
        assert!(!step.outcome_unknown, "{id} must have a known Core outcome");
    }

    async fn shutdown_fixture(fixture: WorkflowDemoFixture) {
        fixture
            .tasks
            .shutdown_all()
            .await
            .expect("workflow task cleanup");
        fixture
            .service
            .stop_plugin(&fixture.owner.plugin_id)
            .await
            .expect("plugin host cleanup");
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf()
    }
}
