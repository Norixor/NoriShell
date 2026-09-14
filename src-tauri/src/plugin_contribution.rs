use std::collections::{BTreeMap, BTreeSet};

use norishell_core_api::{
    PluginContributionNode, PluginContributionSlot, PluginHostDomOperationBatch,
    PluginHostMutationProposalRequest, PluginHostSessionRequest, PluginIsolatedSurfaceOpenRequest,
    PluginNavigationContribution, PluginPageContribution, PluginRuntimeOutput,
    PluginSshSyncRequest, PluginTerminalInputSuggestion, PluginUiActionId, PluginUiFieldValue,
    PluginUiNode, PluginUiTemplate,
};
use norishell_plugin_platform::{
    validate_plugin_dialog_document, validate_plugin_navigation, validate_plugin_page_document,
    validate_plugin_ui_document,
};

const UI_PANEL_OUTPUT_KIND: &str = "ui.panel";
const UI_DOCUMENT_OUTPUT_KIND: &str = "ui.document";
const CLIPBOARD_WRITE_OUTPUT_KIND: &str = "clipboard.write";
const UI_NAVIGATION_OUTPUT_KIND: &str = "ui.navigation";
const UI_PAGE_OUTPUT_KIND: &str = "ui.page";
const STORAGE_WRITE_OUTPUT_KIND: &str = "storage.write";
const HOST_MUTATION_OUTPUT_KIND: &str = "host.mutation.propose";
const HOST_SESSION_OUTPUT_KIND: &str = "host.session.request";
const HOST_DOM_OPERATIONS_OUTPUT_KIND: &str = "ui.hostDom.operations";
const TERMINAL_PROPOSE_INPUT_OUTPUT_KIND: &str = "terminal.proposeInput";
const ISOLATED_SURFACE_OPEN_OUTPUT_KIND: &str = "ui.webview.open";
const SSH_SYNC_REQUEST_OUTPUT_KIND: &str = "ssh.sync.request";
const MAX_CONTRIBUTION_NODES: usize = 64;
const MAX_CONTRIBUTION_TEXT_BYTES: usize = 2_048;
const MAX_COPY_TEXT_BYTES: usize = 512;
const MAX_CLIPBOARD_WRITE_BYTES: usize = 16 * 1024;
const MAX_PLUGIN_STORAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlottedContribution {
    slot: PluginContributionSlot,
    nodes: Vec<PluginContributionNode>,
    #[serde(default)]
    clipboard_values: Vec<ClipboardValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClipboardValue {
    copy_id: String,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPluginContribution {
    pub(crate) slot: PluginContributionSlot,
    pub(crate) nodes: Vec<PluginContributionNode>,
    pub(crate) clipboard_values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPluginUiOutputs {
    pub(crate) panels: Vec<ParsedPluginContribution>,
    pub(crate) templates: Vec<PluginUiTemplate>,
    pub(crate) clipboard_text: Option<String>,
    pub(crate) navigation: Vec<PluginNavigationContribution>,
    pub(crate) pages: Vec<PluginPageContribution>,
    pub(crate) storage_write: Option<PluginStorageWrite>,
    pub(crate) host_mutation: Option<PluginHostMutationProposalRequest>,
    pub(crate) host_session: Option<PluginHostSessionRequest>,
    pub(crate) host_dom_operations: Option<PluginHostDomOperationBatch>,
    pub(crate) terminal_input_suggestion: Option<PluginTerminalInputSuggestion>,
    pub(crate) isolated_surface: Option<PluginIsolatedSurfaceOpenRequest>,
    pub(crate) ssh_sync_request: Option<PluginSshSyncRequest>,
    pub(crate) remote_operation: Option<norishell_core_api::PluginRemoteOperationRequest>,
    pub(crate) resource_operation: Option<norishell_core_api::PluginResourceOperationRequest>,
    pub(crate) api_call: Option<norishell_core_api::PluginApiCall>,
    /// Created only after Core admission, never parsed from guest output.
    pub(crate) terminal_launch: Option<norishell_core_api::PluginApprovedTerminalChannelLaunch>,
    pub(crate) resource_navigation: Option<norishell_core_api::PluginHostNavigationEvent>,
    pub(crate) ui_state: Option<PluginUiState>,
}

impl ParsedPluginUiOutputs {
    pub(crate) fn has_operation_broker_request(&self) -> bool {
        self.remote_operation.is_some()
            || self.resource_operation.is_some()
            || self.api_call.is_some()
    }

    /// A broker result supplies the replacement document later. The existing page stays
    /// visible while a document-free request runs.
    pub(crate) fn valid_initial_document_count(&self) -> bool {
        self.templates.len() == 1
            || (self.has_operation_broker_request() && self.templates.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PluginUiState {
    pub(crate) value_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PluginStorageWrite {
    pub(crate) write_token: String,
    pub(crate) value_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClipboardWritePayload {
    text: String,
}

/// Partitions initialization/action output by its declared capability while
/// rejecting unknown output kinds. Each branch applies its own strict parser.
pub(crate) fn parse_plugin_ui_outputs(
    request_id: &str,
    outputs: Vec<PluginRuntimeOutput>,
) -> Result<ParsedPluginUiOutputs, ()> {
    let mut panel_outputs = Vec::new();
    let mut document_outputs = Vec::new();
    let mut clipboard_text = None;
    let mut navigation = Vec::new();
    let mut pages = Vec::new();
    let mut storage_write = None;
    let mut host_mutation = None;
    let mut host_session = None;
    let mut host_dom_operations = None;
    let mut terminal_input_suggestion = None;
    let mut isolated_surface = None;
    let mut ssh_sync_request = None;
    let mut remote_operation = None;
    let mut resource_operation = None;
    let mut api_call = None;
    let mut ui_state = None;
    for output in outputs {
        match output.kind.as_str() {
            "api.request" => {
                if output.request_id != request_id
                    || api_call.is_some()
                    || output.payload_json.len() > crate::plugin_api::MAX_CALL_BYTES as usize
                {
                    return Err(());
                }
                let call =
                    serde_json::from_str::<norishell_core_api::PluginApiCall>(&output.payload_json)
                        .map_err(|_| ())?;
                crate::plugin_api::validate_call(&call).map_err(|_| ())?;
                api_call = Some(call);
            }
            "resource.operation.request" => {
                if output.request_id != request_id
                    || resource_operation.is_some()
                    || remote_operation.is_some()
                    || output.payload_json.len() > 64 * 1024
                {
                    return Err(());
                }
                let request = serde_json::from_str::<
                    norishell_core_api::PluginResourceOperationRequest,
                >(&output.payload_json)
                .map_err(|_| ())?;
                if !valid_host_reason(&request.reason) {
                    return Err(());
                }
                resource_operation = Some(request);
            }
            "ui.state" => {
                if output.request_id != request_id || ui_state.is_some() {
                    return Err(());
                }
                let value =
                    serde_json::from_str::<PluginUiState>(&output.payload_json).map_err(|_| ())?;
                if value.value_json.len() > 64 * 1024
                    || !serde_json::from_str::<serde_json::Value>(&value.value_json)
                        .is_ok_and(|value| value.is_object())
                {
                    return Err(());
                }
                ui_state = Some(value);
            }
            "remote.operation.request" => {
                if output.request_id != request_id
                    || remote_operation.is_some()
                    || resource_operation.is_some()
                {
                    return Err(());
                }
                let request = serde_json::from_str::<
                    norishell_core_api::PluginRemoteOperationRequest,
                >(&output.payload_json)
                .map_err(|_| ())?;
                if request.terminal_handle.len() != 64
                    || !request
                        .terminal_handle
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    || request.operation_json.is_empty()
                    || request.operation_json.len() > 64 * 1024
                    || !valid_host_reason(&request.reason)
                {
                    return Err(());
                }
                remote_operation = Some(request);
            }
            UI_PANEL_OUTPUT_KIND => panel_outputs.push(output),
            UI_DOCUMENT_OUTPUT_KIND => document_outputs.push(output),
            CLIPBOARD_WRITE_OUTPUT_KIND => {
                if output.request_id != request_id || clipboard_text.is_some() {
                    return Err(());
                }
                let payload = serde_json::from_str::<ClipboardWritePayload>(&output.payload_json)
                    .map_err(|_| ())?;
                if !valid_clipboard_write_text(&payload.text) {
                    return Err(());
                }
                clipboard_text = Some(payload.text);
            }
            UI_NAVIGATION_OUTPUT_KIND => {
                if output.request_id != request_id || navigation.len() >= 16 {
                    return Err(());
                }
                navigation.push(
                    serde_json::from_str::<PluginNavigationContribution>(&output.payload_json)
                        .map_err(|_| ())?,
                );
            }
            UI_PAGE_OUTPUT_KIND => {
                if output.request_id != request_id || pages.len() >= 16 {
                    return Err(());
                }
                pages.push(
                    serde_json::from_str::<PluginPageContribution>(&output.payload_json)
                        .map_err(|_| ())?,
                );
            }
            STORAGE_WRITE_OUTPUT_KIND => {
                if output.request_id != request_id || storage_write.is_some() {
                    return Err(());
                }
                let write = serde_json::from_str::<PluginStorageWrite>(&output.payload_json)
                    .map_err(|_| ())?;
                if write.write_token.len() != 36
                    || !write.write_token.is_ascii()
                    || write.value_json.len() > MAX_PLUGIN_STORAGE_BYTES
                    || !serde_json::from_str::<serde_json::Value>(&write.value_json)
                        .is_ok_and(|value| value.is_object())
                {
                    return Err(());
                }
                storage_write = Some(write);
            }
            HOST_MUTATION_OUTPUT_KIND => {
                if output.request_id != request_id
                    || host_mutation.is_some()
                    || host_session.is_some()
                {
                    return Err(());
                }
                let proposal =
                    serde_json::from_str::<PluginHostMutationProposalRequest>(&output.payload_json)
                        .map_err(|_| ())?;
                if !valid_host_reason(&proposal.reason) || !valid_host_mutation(&proposal.patch) {
                    return Err(());
                }
                host_mutation = Some(proposal);
            }
            HOST_SESSION_OUTPUT_KIND => {
                if output.request_id != request_id
                    || host_session.is_some()
                    || host_mutation.is_some()
                {
                    return Err(());
                }
                let proposal =
                    serde_json::from_str::<PluginHostSessionRequest>(&output.payload_json)
                        .map_err(|_| ())?;
                if !valid_host_reason(&proposal.reason) {
                    return Err(());
                }
                host_session = Some(proposal);
            }
            HOST_DOM_OPERATIONS_OUTPUT_KIND => {
                if output.request_id != request_id || host_dom_operations.is_some() {
                    return Err(());
                }
                host_dom_operations = Some(
                    serde_json::from_str::<PluginHostDomOperationBatch>(&output.payload_json)
                        .map_err(|_| ())?,
                );
            }
            TERMINAL_PROPOSE_INPUT_OUTPUT_KIND => {
                if output.request_id != request_id || terminal_input_suggestion.is_some() {
                    return Err(());
                }
                let suggestion =
                    serde_json::from_str::<PluginTerminalInputSuggestion>(&output.payload_json)
                        .map_err(|_| ())?;
                if suggestion.text.is_empty()
                    || suggestion.text.len() > 8 * 1024
                    || suggestion.text.chars().any(|character| {
                        character.is_control() && !matches!(character, '\t' | '\n')
                    })
                    || suggestion.description.as_ref().is_some_and(|value| {
                        value.trim().is_empty()
                            || value.len() > 512
                            || value.chars().any(char::is_control)
                    })
                {
                    return Err(());
                }
                terminal_input_suggestion = Some(suggestion);
            }
            ISOLATED_SURFACE_OPEN_OUTPUT_KIND => {
                if output.request_id != request_id || isolated_surface.is_some() {
                    return Err(());
                }
                let surface =
                    serde_json::from_str::<PluginIsolatedSurfaceOpenRequest>(&output.payload_json)
                        .map_err(|_| ())?;
                if !valid_action_id(&surface.surface_id)
                    || !valid_action_label(&surface.title)
                    || !(480..=1_600).contains(&surface.width)
                    || !(360..=1_200).contains(&surface.height)
                {
                    return Err(());
                }
                isolated_surface = Some(surface);
            }
            SSH_SYNC_REQUEST_OUTPUT_KIND => {
                if output.request_id != request_id || ssh_sync_request.is_some() {
                    return Err(());
                }
                ssh_sync_request = Some(
                    serde_json::from_str::<PluginSshSyncRequest>(&output.payload_json)
                        .map_err(|_| ())?,
                );
            }
            _ => return Err(()),
        }
    }
    validate_plugin_navigation(&navigation, &pages).map_err(|_| ())?;
    if api_call.is_some() && (remote_operation.is_some() || resource_operation.is_some()) {
        return Err(());
    }
    if (remote_operation.is_some() || resource_operation.is_some() || api_call.is_some())
        && (host_mutation.is_some()
            || host_session.is_some()
            || ssh_sync_request.is_some()
            || clipboard_text.is_some()
            || storage_write.is_some()
            || host_dom_operations.is_some()
            || terminal_input_suggestion.is_some()
            || isolated_surface.is_some())
    {
        return Err(());
    }
    Ok(ParsedPluginUiOutputs {
        panels: parse_ui_panel_outputs(request_id, panel_outputs)?,
        templates: parse_ui_template_outputs(request_id, document_outputs)?,
        clipboard_text,
        navigation,
        pages,
        storage_write,
        host_mutation,
        host_session,
        host_dom_operations,
        terminal_input_suggestion,
        isolated_surface,
        ssh_sync_request,
        remote_operation,
        resource_operation,
        api_call,
        resource_navigation: None,
        terminal_launch: None,
        ui_state,
    })
}

pub(crate) fn parse_ui_template_outputs(
    request_id: &str,
    outputs: Vec<PluginRuntimeOutput>,
) -> Result<Vec<PluginUiTemplate>, ()> {
    if outputs.len() > 64 {
        return Err(());
    }
    let mut targets = BTreeSet::new();
    let mut templates = Vec::with_capacity(outputs.len());
    for output in outputs {
        if output.request_id != request_id || output.kind != UI_DOCUMENT_OUTPUT_KIND {
            return Err(());
        }
        let template =
            serde_json::from_str::<PluginUiTemplate>(&output.payload_json).map_err(|_| ())?;
        let document_is_valid = match template.target_id.as_str() {
            "app.page" => validate_plugin_page_document(&template.document).is_ok(),
            "app.header.actions" => validate_plugin_dialog_document(&template.document).is_ok(),
            _ => validate_plugin_ui_document(&template.document).is_ok(),
        };
        if !targets.insert(template.target_id.clone())
            || !document_is_valid
            || validate_template_lifecycle(&template).is_err()
            || norishell_plugin_platform::validate_plugin_ui_placement(
                template.route_paths.as_deref(),
                template.icon.as_deref(),
            )
            .is_err()
        {
            return Err(());
        }
        templates.push(template);
    }
    Ok(templates)
}

/// Validates a current template lifecycle action.
pub(crate) fn validate_template_lifecycle(template: &PluginUiTemplate) -> Result<(), ()> {
    if template.on_open_action_id.is_some()
        && (template.target_id.as_str() == "app.page"
            || template_on_open_action_conflicts_with_document(template))
    {
        return Err(());
    }
    if let Some(auto_refresh) = &template.auto_refresh
        && (template.target_id.as_str() != "terminal.footer"
            || !(5_000..=300_000).contains(&auto_refresh.interval_ms)
            || template_action_conflicts_with_document(template, &auto_refresh.action_id)
            || template.document.nodes.iter().any(|node| {
                matches!(
                    node,
                    PluginUiNode::TextField { .. }
                        | PluginUiNode::Select { .. }
                        | PluginUiNode::Checkbox { .. }
                        | PluginUiNode::Switch { .. }
                )
            }))
    {
        return Err(());
    }
    Ok(())
}

fn template_on_open_action_conflicts_with_document(template: &PluginUiTemplate) -> bool {
    let Some(action_id) = &template.on_open_action_id else {
        return false;
    };
    template_action_conflicts_with_document(template, action_id)
}

fn template_action_conflicts_with_document(
    template: &PluginUiTemplate,
    action_id: &PluginUiActionId,
) -> bool {
    norishell_plugin_platform::plugin_ui_document_contains_action(&template.document, action_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateLifecycleActionAdmission {
    NotLifecycle,
    Admitted,
    AutoRefreshAdmitted,
    Rejected,
}

/// Decides whether a UI action is the template lifecycle hook before normal
/// user-action validation. Lifecycle calls never accept form values.
pub(crate) fn template_lifecycle_action_admission(
    template: &PluginUiTemplate,
    action_id: &PluginUiActionId,
    fields: &[PluginUiFieldValue],
) -> TemplateLifecycleActionAdmission {
    if template
        .auto_refresh
        .as_ref()
        .is_some_and(|auto_refresh| &auto_refresh.action_id == action_id)
    {
        return if fields.is_empty() && validate_template_lifecycle(template).is_ok() {
            TemplateLifecycleActionAdmission::AutoRefreshAdmitted
        } else {
            TemplateLifecycleActionAdmission::Rejected
        };
    }
    if template.on_open_action_id.as_ref() != Some(action_id) {
        return TemplateLifecycleActionAdmission::NotLifecycle;
    }
    if fields.is_empty() && validate_template_lifecycle(template).is_ok() {
        TemplateLifecycleActionAdmission::Admitted
    } else {
        TemplateLifecycleActionAdmission::Rejected
    }
}

/// Converts Plugin Host output into the only host-owned UI vocabulary exposed
/// by M5. The parser is intentionally independent from Vue so malformed or
/// imperative shapes fail before they reach the WebView.
pub(crate) fn parse_ui_panel_outputs(
    request_id: &str,
    outputs: Vec<PluginRuntimeOutput>,
) -> Result<Vec<ParsedPluginContribution>, ()> {
    let mut contributions = Vec::new();
    let mut slots = BTreeSet::new();
    let mut total_nodes = 0_usize;
    for output in outputs {
        if output.request_id != request_id || output.kind != UI_PANEL_OUTPUT_KIND {
            return Err(());
        }
        let contribution =
            serde_json::from_str::<SlottedContribution>(&output.payload_json).map_err(|_| ())?;
        let (slot, nodes, clipboard_values) = (
            contribution.slot,
            contribution.nodes,
            contribution.clipboard_values,
        );
        if !slots.insert(slot)
            || total_nodes.saturating_add(nodes.len()) > MAX_CONTRIBUTION_NODES
            || !supported_slot(slot)
        {
            return Err(());
        }
        let mut action_ids = BTreeSet::new();
        let mut copy_ids = BTreeSet::new();
        for node in &nodes {
            let value = match node {
                PluginContributionNode::Text { text } => text,
                PluginContributionNode::Status { label, .. } => label,
                PluginContributionNode::Action { action_id, label } => {
                    if !valid_action_id(action_id)
                        || !valid_action_label(label)
                        || !action_ids.insert(action_id.clone())
                    {
                        return Err(());
                    }
                    continue;
                }
                PluginContributionNode::Copy { copy_id, label } => {
                    if !valid_action_id(copy_id)
                        || !valid_action_label(label)
                        || !copy_ids.insert(copy_id.clone())
                    {
                        return Err(());
                    }
                    continue;
                }
            };
            if !valid_text(value) {
                return Err(());
            }
        }
        let mut values = BTreeMap::new();
        for value in clipboard_values {
            if !valid_action_id(&value.copy_id)
                || !valid_copy_text(&value.text)
                || values.insert(value.copy_id.clone(), value.text).is_some()
            {
                return Err(());
            }
        }
        if values.len() != copy_ids.len()
            || !copy_ids.iter().all(|copy_id| values.contains_key(copy_id))
        {
            return Err(());
        }
        total_nodes += nodes.len();
        contributions.push(ParsedPluginContribution {
            slot,
            nodes,
            clipboard_values: values,
        });
    }
    Ok(contributions)
}

pub(crate) fn valid_action_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_action_label(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 160
        && !value
            .chars()
            .any(|character| character.is_control() || is_bidi_control(character))
}

fn supported_slot(slot: PluginContributionSlot) -> bool {
    matches!(
        slot,
        PluginContributionSlot::PluginsPage
            | PluginContributionSlot::TerminalSidebar
            | PluginContributionSlot::TerminalToolbar
            | PluginContributionSlot::SftpContextMenu
            | PluginContributionSlot::HostDetailTools
            | PluginContributionSlot::OverviewCardActions
            | PluginContributionSlot::CommandPalette
    )
}

fn is_bidi_control(character: char) -> bool {
    matches!(character, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CONTRIBUTION_TEXT_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\t' | '\n'))
}

fn valid_copy_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_COPY_TEXT_BYTES && !value.chars().any(char::is_control)
}

// Declarative copy actions may return multiline code or reports; legacy panel copyValues remain single-line validated.
fn valid_clipboard_write_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CLIPBOARD_WRITE_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn valid_host_reason(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 512
        && !value
            .chars()
            .any(|character| character.is_control() || is_bidi_control(character))
}

fn valid_host_mutation(patch: &norishell_core_api::PluginHostMutationPatch) -> bool {
    let changes = usize::from(patch.label.is_some())
        + usize::from(patch.address.is_some())
        + usize::from(patch.port.is_some())
        + usize::from(patch.username.is_some())
        + usize::from(patch.clear_username)
        + usize::from(patch.favorite.is_some());
    changes > 0
        && !(patch.username.is_some() && patch.clear_username)
        && patch
            .label
            .as_ref()
            .is_none_or(|value| valid_action_label(value))
        && patch.address.as_ref().is_none_or(|value| {
            !value.trim().is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
        })
        && patch.username.as_ref().is_none_or(|value| {
            !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PluginContributionNode, PluginContributionSlot, PluginContributionTone,
        PluginRuntimeOutput, PluginSshSyncRequest, PluginUiActionId, PluginUiFieldId,
    };

    use super::{
        TemplateLifecycleActionAdmission, parse_plugin_ui_outputs, parse_ui_panel_outputs,
        template_lifecycle_action_admission, valid_action_id,
    };

    #[test]
    fn operation_broker_and_ephemeral_state_reject_mixed_or_unbounded_payloads() {
        let listing = PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "remote.operation.request".to_owned(),
            payload_json: serde_json::json!({"terminalHandle":"a".repeat(64),"operationJson":"{}","reason":"Inspect current connection"}).to_string(),
        };
        let initial = parse_plugin_ui_outputs("request-1", vec![listing.clone()]).unwrap();
        assert!(initial.valid_initial_document_count());
        assert!(
            !parse_plugin_ui_outputs("request-1", Vec::new())
                .unwrap()
                .valid_initial_document_count()
        );
        assert!(
            parse_plugin_ui_outputs("request-1", vec![listing.clone(), listing.clone()]).is_err()
        );
        let privileged = PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "ssh.sync.request".to_owned(),
            payload_json: r#"{"action":"status","profileId":"primary"}"#.to_owned(),
        };
        assert!(parse_plugin_ui_outputs("request-1", vec![listing, privileged]).is_err());
        for value in [
            "[]".to_owned(),
            serde_json::json!({"text":"x".repeat(64 * 1024)}).to_string(),
        ] {
            let state = PluginRuntimeOutput {
                request_id: "request-1".to_owned(),
                kind: "ui.state".to_owned(),
                payload_json: serde_json::json!({"valueJson":value}).to_string(),
            };
            assert!(parse_plugin_ui_outputs("request-1", vec![state]).is_err());
        }
    }

    fn output(payload: &str) -> PluginRuntimeOutput {
        PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "ui.panel".to_owned(),
            payload_json: if payload.trim_start().starts_with('{') {
                payload.to_owned()
            } else {
                serde_json::json!({
                    "slot": "pluginsPage",
                    "nodes": serde_json::from_str::<serde_json::Value>(payload).unwrap(),
                })
                .to_string()
            },
        }
    }

    #[test]
    fn accepts_only_bounded_host_owned_nodes() {
        let nodes = parse_ui_panel_outputs(
            "request-1",
            vec![output(
                r#"[{"kind":"text","text":"<b>untrusted text</b>"},{"kind":"status","label":"Ready","tone":"success"},{"kind":"action","actionId":"generateUuid","label":"Generate UUID"}]"#,
            )],
        )
        .expect("valid contribution");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].slot, PluginContributionSlot::PluginsPage);
        assert_eq!(
            nodes[0].nodes,
            vec![
                PluginContributionNode::Text {
                    text: "<b>untrusted text</b>".to_owned(),
                },
                PluginContributionNode::Status {
                    label: "Ready".to_owned(),
                    tone: PluginContributionTone::Success,
                },
                PluginContributionNode::Action {
                    action_id: "generateUuid".to_owned(),
                    label: "Generate UUID".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn rejects_imperative_unknown_and_unbounded_shapes() {
        for payload in [
            r#"[{"kind":"action","actionId":"javascript:alert(1)","label":"Run"}]"#,
            r#"[{"kind":"action","actionId":"run","label":"Run","url":"https://example.test"}]"#,
            r#"[{"kind":"action","actionId":"run","label":"Run"},{"kind":"action","actionId":"run","label":"Run again"}]"#,
            r#"[{"kind":"action","actionId":"run","label":"line one\nline two"}]"#,
            r#"[{"kind":"text","text":"safe","html":"<script>"}]"#,
            r#"[{"kind":"text","text":""}]"#,
        ] {
            assert!(parse_ui_panel_outputs("request-1", vec![output(payload)]).is_err());
        }
        assert!(
            parse_ui_panel_outputs(
                "request-1",
                vec![output(
                    &serde_json::json!([{
                        "kind": "text",
                        "text": "x".repeat(2_049),
                    }])
                    .to_string()
                )],
            )
            .is_err()
        );
        assert!(
            parse_ui_panel_outputs(
                "request-1",
                (0..65)
                    .map(|_| output(r#"[{"kind":"text","text":"x"}]"#))
                    .collect(),
            )
            .is_err()
        );
    }

    #[test]
    fn action_identifiers_are_bounded_ascii_tokens() {
        assert!(valid_action_id("generateUuid"));
        assert!(valid_action_id("utility.generate-password_2"));
        assert!(!valid_action_id(""));
        assert!(!valid_action_id("contains space"));
        assert!(!valid_action_id("打开"));
        assert!(!valid_action_id(&"x".repeat(81)));
    }

    #[test]
    fn rejects_cross_request_or_cross_capability_output() {
        let mut wrong_request = output(r#"[{"kind":"text","text":"x"}]"#);
        wrong_request.request_id = "request-2".to_owned();
        assert!(parse_ui_panel_outputs("request-1", vec![wrong_request]).is_err());

        let mut wrong_kind = output(r#"[{"kind":"text","text":"x"}]"#);
        wrong_kind.kind = "terminal.requestInput".to_owned();
        assert!(parse_ui_panel_outputs("request-1", vec![wrong_kind]).is_err());
    }

    #[test]
    fn accepts_terminal_toolbar_copy_values_only_with_exact_declared_nodes() {
        let parsed = parse_ui_panel_outputs(
            "request-1",
            vec![output(
                r#"{"slot":"terminalToolbar","nodes":[{"kind":"text","text":"UUID: request-1"},{"kind":"copy","copyId":"copyUuid","label":"Copy UUID"}],"clipboardValues":[{"copyId":"copyUuid","text":"request-1"}]}"#,
            )],
        )
        .expect("valid terminal toolbar contribution");
        assert_eq!(parsed[0].slot, PluginContributionSlot::TerminalToolbar);
        assert_eq!(
            parsed[0].clipboard_values.get("copyUuid"),
            Some(&"request-1".to_owned())
        );
    }

    #[test]
    fn accepts_all_named_slots_and_rejects_copy_value_mismatches() {
        for slot in [
            "terminalSidebar",
            "terminalToolbar",
            "sftpContextMenu",
            "hostDetailTools",
            "overviewCardActions",
            "commandPalette",
        ] {
            let payload = format!(r#"{{"slot":"{slot}","nodes":[{{"kind":"text","text":"x"}}]}}"#);
            assert!(parse_ui_panel_outputs("request-1", vec![output(&payload)]).is_ok());
        }
        for payload in [
            r#"{"slot":"terminalToolbar","nodes":[{"kind":"copy","copyId":"copyUuid","label":"Copy UUID"}],"clipboardValues":[]}"#,
            r#"{"slot":"terminalToolbar","nodes":[{"kind":"text","text":"x"}],"clipboardValues":[{"copyId":"copyUuid","text":"x"}]}"#,
        ] {
            assert!(parse_ui_panel_outputs("request-1", vec![output(payload)]).is_err());
        }
    }

    #[test]
    fn accepts_bounded_ui_documents_and_rejects_unknown_output_kinds() {
        let document = serde_json::json!({
            "targetId": "terminal.toolbar",
            "document": {
                "schemaVersion": 1,
                "rootNodeId": "root",
                "nodes": [{
                    "kind": "text",
                    "nodeId": "root",
                    "text": "Ready",
                    "style": "body",
                    "tone": "neutral"
                }]
            }
        });
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![PluginRuntimeOutput {
                request_id: "request-1".to_owned(),
                kind: "ui.document".to_owned(),
                payload_json: document.to_string(),
            }],
        )
        .expect("valid document");
        assert_eq!(parsed.templates.len(), 1);
        assert!(parsed.panels.is_empty());
        assert!(parsed.clipboard_text.is_none());
        assert!(parsed.navigation.is_empty());
        assert!(parsed.pages.is_empty());
        assert!(parsed.host_mutation.is_none());
        assert!(parsed.host_session.is_none());
        assert!(parsed.host_dom_operations.is_none());
        assert!(parsed.ssh_sync_request.is_none());

        assert!(
            parse_plugin_ui_outputs(
                "request-1",
                vec![PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.rawHtml".to_owned(),
                    payload_json: "{}".to_owned(),
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn current_lifecycle_contract_rejects_conflicting_or_interactive_refresh() {
        let template: norishell_core_api::PluginUiTemplate = serde_json::from_value(serde_json::json!({
            "targetId": "terminal.footer", "onOpenActionId": "refresh",
            "autoRefresh": {"actionId": "refresh", "intervalMs": 5000},
            "document": {"schemaVersion": 1, "rootNodeId": "root", "nodes": [{
                "kind": "text", "nodeId": "root", "text": "Ready", "style": "body", "tone": "neutral"
            }]}
        })).unwrap();
        let action = PluginUiActionId::parse("refresh").unwrap();
        assert_eq!(
            template_lifecycle_action_admission(&template, &action, &[]),
            TemplateLifecycleActionAdmission::AutoRefreshAdmitted
        );
        let mut invalid = template.clone();
        invalid.document.nodes = vec![norishell_core_api::PluginUiNode::TextField {
            node_id: norishell_core_api::PluginUiNodeId::parse("path").unwrap(),
            field_id: PluginUiFieldId::parse("path").unwrap(),
            label: "Path".to_owned(),
            value: String::new(),
            placeholder: None,
            field_kind: norishell_core_api::PluginUiFieldKind::Text,
            required: false,
            disabled: false,
        }];
        assert_eq!(
            template_lifecycle_action_admission(&invalid, &action, &[]),
            TemplateLifecycleActionAdmission::Rejected
        );
    }

    #[test]
    fn ssh_sync_runtime_output_is_single_fenced_and_strict() {
        let valid = PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "ssh.sync.request".to_owned(),
            payload_json: r#"{"action":"status","profileId":"primary"}"#.to_owned(),
        };
        let parsed = parse_plugin_ui_outputs("request-1", vec![valid.clone()])
            .expect("valid SSH sync request");
        assert_eq!(
            parsed.ssh_sync_request,
            Some(PluginSshSyncRequest::Status {
                profile_id: "primary".to_owned(),
                auth: None,
            })
        );

        assert!(parse_plugin_ui_outputs("request-1", vec![valid.clone(), valid.clone()]).is_err());
        let mut wrong_request = valid.clone();
        wrong_request.request_id = "request-2".to_owned();
        assert!(parse_plugin_ui_outputs("request-1", vec![wrong_request]).is_err());
        for payload_json in [
            r#"{"action":"syncNow"}"#,
            r#"{"action":"status","profileId":"primary","token":"forbidden"}"#,
            r#"{"action":"authorize","profileId":"primary","url":"https://example.test"}"#,
        ] {
            assert!(
                parse_plugin_ui_outputs(
                    "request-1",
                    vec![PluginRuntimeOutput {
                        request_id: "request-1".to_owned(),
                        kind: "ssh.sync.request".to_owned(),
                        payload_json: payload_json.to_owned(),
                    }],
                )
                .is_err(),
                "accepted {payload_json}"
            );
        }
    }

    #[test]
    fn initialization_panel_keeps_sync_authorization_as_an_explicit_parsed_request() {
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![
                output(r#"[{"kind":"status","label":"Disconnected","tone":"neutral"}]"#),
                PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ssh.sync.request".to_owned(),
                    payload_json: r#"{"action":"status","profileId":"primary"}"#.to_owned(),
                },
            ],
        )
        .expect("initialization panel and explicit sync request");
        assert_eq!(parsed.panels.len(), 1);
        assert_eq!(
            parsed.ssh_sync_request,
            Some(PluginSshSyncRequest::Status {
                profile_id: "primary".to_owned(),
                auth: None,
            })
        );
        assert!(parsed.host_mutation.is_none());
        assert!(parsed.host_session.is_none());
        assert!(parsed.isolated_surface.is_none());
    }

    #[test]
    fn parses_only_one_fenced_host_dom_operation_batch() {
        let payload = serde_json::json!({
            "contextHandle": "019d0000-0000-4000-8000-000000000001",
            "snapshotRevision": "7",
            "operations": [{
                "kind": "setStyle",
                "nodeHandle": "node:7:0",
                "property": "color",
                "value": "#ff8800"
            }]
        });
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![PluginRuntimeOutput {
                request_id: "request-1".to_owned(),
                kind: "ui.hostDom.operations".to_owned(),
                payload_json: payload.to_string(),
            }],
        )
        .expect("host DOM operation batch");
        assert_eq!(
            parsed
                .host_dom_operations
                .expect("operations")
                .snapshot_revision,
            norishell_core_api::WireSequence::new(7)
        );
        assert!(
            parse_plugin_ui_outputs(
                "request-1",
                vec![
                    PluginRuntimeOutput {
                        request_id: "request-1".to_owned(),
                        kind: "ui.hostDom.operations".to_owned(),
                        payload_json: payload.to_string(),
                    },
                    PluginRuntimeOutput {
                        request_id: "request-1".to_owned(),
                        kind: "ui.hostDom.operations".to_owned(),
                        payload_json: payload.to_string(),
                    },
                ],
            )
            .is_err()
        );
    }

    #[test]
    fn navigation_must_reference_a_valid_bounded_page() {
        let page = serde_json::json!({
            "pageId": "tools",
            "title": "Tools",
            "icon": "sparkles",
            "document": {
                "schemaVersion": 1,
                "rootNodeId": "root",
                "nodes": [{
                    "kind": "text",
                    "nodeId": "root",
                    "text": "Plugin tools",
                    "style": "body",
                    "tone": "neutral"
                }]
            }
        });
        let navigation = serde_json::json!({
            "navigationId": "tools",
            "label": "Tools",
            "icon": "sparkles",
            "pageId": "tools",
            "order": 10
        });
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![
                PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.page".to_owned(),
                    payload_json: page.to_string(),
                },
                PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.navigation".to_owned(),
                    payload_json: navigation.to_string(),
                },
            ],
        )
        .expect("valid navigation and page");
        assert_eq!(parsed.pages.len(), 1);
        assert_eq!(parsed.navigation.len(), 1);

        let missing_page = serde_json::json!({
            "navigationId": "missing",
            "label": "Missing",
            "icon": "plugin",
            "pageId": "missing",
            "order": 0
        });
        assert!(
            parse_plugin_ui_outputs(
                "request-1",
                vec![PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.navigation".to_owned(),
                    payload_json: missing_page.to_string(),
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn plugin_storage_write_is_single_bounded_and_requires_a_json_object() {
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![PluginRuntimeOutput {
                request_id: "request-1".to_owned(),
                kind: "storage.write".to_owned(),
                payload_json: serde_json::json!({
                    "writeToken": "019d0000-0000-4000-8000-000000000903",
                    "valueJson": "{\"loggedIn\":true}"
                })
                .to_string(),
            }],
        )
        .expect("bounded storage write");
        let write = parsed.storage_write.expect("storage write");
        assert_eq!(write.write_token, "019d0000-0000-4000-8000-000000000903");
        assert_eq!(write.value_json, "{\"loggedIn\":true}");

        assert!(
            parse_plugin_ui_outputs(
                "request-1",
                vec![PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "storage.write".to_owned(),
                    payload_json: serde_json::json!({
                        "writeToken": "019d0000-0000-4000-8000-000000000903",
                        "valueJson": "[]"
                    })
                    .to_string(),
                }],
            )
            .is_err()
        );
    }

    #[test]
    fn isolated_surface_requests_are_bounded_and_single_output() {
        let surface = serde_json::json!({
            "surfaceId": "toolbox",
            "title": "Plugin Toolbox",
            "width": 720,
            "height": 540
        });
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![PluginRuntimeOutput {
                request_id: "request-1".to_owned(),
                kind: "ui.webview.open".to_owned(),
                payload_json: surface.to_string(),
            }],
        )
        .expect("bounded isolated surface");
        assert_eq!(
            parsed.isolated_surface.expect("surface").surface_id,
            "toolbox"
        );

        let oversized = serde_json::json!({
            "surfaceId": "toolbox",
            "title": "Plugin Toolbox",
            "width": 2000,
            "height": 540
        });
        assert!(
            parse_plugin_ui_outputs(
                "request-1",
                vec![PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.webview.open".to_owned(),
                    payload_json: oversized.to_string(),
                }],
            )
            .is_err()
        );
    }

    #[test]
    fn clipboard_write_accepts_bounded_multiline_text_but_keeps_panel_values_single_line() {
        let parse = |text: &str| {
            parse_plugin_ui_outputs(
                "request-1",
                vec![PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "clipboard.write".to_owned(),
                    payload_json: serde_json::json!({"text": text}).to_string(),
                }],
            )
        };
        let report = format!("Core API 1.13\r\n\tMethods:\n{}", "describe\n".repeat(80));
        assert_eq!(
            parse(&report).unwrap().clipboard_text.as_deref(),
            Some(report.as_str())
        );
        assert!(parse(&"x".repeat(super::MAX_CLIPBOARD_WRITE_BYTES)).is_ok());
        for invalid in [
            String::new(),
            "x".repeat(super::MAX_CLIPBOARD_WRITE_BYTES + 1),
            "bad\0text".into(),
            "bad\u{1b}text".into(),
            "bad\u{0085}text".into(),
        ] {
            assert!(parse(&invalid).is_err());
        }
        let panel = serde_json::json!({
            "slot": "terminalToolbar",
            "nodes": [{"kind": "copy", "copyId": "copyReport", "label": "Copy"}],
            "clipboardValues": [{"copyId": "copyReport", "text": report}],
        });
        assert!(parse_ui_panel_outputs("request-1", vec![output(&panel.to_string())]).is_err());
    }
}
