use super::*;

fn owner(plugin_id: &str, package: char, generation: u64) -> PluginResourceOwner {
    PluginResourceOwner {
        plugin_id: PluginId::parse(plugin_id.to_owned()).unwrap(),
        signer_fingerprint_sha256: "a".repeat(64),
        package_sha256: package.to_string().repeat(64),
        instance_generation: WireSequence::new(generation),
        capability_grant_epoch: WireSequence::new(7),
    }
}

#[test]
fn resource_scope_changes_with_every_owner_and_parent_generation_fence() {
    let base_owner = owner("org.norishell.logs", 'b', 1);
    let first = scope_key_parts(&base_owner, "ssh-session", 9, "parent-channel");
    let package_changed = scope_key_parts(
        &owner("org.norishell.logs", 'c', 1),
        "ssh-session",
        9,
        "parent-channel",
    );
    let generation_changed = scope_key_parts(
        &owner("org.norishell.logs", 'b', 2),
        "ssh-session",
        9,
        "parent-channel",
    );
    let grant_changed = scope_key_parts(
        &PluginResourceOwner {
            capability_grant_epoch: WireSequence::new(8),
            ..base_owner.clone()
        },
        "ssh-session",
        9,
        "parent-channel",
    );
    let session_changed = scope_key_parts(&base_owner, "other-ssh-session", 9, "parent-channel");
    let ssh_generation_changed = scope_key_parts(&base_owner, "ssh-session", 10, "parent-channel");
    let parent_channel_changed =
        scope_key_parts(&base_owner, "ssh-session", 9, "other-parent-channel");
    assert_ne!(first, package_changed);
    assert_ne!(first, generation_changed);
    assert_ne!(first, grant_changed);
    assert_ne!(first, session_changed);
    assert_ne!(first, ssh_generation_changed);
    assert_ne!(first, parent_channel_changed);
    assert!(resource_scope_matches(
        &first,
        "org.norishell.logs",
        Some(WireSequence::new(1))
    ));
    assert!(!resource_scope_matches(
        &first,
        "org.norishell.logs",
        Some(WireSequence::new(2))
    ));
    assert!(resource_scope_matches(&first, "org.norishell.logs", None));
    assert!(!resource_scope_matches(&first, "org.norishell.other", None));
}

#[test]
fn log_reads_are_absolute_unique_and_bounded() {
    assert!(
        validate_log_requests(&[PluginLogReadRequest {
            path: "/var/log/app.log".to_owned(),
            offset: None,
            max_bytes: 8192,
        }])
        .is_ok()
    );
    assert!(
        validate_log_requests(&[PluginLogReadRequest {
            path: "../secrets.log".to_owned(),
            offset: None,
            max_bytes: 8192,
        }])
        .is_err()
    );
    assert!(
        validate_log_requests(&[
            PluginLogReadRequest {
                path: "/var/log/app.log".to_owned(),
                offset: None,
                max_bytes: 8192,
            },
            PluginLogReadRequest {
                path: "/var/log/app.log".to_owned(),
                offset: Some(WireSequence::new(10)),
                max_bytes: 8192,
            },
        ])
        .is_err()
    );
}

#[test]
fn forward_rule_injects_the_core_resolved_host() {
    let session_id = "019d0000-0000-7000-8000-000000000001";
    let rule = forward_rule(
        session_id,
        &PluginForwardRule::Dynamic {
            local_bind_address: "127.0.0.1".to_owned(),
            local_listen_port: 1080,
        },
    )
    .unwrap();
    assert!(matches!(
        rule,
        PortForwardRule::Dynamic {
            host_ref,
            local_bind_address,
            local_listen_port: 1080,
        } if host_ref == session_id && local_bind_address == "127.0.0.1".parse::<IpAddr>().unwrap()
    ));
}

#[test]
fn log_text_removes_terminal_controls_and_marks_invalid_utf8_separately() {
    assert_eq!(safe_log_text(b"ok\n\x1b[31mbad\0\xff"), "ok\n[31mbad�");
    let invalid = vec![0xff];
    assert!(std::str::from_utf8(&invalid).is_err());
}

#[test]
fn navigation_intent_is_not_a_completed_navigation_fact() {
    let result = PluginResourceOperationResult::NavigationRequested {
        request: PluginHostNavigationRequest::Sftp {
            path: "/etc/nginx/nginx.conf".to_owned(),
            edit: true,
        },
    };
    let encoded = serde_json::to_value(result).unwrap();
    assert_eq!(encoded["kind"], "navigationRequested");
    assert!(encoded.get("completed").is_none());
}

#[tokio::test]
#[ignore = "requires the isolated NORISHELL_PLUGIN_RESOURCE_QA_STATE SSH fixture"]
async fn real_plugin_resources_share_one_parent_ssh_transport_and_cleanup_children() {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use norishell_core_api::{
        PluginActualForwardBind, PluginOwnedForwardState, SshChannelId, SshSessionEndpoint,
        SshSessionId, SshSessionTarget,
    };
    use norishell_ssh_domain::Endpoint;
    use norishell_ssh_transport::{
        Authentication, ConnectRequest, HostKeyDecision, HostKeyVerifier, ObservedHostKey, PtySize,
        ShellEvent, VerifiedTransport, VerifyFuture,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::timeout,
    };

    use crate::{
        forward_session_service::ForwardSessionService, host_service::HostService,
        ssh_agent_service::SshAgentService,
        transient_credential_service::TransientCredentialService, vault_service::VaultService,
    };

    struct FixtureVerifier;
    impl HostKeyVerifier for FixtureVerifier {
        fn verify(&self, _endpoint: Endpoint, _observed: ObservedHostKey) -> VerifyFuture {
            Box::pin(async { Ok(HostKeyDecision::Trusted) })
        }
    }

    async fn parent_marker(
        shell: &mut norishell_ssh_transport::RemoteShell<FixtureVerifier>,
        marker: &str,
    ) {
        shell
            .send_input(format!("printf '{marker}\\n'\n"))
            .await
            .unwrap();
        timeout(Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match shell.next_event().await.unwrap() {
                    ShellEvent::Data(data) | ShellEvent::ExtendedData { data, .. } => {
                        output.extend_from_slice(&data);
                        if String::from_utf8_lossy(&output).contains(marker) {
                            return;
                        }
                    }
                    ShellEvent::Eof | ShellEvent::Closed => panic!("parent PTY closed"),
                    _ => {}
                }
            }
        })
        .await
        .expect("parent PTY marker timeout");
    }

    async fn running_forward(
        resources: &PluginResourceService,
        invocation: &PluginResourceInvocation,
        handle: &str,
    ) -> PluginOwnedForwardSummary {
        timeout(Duration::from_secs(5), async {
            loop {
                let PluginResourceOperationResult::ForwardList { forwards } = resources
                    .invoke(invocation.clone(), PluginResourceOperation::ForwardList)
                    .await
                    .unwrap()
                else {
                    unreachable!();
                };
                if let Some(summary) = forwards
                    .into_iter()
                    .find(|forward| forward.forward_handle == handle)
                    && summary.state == PluginOwnedForwardState::Running
                {
                    return summary;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("forward did not reach Running")
    }

    let state_path = std::env::var("NORISHELL_PLUGIN_RESOURCE_QA_STATE")
        .expect("NORISHELL_PLUGIN_RESOURCE_QA_STATE");
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state_path).unwrap()).unwrap();
    let port = state["port"].as_u64().unwrap() as u16;
    let private_key = std::fs::read(state["key"].as_str().unwrap()).unwrap();
    let authenticated = VerifiedTransport::connect(
        ConnectRequest::new("127.0.0.1", port).unwrap(),
        Arc::new(FixtureVerifier),
    )
    .await
    .unwrap()
    .authenticate("root", Authentication::private_key(private_key, None))
    .await
    .unwrap();
    let channels = authenticated.shared_channels();
    let mut parent = authenticated
        .open_remote_shell(PtySize::new(80, 24, 0, 0).unwrap())
        .await
        .unwrap();
    parent_marker(&mut parent, "__RESOURCE_PARENT_INITIAL__").await;

    let directory = tempfile::tempdir().unwrap();
    let hosts = HostService::start(directory.path()).unwrap();
    let vault = VaultService::start(directory.path());
    let transient = TransientCredentialService::default();
    let agent = SshAgentService::default();
    let sftp = SftpSessionService::production(
        hosts.clone(),
        vault.clone(),
        transient.clone(),
        agent.clone(),
    );
    let forwards = ForwardSessionService::production(hosts, vault, transient, agent);
    let resources = PluginResourceService::new(sftp.clone(), forwards, LifecycleState::default());
    let (_parent_cancel, cancelled) = tokio::sync::watch::channel(false);
    let lease = SessionChannelLease {
        session_id: SshSessionId::new(),
        generation: WireSequence::new(1),
        parent_channel_id: SshChannelId::new(),
        target: SshSessionTarget::QuickConnect {
            endpoint: SshSessionEndpoint {
                address: "127.0.0.1".to_owned(),
                port,
                username: Some("root".to_owned()),
            },
        },
        endpoint: SshSessionEndpoint {
            address: "127.0.0.1".to_owned(),
            port,
            username: Some("root".to_owned()),
        },
        credential_ref_id: None,
        channels: channels.clone(),
        cancelled,
    };
    let plugin_lifetime_current = Arc::new(AtomicBool::new(true));
    let lifetime_fence = {
        let plugin_lifetime_current = plugin_lifetime_current.clone();
        Arc::new(move || plugin_lifetime_current.load(Ordering::Acquire))
    };
    let invocation = PluginResourceInvocation {
        owner: owner("org.norishell.resource-qa", 'b', 1),
        lease: lease.clone(),
        reason: "isolated same-parent resource QA".to_owned(),
        action_fence: Arc::new(|| true),
        lifetime_fence,
    };

    let (_setup_cancel, setup_cancelled) = tokio::sync::watch::channel(false);
    lease
        .channels
        .execute_capture_request(
            b"printf 'old-line\\nresource-log-marker\\n' > /tmp/norishell-resource-qa.log",
            None,
            Duration::from_secs(5),
            4096,
            || true,
            setup_cancelled,
        )
        .await
        .unwrap();

    assert!(matches!(
        resources
            .invoke(
                invocation.clone(),
                PluginResourceOperation::SftpOpen {
                    path: "/tmp/norishell-resource-qa.log".to_owned(),
                    edit: false,
                },
            )
            .await
            .unwrap(),
        PluginResourceOperationResult::NavigationRequested { .. }
    ));
    let navigation = resources
        .sftp_navigation_summary(&invocation)
        .await
        .unwrap();
    assert_eq!(
        navigation.parent_ssh_session.unwrap().session_id,
        lease.session_id
    );
    let PluginResourceOperationResult::LogsRead { files, .. } = resources
        .invoke(
            invocation.clone(),
            PluginResourceOperation::LogsRead {
                files: vec![PluginLogReadRequest {
                    path: "/tmp/norishell-resource-qa.log".to_owned(),
                    offset: None,
                    max_bytes: 1024,
                }],
            },
        )
        .await
        .unwrap()
    else {
        unreachable!();
    };
    assert!(files[0].text.contains("resource-log-marker"));
    parent_marker(&mut parent, "__RESOURCE_PARENT_AFTER_SFTP__").await;

    let PluginResourceOperationResult::ForwardStarted { forward } = resources
        .invoke(
            invocation.clone(),
            PluginResourceOperation::ForwardStart {
                rule: PluginForwardRule::Local {
                    local_bind_address: "127.0.0.1".to_owned(),
                    local_listen_port: 0,
                    remote_target_host: "127.0.0.1".to_owned(),
                    remote_target_port: 22,
                },
            },
        )
        .await
        .unwrap()
    else {
        unreachable!();
    };
    let local = running_forward(&resources, &invocation, &forward.forward_handle).await;
    let Some(PluginActualForwardBind::Local { address, port }) = local.actual_bind else {
        panic!("missing local bind");
    };
    let mut local_client = TcpStream::connect((address.as_str(), port)).await.unwrap();
    let mut banner = [0_u8; 4];
    timeout(Duration::from_secs(5), local_client.read_exact(&mut banner))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&banner, b"SSH-");
    drop(local_client);
    assert!(matches!(
        resources
            .invoke(
                invocation.clone(),
                PluginResourceOperation::ForwardStop {
                    forward_handle: forward.forward_handle,
                },
            )
            .await
            .unwrap(),
        PluginResourceOperationResult::ForwardStopped { forward }
            if !forward.cleanup_uncertain
    ));
    parent_marker(&mut parent, "__RESOURCE_PARENT_AFTER_LOCAL__").await;

    let target = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let target_port = target.local_addr().unwrap().port();
    let echo = tokio::spawn(async move {
        let (mut socket, _) = target.accept().await.unwrap();
        let mut payload = [0_u8; 11];
        socket.read_exact(&mut payload).await.unwrap();
        socket.write_all(&payload).await.unwrap();
    });
    let PluginResourceOperationResult::ForwardStarted { forward } = resources
        .invoke(
            invocation.clone(),
            PluginResourceOperation::ForwardStart {
                rule: PluginForwardRule::Remote {
                    remote_bind_address: "127.0.0.1".to_owned(),
                    remote_listen_port: 0,
                    local_target_host: "127.0.0.1".to_owned(),
                    local_target_port: target_port,
                },
            },
        )
        .await
        .unwrap()
    else {
        unreachable!();
    };
    let remote = running_forward(&resources, &invocation, &forward.forward_handle).await;
    let Some(PluginActualForwardBind::Remote { port, .. }) = remote.actual_bind else {
        panic!("missing remote bind");
    };
    let command = format!(
        "python3 -c 'import socket;s=socket.create_connection((\"127.0.0.1\",{port}));s.sendall(b\"remote-echo\");print(s.recv(11).decode())'"
    );
    let (_trigger_cancel, trigger_cancelled) = tokio::sync::watch::channel(false);
    let trigger = lease
        .channels
        .execute_capture_request(
            command.as_bytes(),
            None,
            Duration::from_secs(10),
            4096,
            || true,
            trigger_cancelled,
        )
        .await
        .unwrap();
    assert_eq!(trigger.exit_status, Some(0));
    assert!(String::from_utf8_lossy(&trigger.stdout).contains("remote-echo"));
    timeout(Duration::from_secs(5), echo)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        resources
            .invoke(
                invocation.clone(),
                PluginResourceOperation::ForwardStop {
                    forward_handle: forward.forward_handle,
                },
            )
            .await
            .unwrap(),
        PluginResourceOperationResult::ForwardStopped { forward }
            if !forward.cleanup_uncertain
    ));
    parent_marker(&mut parent, "__RESOURCE_PARENT_AFTER_REMOTE__").await;

    let (_fixture_cleanup, fixture_cleanup_cancelled) = tokio::sync::watch::channel(false);
    let cleanup = lease
        .channels
        .execute_capture_request(
            b"python3 -c 'from pathlib import Path; Path(\"/tmp/norishell-resource-qa.log\").unlink(missing_ok=True)'",
            None,
            Duration::from_secs(5),
            4096,
            || true,
            fixture_cleanup_cancelled,
        )
        .await
        .unwrap();
    assert_eq!(cleanup.exit_status, Some(0));

    // The combined plugin lease is cancelled after revocation, but that does
    // not terminate the user-owned parent transport. Child cleanup must stay
    // within its SFTP channel and the parent PTY must keep serving input.
    plugin_lifetime_current.store(false, Ordering::Release);
    resources
        .stop_plugin(&invocation.owner.plugin_id)
        .await
        .unwrap();
    assert!(resources.active_plugin_ids().is_empty());
    assert!(!lease.channels.is_closed());
    parent_marker(&mut parent, "__RESOURCE_PARENT_AFTER_REVOKE__").await;

    parent.disconnect().await.unwrap();
    timeout(Duration::from_secs(5), channels.wait_closed())
        .await
        .unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            let summaries = sftp.wire_summaries().await.unwrap();
            if summaries.iter().all(|summary| {
                matches!(
                    summary.state,
                    SftpSessionState::Closed | SftpSessionState::Failed
                )
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("parent disconnect did not close SFTP child");
    assert!(resources.active_plugin_ids().is_empty());
}
