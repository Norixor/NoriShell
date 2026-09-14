use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_ssh_profile_sync::*;
use serde_json::{Value, json};
use uuid::Uuid;
use zeroize::Zeroizing;

fn id(value: u128) -> PortableObjectId {
    PortableObjectId::from_uuid(Uuid::from_u128(value)).unwrap()
}

fn sample_bundle() -> PortableBundleV1 {
    let host_id = id(1);
    let identity_id = id(2);
    let credential_id = id(3);
    let route_id = id(4);
    let auth_id = id(5);
    let algorithm_id = id(6);
    let heartbeat_id = id(7);
    let monitoring_id = id(8);
    let automation_id = id(9);
    let password_id = id(10);
    let private_key_id = id(11);
    let passphrase_id = id(12);
    let certificate_id = id(13);
    let login_automation_secret_id = id(14);

    PortableBundleV1 {
        schema: BundleSchema::V1,
        revision: 2,
        objects: PortableObjects {
            hosts: vec![PortableHost {
                id: host_id,
                label: "production".into(),
                address: "ssh.example.test".into(),
                port: 22,
                username: Some("nori".into()),
                favorite: true,
                tags: BTreeSet::from(["critical".into(), "linux".into()]),
                identity_id: Some(identity_id),
                route_id,
                authentication_plan_id: auth_id,
                algorithm_policy_id: algorithm_id,
                heartbeat_policy_id: heartbeat_id,
                monitoring_policy_id: monitoring_id,
                login_automation_id: Some(automation_id),
            }],
            desktop_profiles: Vec::new(),
            identities: vec![PortableIdentity {
                id: identity_id,
                label: "production identity".into(),
                username: Some("nori".into()),
                credential_ids: vec![credential_id],
            }],
            credentials: vec![PortableCredential {
                id: credential_id,
                identity_id,
                label: "selected password".into(),
                material: PortableCredentialMaterial::Password {
                    password_secret_id: password_id,
                },
            }],
            routes: vec![PortableRoute {
                id: route_id,
                ingress: RouteIngress::Direct,
                jump_hops: Vec::new(),
            }],
            authentication_plans: vec![PortableAuthenticationPlan {
                id: auth_id,
                credential_ids: vec![credential_id],
            }],
            algorithm_policies: vec![PortableAlgorithmPolicy {
                id: algorithm_id,
                policy_id: "secure-default".into(),
                compatibility_exceptions: Vec::new(),
            }],
            heartbeat_policies: vec![PortableHeartbeatPolicy {
                id: heartbeat_id,
                mode: HeartbeatMode::TransportKeepalive {
                    interval_seconds: 30,
                    reply_timeout_seconds: 10,
                    max_missed_replies: 3,
                },
            }],
            monitoring_policies: vec![PortableMonitoringPolicy {
                id: monitoring_id,
                enabled: true,
                interval_seconds: 30,
                timeout_seconds: 5,
                resources: BTreeSet::from([
                    MonitoringResource::Cpu,
                    MonitoringResource::Memory,
                    MonitoringResource::RootDisk,
                    MonitoringResource::AggregateNonLoopbackNetwork,
                ]),
            }],
            login_automations: vec![PortableLoginAutomation {
                id: automation_id,
                steps: vec![
                    LoginAutomationStep::Expect {
                        pattern: "$ ".into(),
                        timeout_seconds: 10,
                    },
                    LoginAutomationStep::SendText {
                        text: "sudo -S true".into(),
                        append_enter: true,
                        timeout_seconds: 10,
                    },
                    LoginAutomationStep::SendSecret {
                        secret_id: login_automation_secret_id,
                        secret_label: "sudo password".into(),
                        append_enter: true,
                        timeout_seconds: 10,
                    },
                ],
            }],
        },
        secrets: vec![
            PortableSecret {
                id: password_id,
                kind: PortableSecretKind::Password,
                selected_by_user: true,
                payload: SecretBytes::new(b"correct horse battery staple".to_vec()).unwrap(),
            },
            PortableSecret {
                id: private_key_id,
                kind: PortableSecretKind::PrivateKey,
                selected_by_user: true,
                payload: SecretBytes::new(b"-----BEGIN PRIVATE KEY-----".to_vec()).unwrap(),
            },
            PortableSecret {
                id: passphrase_id,
                kind: PortableSecretKind::Passphrase,
                selected_by_user: true,
                payload: SecretBytes::new(b"private key passphrase".to_vec()).unwrap(),
            },
            PortableSecret {
                id: certificate_id,
                kind: PortableSecretKind::Certificate,
                selected_by_user: true,
                payload: SecretBytes::new(b"ssh-ed25519-cert-v01@openssh.com ...".to_vec())
                    .unwrap(),
            },
            PortableSecret {
                id: login_automation_secret_id,
                kind: PortableSecretKind::LoginAutomation,
                selected_by_user: true,
                payload: SecretBytes::new(b"login automation secret".to_vec()).unwrap(),
            },
        ],
        skipped_machine_bound: vec![
            SkippedMachineBoundObject {
                id: id(20),
                kind: MachineBoundKind::SshAgent,
                reason: MachineBoundSkipReason::MachineBound,
            },
            SkippedMachineBoundObject {
                id: id(21),
                kind: MachineBoundKind::Fido,
                reason: MachineBoundSkipReason::MachineBound,
            },
            SkippedMachineBoundObject {
                id: id(22),
                kind: MachineBoundKind::HardwareKey,
                reason: MachineBoundSkipReason::MachineBound,
            },
        ],
        tombstones: Vec::new(),
    }
}

fn sample_desktop_bundle() -> PortableBundleV1 {
    let mut bundle = sample_bundle();
    bundle.schema = BundleSchema::V3;
    bundle
        .objects
        .desktop_profiles
        .push(PortableDesktopProfile {
            id: id(30),
            label: "production desktop".into(),
            protocol: PortableDesktopProtocol::Rdp,
            address: "desktop.example.test".into(),
            port: 3389,
            username: "nori".into(),
            domain: "EXAMPLE".into(),
            host_id: Some(id(1)),
            gateway_host_id: Some(id(1)),
            credential_id: Some(id(3)),
            width: 1_920,
            height: 1_080,
            clipboard_enabled: true,
            audio_playback_enabled: true,
        });
    bundle
}

#[test]
fn portable_host_configuration_can_explicitly_exclude_credentials() {
    let mut bundle = sample_bundle();
    bundle.objects.identities[0].credential_ids.clear();
    bundle.objects.credentials.clear();
    bundle.objects.authentication_plans[0]
        .credential_ids
        .clear();
    bundle.objects.hosts[0].login_automation_id = None;
    bundle.objects.login_automations.clear();
    bundle.secrets.clear();

    bundle.validate().expect("config-only portable bundle");
}

#[test]
fn v3_desktop_profile_roundtrips_without_local_or_secret_fields() {
    let bundle = sample_desktop_bundle();
    let canonical = canonical_bundle_bytes(&bundle).expect("canonical desktop bundle");
    let value: Value = serde_json::from_slice(canonical.as_slice()).expect("bundle JSON");
    assert_eq!(value["schema"], BUNDLE_SCHEMA_V3);
    let profile = &value["objects"]["desktopProfiles"][0];
    assert_eq!(profile["protocol"], "rdp");
    assert_eq!(profile["credentialId"], id(3).as_uuid().to_string());
    for forbidden in [
        "revision",
        "localId",
        "credentialRefId",
        "password",
        "runtimeSession",
        "certificateApproval",
    ] {
        assert!(profile.get(forbidden).is_none(), "unexpected {forbidden}");
    }
    let decoded = decode_bundle(Zeroizing::new(canonical.to_vec())).expect("decode desktop");
    assert_eq!(decoded, bundle);
    assert!(!format!("{bundle:?}").contains("correct horse battery staple"));
}

#[test]
fn desktop_profiles_require_v3_and_reject_dangling_or_unsafe_references() {
    for schema in [BundleSchema::V1, BundleSchema::V2] {
        let mut bundle = sample_desktop_bundle();
        bundle.schema = schema;
        assert!(matches!(
            bundle.validate(),
            Err(SyncCodecError::InvalidBundle(
                "desktop profiles require bundle v3"
            ))
        ));
    }

    let mut old_tombstone = sample_bundle();
    old_tombstone.schema = BundleSchema::V2;
    old_tombstone.tombstones.push(PortableTombstone {
        kind: PortableObjectKind::DesktopProfile,
        id: id(30),
    });
    assert!(matches!(
        old_tombstone.validate(),
        Err(SyncCodecError::InvalidBundle(
            "desktop profile tombstones require bundle v3"
        ))
    ));

    let mut missing_host = sample_desktop_bundle();
    missing_host.objects.desktop_profiles[0].host_id = Some(id(99));
    assert!(matches!(
        missing_host.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut missing_gateway = sample_desktop_bundle();
    missing_gateway.objects.desktop_profiles[0].gateway_host_id = Some(id(99));
    assert!(matches!(
        missing_gateway.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut missing_credential = sample_desktop_bundle();
    missing_credential.objects.desktop_profiles[0].credential_id = Some(id(99));
    assert!(matches!(
        missing_credential.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut non_password_credential = sample_desktop_bundle();
    non_password_credential.objects.credentials[0].material =
        PortableCredentialMaterial::KeyboardInteractive { max_rounds: 1 };
    assert!(matches!(
        non_password_credential.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut incomplete_identity = sample_desktop_bundle();
    incomplete_identity.objects.identities[0]
        .credential_ids
        .clear();
    assert!(matches!(
        incomplete_identity.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut missing_secret = sample_desktop_bundle();
    missing_secret.secrets.retain(|secret| secret.id != id(10));
    assert!(matches!(
        missing_secret.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));
}

#[test]
fn desktop_profile_validation_matches_runtime_endpoint_and_protocol_limits() {
    let mut invalid = sample_desktop_bundle();
    invalid.objects.desktop_profiles[0].address = "bad..desktop".into();
    assert!(invalid.validate().is_err());

    let mut vnc_audio = sample_desktop_bundle();
    vnc_audio.objects.desktop_profiles[0].protocol = PortableDesktopProtocol::Vnc;
    assert!(vnc_audio.validate().is_err());

    let mut invalid_size = sample_desktop_bundle();
    invalid_size.objects.desktop_profiles[0].width = 8_192;
    invalid_size.objects.desktop_profiles[0].height = 8_192;
    assert!(invalid_size.validate().is_err());
}

#[test]
fn login_automation_secret_requires_its_dedicated_secret_kind() {
    let mut bundle = sample_bundle();
    let automation_secret = bundle
        .secrets
        .iter_mut()
        .find(|secret| secret.id == id(14))
        .expect("automation secret");
    automation_secret.kind = PortableSecretKind::Password;
    assert!(matches!(
        bundle.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));
}

#[test]
fn login_automation_send_steps_preserve_and_bound_timeout_and_secret_label() {
    let bundle = sample_bundle();
    let value: Value = serde_json::from_slice(
        canonical_bundle_bytes(&bundle)
            .expect("canonical bundle")
            .as_slice(),
    )
    .expect("json");
    assert_eq!(
        value["objects"]["loginAutomations"][0]["steps"][1]["timeoutSeconds"],
        10
    );
    assert_eq!(
        value["objects"]["loginAutomations"][0]["steps"][2]["secretLabel"],
        "sudo password"
    );
    assert_eq!(
        value["objects"]["loginAutomations"][0]["steps"][2]["timeoutSeconds"],
        10
    );

    let mut invalid_timeout = sample_bundle();
    invalid_timeout.objects.login_automations[0].steps[1] = LoginAutomationStep::SendText {
        text: "true".into(),
        append_enter: true,
        timeout_seconds: 0,
    };
    assert!(matches!(
        invalid_timeout.validate(),
        Err(SyncCodecError::InvalidBundle("send text timeout"))
    ));

    let mut invalid_label = sample_bundle();
    invalid_label.objects.login_automations[0].steps[2] = LoginAutomationStep::SendSecret {
        secret_id: id(14),
        secret_label: "x".repeat(1_025),
        append_enter: true,
        timeout_seconds: 10,
    };
    assert!(matches!(
        invalid_label.validate(),
        Err(SyncCodecError::BoundExceeded("send secret label"))
    ));
}

#[test]
fn proxy_route_selects_an_exact_password_credential_even_when_accounts_share_a_secret() {
    let mut bundle = sample_bundle();
    let second_identity_id = id(30);
    let second_credential_id = id(31);
    bundle.objects.identities.push(PortableIdentity {
        id: second_identity_id,
        label: "proxy account".into(),
        username: Some("proxy-bob".into()),
        credential_ids: vec![second_credential_id],
    });
    bundle.objects.credentials.push(PortableCredential {
        id: second_credential_id,
        identity_id: second_identity_id,
        label: "proxy bob".into(),
        material: PortableCredentialMaterial::Password {
            password_secret_id: id(10),
        },
    });
    bundle.objects.routes[0].ingress = RouteIngress::HttpConnect {
        host: "proxy.example".into(),
        port: 8080,
        username: Some("proxy-bob".into()),
        credential_id: Some(second_credential_id),
    };
    bundle.validate().expect("shared proxy secret is valid");

    let value: Value = serde_json::from_slice(
        canonical_bundle_bytes(&bundle)
            .expect("canonical bundle")
            .as_slice(),
    )
    .expect("json");
    assert_eq!(
        value["objects"]["routes"][0]["ingress"]["credentialId"],
        second_credential_id.as_uuid().to_string()
    );
    assert!(
        value["objects"]["routes"][0]["ingress"]
            .get("passwordSecretId")
            .is_none()
    );

    let mut missing = bundle.clone();
    missing.objects.routes[0].ingress = RouteIngress::HttpConnect {
        host: "proxy.example".into(),
        port: 8080,
        username: Some("proxy-bob".into()),
        credential_id: Some(id(99)),
    };
    assert!(matches!(
        missing.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut wrong_type = bundle;
    wrong_type.objects.credentials[1].material =
        PortableCredentialMaterial::KeyboardInteractive { max_rounds: 3 };
    assert!(matches!(
        wrong_type.validate(),
        Err(SyncCodecError::DanglingObjectReference)
    ));
}

fn binding() -> SyncObjectBinding {
    SyncObjectBinding {
        application_id: "app.norishell.desktop".into(),
        account_id: "account-123".into(),
        schema: BundleSchema::V1,
        object_kind: SyncObjectKind::SshProfileBundle,
        revision: 2,
        base_revision: Some(1),
        base_etag: Some("etag-base-1".into()),
        key_version: 7,
    }
}

#[test]
fn canonical_roundtrip_and_digest_are_deterministic() {
    let bundle = sample_bundle();
    let canonical = canonical_bundle_bytes(&bundle).unwrap();
    let decoded = decode_bundle(Zeroizing::new(canonical.to_vec())).unwrap();
    assert_eq!(canonical_bundle_bytes(&decoded).unwrap(), canonical);

    let mut reordered = bundle.clone();
    reordered.secrets.reverse();
    reordered.skipped_machine_bound.reverse();
    assert_eq!(
        canonical_bundle_bytes(&reordered).unwrap(),
        canonical_bundle_bytes(&bundle).unwrap()
    );
    assert_eq!(
        canonical_bundle_digest(&reordered).unwrap(),
        canonical_bundle_digest(&bundle).unwrap()
    );
}

#[test]
fn encrypted_bundle_roundtrip_binds_owner_aad_key_and_revision() {
    let bundle = sample_bundle();
    let key = SyncKey::from_bytes([7; 32]);
    let envelope = encrypt_bundle(&bundle, &key, [9; 24], &binding()).unwrap();
    let opened = decrypt_bundle(&envelope, &key, &binding()).unwrap();
    assert_eq!(
        canonical_bundle_digest(&opened).unwrap(),
        canonical_bundle_digest(&bundle).unwrap()
    );

    let mut wrong_account = binding();
    wrong_account.account_id = "different-account".into();
    assert!(matches!(
        decrypt_bundle(&envelope, &key, &wrong_account),
        Err(SyncCodecError::BindingMismatch)
    ));

    let mut wrong_aad = binding();
    wrong_aad.base_etag = Some("different-etag".into());
    assert!(matches!(
        decrypt_bundle(&envelope, &key, &wrong_aad),
        Err(SyncCodecError::BindingMismatch)
    ));

    let mut wrong_revision = binding();
    wrong_revision.revision = 3;
    assert!(matches!(
        decrypt_bundle(&envelope, &key, &wrong_revision),
        Err(SyncCodecError::BindingMismatch)
    ));

    assert!(matches!(
        decrypt_bundle(&envelope, &SyncKey::from_bytes([8; 32]), &binding()),
        Err(SyncCodecError::AuthenticationFailed)
    ));
}

#[test]
fn encrypted_bundle_rejects_ciphertext_tampering() {
    let key = SyncKey::from_bytes([7; 32]);
    let envelope = encrypt_bundle(&sample_bundle(), &key, [9; 24], &binding()).unwrap();
    let mut value: Value = serde_json::from_slice(&envelope).unwrap();
    let encoded = value["ciphertext"].as_str().unwrap();
    let mut ciphertext = BASE64.decode(encoded).unwrap();
    ciphertext[0] ^= 1;
    value["ciphertext"] = Value::String(BASE64.encode(ciphertext));
    let tampered = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        decrypt_bundle(&tampered, &key, &binding()),
        Err(SyncCodecError::AuthenticationFailed)
    ));
}

#[test]
fn service_wire_roundtrip_matches_norixor_outer_fields() {
    let mut bundle = sample_bundle();
    bundle.revision = 1;
    let binding = SyncObjectBinding {
        application_id: "norishell".into(),
        account_id: "123".into(),
        schema: BundleSchema::V1,
        object_kind: SyncObjectKind::SshProfileBundle,
        revision: 1,
        base_revision: Some(0),
        base_etag: Some("0".repeat(64)),
        key_version: 1,
    };
    let sync_key = SyncKey::from_bytes([11; 32]);
    let encrypted = encrypt_bundle_for_service(&bundle, &sync_key, &binding).unwrap();
    assert_eq!(encrypted.algorithm, "xchacha20poly1305-ietf");
    assert_eq!(encrypted.nonce_base64url.len(), 32);
    assert_eq!(encrypted.ciphertext_sha256.len(), 64);
    let opened = decrypt_bundle_from_service(&encrypted, &sync_key, &binding).unwrap();
    assert_eq!(
        canonical_bundle_digest(&opened).unwrap(),
        canonical_bundle_digest(&bundle).unwrap()
    );

    let recovery_password = RecoveryPassword::new("independent recovery password").unwrap();
    let owner = RecoveryOwnerBinding {
        application_id: "norishell".into(),
        account_id: "123".into(),
        key_version: 1,
    };
    let recovery =
        create_recovery_envelope_for_service(&sync_key, &recovery_password, &owner).unwrap();
    assert_eq!(recovery.algorithm, "xchacha20poly1305-recovery-v1");
    assert_eq!(recovery.kdf, "argon2id-v1:m=65536,t=3,p=1,l=32");
    assert_eq!(
        recovery.aad,
        "norishell:ssh-recovery:v1|application=norishell|account=123|key_version=1"
    );
    let recovered =
        open_recovery_envelope_from_service(&recovery, &recovery_password, &owner).unwrap();
    let second = encrypt_bundle_for_service(&bundle, &recovered, &binding).unwrap();
    decrypt_bundle_from_service(&second, &sync_key, &binding).unwrap();

    let wrong_owner = RecoveryOwnerBinding {
        account_id: "124".into(),
        ..owner
    };
    assert!(
        open_recovery_envelope_from_service(&recovery, &recovery_password, &wrong_owner).is_err()
    );
}

#[test]
fn strict_schema_rejects_unknown_and_forbidden_fields() {
    let bytes = canonical_bundle_bytes(&sample_bundle()).unwrap();
    let mut unknown: Value = serde_json::from_slice(bytes.as_slice()).unwrap();
    unknown["unexpected"] = json!(true);
    assert!(decode_bundle(Zeroizing::new(serde_json::to_vec(&unknown).unwrap())).is_err());

    for forbidden in [
        "knownHosts",
        "vaultPassword",
        "vaultEnvelope",
        "autoUnlock",
        "oauthPkce",
        "privateKeyPath",
        "runtimeSession",
        "uiState",
        "pluginState",
    ] {
        let mut value: Value = serde_json::from_slice(bytes.as_slice()).unwrap();
        value["objects"]["hosts"][0][forbidden] = json!("must fail closed");
        assert!(
            decode_bundle(Zeroizing::new(serde_json::to_vec(&value).unwrap())).is_err(),
            "accepted forbidden field {forbidden}"
        );
    }
}

#[test]
fn encrypted_and_recovery_envelopes_reject_unknown_fields_and_profile_changes() {
    let key = SyncKey::from_bytes([7; 32]);
    let encrypted = encrypt_bundle(&sample_bundle(), &key, [9; 24], &binding()).unwrap();
    let mut sync_value: Value = serde_json::from_slice(&encrypted).unwrap();
    sync_value["vaultEnvelope"] = json!("forbidden");
    assert!(decrypt_bundle(&serde_json::to_vec(&sync_value).unwrap(), &key, &binding()).is_err());

    let password = RecoveryPassword::new("dedicated recovery password").unwrap();
    let owner = RecoveryOwnerBinding {
        application_id: "app.norishell.desktop".into(),
        account_id: "account-123".into(),
        key_version: 7,
    };
    let recovery = create_recovery_envelope(&key, &password, &owner, [3; 16], [4; 24]).unwrap();
    let mut unknown: Value = serde_json::from_slice(&recovery).unwrap();
    unknown["autoUnlock"] = json!(true);
    assert!(
        open_recovery_envelope(&serde_json::to_vec(&unknown).unwrap(), &password, &owner).is_err()
    );

    let mut changed_profile: Value = serde_json::from_slice(&recovery).unwrap();
    changed_profile["kdf"]["memoryKib"] = json!(8);
    assert!(matches!(
        open_recovery_envelope(
            &serde_json::to_vec(&changed_profile).unwrap(),
            &password,
            &owner
        ),
        Err(SyncCodecError::UnsupportedKdfProfile)
    ));
}

#[test]
fn schema_enforces_secret_selection_and_bounds() {
    let mut bundle = sample_bundle();
    bundle.secrets[0].selected_by_user = false;
    assert!(matches!(
        canonical_bundle_bytes(&bundle),
        Err(SyncCodecError::InvalidBundle(_))
    ));

    let mut bundle = sample_bundle();
    bundle.objects.hosts[0].label = "x".repeat(1_025);
    assert!(matches!(
        canonical_bundle_bytes(&bundle),
        Err(SyncCodecError::BoundExceeded("host label"))
    ));

    assert!(matches!(
        SecretBytes::new(vec![1; 16 * 1024 * 1024 + 1]),
        Err(SyncCodecError::BoundExceeded("secret payload"))
    ));

    let mut wrong_reference_kind = sample_bundle();
    wrong_reference_kind.objects.hosts[0].route_id = id(10);
    assert!(matches!(
        canonical_bundle_bytes(&wrong_reference_kind),
        Err(SyncCodecError::DanglingObjectReference)
    ));

    let mut wrong_secret_kind = sample_bundle();
    wrong_secret_kind.secrets[0].kind = PortableSecretKind::Certificate;
    assert!(matches!(
        canonical_bundle_bytes(&wrong_secret_kind),
        Err(SyncCodecError::DanglingObjectReference)
    ));
}

#[test]
fn recovery_envelope_roundtrip_uses_fixed_profile_and_owner_aad() {
    let bundle = sample_bundle();
    let sync_key = SyncKey::from_bytes([42; 32]);
    let password = RecoveryPassword::new("dedicated recovery password").unwrap();
    let owner = RecoveryOwnerBinding {
        application_id: "app.norishell.desktop".into(),
        account_id: "account-123".into(),
        key_version: 7,
    };
    let recovery =
        create_recovery_envelope(&sync_key, &password, &owner, [3; 16], [4; 24]).unwrap();
    let recovered = open_recovery_envelope(&recovery, &password, &owner).unwrap();
    let encrypted = encrypt_bundle(&bundle, &recovered, [5; 24], &binding()).unwrap();
    decrypt_bundle(&encrypted, &sync_key, &binding()).unwrap();

    let value: Value = serde_json::from_slice(&recovery).unwrap();
    assert_eq!(value["kdf"]["memoryKib"], 65_536);
    assert_eq!(value["kdf"]["iterations"], 3);
    assert_eq!(value["kdf"]["parallelism"], 1);
    assert_eq!(value["kdf"]["outputBytes"], 32);

    let mut wrong_owner = owner.clone();
    wrong_owner.account_id = "other-account".into();
    assert!(matches!(
        open_recovery_envelope(&recovery, &password, &wrong_owner),
        Err(SyncCodecError::BindingMismatch)
    ));
}

#[test]
fn secret_and_key_debug_output_is_redacted() {
    assert!(!format!("{:?}", SyncKey::from_bytes([1; 32])).contains('1'));
    assert!(
        !format!("{:?}", RecoveryPassword::new("recovery password").unwrap())
            .contains("recovery password")
    );
    assert!(!format!("{:?}", SecretBytes::new(b"private".to_vec()).unwrap()).contains("private"));
}
