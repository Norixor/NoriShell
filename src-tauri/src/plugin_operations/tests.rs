use super::*;

pub(super) fn fixture() -> (tempfile::TempDir, PluginOperationsService) {
    let directory = tempfile::tempdir().expect("private operation test directory");
    let service = PluginOperationsService {
        hosts: HostService::start(directory.path()).expect("Host repository"),
        lifecycle: LifecycleState::default(),
        app: None,
        state: Arc::new(Mutex::new(OperationState::default())),
        changed: Arc::new(Notify::new()),
    };
    (directory, service)
}

#[tokio::test]
async fn operation_registry_serializes_session_mutations_and_cancels_only_its_owner() {
    let (_directory, service) = fixture();
    let session = SshSessionId::new();
    let first = PluginId::parse("test.ops.first").unwrap();
    let second = PluginId::parse("test.ops.second").unwrap();
    let (first_guard, first_stop) = service
        .register(
            &PluginApprovalId::new(),
            &first,
            WireSequence::new(1),
            &session,
            true,
        )
        .expect("first mutation");
    assert!(
        service
            .register(
                &PluginApprovalId::new(),
                &second,
                WireSequence::new(1),
                &session,
                true
            )
            .is_err()
    );
    let (second_guard, second_stop) = service
        .register(
            &PluginApprovalId::new(),
            &second,
            WireSequence::new(1),
            &session,
            false,
        )
        .expect("parallel inspection");
    service.cancel_plugin(&first);
    assert!(*first_stop.borrow());
    assert!(!*second_stop.borrow());
    drop(first_guard);
    assert!(service.stop_plugin(&first).await);
    assert_eq!(service.active_plugin_ids(), vec![second]);
    drop(second_guard);
    assert!(service.active_plugin_ids().is_empty());
}

#[test]
fn registry_isolates_distinct_sessions_and_bounds_parallel_requests() {
    let (_directory, service) = fixture();
    let first = PluginId::parse("test.ops.first").unwrap();
    let second = PluginId::parse("test.ops.second").unwrap();
    let third = PluginId::parse("test.ops.third").unwrap();
    let session = SshSessionId::new();
    let (_first, _) = service
        .register(
            &PluginApprovalId::new(),
            &first,
            WireSequence::new(1),
            &session,
            true,
        )
        .unwrap();
    let (_second, _) = service
        .register(
            &PluginApprovalId::new(),
            &second,
            WireSequence::new(1),
            &session,
            false,
        )
        .unwrap();
    assert!(
        service
            .register(
                &PluginApprovalId::new(),
                &third,
                WireSequence::new(1),
                &session,
                false
            )
            .is_err()
    );
    assert!(
        service
            .register(
                &PluginApprovalId::new(),
                &first,
                WireSequence::new(1),
                &SshSessionId::new(),
                false
            )
            .is_err()
    );
    let (_third, _) = service
        .register(
            &PluginApprovalId::new(),
            &third,
            WireSequence::new(1),
            &SshSessionId::new(),
            true,
        )
        .expect("another session owns its mutation gate");
}

#[test]
fn delayed_generation_cleanup_cannot_cancel_a_restarted_plugin() {
    let (_directory, service) = fixture();
    let plugin = PluginId::parse("test.ops.restarted").unwrap();
    let (_guard, stop) = service
        .register(
            &PluginApprovalId::new(),
            &plugin,
            WireSequence::new(2),
            &SshSessionId::new(),
            false,
        )
        .unwrap();
    service.cancel_plugin_generation(&plugin, WireSequence::new(1));
    assert!(!*stop.borrow());
    service.cancel_plugin_generation(&plugin, WireSequence::new(2));
    assert!(*stop.borrow());
}

#[test]
fn output_projection_does_not_emit_terminal_control_bytes() {
    assert_eq!(
        safe_output(b"\x1b[31mvalue\x00\n\tend\x07"),
        "[31mvalue\n\tend"
    );
    assert_eq!(
        review_text("printf '\u{202e}hidden'"),
        "printf '\\u{202e}hidden'"
    );
}

#[test]
fn cpu_usage_projection_hides_raw_procfs_counters() {
    let id = PluginApprovalId::new();
    let mut result = failure(&id, PluginRemoteOperationState::Succeeded, "unavailable");
    result.stdout = "NORISHELL_CPU_STAT:first:cpu 1 2 3 4\n".to_owned();
    let data = project_success_data(
        ParsedOperationOutput::CpuUsage {
            basis_points: 5_000,
            sample_duration_ms: 200,
        },
        &mut result,
    );
    assert_eq!(result.stdout, "");
    assert_eq!(
        data,
        Some(PluginRemoteOperationData::CpuUsage {
            basis_points: 5_000,
            sample_duration_ms: 200,
        })
    );
}

#[test]
fn exit_gate_rejects_new_operation_registrations() {
    let (_directory, service) = fixture();
    service.lifecycle.authorize_exit();
    assert!(
        service
            .register(
                &PluginApprovalId::new(),
                &PluginId::parse("test.ops.exiting").unwrap(),
                WireSequence::new(1),
                &SshSessionId::new(),
                false
            )
            .is_err()
    );
    assert!(service.active_plugin_ids().is_empty());
}
