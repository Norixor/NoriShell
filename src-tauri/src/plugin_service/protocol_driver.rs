//! The session driver owns the protocol event pump and resource set; actual owner joins prove closure.
use super::{PluginService, protocols::ProtocolBinding};
use crate::plugin_api::{ResourceConsumer, ResourceRegistry};
use crate::plugin_terminal_session_service as terminal;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiResourceEventKind, PluginNetworkEvent, PluginProtocolEvent,
    PluginProtocolEventKind, PluginProtocolOutput, PluginSerialEvent, PluginSettingsValues,
    WireSequence,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

pub(super) struct ProtocolDriverFactory {
    pub service: PluginService,
    pub binding: ProtocolBinding,
    pub configuration: PluginSettingsValues,
}

struct DriverRuntime {
    active: Arc<AtomicBool>,
    cancel: watch::Sender<bool>,
}
impl terminal::PluginTerminalDriverRuntime for DriverRuntime {
    fn cancel(&self) {
        self.active.store(false, Ordering::Release);
        self.cancel.send_replace(true);
    }
}

impl terminal::PluginTerminalDriverFactory for ProtocolDriverFactory {
    fn start(
        &self,
        start: terminal::PluginTerminalDriverStart,
    ) -> Result<terminal::PluginTerminalDriverBinding, terminal::PluginTerminalDriverError> {
        if !(self.binding.authority)()
            || start.provider.plugin_id != self.binding.owner.plugin_id.as_str()
            || start.provider.provider_id != self.binding.provider_id
        {
            return Err(terminal::PluginTerminalDriverError::Rejected);
        }
        let active = Arc::new(AtomicBool::new(true));
        let mut binding = self.binding.clone();
        binding.consumer = ResourceConsumer {
            connection: start.connection_handle().as_uuid(),
            generation: WireSequence::new(start.generation),
            stream: start.stream_id,
        };
        binding.connection_handle = start.connection_handle().as_uuid().to_string();
        let parent = binding.authority.clone();
        let live = active.clone();
        binding.authority = Arc::new(move || live.load(Ordering::Acquire) && parent());
        let parent = binding.transaction_authority.clone();
        let live = active.clone();
        binding.transaction_authority = Arc::new(move || live.load(Ordering::Acquire) && parent());
        let (commands, incoming) = mpsc::channel(16);
        let (events, outgoing) = mpsc::channel(128);
        let (cancel, cancellation) = watch::channel(false);
        let runtime = Arc::new(DriverRuntime { active, cancel });
        let service = self.service.clone();
        let configuration = self.configuration.clone();
        let task_runtime = runtime.clone();
        tokio::spawn(async move {
            run_driver(
                service,
                binding,
                start,
                configuration,
                incoming,
                events,
                cancellation,
                task_runtime,
            )
            .await;
        });
        Ok(terminal::PluginTerminalDriverBinding {
            commands,
            events: outgoing,
            runtime,
        })
    }
}

type DriverResult = Result<(), PluginApiErrorCode>;

#[allow(clippy::too_many_arguments)]
async fn run_driver(
    service: PluginService,
    binding: ProtocolBinding,
    start: terminal::PluginTerminalDriverStart,
    configuration: PluginSettingsValues,
    mut commands: mpsc::Receiver<terminal::PluginTerminalDriverCommand>,
    events: mpsc::Sender<terminal::PluginTerminalDriverEvent>,
    mut cancel: watch::Receiver<bool>,
    runtime: Arc<DriverRuntime>,
) {
    let resources = service.protocol_resources(&binding);
    let (output, mut outputs) = mpsc::channel(64);
    let (opened, mut transport_count) = watch::channel(0usize);
    let (stop, mut stops) = mpsc::channel::<bool>(4);
    let mut tasks: JoinSet<DriverResult> = JoinSet::new();
    let connect = event(
        &binding,
        PluginProtocolEventKind::Connect {
            provider_id: binding.provider_id.clone(),
            configuration,
            rows: start.rows,
            cols: start.cols,
        },
    );
    let svc = service.clone();
    let current = binding.clone();
    let sink = output.clone();
    tasks.spawn(async move {
        svc.execute_protocol_event(&current, connect, true, &sink)
            .await
    });
    let svc = service.clone();
    let current = binding.clone();
    let sink = output.clone();
    let resource_view = resources.clone();
    let stop_signal = stop.clone();
    tasks.spawn(async move {
        pump_resources(svc, current, resource_view, sink, opened, stop_signal).await
    });
    let mut requested_ready = false;
    let mut reported_ready = false;
    let mut closing = false;
    loop {
        let mut close_ack: Option<
            oneshot::Sender<Result<(), terminal::PluginTerminalDriverError>>,
        > = None;
        let mut close_now = false;
        tokio::select! {
            command = commands.recv() => match command {
                Some(terminal::PluginTerminalDriverCommand::Close { ack }) => { close_ack = Some(ack); close_now = true; }
                Some(command) if !closing && tasks.len() < 18 => {
                    let (kind, ack) = match command {
                        terminal::PluginTerminalDriverCommand::Input { bytes, ack } =>
                            (PluginProtocolEventKind::Input { data_base64: BASE64.encode(bytes) }, ack),
                        terminal::PluginTerminalDriverCommand::Resize { rows, cols, ack } =>
                            (PluginProtocolEventKind::Resize { rows, cols }, ack),
                        terminal::PluginTerminalDriverCommand::Close { .. } => unreachable!(),
                    };
                    let svc = service.clone(); let current = binding.clone(); let sink = output.clone();
                    tasks.spawn(async move {
                        let result = svc.execute_protocol_event(&current, event(&current, kind), false, &sink).await;
                        let _ = ack.send(result.map_err(driver_error));
                        result
                    });
                }
                Some(command) => reject_command(command),
                None => { if closing { tokio::time::sleep(Duration::from_millis(100)).await; } close_now = true; }
            },
            value = outputs.recv(), if !closing => match value {
                Some(PluginProtocolOutput::Ready {}) => requested_ready = true,
                Some(PluginProtocolOutput::Output { data_base64 }) => {
                    match BASE64.decode(data_base64) {
                        Ok(bytes) if bytes.len() <= 64 * 1024 => {
                            if events.send(terminal::PluginTerminalDriverEvent::Output(bytes)).await.is_err() { close_now = true; }
                        }
                        _ => { let _ = events.send(terminal::PluginTerminalDriverEvent::Failed(terminal::PluginTerminalDriverError::Failed)).await; close_now = true; }
                    }
                }
                Some(PluginProtocolOutput::Exit { .. }) => close_now = true,
                Some(PluginProtocolOutput::Fail { stable_code }) => {
                    let _ = events.send(terminal::PluginTerminalDriverEvent::Failed(driver_error(stable_code))).await;
                    close_now = true;
                }
                None => close_now = true,
            },
            _ = transport_count.changed(), if !closing => {},
            result = tasks.join_next(), if !closing && !tasks.is_empty() => {
                if !matches!(result, Some(Ok(Ok(())))) {
                    let _ = events.send(terminal::PluginTerminalDriverEvent::Failed(terminal::PluginTerminalDriverError::Failed)).await;
                    close_now = true;
                }
            },
            _ = cancel.changed(), if !closing => { close_now = true; },
            _ = stops.recv(), if !closing => { close_now = true; },
            _ = tokio::time::sleep(Duration::from_millis(100)), if !closing => {
                if !(binding.authority)() { close_now = true; }
            }
        }
        if !closing && !reported_ready && requested_ready && *transport_count.borrow() > 0 {
            reported_ready = true;
            if events
                .send(terminal::PluginTerminalDriverEvent::Ready)
                .await
                .is_err()
            {
                close_now = true;
            }
        }
        if close_now {
            closing = true;
            runtime.active.store(false, Ordering::Release);
            runtime.cancel.send_replace(true);
            // Cancellation first releases all blocked network readers and pending approval windows.
            let closed = resources
                .stop_plugin_generation(&binding.owner.plugin_id, binding.owner.generation)
                .await;
            let joined = tokio::time::timeout(Duration::from_secs(3), async {
                while tasks.join_next().await.is_some() {}
            })
            .await;
            let success = closed.is_ok() && joined.is_ok();
            if let Some(ack) = close_ack {
                let _ = ack.send(if success {
                    Ok(())
                } else {
                    Err(terminal::PluginTerminalDriverError::Unknown)
                });
            }
            if success {
                while let Ok(value) = outputs.try_recv() {
                    if let PluginProtocolOutput::Output { data_base64 } = value
                        && let Ok(bytes) = BASE64.decode(data_base64)
                    {
                        let _ = events
                            .send(terminal::PluginTerminalDriverEvent::Output(bytes))
                            .await;
                    }
                }
                service.notify_protocol_closed(&binding).await;
                let _ = events.send(terminal::PluginTerminalDriverEvent::Exit).await;
                return;
            }
            let _ = events
                .send(terminal::PluginTerminalDriverEvent::Failed(
                    terminal::PluginTerminalDriverError::Unknown,
                ))
                .await;
            // Keep the join set and registry entries owned for an explicit cleanup retry.
        }
    }
}

fn event(binding: &ProtocolBinding, kind: PluginProtocolEventKind) -> PluginProtocolEvent {
    PluginProtocolEvent {
        connection_handle: binding.connection_handle.clone(),
        event_id: uuid::Uuid::new_v4().to_string(),
        event: kind,
    }
}

async fn pump_resources(
    service: PluginService,
    binding: ProtocolBinding,
    resources: ResourceRegistry,
    output: mpsc::Sender<PluginProtocolOutput>,
    opened: watch::Sender<usize>,
    stop: mpsc::Sender<bool>,
) -> DriverResult {
    let mut active = std::collections::BTreeSet::new();
    loop {
        if !(binding.authority)() {
            return Ok(());
        }
        tokio::select! {
            _ = resources.wait_events(&binding.owner) => {},
            _ = tokio::time::sleep(Duration::from_millis(100)) => { continue; }
        }
        for resource in resources.list(&binding.owner) {
            // One bounded resource frame per Wasm event keeps total frame bytes below 64 KiB.
            let (frames, _) = resources.take_events(&binding.owner, &resource.handle, 1)?;
            if frames.is_empty() {
                continue;
            }
            let mut ended = false;
            for frame in &frames {
                match &frame.kind {
                    PluginApiResourceEventKind::Network {
                        event: PluginNetworkEvent::Opened { .. },
                    }
                    | PluginApiResourceEventKind::Serial {
                        event: PluginSerialEvent::Opened { .. },
                    } => {
                        active.insert(resource.handle.clone());
                    }
                    PluginApiResourceEventKind::Network {
                        event: PluginNetworkEvent::Closed { .. } | PluginNetworkEvent::Error { .. },
                    }
                    | PluginApiResourceEventKind::Serial {
                        event: PluginSerialEvent::Closed { .. } | PluginSerialEvent::Error { .. },
                    } => {
                        active.remove(&resource.handle);
                        ended = true;
                    }
                    _ => {}
                }
            }
            opened.send_replace(active.len());
            service
                .execute_protocol_event(
                    &binding,
                    event(
                        &binding,
                        PluginProtocolEventKind::Resource {
                            resource_handle: resource.handle,
                            events: frames,
                        },
                    ),
                    false,
                    &output,
                )
                .await?;
            if ended {
                let _ = stop.send(true).await;
                return Ok(());
            }
        }
    }
}

fn reject_command(command: terminal::PluginTerminalDriverCommand) {
    let ack = match command {
        terminal::PluginTerminalDriverCommand::Input { ack, .. }
        | terminal::PluginTerminalDriverCommand::Resize { ack, .. }
        | terminal::PluginTerminalDriverCommand::Close { ack } => ack,
    };
    let _ = ack.send(Err(terminal::PluginTerminalDriverError::Rejected));
}
fn driver_error(error: PluginApiErrorCode) -> terminal::PluginTerminalDriverError {
    match error {
        PluginApiErrorCode::OutcomeUnknown | PluginApiErrorCode::CleanupIncomplete => {
            terminal::PluginTerminalDriverError::Unknown
        }
        PluginApiErrorCode::PermissionDenied
        | PluginApiErrorCode::Revoked
        | PluginApiErrorCode::InvalidRequest => terminal::PluginTerminalDriverError::Rejected,
        _ => terminal::PluginTerminalDriverError::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use norishell_core_api::{
        PluginApprovalDecision, PluginApprovalOperation, PluginCapability, PluginCapabilityGrant,
        PluginHostMessageKind, PluginInstallState, PluginLocalInstallRequest,
        PluginNetworkEndpointRequest, PluginNetworkOperation, PluginNetworkStartRequest,
        PluginProtocolResource, PluginSettingValue, PluginSpecialPermissionDecisionRequest,
        PluginSpecialPermissionOpenRequest, PluginSpecialPermissionOutcome,
        PluginSpecialPermissionTarget, RequestId, RequestMeta, WireSequence,
    };
    use sha2::{Digest as _, Sha256};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        sync::broadcast,
    };
    use uuid::Uuid;

    use super::super::{
        ActivePluginInstance, PluginService, current_plugin_permission_binding, plugin_host_payload,
    };
    use super::*;
    use crate::{
        host_service::HostService,
        lifecycle::LifecycleState,
        plugin_api::{ResourceConsumer, ResourceOwner},
        plugin_host_process::PluginHostProcess,
        plugin_operations::PluginOperationsService,
        plugin_terminal_session_service::{
            PluginTerminalConnectionHandle, PluginTerminalDisconnectRequest,
            PluginTerminalFrontendLaunchClaim, PluginTerminalInputFence,
            PluginTerminalInputGrantRequest, PluginTerminalInputRequest, PluginTerminalOpenRequest,
            PluginTerminalProviderMetadata, PluginTerminalReconnectRequest,
            PluginTerminalResizeRequest, PluginTerminalSessionError, PluginTerminalSessionEvent,
            PluginTerminalSessionService, PluginTerminalSessionState,
        },
        ssh_session_service::SshSessionService,
        transient_credential_service::TransientCredentialService,
        vault_service::VaultService,
    };

    const PROTOCOL_DEMO_PACKAGE: &str = "/tmp/NoriShell-Framed-TCP-Protocol-Demo-1.0.1.zip";
    const PROTOCOL_DEMO_PACKAGE_SHA256: &str =
        "27ea7b2cb0ebfc398395d54a98e620075fed71753aa442130e2604bc9733641a";

    struct Fixture {
        _directory: tempfile::TempDir,
        service: PluginService,
        actor: PluginTerminalSessionService,
        owner: ResourceOwner,
        provider: PluginTerminalProviderMetadata,
        configuration: BTreeMap<String, PluginSettingValue>,
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires target/debug/norishell and the reviewed protocol-demo package"]
    async fn real_core_framed_tcp_driver_cleans_resources_and_rejects_stale_generation() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback protocol peer");
        let endpoint = format!("tcp://{}", listener.local_addr().expect("peer address"));
        let peer = tokio::spawn(protocol_peer(listener));
        let fixture = fixture(&endpoint).await;

        let binding = open(&fixture, 24, 80).await;
        let first = wait_for_running(&fixture.actor, binding.response.session.session_id).await;
        let mut events = binding.events;
        let lease = grant_input(&fixture.actor, &binding.response).await;
        let fence = PluginTerminalInputFence::from(&lease);
        fixture
            .actor
            .input(PluginTerminalInputRequest {
                fence: fence.clone(),
                client_sequence: 1,
                data: b"first input\n".to_vec(),
            })
            .await
            .expect("Core resource owner accepts first input");
        fixture
            .actor
            .resize(PluginTerminalResizeRequest {
                fence: fence.clone(),
                resize_sequence: 1,
                rows: 33,
                cols: 101,
            })
            .await
            .expect("Core resource owner accepts resize");
        assert_eq!(
            next_output(&mut events).await,
            b"\x1b[32mfirst: \xe4\xb8\x96\xe7\x95\x8c\x1b[0m\r\n"
        );

        let closed = fixture
            .actor
            .disconnect(PluginTerminalDisconnectRequest {
                operation_id: Uuid::new_v4(),
                idempotency_key: "first-close".to_owned(),
                session_id: first.session_id,
                expected_generation: first.generation,
                expected_stream_id: first.stream_id,
                expected_state_revision: fixture
                    .actor
                    .get(first.session_id)
                    .await
                    .expect("first session")
                    .state_revision,
            })
            .await
            .expect("close waits for real resource cleanup");
        assert_eq!(closed.state, PluginTerminalSessionState::Closed);
        assert!(!has_owned_resources(&fixture.service, &fixture.owner));
        assert!(matches!(
            fixture.actor.validate_fence(fence.clone()).await,
            Err(PluginTerminalSessionError::Conflict)
                | Err(PluginTerminalSessionError::InputNotAuthorized)
        ));

        let reconnected = fixture
            .actor
            .reconnect_after_frontend_launch(
                PluginTerminalFrontendLaunchClaim::new(Uuid::new_v4()).expect("launch claim"),
                PluginTerminalReconnectRequest {
                    operation_id: Uuid::new_v4(),
                    idempotency_key: "real-core-reconnect".to_owned(),
                    session_id: closed.session_id,
                    expected_generation: closed.generation,
                    expected_stream_id: closed.stream_id,
                    expected_state_revision: closed.state_revision,
                    connection_handle: PluginTerminalConnectionHandle::new(Uuid::new_v4())
                        .expect("new Core-only connection handle"),
                    provider: fixture.provider.clone(),
                    rows: 24,
                    cols: 80,
                },
                factory(&fixture),
            )
            .await
            .expect("reconnect starts a fresh Core-owned resource generation");
        assert_eq!(reconnected.generation, first.generation + 1);
        assert!(matches!(
            fixture.actor.validate_fence(fence).await,
            Err(PluginTerminalSessionError::Conflict)
                | Err(PluginTerminalSessionError::InputNotAuthorized)
        ));
        let second = wait_for_running(&fixture.actor, reconnected.session_id).await;
        let mut events = fixture
            .actor
            .subscribe(second.session_id)
            .await
            .expect("subscribe reconnected session");
        let attachment = fixture
            .actor
            .get(second.session_id)
            .await
            .expect("reconnected session");
        let binding = fixture
            .actor
            .attach(
                crate::plugin_terminal_session_service::PluginTerminalAttachRequest {
                    operation_id: Uuid::new_v4(),
                    idempotency_key: "reconnect-attachment".to_owned(),
                    session_id: second.session_id,
                    expected_generation: second.generation,
                    expected_stream_id: second.stream_id,
                    expected_state_revision: attachment.state_revision,
                    attach_attempt_id: Uuid::new_v4(),
                    view_id: "reconnect-view".to_owned(),
                    after_output_sequence: None,
                },
            )
            .await
            .expect("second frontend attachment");
        let lease = fixture
            .actor
            .grant_input(PluginTerminalInputGrantRequest {
                session_id: second.session_id,
                expected_generation: second.generation,
                expected_stream_id: second.stream_id,
                attachment_id: binding.response.attachment.attachment_id,
                view_id: binding.response.attachment.view_id.clone(),
                expected_state_revision: binding.response.session.state_revision,
                focus_epoch: 2,
            })
            .await
            .expect("input lease for fresh generation");
        fixture
            .actor
            .input(PluginTerminalInputRequest {
                fence: PluginTerminalInputFence::from(&lease),
                client_sequence: 1,
                data: b"second input\n".to_vec(),
            })
            .await
            .expect("second generation input");
        assert_eq!(
            next_output(&mut events).await,
            b"\x1b[35msecond: \xe6\xad\xa3\xe5\xb8\xb8\x1b[0m\r\n"
        );

        let current = fixture
            .actor
            .get(second.session_id)
            .await
            .expect("second session");
        let closed = fixture
            .actor
            .disconnect(PluginTerminalDisconnectRequest {
                operation_id: Uuid::new_v4(),
                idempotency_key: "second-close".to_owned(),
                session_id: current.session_id,
                expected_generation: current.generation,
                expected_stream_id: current.stream_id,
                expected_state_revision: current.state_revision,
            })
            .await
            .expect("second resource cleanup");
        assert_eq!(closed.state, PluginTerminalSessionState::Closed);
        assert!(!has_owned_resources(&fixture.service, &fixture.owner));
        tokio::time::timeout(Duration::from_secs(3), peer)
            .await
            .expect("loopback peer finishes")
            .expect("loopback peer task")
            .expect("protocol bytes and close semantics");
        fixture
            .actor
            .shutdown_all()
            .await
            .expect("terminal actor shutdown");
        fixture
            .service
            .stop_plugin(&fixture.owner.plugin_id)
            .await
            .expect("plugin host cleanup");
    }

    async fn fixture(endpoint: &str) -> Fixture {
        let package = std::fs::read(PROTOCOL_DEMO_PACKAGE).expect("reviewed protocol-demo package");
        assert_eq!(
            hex::encode(Sha256::digest(&package)),
            PROTOCOL_DEMO_PACKAGE_SHA256,
            "the fixture must use the reviewed versioned protocol-demo package"
        );
        let directory = tempfile::tempdir().expect("fixture directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let lifecycle = LifecycleState::default();
        let operations = PluginOperationsService::new_test(hosts.clone(), lifecycle.clone());
        let service = PluginService::start(directory.path(), hosts, sessions)
            .expect("plugin service")
            .with_operations(operations);
        service.attach_test_lifecycle(lifecycle);
        let preview = service
            .prepare_local_package(Path::new(PROTOCOL_DEMO_PACKAGE), RequestId::new())
            .expect("production local package preparation");
        let permission = service
            .prepare_special_permission(PluginSpecialPermissionOpenRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                target: PluginSpecialPermissionTarget::PreparedPackage {
                    preparation_id: preview.preparation_id.clone(),
                    expected_package_sha256: preview.package_sha256.clone(),
                    expected_state_version: preview.current_state_version,
                },
                requested_capability: Some(PluginCapability::TerminalProvider),
            })
            .expect("open the real protected capability decision");
        let special_grants = permission
            .special_grants
            .iter()
            .map(|grant| PluginCapabilityGrant {
                capability: grant.capability,
                granted: true,
            })
            .collect();
        let approved_preview = match service
            .decide_special_permission(PluginSpecialPermissionDecisionRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                approval_id: permission.approval_id,
                decision: PluginApprovalDecision::Approve,
                expected_approval_state_version: permission.approval_state_version,
                special_grants,
                host_selections: Vec::new(),
            })
            .await
            .expect("approve declared provider and network capabilities")
            .target
        {
            PluginSpecialPermissionOutcome::PreparedPackage { preview } => preview,
            PluginSpecialPermissionOutcome::Installed { .. } => {
                panic!("prepared package decision changed target")
            }
        };
        let _operation = service
            .install_local_plugin(PluginLocalInstallRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: norishell_core_api::PluginOperationId::new(),
                idempotency_key: Uuid::new_v4().to_string(),
                preparation_id: approved_preview.preparation_id,
                expected_package_sha256: approved_preview.package_sha256,
                expected_state_version: approved_preview.current_state_version,
                capability_grants: approved_preview
                    .capabilities
                    .iter()
                    .copied()
                    .map(|capability| PluginCapabilityGrant {
                        capability,
                        granted: true,
                    })
                    .collect(),
            })
            .expect("install protocol-demo with its protected grants");
        let installed = service
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&preview.plugin_id)
            })
            .expect("installed protocol-demo record");
        let installed = service
            .set_plugin_state_convergent(RequestId::new(), &installed, PluginInstallState::Enabled)
            .expect("enable installed protocol-demo");
        let module = service
            .installer
            .read_active_module(
                &installed.plugin_id,
                &installed.active_version,
                &installed.package_sha256,
                16 * 1024 * 1024,
            )
            .expect("installed protocol-demo module");
        let executable = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("target/debug")
            .join(format!("norishell{}", std::env::consts::EXE_SUFFIX));
        assert!(
            executable.is_file(),
            "build target/debug/norishell before this fixture"
        );
        let locale = norishell_core_api::PluginLocale::parse("en").expect("fixture locale");
        let mut process = PluginHostProcess::spawn_with_executable_for_tests(
            &executable,
            &module,
            installed.state_version.get(),
        )
        .expect("production isolated plugin host");
        process
            .execute(norishell_core_api::PluginHostRequest {
                protocol_major: norishell_core_api::PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: norishell_core_api::PLUGIN_PROTOCOL_MINOR,
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
            .expect("initialize the real protocol guest");
        let owner = ResourceOwner {
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
                    signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
                    package_sha256: installed.package_sha256.clone(),
                    instance_generation: installed.state_version,
                    state_version: installed.state_version,
                    previously_crashed: false,
                    protocol_minor: norishell_core_api::PLUGIN_PROTOCOL_MINOR,
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
        let request = PluginNetworkStartRequest {
            timeout_ms: 5_000,
            credential: None,
            operation: PluginNetworkOperation::Tcp {},
        };
        let frozen = crate::plugin_api::network::prepare_endpoint(&PluginNetworkEndpointRequest {
            endpoint: endpoint.to_owned(),
        })
        .await
        .expect("freeze loopback TCP endpoint");
        let prepared = service
            .operation_policies
            .prepare(
                &installed,
                PluginCapability::NetworkDomain,
                current_plugin_permission_binding(&installed.package_sha256),
                PluginApprovalOperation::NetworkRequest,
                "protocol:framedTcp",
                &format!("{}:{}", frozen.host, frozen.port),
                &(
                    "protocol:framedTcp",
                    serde_json::json!({ "endpoint": frozen, "request": request, "credential": null }),
                ),
            )
            .expect("prepare exact remembered loopback network decision");
        service
            .operation_policies
            .remember(
                &prepared,
                norishell_core_api::PluginApprovalExpiry::Unlimited,
            )
            .expect("remember exact network decision without a test approval bypass");
        assert!(
            service
                .operation_policies
                .find(&prepared)
                .expect("read exact remembered policy")
                .is_some(),
            "the driver must enter Core through the remembered exact scope"
        );
        service
            .has_capability(
                RequestId::new(),
                &owner.plugin_id,
                &owner.signer,
                PluginCapability::TerminalProvider,
            )
            .expect("protected terminal-provider grant remains effective");
        service
            .has_capability(
                RequestId::new(),
                &owner.plugin_id,
                &owner.signer,
                PluginCapability::NetworkDomain,
            )
            .expect("protected network-domain grant remains effective");
        let actor = PluginTerminalSessionService::start();
        let provider = PluginTerminalProviderMetadata {
            plugin_id: installed.plugin_id.as_str().to_owned(),
            provider_id: "framedTcp".to_owned(),
            display_label: "Framed TCP demo".to_owned(),
        };
        Fixture {
            _directory: directory,
            service,
            actor,
            owner,
            provider,
            configuration: BTreeMap::from([(
                "endpoint".to_owned(),
                PluginSettingValue::String(endpoint.to_owned()),
            )]),
        }
    }

    fn factory(
        fixture: &Fixture,
    ) -> Arc<dyn crate::plugin_terminal_session_service::PluginTerminalDriverFactory> {
        Arc::new(ProtocolDriverFactory {
            service: fixture.service.clone(),
            binding: ProtocolBinding {
                owner: fixture.owner.clone(),
                consumer: ResourceConsumer {
                    connection: Uuid::new_v4(),
                    generation: fixture.owner.generation,
                    stream: Uuid::new_v4(),
                },
                connection_handle: Uuid::new_v4().to_string(),
                provider_id: fixture.provider.provider_id.clone(),
                resources: vec![PluginProtocolResource::Tcp],
                authority: Arc::new(|| true),
                transaction_authority: Arc::new(|| true),
            },
            configuration: fixture.configuration.clone(),
        })
    }

    async fn open(
        fixture: &Fixture,
        rows: u16,
        cols: u16,
    ) -> crate::plugin_terminal_session_service::PluginTerminalOpenBinding {
        fixture
            .actor
            .open_after_frontend_launch(
                PluginTerminalFrontendLaunchClaim::new(Uuid::new_v4()).expect("launch claim"),
                PluginTerminalOpenRequest {
                    operation_id: Uuid::new_v4(),
                    idempotency_key: "real-core-open".to_owned(),
                    connection_handle: PluginTerminalConnectionHandle::new(Uuid::new_v4())
                        .expect("Core-only connection handle"),
                    provider: fixture.provider.clone(),
                    attach_attempt_id: Uuid::new_v4(),
                    view_id: "protocol-demo-view".to_owned(),
                    rows,
                    cols,
                },
                factory(fixture),
            )
            .await
            .expect("start production Core protocol driver")
    }

    async fn wait_for_running(
        actor: &PluginTerminalSessionService,
        session_id: Uuid,
    ) -> crate::plugin_terminal_session_service::PluginTerminalSessionSummary {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let summary = actor.get(session_id).await.expect("protocol session");
                if summary.state == PluginTerminalSessionState::Running {
                    return summary;
                }
                assert_ne!(
                    summary.state,
                    PluginTerminalSessionState::Failed,
                    "driver failed: {summary:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("protocol session becomes running")
    }

    async fn grant_input(
        actor: &PluginTerminalSessionService,
        opened: &crate::plugin_terminal_session_service::PluginTerminalOpenResponse,
    ) -> crate::plugin_terminal_session_service::PluginTerminalInputLease {
        let current = actor
            .get(opened.session.session_id)
            .await
            .expect("running session");
        actor
            .grant_input(PluginTerminalInputGrantRequest {
                session_id: current.session_id,
                expected_generation: current.generation,
                expected_stream_id: current.stream_id,
                attachment_id: opened.attachment.attachment_id,
                view_id: opened.attachment.view_id.clone(),
                expected_state_revision: current.state_revision,
                focus_epoch: 1,
            })
            .await
            .expect("grant Core input lease")
    }

    async fn next_output(events: &mut broadcast::Receiver<PluginTerminalSessionEvent>) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let PluginTerminalSessionEvent::Output { frame, .. } =
                    events.recv().await.expect("protocol session event")
                {
                    return frame.data;
                }
            }
        })
        .await
        .expect("raw ANSI/UTF-8 output reaches terminal actor")
    }

    fn has_owned_resources(service: &PluginService, owner: &ResourceOwner) -> bool {
        service.api.resources.exit_blockers().iter().any(|blocker| {
            matches!(
                blocker,
                norishell_core_api::ExitBlocker::PluginResource { plugin_id, .. }
                    if plugin_id == &owner.plugin_id
            )
        })
    }

    async fn protocol_peer(listener: TcpListener) -> Result<(), String> {
        for sequence in 1..=2 {
            let (mut stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
            let (kind, payload) = read_frame(&mut stream).await?;
            let expected = if sequence == 1 {
                b"first input\n".as_slice()
            } else {
                b"second input\n".as_slice()
            };
            if kind != 0x01 || payload != expected {
                return Err(format!(
                    "connection {sequence} did not receive its input frame"
                ));
            }
            if sequence == 1 {
                let (kind, payload) = read_frame(&mut stream).await?;
                if kind != 0x02 || payload != [0, 33, 0, 101] {
                    return Err("resize frame was not big-endian 33x101".to_owned());
                }
                write_split(
                    &mut stream,
                    b"\x1b[32mfirst: \xe4\xb8\x96\xe7\x95\x8c\x1b[0m\r\n",
                )
                .await?;
                let mut eof = [0_u8; 1];
                if stream
                    .read(&mut eof)
                    .await
                    .map_err(|error| error.to_string())?
                    != 0
                {
                    return Err("first transport remained writable after Core close".to_owned());
                }
            } else {
                write_split(
                    &mut stream,
                    b"\x1b[35msecond: \xe6\xad\xa3\xe5\xb8\xb8\x1b[0m\r\n",
                )
                .await?;
                let mut eof = [0_u8; 1];
                if stream
                    .read(&mut eof)
                    .await
                    .map_err(|error| error.to_string())?
                    != 0
                {
                    return Err("second transport remained writable after Core close".to_owned());
                }
            }
        }
        Ok(())
    }

    async fn read_frame(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
        let mut length = [0_u8; 4];
        stream
            .read_exact(&mut length)
            .await
            .map_err(|error| error.to_string())?;
        let length = u32::from_be_bytes(length) as usize;
        if !(1..=32 * 1024).contains(&length) {
            return Err("invalid frame length".to_owned());
        }
        let mut body = vec![0_u8; length];
        stream
            .read_exact(&mut body)
            .await
            .map_err(|error| error.to_string())?;
        Ok((body[0], body[1..].to_vec()))
    }

    async fn write_split(stream: &mut TcpStream, output: &[u8]) -> Result<(), String> {
        let mut frame = Vec::with_capacity(output.len() + 5);
        frame.extend_from_slice(&((output.len() + 1) as u32).to_be_bytes());
        frame.push(0x81);
        frame.extend_from_slice(output);
        stream
            .write_all(&frame[..2])
            .await
            .map_err(|error| error.to_string())?;
        stream
            .write_all(&frame[2..5])
            .await
            .map_err(|error| error.to_string())?;
        stream
            .write_all(&frame[5..])
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}
