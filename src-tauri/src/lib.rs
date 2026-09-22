mod algorithm_policy;
mod connection_profile;
mod connection_test_service;
mod core_api_error;
mod desktop_preferences;
mod desktop_service;
mod device_key_store;
mod forward_session_service;
mod host_service;
mod json_export;
mod lifecycle;
mod local_terminal_platform;
mod metrics_session_service;
mod native_notification_service;
mod native_notifications;
mod native_terminal;
mod native_terminal_scripts;
mod openssh_import_service;
mod overview_service;
mod plugin_api;
mod plugin_contribution;
mod plugin_credential_service;
mod plugin_extension_registry;
mod plugin_host_entry;
mod plugin_host_process;
mod plugin_host_protocol;
mod plugin_oauth;
mod plugin_operation_policy;
mod plugin_operations;
mod plugin_resources;
mod plugin_service;
mod plugin_task_service;
mod plugin_terminal_session_service;
#[cfg(unix)]
#[allow(dead_code)]
mod process_control;
mod release_check;
mod secure_credential_service;
mod secure_ssh_challenge;
mod secure_vault_service;
mod secure_window_frame;
mod sftp_session_service;
mod ssh_agent_service;
mod ssh_connection_orchestrator;
mod ssh_operation_ledger;
mod ssh_session_service;
mod ssh_sync_browser_cache;
mod ssh_sync_exchange;
mod ssh_sync_exchange_local;
#[allow(dead_code)]
mod telnet_session_service;
mod terminal_focus_broker;
mod time;
mod tool_window_exit;
mod tool_windows;
mod transient_credential_service;
mod tray_service;
mod vault_service;
mod window_frame;
#[cfg(windows)]
#[allow(dead_code)]
mod windows_process_control;

use tauri::{Emitter, Manager};

/// This is intentionally callable by the tiny binary entrypoint before
/// `run()` constructs any Tauri window, repository, Vault or network service.
pub fn run_plugin_host_if_requested() -> Option<i32> {
    plugin_host_entry::run_if_requested()
}

use lifecycle::{
    LifecycleState, application_menu, application_request_exit, handle_application_menu_event,
    hide_window_on_close, install_tray, window_request_close,
};

macro_rules! production_invoke_handler {
    ($builder:expr $(, $runtime_command:path)* $(,)?) => {
        $builder.invoke_handler(tauri::generate_handler![
            host_service::host_list,
            host_service::host_catalog_list,
            host_service::terminal_workspace_layout_get,
            host_service::terminal_workspace_layout_replace,
            host_service::host_create,
            host_service::host_configured_create,
            host_service::host_create_password_stage,
            host_service::host_create_password_cancel,
            host_service::host_update,
            host_service::host_delete,
            host_service::host_group_list,
            host_service::host_group_create,
            host_service::host_group_update,
            host_service::host_group_delete,
            host_service::host_tag_list,
            host_service::host_tag_create,
            host_service::host_tag_update,
            host_service::host_tag_delete,
            host_service::host_organization_get,
            host_service::host_organization_replace,
            host_service::host_favorite_update,
            host_service::recent_connection_list,
            host_service::host_connection_config_get,
            host_service::route_plan_replace,
            host_service::authentication_plan_replace,
            host_service::algorithm_policy_catalog_get,
            host_service::algorithm_policy_replace,
            host_service::heartbeat_policy_replace,
            host_service::monitoring_policy_replace,
            host_service::login_automation_replace,
            host_service::login_automation_confirm,
            host_service::login_automation_secret_create,
            host_service::login_automation_secret_cancel,
            plugin_service::plugin_installed_list,
            plugin_service::plugin_theme_list,
            plugin_service::plugin_readiness_get,
            plugin_service::plugin_audit_list,
            plugin_service::plugin_safe_mode_next_start,
            plugin_service::plugin_safe_mode_startup_complete,
            plugin_service::plugin_local_install,
            plugin_service::plugin_local_package_cancel,
            plugin_service::plugin_capability_grants_replace,
            plugin_service::plugin_enable,
            plugin_service::plugin_disable,
            plugin_service::plugin_locale_set,
            plugin_service::plugin_uninstall,
            plugin_service::plugin_operation_get,
            plugin_service::plugin_operation_cancel,
            plugin_service::plugin_operation_permissions_list,
            plugin_service::plugin_operation_permission_revoke,
            plugin_service::plugin_operation_permissions_clear,
            plugin_service::plugin_contribution_list,
            plugin_service::plugin_contribution_invoke,
            plugin_service::plugin_contribution_copy,
            plugin_service::plugin_extension_target_list,
            plugin_service::plugin_target_context_open,
            plugin_service::plugin_target_context_close,
            plugin_service::plugin_ui_contribution_list,
            plugin_service::plugin_ui_action,
            plugin_service::settings::plugin_settings_get,
            plugin_service::settings::plugin_settings_replace,
            plugin_service::settings::plugin_settings_reset,
            plugin_service::plugin_ssh_sync_browser_read,
            plugin_service::plugin_navigation_list,
            plugin_service::app_integration::plugin_app_integration_list,
            plugin_service::plugin_host_scope_list,
            plugin_service::plugin_host_scope_replace,
            plugin_service::plugin_terminal_input_pending_list,
            plugin_service::plugin_terminal_observe_attach,
            plugin_service::plugin_terminal_observe_detach,
            overview_service::server_overview_snapshot,
            metrics_session_service::metrics_reconcile,
            metrics_session_service::metrics_retry,
            metrics_session_service::metrics_stop,
            metrics_session_service::metrics_host_key_decide,
            metrics_session_service::metrics_keyboard_interactive_answer_prepare,
            metrics_session_service::metrics_keyboard_interactive_respond,
            host_service::identity_list,
            host_service::identity_create,
            host_service::identity_update,
            host_service::identity_delete_impact,
            host_service::identity_delete,
            host_service::credential_ref_list,
            host_service::credential_import,
            transient_credential_service::credential_transient_prepare,
            ssh_agent_service::ssh_agent_key_list,
            ssh_agent_service::ssh_agent_credential_create,
            host_service::keyboard_interactive_credential_create,
            openssh_import_service::openssh_config_preview,
            openssh_import_service::openssh_config_commit,
            host_service::known_host_list,
            host_service::known_host_delete,
            connection_test_service::ssh_connection_test,
            ssh_session_service::ssh_terminal_open,
            ssh_session_service::ssh_terminal_snapshot,
            ssh_session_service::ssh_terminal_get,
            ssh_session_service::ssh_terminal_attach,
            ssh_session_service::ssh_terminal_attachment_heartbeat,
            ssh_session_service::ssh_terminal_detach,
            ssh_session_service::ssh_terminal_host_key_decide,
            ssh_session_service::ssh_terminal_keyboard_interactive_answer_prepare,
            ssh_session_service::ssh_terminal_keyboard_interactive_respond,
            ssh_session_service::ssh_terminal_login_automation_takeover,
            ssh_session_service::ssh_terminal_input_focus_snapshot,
            ssh_session_service::ssh_terminal_input_focus_change,
            ssh_session_service::terminal_input_focus_snapshot,
            ssh_session_service::terminal_input_focus_change,
            ssh_session_service::ssh_terminal_input_lease_renew,
            ssh_session_service::ssh_terminal_input,
            ssh_session_service::ssh_terminal_resize,
            ssh_session_service::ssh_terminal_reconnect,
            ssh_session_service::ssh_terminal_disconnect,
            ssh_session_service::local_terminal_open,
            ssh_session_service::local_terminal_snapshot,
            ssh_session_service::local_terminal_get,
            ssh_session_service::local_terminal_attach,
            ssh_session_service::local_terminal_attachment_heartbeat,
            ssh_session_service::local_terminal_detach,
            ssh_session_service::local_terminal_input_lease_renew,
            ssh_session_service::local_terminal_input,
            ssh_session_service::local_terminal_resize,
            ssh_session_service::local_terminal_terminate,
            native_notification_service::native_notification_permission_get,
            native_notification_service::native_notification_permission_request,
            native_notification_service::native_notification_test,
            native_notification_service::native_notification_context_set,
            native_terminal::native_terminal_settings_get,
            native_terminal::native_terminal_settings_replace,
            native_terminal::native_terminal_enable,
            native_terminal::native_terminal_snapshot,
            native_terminal::native_terminal_history_list,
            native_terminal::native_terminal_history_delete,
            native_terminal::native_terminal_history_clear,
            native_terminal::native_terminal_history_pause,
            tray_service::tray_action_take,
            tray_service::tray_actions_ready,
            tray_service::panel::tray_panel_snapshot,
            tray_service::panel::tray_panel_hide,
            desktop_preferences::desktop_preferences_get,
            desktop_preferences::desktop_preferences_replace,
            release_check::release_check,
            telnet_session_service::telnet_terminal_open,
            telnet_session_service::telnet_terminal_snapshot,
            plugin_service::protocol_terminal::plugin_protocol_launch_list,
            plugin_service::protocol_terminal::plugin_protocol_launch_claim,
            plugin_service::protocol_terminal::plugin_terminal_open,
            plugin_service::protocol_terminal::plugin_terminal_snapshot,
            plugin_service::protocol_terminal::plugin_terminal_attach,
            plugin_service::protocol_terminal::plugin_terminal_attachment_heartbeat,
            plugin_service::protocol_terminal::plugin_terminal_detach,
            plugin_service::protocol_terminal::plugin_terminal_input_lease_renew,
            plugin_service::protocol_terminal::plugin_terminal_input,
            plugin_service::protocol_terminal::plugin_terminal_resize,
            plugin_service::protocol_terminal::plugin_terminal_disconnect,
            plugin_service::protocol_terminal::plugin_terminal_reconnect,
            telnet_session_service::telnet_terminal_attach,
            telnet_session_service::telnet_terminal_attachment_heartbeat,
            telnet_session_service::telnet_terminal_detach,
            telnet_session_service::telnet_terminal_input_lease_renew,
            telnet_session_service::telnet_terminal_input,
            telnet_session_service::telnet_terminal_resize,
            telnet_session_service::telnet_terminal_disconnect,
            telnet_session_service::telnet_terminal_reconnect,
            forward_session_service::forward_rule_list,
            forward_session_service::forward_rule_create,
            forward_session_service::forward_rule_update,
            forward_session_service::forward_rule_delete,
            forward_session_service::forward_rule_preflight,
            forward_session_service::forward_session_start,
            forward_session_service::forward_session_snapshot,
            forward_session_service::forward_session_stop,
            forward_session_service::forward_cleanup_retain,
            sftp_session_service::sftp_session_open,
            sftp_session_service::sftp_session_snapshot,
            sftp_session_service::sftp_session_disconnect,
            sftp_session_service::sftp_local_boundary_register,
            sftp_session_service::sftp_local_directory_register,
            sftp_session_service::sftp_local_directory_list,
            sftp_session_service::sftp_local_directory_open_child,
            sftp_session_service::sftp_local_directory_create_child,
            sftp_session_service::sftp_local_directory_release,
            sftp_session_service::sftp_directory_list,
            sftp_session_service::sftp_directory_list_cancel,
            sftp_session_service::sftp_file_preview,
            sftp_session_service::sftp_file_tail,
            sftp_session_service::sftp_file_mutate,
            sftp_session_service::sftp_transfer_enqueue,
            sftp_session_service::sftp_transfer_intent_prepare,
            sftp_session_service::sftp_transfer_intent_enqueue,
            sftp_session_service::sftp_transfer_intent_snapshot,
            sftp_session_service::sftp_transfer_intent_cancel,
            sftp_session_service::sftp_transfer_intent_cleanup_retry,
            sftp_session_service::sftp_transfer_intent_cleanup_retain,
            sftp_session_service::sftp_transfer_cancel,
            sftp_session_service::sftp_transfer_resume,
            sftp_session_service::sftp_remote_cleanup_retry,
            sftp_session_service::sftp_remote_cleanup_retain,
            vault_service::vault_status,
            vault_service::vault_create,
            vault_service::vault_unlock,
            vault_service::vault_lock,
            vault_service::vault_auto_unlock_enable,
            vault_service::vault_auto_unlock_disable,
            vault_service::vault_change_password
            $(, $runtime_command)*
        ])
    };
}

pub(crate) trait ProductionInvokeRuntime: tauri::Runtime + Sized {
    fn with_production_invoke_handler(builder: tauri::Builder<Self>) -> tauri::Builder<Self>;
}

impl ProductionInvokeRuntime for tauri::Wry {
    fn with_production_invoke_handler(builder: tauri::Builder<Self>) -> tauri::Builder<Self> {
        production_invoke_handler!(
            builder,
            desktop_service::desktop_profile_list,
            desktop_service::desktop_availability,
            desktop_service::desktop_profile_save,
            desktop_service::desktop_profile_delete,
            desktop_service::desktop_session_open,
            desktop_service::desktop_session_snapshot,
            desktop_service::desktop_session_disconnect,
            desktop_service::desktop_session_close,
            desktop_service::desktop_frame_get,
            desktop_service::desktop_focus_change,
            desktop_service::desktop_input,
            desktop_service::desktop_clipboard_get,
            desktop_service::desktop_audio_mute,
            desktop_service::desktop_prompt_get,
            desktop_service::desktop_prompt_decide,
            window_frame::window_native_controls_inset,
            window_frame::window_set_native_header_height,
            window_frame::window_set_windows_maximize_hit_region,
            plugin_service::plugin_local_package_prepare,
            plugin_service::plugin_host_approval_open,
            plugin_service::plugin_host_approval_get,
            plugin_service::plugin_host_approval_decide,
            plugin_operations::prompts::plugin_remote_approval_get,
            plugin_operations::prompts::plugin_remote_approval_decide,
            plugin_operations::prompts::plugin_credential_input_submit,
            plugin_service::plugin_special_permission_open,
            plugin_service::plugin_special_permission_get,
            plugin_service::plugin_special_permission_decide,
            plugin_service::plugin_terminal_input_open,
            plugin_service::plugin_terminal_input_get,
            plugin_service::plugin_terminal_input_decide,
            plugin_service::plugin_isolated_surface_content,
            plugin_service::isolated::plugin_isolated_bridge,
            ssh_sync_exchange_local::ssh_sync_secure_prompt_get,
            ssh_sync_exchange_local::ssh_sync_secure_prompt_decide,
            host_service::private_key_file_import,
            window_request_close,
            application_request_exit,
            json_export::native_json_export,
            secure_credential_service::secure_credential_open,
            secure_credential_service::secure_credential_get,
            secure_credential_service::secure_credential_submit,
            secure_credential_service::secure_credential_cancel,
            secure_ssh_challenge::secure_ssh_challenge_open,
            secure_ssh_challenge::secure_ssh_challenge_get,
            secure_ssh_challenge::secure_ssh_challenge_submit,
            secure_ssh_challenge::secure_ssh_challenge_cancel,
            secure_vault_service::secure_vault_open,
            secure_vault_service::secure_vault_ensure_for_host,
            secure_vault_service::secure_vault_get,
            secure_vault_service::secure_vault_submit,
            secure_vault_service::secure_vault_cancel,
            tool_window_exit::tool_window_exit_reply,
            tool_windows::tool_window_open,
            tool_windows::tool_window_get,
            tool_windows::tool_window_close,
            tool_windows::tool_window_changed,
            tool_windows::tool_file_preview,
            tool_windows::tool_file_tail,
            tool_windows::tool_file_save,
            tray_service::panel::tray_panel_execute,
        )
    }
}

#[cfg(test)]
impl ProductionInvokeRuntime for tauri::test::MockRuntime {
    fn with_production_invoke_handler(builder: tauri::Builder<Self>) -> tauri::Builder<Self> {
        // The mock runtime cannot extract the Wry-specific window/AppHandle
        // parameters of the four native lifecycle commands. The complete
        // runtime-neutral production registry, including every SSH wrapper,
        // remains shared with `run()` through the macro above.
        production_invoke_handler!(builder)
    }
}

pub(crate) fn with_production_invoke_handler<R: ProductionInvokeRuntime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    R::with_production_invoke_handler(builder)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .register_uri_scheme_protocol("norishell-plugin", |context, request| {
            let service = context
                .app_handle()
                .try_state::<plugin_service::PluginService>();
            plugin_service::isolated::document_response(
                service.as_deref(),
                context.webview_label(),
                request.uri().path(),
            )
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .menu(application_menu)
        .on_menu_event(handle_application_menu_event)
        .manage(LifecycleState::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            if let Some(main_window) = app.get_webview_window("main") {
                window_frame::install_macos_header_bridge(&main_window)
                    .map_err(std::io::Error::other)?;
            }
            #[cfg(windows)]
            if let Some(main_window) = app.get_webview_window("main") {
                window_frame::install_windows_caption_bridge(&main_window)
                    .map_err(std::io::Error::other)?;
            }
            let app_data_directory = app.path().app_data_dir()?;
            let host_service = host_service::HostService::start(&app_data_directory)
                .map_err(std::io::Error::other)?;
            let desktop_preferences_service =
                desktop_preferences::DesktopPreferencesService::new(host_service.clone());
            let vault_service = vault_service::VaultService::start(&app_data_directory);
            host_service
                .start_login_automation_secret_reconciler(vault_service.clone())
                .map_err(std::io::Error::other)?;
            let transient_credential_service =
                transient_credential_service::TransientCredentialService::default();
            let ssh_agent_service = ssh_agent_service::SshAgentService::default();
            let native_terminal_service = native_terminal::NativeTerminalService::start(
                &app_data_directory,
                vault_service.clone(),
            );
            let plugin_credential_service = plugin_credential_service::PluginCredentialService::new(
                host_service.clone(),
                vault_service.clone(),
            );
            plugin_credential_service
                .recover_startup()
                .map_err(std::io::Error::other)?;
            let terminal_availability = native_terminal_service.availability_observer();
            let credential_cleanup = plugin_credential_service.clone();
            vault_service.set_availability_observer(std::sync::Arc::new(move |availability| {
                terminal_availability(availability);
                if availability.available {
                    let service = credential_cleanup.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        // Tombstones remain durable if Vault cleanup cannot finish now.
                        let _ = service.cleanup_pending();
                    });
                }
            }));
            let ssh_session_service =
                ssh_session_service::SshSessionService::start_with_agent_and_native(
                    host_service.clone(),
                    vault_service.clone(),
                    transient_credential_service.clone(),
                    ssh_agent_service.clone(),
                    native_terminal_service.clone(),
                );
            let ssh_sync_local = ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter::new(
                app.handle().clone(),
                host_service.clone(),
                vault_service.clone(),
            );
            let ssh_sync_broker = ssh_sync_exchange::SshSyncExchangeBroker::new(
                &app_data_directory,
                vault_service.clone(),
                std::sync::Arc::new(ssh_sync_local.clone()),
                std::sync::Arc::new(ssh_sync_local.clone()),
            );
            let plugin_operations = plugin_operations::PluginOperationsService::new(
                host_service.clone(),
                app.state::<LifecycleState>().inner().clone(),
                app.handle().clone(),
            );
            let forward_session_service =
                forward_session_service::ForwardSessionService::production(
                    host_service.clone(),
                    vault_service.clone(),
                    transient_credential_service.clone(),
                    ssh_agent_service.clone(),
                );
            let sftp_session_service = sftp_session_service::SftpSessionService::production(
                host_service.clone(),
                vault_service.clone(),
                transient_credential_service.clone(),
                ssh_agent_service.clone(),
            );
            let plugin_resources = plugin_resources::PluginResourceService::new(
                sftp_session_service.clone(),
                forward_session_service.clone(),
                app.state::<LifecycleState>().inner().clone(),
            );
            let plugin_service = plugin_service::PluginService::start_with_sync(
                &app_data_directory,
                host_service.clone(),
                ssh_session_service.clone(),
                ssh_sync_broker,
            )
            .map_err(std::io::Error::other)?
            .with_operations(plugin_operations.clone())
            .with_resources(plugin_resources);
            let startup_plugins = plugin_service
                .enabled_plugins_for_startup()
                .map_err(std::io::Error::other)?;
            plugin_service.attach_app_handle(app.handle().clone());
            let workflow_service = plugin_service.workflow_service();
            workflow_service
                .recover_startup()
                .map_err(|error| std::io::Error::other(format!("workflow recovery: {error:?}")))?;
            app.manage(workflow_service);
            let metrics_session_service = metrics_session_service::MetricsSessionService::start(
                host_service.clone(),
                vault_service.clone(),
                transient_credential_service.clone(),
                ssh_agent_service.clone(),
            );
            let telnet_session_service = telnet_session_service::TelnetSessionService::start();
            let protocol_terminal =
                plugin_terminal_session_service::PluginTerminalSessionService::start();
            plugin_service.bind_protocol_terminals(protocol_terminal.clone());
            app.manage(protocol_terminal);
            let desktop_service = desktop_service::DesktopService::new(
                app.handle().clone(),
                host_service.clone(),
                vault_service.clone(),
                transient_credential_service.clone(),
                ssh_agent_service.clone(),
            );
            let desktop_focus_owner = desktop_service.clone();
            ssh_session_service
                .focus_broker()
                .register_desktop_invalidator(std::sync::Arc::new(move || {
                    desktop_focus_owner.invalidate_input()
                }));
            app.manage(desktop_service);
            app.manage(host_service);
            app.manage(tool_windows::ToolWindows::default());
            app.manage(tool_window_exit::ToolWindowExit::default());
            app.manage(secure_vault_service::SecureVaultService::default());
            app.manage(secure_credential_service::SecureCredentialService::default());
            app.manage(secure_ssh_challenge::SecureSshChallengeService::default());
            app.manage(desktop_preferences_service);
            app.manage(plugin_operations);
            app.manage(vault_service);
            app.manage(plugin_credential_service);
            app.manage(
                native_notification_service::NativeNotificationService::start(
                    app.handle().clone(),
                    native_terminal_service.clone(),
                ),
            );
            app.manage(native_terminal_service);
            app.manage(ssh_sync_local);
            app.manage(transient_credential_service);
            app.manage(ssh_agent_service);
            app.manage(openssh_import_service::OpenSshImportService::default());
            app.manage(ssh_session_service);
            app.manage(metrics_session_service);
            app.manage(telnet_session_service);
            app.manage(forward_session_service);
            app.manage(sftp_session_service);
            app.manage(plugin_service.clone());
            install_tray(app)?;
            if !startup_plugins.is_empty() {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    for installed in startup_plugins {
                        let service = app_handle.state::<plugin_service::PluginService>();
                        let request = norishell_core_api::PluginStateChangeRequest {
                            meta: norishell_core_api::RequestMeta {
                                request_id: norishell_core_api::RequestId::new(),
                            },
                            plugin_id: installed.plugin_id,
                            expected_state_version: installed.state_version,
                        };
                        let lifecycle = app_handle.state::<LifecycleState>();
                        let plugin_id = request.plugin_id.clone();
                        let expected_state_version = request.expected_state_version;
                        match plugin_service::plugin_enable(request, service, lifecycle).await {
                            Ok(plugin) => {
                                let _ = app_handle.emit_to("main", "plugin-runtime-ready", &plugin);
                            }
                            Err(error) => {
                                app_handle
                                    .state::<plugin_service::PluginService>()
                                    .record_startup_restore_failure(
                                        &plugin_id,
                                        expected_state_version,
                                        &error.code,
                                    );
                            }
                        }
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(target_os = "macos")]
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                window_frame::cleanup_macos_header_bridge();
            }
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::Focused(false))
                && let Some(service) = window.try_state::<desktop_service::DesktopService>()
            {
                service.invalidate_input();
            }
            hide_window_on_close(window, event);
        });
    let app = with_production_invoke_handler(builder)
        .build(tauri::generate_context!())
        .expect("NoriShell desktop runtime failed");

    app.run(|app, event| match event {
        tauri::RunEvent::Exit => {
            #[cfg(target_os = "macos")]
            window_frame::cleanup_macos_header_bridge();
            if let Some(service) = app.try_state::<tray_service::NativeTrayService>() {
                service.stop(app);
            }
            if let Some(service) =
                app.try_state::<native_notification_service::NativeNotificationService>()
            {
                service.stop();
            }
        }
        tauri::RunEvent::ExitRequested { api, .. } => {
            let lifecycle = app.state::<LifecycleState>();
            if !lifecycle.is_exit_authorized() {
                api.prevent_exit();
                lifecycle::request_application_exit(app);
            }
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => {
            if let Err(error) = lifecycle::show_main_window(app) {
                eprintln!("failed to reopen main window: {error}");
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod permission_tests {
    #[test]
    fn main_window_acl_allows_plugin_locale_synchronization() {
        let permission = include_str!("../permissions/app.toml");
        assert!(
            permission
                .lines()
                .any(|line| line.trim() == "\"plugin_locale_set\",")
        );
    }
}
