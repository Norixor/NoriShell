# Generated Tauri handler and main-window ACL directory

Generated from `production_invoke_handler!` in `src-tauri/src/lib.rs` and the trusted-main allowlist in `src-tauri/permissions/app.toml`. It lists **264 registered command handlers**. Registration does not grant plugin access.

| Handler | Stable Core symbol | Main-window ACL |
| --- | --- | --- |
| `host_list` | `COMMAND_HOST_LIST` | allowed |
| `host_catalog_list` | `COMMAND_HOST_CATALOG_LIST` | allowed |
| `terminal_workspace_layout_get` | `COMMAND_TERMINAL_WORKSPACE_LAYOUT_GET` | allowed |
| `terminal_workspace_layout_replace` | `COMMAND_TERMINAL_WORKSPACE_LAYOUT_REPLACE` | allowed |
| `host_create` | `COMMAND_HOST_CREATE` | allowed |
| `host_configured_create` | `COMMAND_HOST_CONFIGURED_CREATE` | allowed |
| `host_create_password_stage` | `COMMAND_HOST_CREATE_PASSWORD_STAGE` | not allowed |
| `host_create_password_cancel` | `COMMAND_HOST_CREATE_PASSWORD_CANCEL` | allowed |
| `host_update` | `COMMAND_HOST_UPDATE` | allowed |
| `host_delete` | `COMMAND_HOST_DELETE` | allowed |
| `host_group_list` | `COMMAND_HOST_GROUP_LIST` | allowed |
| `host_group_create` | `COMMAND_HOST_GROUP_CREATE` | allowed |
| `host_group_update` | `COMMAND_HOST_GROUP_UPDATE` | allowed |
| `host_group_delete` | `COMMAND_HOST_GROUP_DELETE` | allowed |
| `host_tag_list` | `COMMAND_HOST_TAG_LIST` | allowed |
| `host_tag_create` | `COMMAND_HOST_TAG_CREATE` | allowed |
| `host_tag_update` | `COMMAND_HOST_TAG_UPDATE` | allowed |
| `host_tag_delete` | `COMMAND_HOST_TAG_DELETE` | allowed |
| `host_organization_get` | `COMMAND_HOST_ORGANIZATION_GET` | allowed |
| `host_organization_replace` | `COMMAND_HOST_ORGANIZATION_REPLACE` | allowed |
| `host_favorite_update` | `COMMAND_HOST_FAVORITE_UPDATE` | allowed |
| `recent_connection_list` | `COMMAND_RECENT_CONNECTION_LIST` | allowed |
| `host_connection_config_get` | `COMMAND_HOST_CONNECTION_CONFIG_GET` | allowed |
| `route_plan_replace` | `COMMAND_ROUTE_PLAN_REPLACE` | allowed |
| `authentication_plan_replace` | `COMMAND_AUTHENTICATION_PLAN_REPLACE` | allowed |
| `algorithm_policy_catalog_get` | `COMMAND_ALGORITHM_POLICY_CATALOG_GET` | allowed |
| `algorithm_policy_replace` | `COMMAND_ALGORITHM_POLICY_REPLACE` | allowed |
| `heartbeat_policy_replace` | `COMMAND_HEARTBEAT_POLICY_REPLACE` | allowed |
| `monitoring_policy_replace` | `COMMAND_MONITORING_POLICY_REPLACE` | allowed |
| `login_automation_replace` | `COMMAND_LOGIN_AUTOMATION_REPLACE` | allowed |
| `login_automation_confirm` | `COMMAND_LOGIN_AUTOMATION_CONFIRM` | allowed |
| `login_automation_secret_create` | `COMMAND_LOGIN_AUTOMATION_SECRET_CREATE` | not allowed |
| `login_automation_secret_cancel` | `COMMAND_LOGIN_AUTOMATION_SECRET_CANCEL` | allowed |
| `plugin_installed_list` | `COMMAND_PLUGIN_INSTALLED_LIST` | allowed |
| `plugin_theme_list` | `COMMAND_PLUGIN_THEME_LIST` | allowed |
| `plugin_readiness_get` | `COMMAND_PLUGIN_READINESS_GET` | allowed |
| `plugin_audit_list` | `COMMAND_PLUGIN_AUDIT_LIST` | allowed |
| `plugin_safe_mode_next_start` | `COMMAND_PLUGIN_SAFE_MODE_NEXT_START` | allowed |
| `plugin_safe_mode_startup_complete` | `COMMAND_PLUGIN_SAFE_MODE_STARTUP_COMPLETE` | allowed |
| `plugin_local_install` | `COMMAND_PLUGIN_LOCAL_INSTALL` | allowed |
| `plugin_local_package_cancel` | `COMMAND_PLUGIN_LOCAL_PACKAGE_CANCEL` | allowed |
| `plugin_capability_grants_replace` | `COMMAND_PLUGIN_CAPABILITY_GRANTS_REPLACE` | allowed |
| `plugin_enable` | `COMMAND_PLUGIN_ENABLE` | allowed |
| `plugin_disable` | `COMMAND_PLUGIN_DISABLE` | allowed |
| `plugin_locale_set` | `COMMAND_PLUGIN_LOCALE_SET` | allowed |
| `plugin_uninstall` | `COMMAND_PLUGIN_UNINSTALL` | allowed |
| `plugin_operation_get` | `COMMAND_PLUGIN_OPERATION_GET` | allowed |
| `plugin_operation_cancel` | `COMMAND_PLUGIN_OPERATION_CANCEL` | allowed |
| `plugin_operation_permissions_list` | `COMMAND_PLUGIN_OPERATION_PERMISSIONS_LIST` | allowed |
| `plugin_operation_permission_revoke` | `COMMAND_PLUGIN_OPERATION_PERMISSION_REVOKE` | allowed |
| `plugin_operation_permissions_clear` | `COMMAND_PLUGIN_OPERATION_PERMISSIONS_CLEAR` | allowed |
| `plugin_contribution_list` | `COMMAND_PLUGIN_CONTRIBUTION_LIST` | allowed |
| `plugin_contribution_invoke` | `COMMAND_PLUGIN_CONTRIBUTION_INVOKE` | allowed |
| `plugin_contribution_copy` | `COMMAND_PLUGIN_CONTRIBUTION_COPY` | allowed |
| `plugin_extension_target_list` | `COMMAND_PLUGIN_EXTENSION_TARGET_LIST` | allowed |
| `plugin_target_context_open` | `COMMAND_PLUGIN_TARGET_CONTEXT_OPEN` | allowed |
| `plugin_target_context_close` | `COMMAND_PLUGIN_TARGET_CONTEXT_CLOSE` | allowed |
| `plugin_ui_contribution_list` | `COMMAND_PLUGIN_UI_CONTRIBUTION_LIST` | allowed |
| `plugin_ui_action` | `COMMAND_PLUGIN_UI_ACTION` | allowed |
| `plugin_settings_get` | `COMMAND_PLUGIN_SETTINGS_GET` | allowed |
| `plugin_settings_replace` | `COMMAND_PLUGIN_SETTINGS_REPLACE` | allowed |
| `plugin_settings_reset` | `COMMAND_PLUGIN_SETTINGS_RESET` | allowed |
| `plugin_ssh_sync_browser_read` | `COMMAND_PLUGIN_SSH_SYNC_BROWSER_READ` | allowed |
| `plugin_navigation_list` | `COMMAND_PLUGIN_NAVIGATION_LIST` | allowed |
| `plugin_app_integration_list` | `COMMAND_PLUGIN_APP_INTEGRATION_LIST` | allowed |
| `plugin_host_scope_list` | `COMMAND_PLUGIN_HOST_SCOPE_LIST` | allowed |
| `plugin_host_scope_replace` | `COMMAND_PLUGIN_HOST_SCOPE_REPLACE` | not allowed |
| `plugin_terminal_input_pending_list` | `COMMAND_PLUGIN_TERMINAL_INPUT_PENDING_LIST` | allowed |
| `plugin_terminal_observe_attach` | `COMMAND_PLUGIN_TERMINAL_OBSERVE_ATTACH` | allowed |
| `plugin_terminal_observe_detach` | `COMMAND_PLUGIN_TERMINAL_OBSERVE_DETACH` | allowed |
| `server_overview_snapshot` | `COMMAND_SERVER_OVERVIEW_SNAPSHOT` | allowed |
| `metrics_reconcile` | `COMMAND_METRICS_RECONCILE` | allowed |
| `metrics_retry` | `COMMAND_METRICS_RETRY` | allowed |
| `metrics_stop` | `COMMAND_METRICS_STOP` | allowed |
| `metrics_host_key_decide` | `COMMAND_METRICS_HOST_KEY_DECIDE` | not allowed |
| `metrics_keyboard_interactive_answer_prepare` | internal | not allowed |
| `metrics_keyboard_interactive_respond` | internal | not allowed |
| `identity_list` | `COMMAND_IDENTITY_LIST` | allowed |
| `identity_create` | `COMMAND_IDENTITY_CREATE` | allowed |
| `identity_update` | `COMMAND_IDENTITY_UPDATE` | allowed |
| `identity_delete_impact` | `COMMAND_IDENTITY_DELETE_IMPACT` | allowed |
| `identity_delete` | `COMMAND_IDENTITY_DELETE` | allowed |
| `credential_ref_list` | `COMMAND_CREDENTIAL_REF_LIST` | allowed |
| `credential_import` | `COMMAND_CREDENTIAL_IMPORT` | not allowed |
| `credential_transient_prepare` | `COMMAND_CREDENTIAL_TRANSIENT_PREPARE` | not allowed |
| `ssh_agent_key_list` | `COMMAND_SSH_AGENT_KEY_LIST` | allowed |
| `ssh_agent_credential_create` | `COMMAND_SSH_AGENT_CREDENTIAL_CREATE` | allowed |
| `keyboard_interactive_credential_create` | internal | allowed |
| `openssh_config_preview` | `COMMAND_OPENSSH_CONFIG_PREVIEW` | allowed |
| `openssh_config_commit` | `COMMAND_OPENSSH_CONFIG_COMMIT` | allowed |
| `known_host_list` | `COMMAND_KNOWN_HOST_LIST` | allowed |
| `known_host_delete` | `COMMAND_KNOWN_HOST_DELETE` | allowed |
| `ssh_connection_test` | `COMMAND_SSH_CONNECTION_TEST` | allowed |
| `ssh_terminal_open` | `COMMAND_SSH_TERMINAL_OPEN` | allowed |
| `ssh_terminal_snapshot` | `COMMAND_SSH_TERMINAL_SNAPSHOT` | allowed |
| `ssh_terminal_get` | `COMMAND_SSH_TERMINAL_GET` | allowed |
| `ssh_terminal_attach` | `COMMAND_SSH_TERMINAL_ATTACH` | allowed |
| `ssh_terminal_attachment_heartbeat` | `COMMAND_SSH_TERMINAL_ATTACHMENT_HEARTBEAT` | allowed |
| `ssh_terminal_detach` | `COMMAND_SSH_TERMINAL_DETACH` | allowed |
| `ssh_terminal_host_key_decide` | `COMMAND_SSH_TERMINAL_HOST_KEY_DECIDE` | not allowed |
| `ssh_terminal_keyboard_interactive_answer_prepare` | internal | not allowed |
| `ssh_terminal_keyboard_interactive_respond` | internal | not allowed |
| `ssh_terminal_login_automation_takeover` | internal | allowed |
| `ssh_terminal_input_focus_snapshot` | `COMMAND_SSH_TERMINAL_INPUT_FOCUS_SNAPSHOT` | allowed |
| `ssh_terminal_input_focus_change` | `COMMAND_SSH_TERMINAL_INPUT_FOCUS_CHANGE` | allowed |
| `terminal_input_focus_snapshot` | `COMMAND_TERMINAL_INPUT_FOCUS_SNAPSHOT` | allowed |
| `terminal_input_focus_change` | `COMMAND_TERMINAL_INPUT_FOCUS_CHANGE` | allowed |
| `ssh_terminal_input_lease_renew` | `COMMAND_SSH_TERMINAL_INPUT_LEASE_RENEW` | allowed |
| `ssh_terminal_input` | `COMMAND_SSH_TERMINAL_INPUT` | allowed |
| `ssh_terminal_resize` | `COMMAND_SSH_TERMINAL_RESIZE` | allowed |
| `ssh_terminal_reconnect` | `COMMAND_SSH_TERMINAL_RECONNECT` | allowed |
| `ssh_terminal_disconnect` | `COMMAND_SSH_TERMINAL_DISCONNECT` | allowed |
| `local_terminal_open` | `COMMAND_LOCAL_TERMINAL_OPEN` | allowed |
| `local_terminal_snapshot` | `COMMAND_LOCAL_TERMINAL_SNAPSHOT` | allowed |
| `local_terminal_get` | `COMMAND_LOCAL_TERMINAL_GET` | allowed |
| `local_terminal_attach` | `COMMAND_LOCAL_TERMINAL_ATTACH` | allowed |
| `local_terminal_attachment_heartbeat` | `COMMAND_LOCAL_TERMINAL_ATTACHMENT_HEARTBEAT` | allowed |
| `local_terminal_detach` | `COMMAND_LOCAL_TERMINAL_DETACH` | allowed |
| `local_terminal_input_lease_renew` | `COMMAND_LOCAL_TERMINAL_INPUT_LEASE_RENEW` | allowed |
| `local_terminal_input` | `COMMAND_LOCAL_TERMINAL_INPUT` | allowed |
| `local_terminal_resize` | `COMMAND_LOCAL_TERMINAL_RESIZE` | allowed |
| `local_terminal_terminate` | `COMMAND_LOCAL_TERMINAL_TERMINATE` | allowed |
| `native_notification_permission_get` | internal | allowed |
| `native_notification_permission_request` | internal | allowed |
| `native_notification_test` | internal | allowed |
| `native_notification_context_set` | internal | allowed |
| `native_terminal_settings_get` | `COMMAND_NATIVE_TERMINAL_SETTINGS_GET` | allowed |
| `native_terminal_settings_replace` | `COMMAND_NATIVE_TERMINAL_SETTINGS_REPLACE` | allowed |
| `native_terminal_enable` | `COMMAND_NATIVE_TERMINAL_ENABLE` | allowed |
| `native_terminal_snapshot` | `COMMAND_NATIVE_TERMINAL_SNAPSHOT` | allowed |
| `native_terminal_history_list` | `COMMAND_NATIVE_TERMINAL_HISTORY_LIST` | allowed |
| `native_terminal_history_delete` | `COMMAND_NATIVE_TERMINAL_HISTORY_DELETE` | allowed |
| `native_terminal_history_clear` | `COMMAND_NATIVE_TERMINAL_HISTORY_CLEAR` | allowed |
| `native_terminal_history_pause` | `COMMAND_NATIVE_TERMINAL_HISTORY_PAUSE` | allowed |
| `tray_action_take` | `COMMAND_TRAY_ACTION_TAKE` | allowed |
| `tray_actions_ready` | `COMMAND_TRAY_ACTIONS_READY` | allowed |
| `tray_panel_snapshot` | `COMMAND_TRAY_PANEL_SNAPSHOT` | not allowed |
| `tray_panel_hide` | `COMMAND_TRAY_PANEL_HIDE` | not allowed |
| `desktop_preferences_get` | `COMMAND_DESKTOP_PREFERENCES_GET` | allowed |
| `desktop_preferences_replace` | `COMMAND_DESKTOP_PREFERENCES_REPLACE` | allowed |
| `telnet_terminal_open` | `COMMAND_TELNET_TERMINAL_OPEN` | allowed |
| `telnet_terminal_snapshot` | `COMMAND_TELNET_TERMINAL_SNAPSHOT` | allowed |
| `plugin_protocol_launch_list` | `COMMAND_PLUGIN_PROTOCOL_LAUNCH_LIST` | allowed |
| `plugin_protocol_launch_claim` | `COMMAND_PLUGIN_PROTOCOL_LAUNCH_CLAIM` | allowed |
| `plugin_terminal_open` | `COMMAND_PLUGIN_TERMINAL_OPEN` | allowed |
| `plugin_terminal_snapshot` | `COMMAND_PLUGIN_TERMINAL_SNAPSHOT` | allowed |
| `plugin_terminal_attach` | `COMMAND_PLUGIN_TERMINAL_ATTACH` | allowed |
| `plugin_terminal_attachment_heartbeat` | internal | allowed |
| `plugin_terminal_detach` | `COMMAND_PLUGIN_TERMINAL_DETACH` | allowed |
| `plugin_terminal_input_lease_renew` | `COMMAND_PLUGIN_TERMINAL_INPUT_LEASE_RENEW` | allowed |
| `plugin_terminal_input` | `COMMAND_PLUGIN_TERMINAL_INPUT` | allowed |
| `plugin_terminal_resize` | `COMMAND_PLUGIN_TERMINAL_RESIZE` | allowed |
| `plugin_terminal_disconnect` | `COMMAND_PLUGIN_TERMINAL_DISCONNECT` | allowed |
| `plugin_terminal_reconnect` | `COMMAND_PLUGIN_TERMINAL_RECONNECT` | allowed |
| `telnet_terminal_attach` | `COMMAND_TELNET_TERMINAL_ATTACH` | allowed |
| `telnet_terminal_attachment_heartbeat` | internal | allowed |
| `telnet_terminal_detach` | `COMMAND_TELNET_TERMINAL_DETACH` | allowed |
| `telnet_terminal_input_lease_renew` | `COMMAND_TELNET_TERMINAL_INPUT_LEASE_RENEW` | allowed |
| `telnet_terminal_input` | `COMMAND_TELNET_TERMINAL_INPUT` | allowed |
| `telnet_terminal_resize` | `COMMAND_TELNET_TERMINAL_RESIZE` | allowed |
| `telnet_terminal_disconnect` | `COMMAND_TELNET_TERMINAL_DISCONNECT` | allowed |
| `telnet_terminal_reconnect` | `COMMAND_TELNET_TERMINAL_RECONNECT` | allowed |
| `forward_rule_list` | `COMMAND_FORWARD_RULE_LIST` | allowed |
| `forward_rule_create` | `COMMAND_FORWARD_RULE_CREATE` | allowed |
| `forward_rule_update` | `COMMAND_FORWARD_RULE_UPDATE` | allowed |
| `forward_rule_delete` | `COMMAND_FORWARD_RULE_DELETE` | allowed |
| `forward_rule_preflight` | `COMMAND_FORWARD_RULE_PREFLIGHT` | allowed |
| `forward_session_start` | `COMMAND_FORWARD_SESSION_START` | allowed |
| `forward_session_snapshot` | `COMMAND_FORWARD_SESSION_SNAPSHOT` | allowed |
| `forward_session_stop` | `COMMAND_FORWARD_SESSION_STOP` | allowed |
| `forward_cleanup_retain` | `COMMAND_FORWARD_CLEANUP_RETAIN` | allowed |
| `sftp_session_open` | `COMMAND_SFTP_SESSION_OPEN` | allowed |
| `sftp_session_snapshot` | `COMMAND_SFTP_SESSION_SNAPSHOT` | allowed |
| `sftp_session_disconnect` | `COMMAND_SFTP_SESSION_DISCONNECT` | allowed |
| `sftp_local_boundary_register` | `COMMAND_SFTP_LOCAL_BOUNDARY_REGISTER` | allowed |
| `sftp_local_directory_register` | `COMMAND_SFTP_LOCAL_DIRECTORY_REGISTER` | allowed |
| `sftp_local_directory_list` | `COMMAND_SFTP_LOCAL_DIRECTORY_LIST` | allowed |
| `sftp_local_directory_open_child` | `COMMAND_SFTP_LOCAL_DIRECTORY_OPEN_CHILD` | allowed |
| `sftp_local_directory_create_child` | `COMMAND_SFTP_LOCAL_DIRECTORY_CREATE_CHILD` | allowed |
| `sftp_local_directory_release` | `COMMAND_SFTP_LOCAL_DIRECTORY_RELEASE` | allowed |
| `sftp_directory_list` | `COMMAND_SFTP_DIRECTORY_LIST` | allowed |
| `sftp_directory_list_cancel` | `COMMAND_SFTP_DIRECTORY_LIST_CANCEL` | allowed |
| `sftp_file_preview` | `COMMAND_SFTP_FILE_PREVIEW` | allowed |
| `sftp_file_tail` | `COMMAND_SFTP_FILE_TAIL` | allowed |
| `sftp_file_mutate` | `COMMAND_SFTP_FILE_MUTATE` | allowed |
| `sftp_transfer_enqueue` | `COMMAND_SFTP_TRANSFER_ENQUEUE` | allowed |
| `sftp_transfer_intent_prepare` | `COMMAND_SFTP_TRANSFER_INTENT_PREPARE` | allowed |
| `sftp_transfer_intent_enqueue` | `COMMAND_SFTP_TRANSFER_INTENT_ENQUEUE` | allowed |
| `sftp_transfer_intent_snapshot` | `COMMAND_SFTP_TRANSFER_INTENT_SNAPSHOT` | allowed |
| `sftp_transfer_intent_cancel` | `COMMAND_SFTP_TRANSFER_INTENT_CANCEL` | allowed |
| `sftp_transfer_intent_cleanup_retry` | `COMMAND_SFTP_TRANSFER_INTENT_CLEANUP_RETRY` | allowed |
| `sftp_transfer_intent_cleanup_retain` | `COMMAND_SFTP_TRANSFER_INTENT_CLEANUP_RETAIN` | allowed |
| `sftp_transfer_cancel` | `COMMAND_SFTP_TRANSFER_CANCEL` | allowed |
| `sftp_transfer_resume` | `COMMAND_SFTP_TRANSFER_RESUME` | allowed |
| `sftp_remote_cleanup_retry` | `COMMAND_SFTP_REMOTE_CLEANUP_RETRY` | allowed |
| `sftp_remote_cleanup_retain` | `COMMAND_SFTP_REMOTE_CLEANUP_RETAIN` | allowed |
| `vault_status` | `COMMAND_VAULT_STATUS` | allowed |
| `vault_create` | `COMMAND_VAULT_CREATE` | not allowed |
| `vault_unlock` | `COMMAND_VAULT_UNLOCK` | not allowed |
| `vault_lock` | `COMMAND_VAULT_LOCK` | allowed |
| `vault_auto_unlock_enable` | `COMMAND_VAULT_AUTO_UNLOCK_ENABLE` | not allowed |
| `vault_auto_unlock_disable` | `COMMAND_VAULT_AUTO_UNLOCK_DISABLE` | allowed |
| `vault_change_password` | `COMMAND_VAULT_CHANGE_PASSWORD` | not allowed |
| `desktop_profile_list` | internal | allowed |
| `desktop_availability` | internal | allowed |
| `desktop_profile_save` | internal | allowed |
| `desktop_profile_delete` | internal | allowed |
| `desktop_session_open` | internal | allowed |
| `desktop_session_snapshot` | internal | allowed |
| `desktop_session_disconnect` | internal | allowed |
| `desktop_session_close` | internal | allowed |
| `desktop_frame_get` | internal | allowed |
| `desktop_focus_change` | internal | allowed |
| `desktop_input` | internal | allowed |
| `desktop_clipboard_get` | internal | allowed |
| `desktop_audio_mute` | internal | allowed |
| `desktop_prompt_get` | internal | not allowed |
| `desktop_prompt_decide` | internal | not allowed |
| `window_native_controls_inset` | internal | allowed |
| `window_set_native_header_height` | internal | allowed |
| `window_set_windows_maximize_hit_region` | internal | allowed |
| `plugin_local_package_prepare` | `COMMAND_PLUGIN_LOCAL_PACKAGE_PREPARE` | allowed |
| `plugin_host_approval_open` | `COMMAND_PLUGIN_HOST_APPROVAL_OPEN` | allowed |
| `plugin_host_approval_get` | `COMMAND_PLUGIN_HOST_APPROVAL_GET` | not allowed |
| `plugin_host_approval_decide` | `COMMAND_PLUGIN_HOST_APPROVAL_DECIDE` | not allowed |
| `plugin_remote_approval_get` | `COMMAND_PLUGIN_REMOTE_APPROVAL_GET` | not allowed |
| `plugin_remote_approval_decide` | `COMMAND_PLUGIN_REMOTE_APPROVAL_DECIDE` | not allowed |
| `plugin_credential_input_submit` | `COMMAND_PLUGIN_CREDENTIAL_INPUT_SUBMIT` | not allowed |
| `plugin_special_permission_open` | `COMMAND_PLUGIN_SPECIAL_PERMISSION_OPEN` | allowed |
| `plugin_special_permission_get` | `COMMAND_PLUGIN_SPECIAL_PERMISSION_GET` | not allowed |
| `plugin_special_permission_decide` | `COMMAND_PLUGIN_SPECIAL_PERMISSION_DECIDE` | not allowed |
| `plugin_terminal_input_open` | `COMMAND_PLUGIN_TERMINAL_INPUT_OPEN` | allowed |
| `plugin_terminal_input_get` | `COMMAND_PLUGIN_TERMINAL_INPUT_GET` | not allowed |
| `plugin_terminal_input_decide` | `COMMAND_PLUGIN_TERMINAL_INPUT_DECIDE` | not allowed |
| `plugin_isolated_surface_content` | `COMMAND_PLUGIN_ISOLATED_SURFACE_CONTENT` | not allowed |
| `plugin_isolated_bridge` | `COMMAND_PLUGIN_ISOLATED_BRIDGE` | not allowed |
| `ssh_sync_secure_prompt_get` | internal | not allowed |
| `ssh_sync_secure_prompt_decide` | internal | not allowed |
| `private_key_file_import` | `COMMAND_PRIVATE_KEY_FILE_IMPORT` | not allowed |
| `native_json_export` | internal | allowed |
| `secure_credential_open` | internal | allowed |
| `secure_credential_get` | internal | not allowed |
| `secure_credential_submit` | internal | not allowed |
| `secure_credential_cancel` | internal | not allowed |
| `secure_ssh_challenge_open` | internal | allowed |
| `secure_ssh_challenge_get` | internal | not allowed |
| `secure_ssh_challenge_submit` | internal | not allowed |
| `secure_ssh_challenge_cancel` | internal | not allowed |
| `secure_vault_open` | internal | allowed |
| `secure_vault_ensure_for_host` | internal | allowed |
| `secure_vault_get` | internal | not allowed |
| `secure_vault_submit` | internal | not allowed |
| `secure_vault_cancel` | internal | not allowed |
| `tool_window_exit_reply` | internal | not allowed |
| `tool_window_open` | internal | allowed |
| `tool_window_get` | internal | not allowed |
| `tool_window_close` | internal | not allowed |
| `tool_window_changed` | internal | not allowed |
| `tool_file_preview` | internal | not allowed |
| `tool_file_tail` | internal | not allowed |
| `tool_file_save` | internal | not allowed |
| `tray_panel_execute` | `COMMAND_TRAY_PANEL_EXECUTE` | not allowed |
| `window_request_close` | `COMMAND_WINDOW_REQUEST_CLOSE` | allowed |
| `application_request_exit` | `COMMAND_APPLICATION_REQUEST_EXIT` | allowed |
