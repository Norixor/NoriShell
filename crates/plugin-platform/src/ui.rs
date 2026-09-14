//! Validation for host-rendered plugin UI documents.

use std::collections::{BTreeMap, BTreeSet};

use norishell_core_api::{
    PLUGIN_UI_SCHEMA_VERSION, PluginNavigationContribution, PluginPageContribution,
    PluginUiActionId, PluginUiDocument, PluginUiFieldKind, PluginUiFieldValue, PluginUiNode,
    PluginUiNodeId,
};

const MAX_UI_NODES: usize = 256;
const MAX_UI_DEPTH: usize = 32;
const MAX_CHILDREN: usize = 64;
const MAX_TOTAL_TEXT_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_LABEL_BYTES: usize = 512;
const MAX_PASSWORD_BYTES: usize = 4 * 1024;
const MAX_TABLE_ROWS: usize = 512;
const MAX_TABLE_COLUMNS: usize = 24;
const MAX_SELECT_OPTIONS: usize = 256;

/// Placement hints cannot introduce new routes or executable image sources.
pub fn validate_plugin_ui_placement(
    route_paths: Option<&[String]>,
    icon: Option<&str>,
) -> Result<(), PluginUiValidationError> {
    if let Some(icon) = icon {
        icon_name(icon)?;
    }
    if let Some(paths) = route_paths {
        let mut unique = BTreeSet::new();
        if paths.is_empty() || paths.len() > 6 {
            return Err(PluginUiValidationError::InvalidValue);
        }
        for path in paths {
            if !matches!(
                path.as_str(),
                "/terminal" | "/overview" | "/hosts" | "/sftp" | "/tunnels" | "/plugins"
            ) || !unique.insert(path)
            {
                return Err(PluginUiValidationError::InvalidValue);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod placement_tests {
    use super::validate_plugin_ui_placement;

    #[test]
    fn placement_hints_allow_known_routes_and_icons_only() {
        assert!(validate_plugin_ui_placement(None, None).is_ok());
        assert!(
            validate_plugin_ui_placement(
                Some(&["/terminal".into(), "/hosts".into()]),
                Some("play")
            )
            .is_ok()
        );
        for route in [
            "/settings",
            "/vault",
            "/identities",
            "/known-hosts",
            "/terminal?host=x",
            "/terminal/*",
            "/unreviewed",
        ] {
            assert!(validate_plugin_ui_placement(Some(&[route.into()]), Some("play")).is_err());
        }
        assert!(validate_plugin_ui_placement(Some(&[]), None).is_err());
        assert!(
            validate_plugin_ui_placement(Some(&["/terminal".into(), "/terminal".into()]), None)
                .is_err()
        );
        assert!(
            validate_plugin_ui_placement(None, Some("https://example.invalid/icon.svg")).is_err()
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginUiValidationError {
    UnsupportedSchema,
    LimitExceeded,
    DuplicateIdentifier,
    InvalidReference,
    Cycle,
    InvalidValue,
    UnreachableNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginUiActionKind {
    Standard,
    Copy,
}

#[derive(Debug, Clone, Copy)]
struct PluginUiValidationPolicy {
    allow_password_fields: bool,
    allow_dialogs: bool,
    allow_ssh_sync_browser: bool,
}

const STANDARD_UI_POLICY: PluginUiValidationPolicy = PluginUiValidationPolicy {
    allow_password_fields: false,
    allow_dialogs: false,
    allow_ssh_sync_browser: false,
};
const PAGE_UI_POLICY: PluginUiValidationPolicy = PluginUiValidationPolicy {
    allow_password_fields: true,
    allow_dialogs: true,
    allow_ssh_sync_browser: true,
};
const DIALOG_UI_POLICY: PluginUiValidationPolicy = PluginUiValidationPolicy {
    allow_password_fields: false,
    allow_dialogs: true,
    allow_ssh_sync_browser: false,
};

/// Validates structure, reachability and all host-rendered values before a
/// document can enter the desktop runtime or WebView projection.
pub fn validate_plugin_ui_document(
    document: &PluginUiDocument,
) -> Result<(), PluginUiValidationError> {
    validate_plugin_ui_document_with_policy(document, STANDARD_UI_POLICY)
}

/// Validates a document owned by a plugin Page. Only this surface may contain
/// empty-in-document password fields whose live values are supplied by the
/// user and delivered to the same Plugin Host on an explicit action.
pub fn validate_plugin_page_document(
    document: &PluginUiDocument,
) -> Result<(), PluginUiValidationError> {
    validate_plugin_ui_document_with_policy(document, PAGE_UI_POLICY)
}

/// Validates a host-owned dialog surface without extending the Page-only
/// password-field boundary.
pub fn validate_plugin_dialog_document(
    document: &PluginUiDocument,
) -> Result<(), PluginUiValidationError> {
    validate_plugin_ui_document_with_policy(document, DIALOG_UI_POLICY)
}

fn validate_plugin_ui_document_with_policy(
    document: &PluginUiDocument,
    policy: PluginUiValidationPolicy,
) -> Result<(), PluginUiValidationError> {
    if document.schema_version != PLUGIN_UI_SCHEMA_VERSION
        || document.nodes.is_empty()
        || document.nodes.len() > MAX_UI_NODES
    {
        return Err(PluginUiValidationError::UnsupportedSchema);
    }
    let mut nodes = BTreeMap::new();
    let mut action_ids = BTreeSet::new();
    let mut field_ids = BTreeSet::new();
    let mut text_bytes = 0_usize;
    for node in &document.nodes {
        if nodes.insert(node.node_id(), node).is_some() {
            return Err(PluginUiValidationError::DuplicateIdentifier);
        }
        validate_node(
            node,
            &mut action_ids,
            &mut field_ids,
            &mut text_bytes,
            policy,
        )?;
    }
    if text_bytes > MAX_TOTAL_TEXT_BYTES || !nodes.contains_key(&document.root_node_id) {
        return Err(PluginUiValidationError::LimitExceeded);
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    visit(
        &document.root_node_id,
        1,
        &nodes,
        &mut visiting,
        &mut visited,
    )?;
    if visited.len() != nodes.len() {
        return Err(PluginUiValidationError::UnreachableNode);
    }
    Ok(())
}

/// Revalidates a direct host-rendered action and its current form snapshot.
/// Fields are scoped to the action's nearest host-owned Dialog. Actions outside
/// a Dialog can submit only fields that are likewise outside every Dialog.
/// Unknown, disabled, duplicate or omitted fields in that scope fail closed.
pub fn validate_plugin_ui_action(
    document: &PluginUiDocument,
    action_id: &PluginUiActionId,
    fields: &[PluginUiFieldValue],
) -> Result<PluginUiActionKind, PluginUiValidationError> {
    validate_plugin_ui_action_with_policy(document, action_id, fields, STANDARD_UI_POLICY)
}

/// Revalidates an action originating from a plugin-owned Page, including its
/// explicitly entered password values. Other extension targets use
/// `validate_plugin_ui_action` and continue to reject password fields.
pub fn validate_plugin_page_ui_action(
    document: &PluginUiDocument,
    action_id: &PluginUiActionId,
    fields: &[PluginUiFieldValue],
) -> Result<PluginUiActionKind, PluginUiValidationError> {
    validate_plugin_ui_action_with_policy(document, action_id, fields, PAGE_UI_POLICY)
}

/// Revalidates an action from a host-owned dialog surface while continuing to
/// reject password fields outside plugin Pages.
pub fn validate_plugin_dialog_ui_action(
    document: &PluginUiDocument,
    action_id: &PluginUiActionId,
    fields: &[PluginUiFieldValue],
) -> Result<PluginUiActionKind, PluginUiValidationError> {
    validate_plugin_ui_action_with_policy(document, action_id, fields, DIALOG_UI_POLICY)
}

fn validate_plugin_ui_action_with_policy(
    document: &PluginUiDocument,
    action_id: &PluginUiActionId,
    fields: &[PluginUiFieldValue],
    policy: PluginUiValidationPolicy,
) -> Result<PluginUiActionKind, PluginUiValidationError> {
    validate_plugin_ui_document_with_policy(document, policy)?;
    let action_kind = document
        .nodes
        .iter()
        .find_map(|node| match node {
            PluginUiNode::Button {
                action_id: candidate,
                disabled,
                ..
            } if candidate == action_id && !disabled => Some(PluginUiActionKind::Standard),
            PluginUiNode::CopyButton {
                action_id: candidate,
                disabled,
                ..
            } if candidate == action_id && !disabled => Some(PluginUiActionKind::Copy),
            PluginUiNode::Table { rows, .. }
                if rows
                    .iter()
                    .any(|row| row.action_id.as_ref() == Some(action_id)) =>
            {
                Some(PluginUiActionKind::Standard)
            }
            PluginUiNode::Tree { items, .. } if tree_has_action(items, action_id) => {
                Some(PluginUiActionKind::Standard)
            }
            _ => None,
        })
        .ok_or(PluginUiValidationError::InvalidValue)?;

    let mut parents = BTreeMap::new();
    let mut dialogs = BTreeSet::new();
    for node in &document.nodes {
        if matches!(node, PluginUiNode::Dialog { .. }) {
            dialogs.insert(node.node_id().as_str().to_owned());
        }
        for child in node.referenced_child_ids() {
            parents.insert(
                child.as_str().to_owned(),
                node.node_id().as_str().to_owned(),
            );
        }
    }
    let action_node_id = document
        .nodes
        .iter()
        .find_map(|node| match node {
            PluginUiNode::Button {
                node_id,
                action_id: candidate,
                ..
            }
            | PluginUiNode::CopyButton {
                node_id,
                action_id: candidate,
                ..
            } if candidate == action_id => Some(node_id.as_str()),
            PluginUiNode::Table { node_id, rows, .. }
                if rows
                    .iter()
                    .any(|row| row.action_id.as_ref() == Some(action_id)) =>
            {
                Some(node_id.as_str())
            }
            PluginUiNode::Tree { node_id, items, .. } if tree_has_action(items, action_id) => {
                Some(node_id.as_str())
            }
            _ => None,
        })
        .ok_or(PluginUiValidationError::InvalidValue)?;
    let action_dialog = nearest_dialog(action_node_id, &parents, &dialogs);

    let mut expected = BTreeMap::new();
    for node in &document.nodes {
        if nearest_dialog(node.node_id().as_str(), &parents, &dialogs) != action_dialog {
            continue;
        }
        match node {
            PluginUiNode::TextField {
                field_id,
                field_kind,
                required,
                disabled,
                ..
            } if !disabled => {
                expected.insert(
                    field_id.as_str(),
                    FieldConstraint::Text(*field_kind, *required),
                );
            }
            PluginUiNode::Editor {
                field_id,
                read_only,
                ..
            } if !read_only => {
                expected.insert(field_id.as_str(), FieldConstraint::Editor);
            }
            PluginUiNode::Select {
                field_id,
                value,
                options,
                disabled,
                ..
            } if !disabled => {
                expected.insert(
                    field_id.as_str(),
                    FieldConstraint::Select(
                        options
                            .iter()
                            .filter(|option| !option.disabled)
                            .map(|option| option.value.as_str())
                            .collect(),
                        value.is_none(),
                    ),
                );
            }
            PluginUiNode::Checkbox {
                field_id, disabled, ..
            }
            | PluginUiNode::Switch {
                field_id, disabled, ..
            } if !disabled => {
                expected.insert(field_id.as_str(), FieldConstraint::Boolean);
            }
            _ => {}
        }
    }
    if fields.len() != expected.len() {
        return Err(PluginUiValidationError::InvalidValue);
    }
    let mut supplied = BTreeSet::new();
    for field in fields {
        if !supplied.insert(field.field_id.as_str()) {
            return Err(PluginUiValidationError::DuplicateIdentifier);
        }
        let constraint = expected
            .get(field.field_id.as_str())
            .ok_or(PluginUiValidationError::InvalidValue)?;
        validate_field_value(&field.value, constraint)?;
    }
    Ok(action_kind)
}

fn nearest_dialog(
    node_id: &str,
    parents: &BTreeMap<String, String>,
    dialogs: &BTreeSet<String>,
) -> Option<String> {
    let mut current = node_id;
    loop {
        if dialogs.contains(current) {
            return Some(current.to_owned());
        }
        current = parents.get(current)?.as_str();
    }
}

/// Page lifecycle hooks are declared independently from clickable document actions.
/// They cannot share a user action identity or accept any form values.
pub fn validate_plugin_page_lifecycle(
    page: &PluginPageContribution,
) -> Result<(), PluginUiValidationError> {
    if page
        .on_open_action_id
        .as_ref()
        .is_some_and(|action_id| plugin_ui_document_contains_action(&page.document, action_id))
    {
        return Err(PluginUiValidationError::InvalidReference);
    }
    Ok(())
}

/// Includes disabled and nested actions: lifecycle and click identities must
/// remain disjoint even when a later document update enables a control.
pub fn plugin_ui_document_contains_action(
    document: &PluginUiDocument,
    action_id: &PluginUiActionId,
) -> bool {
    document.nodes.iter().any(|node| match node {
        PluginUiNode::Button {
            action_id: candidate,
            ..
        }
        | PluginUiNode::CopyButton {
            action_id: candidate,
            ..
        } => candidate == action_id,
        PluginUiNode::Table { rows, .. } => rows
            .iter()
            .any(|row| row.action_id.as_ref() == Some(action_id)),
        PluginUiNode::Tree { items, .. } => tree_has_action(items, action_id),
        _ => false,
    })
}

/// Returns true only for the declared background hook. Ordinary UI actions
/// retain their own document and field validation at the caller.
pub fn plugin_page_lifecycle_action_admission(
    page: &PluginPageContribution,
    action_id: &PluginUiActionId,
    fields: &[PluginUiFieldValue],
) -> Result<bool, PluginUiValidationError> {
    if page.on_open_action_id.as_ref() != Some(action_id) {
        return Ok(false);
    }
    validate_plugin_page_lifecycle(page)?;
    if !fields.is_empty() {
        return Err(PluginUiValidationError::InvalidValue);
    }
    Ok(true)
}

pub fn validate_plugin_navigation(
    navigation: &[PluginNavigationContribution],
    pages: &[PluginPageContribution],
) -> Result<(), PluginUiValidationError> {
    if navigation.len() > 16 || pages.len() > 16 {
        return Err(PluginUiValidationError::LimitExceeded);
    }
    let mut page_ids = BTreeSet::new();
    let mut total_text_bytes = 0;
    for page in pages {
        if !page_ids.insert(page.page_id.as_str()) {
            return Err(PluginUiValidationError::DuplicateIdentifier);
        }
        text(&page.title, MAX_LABEL_BYTES, &mut total_text_bytes)?;
        if let Some(icon) = &page.icon {
            icon_name(icon)?;
        }
        validate_plugin_page_document(&page.document)?;
        validate_plugin_page_lifecycle(page)?;
    }
    let mut navigation_ids = BTreeSet::new();
    for item in navigation {
        if !navigation_ids.insert(item.navigation_id.as_str())
            || !page_ids.contains(item.page_id.as_str())
            || !(-1_000..=1_000).contains(&item.order)
        {
            return Err(PluginUiValidationError::InvalidReference);
        }
        text(&item.label, MAX_LABEL_BYTES, &mut total_text_bytes)?;
        icon_name(&item.icon)?;
    }
    Ok(())
}

enum FieldConstraint<'a> {
    Text(PluginUiFieldKind, bool),
    Select(BTreeSet<&'a str>, bool),
    Boolean,
    Editor,
}

fn validate_field_value(
    value: &str,
    constraint: &FieldConstraint<'_>,
) -> Result<(), PluginUiValidationError> {
    if let FieldConstraint::Text(PluginUiFieldKind::Password, required) = constraint {
        return if value.len() <= MAX_PASSWORD_BYTES
            && !value.contains('\0')
            && (!required || !value.is_empty())
        {
            Ok(())
        } else {
            Err(PluginUiValidationError::InvalidValue)
        };
    }
    let mut field_bytes = 0;
    optional_text(value, MAX_TEXT_BYTES, &mut field_bytes)?;
    match constraint {
        FieldConstraint::Text(_, true) if value.is_empty() => {
            Err(PluginUiValidationError::InvalidValue)
        }
        FieldConstraint::Text(_, false) if value.is_empty() => Ok(()),
        FieldConstraint::Text(PluginUiFieldKind::Number, _) if value.parse::<f64>().is_err() => {
            Err(PluginUiValidationError::InvalidValue)
        }
        FieldConstraint::Text(PluginUiFieldKind::Url, _)
            if !(value.starts_with("https://") || value.starts_with("http://")) =>
        {
            Err(PluginUiValidationError::InvalidValue)
        }
        FieldConstraint::Text(_, _) => Ok(()),
        FieldConstraint::Select(options, nullable)
            if options.contains(value) || (*nullable && value.is_empty()) =>
        {
            Ok(())
        }
        FieldConstraint::Boolean if matches!(value, "true" | "false") => Ok(()),
        FieldConstraint::Editor if !known_secret(value) => Ok(()),
        _ => Err(PluginUiValidationError::InvalidValue),
    }
}

fn visit<'a>(
    node_id: &'a PluginUiNodeId,
    depth: usize,
    nodes: &BTreeMap<&'a PluginUiNodeId, &'a PluginUiNode>,
    visiting: &mut BTreeSet<&'a PluginUiNodeId>,
    visited: &mut BTreeSet<&'a PluginUiNodeId>,
) -> Result<(), PluginUiValidationError> {
    if depth > MAX_UI_DEPTH {
        return Err(PluginUiValidationError::LimitExceeded);
    }
    if visited.contains(node_id) {
        return Ok(());
    }
    if !visiting.insert(node_id) {
        return Err(PluginUiValidationError::Cycle);
    }
    let node = nodes
        .get(node_id)
        .ok_or(PluginUiValidationError::InvalidReference)?;
    let children = node.referenced_child_ids();
    if children.len() > MAX_CHILDREN {
        return Err(PluginUiValidationError::LimitExceeded);
    }
    for child_id in children {
        visit(child_id, depth + 1, nodes, visiting, visited)?;
    }
    visiting.remove(node_id);
    visited.insert(node_id);
    Ok(())
}

fn validate_node(
    node: &PluginUiNode,
    action_ids: &mut BTreeSet<String>,
    field_ids: &mut BTreeSet<String>,
    total_text_bytes: &mut usize,
    policy: PluginUiValidationPolicy,
) -> Result<(), PluginUiValidationError> {
    match node {
        PluginUiNode::Stack { gap, .. } => bounded_gap(*gap)?,
        PluginUiNode::Grid {
            columns,
            column_weights,
            gap,
            ..
        } => {
            if !(1..=12).contains(columns) {
                return Err(PluginUiValidationError::InvalidValue);
            }
            if column_weights.as_ref().is_some_and(|weights| {
                weights.len() != usize::from(*columns)
                    || weights.iter().any(|weight| !(1..=4096).contains(weight))
            }) {
                return Err(PluginUiValidationError::InvalidValue);
            }
            bounded_gap(*gap)?;
        }
        PluginUiNode::Section { title, .. } => {
            if let Some(title) = title {
                text(title, MAX_LABEL_BYTES, total_text_bytes)?;
            }
        }
        PluginUiNode::Divider { .. } => {}
        PluginUiNode::Text { text: value, .. } => {
            text(value, MAX_TEXT_BYTES, total_text_bytes)?;
        }
        PluginUiNode::Code {
            text: value,
            language,
            ..
        } => {
            text(value, MAX_TEXT_BYTES, total_text_bytes)?;
            if let Some(language) = language {
                ascii_token(language, 40)?;
            }
        }
        PluginUiNode::Icon {
            icon,
            accessible_label,
            ..
        } => {
            icon_name(icon)?;
            text(accessible_label, MAX_LABEL_BYTES, total_text_bytes)?;
        }
        PluginUiNode::Status { label, .. } => {
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
        }
        PluginUiNode::Progress {
            label,
            value_percent,
            ..
        } => {
            if value_percent.is_some_and(|value| value > 100) {
                return Err(PluginUiValidationError::InvalidValue);
            }
            if let Some(label) = label {
                text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            }
        }
        PluginUiNode::Button {
            action_id,
            label,
            icon,
            ..
        } => {
            unique(action_id.as_str(), action_ids)?;
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            if let Some(icon) = icon {
                icon_name(icon)?;
            }
        }
        PluginUiNode::CopyButton {
            action_id, label, ..
        } => {
            unique(action_id.as_str(), action_ids)?;
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
        }
        PluginUiNode::TextField {
            field_id,
            label,
            value,
            placeholder,
            field_kind,
            ..
        } => {
            if *field_kind == PluginUiFieldKind::Password
                && (!policy.allow_password_fields || !value.is_empty())
            {
                return Err(PluginUiValidationError::InvalidValue);
            }
            unique(field_id.as_str(), field_ids)?;
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            optional_text(value, MAX_TEXT_BYTES, total_text_bytes)?;
            if let Some(placeholder) = placeholder {
                optional_text(placeholder, MAX_LABEL_BYTES, total_text_bytes)?;
            }
        }
        PluginUiNode::Select {
            field_id,
            label,
            value,
            options,
            ..
        } => {
            unique(field_id.as_str(), field_ids)?;
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            if options.len() > MAX_SELECT_OPTIONS {
                return Err(PluginUiValidationError::LimitExceeded);
            }
            let mut values = BTreeSet::new();
            for option in options {
                text(&option.label, MAX_LABEL_BYTES, total_text_bytes)?;
                ascii_token(&option.value, 160)?;
                if !values.insert(option.value.as_str()) {
                    return Err(PluginUiValidationError::DuplicateIdentifier);
                }
            }
            if value
                .as_ref()
                .is_some_and(|value| !values.contains(value.as_str()))
            {
                return Err(PluginUiValidationError::InvalidValue);
            }
        }
        PluginUiNode::Checkbox {
            field_id, label, ..
        }
        | PluginUiNode::Switch {
            field_id, label, ..
        } => {
            unique(field_id.as_str(), field_ids)?;
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
        }
        PluginUiNode::Table {
            label,
            columns,
            rows,
            empty_text,
            ..
        } => {
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            if columns.is_empty()
                || columns.len() > MAX_TABLE_COLUMNS
                || rows.len() > MAX_TABLE_ROWS
            {
                return Err(PluginUiValidationError::LimitExceeded);
            }
            let mut column_ids = BTreeSet::new();
            for column in columns {
                ascii_token(&column.column_id, 80)?;
                if !column_ids.insert(column.column_id.as_str()) {
                    return Err(PluginUiValidationError::DuplicateIdentifier);
                }
                text(&column.label, MAX_LABEL_BYTES, total_text_bytes)?;
            }
            let mut row_ids = BTreeSet::new();
            for row in rows {
                ascii_token(&row.row_id, 160)?;
                if row.cells.len() != columns.len() || !row_ids.insert(row.row_id.as_str()) {
                    return Err(PluginUiValidationError::InvalidValue);
                }
                for cell in &row.cells {
                    optional_text(cell, MAX_TEXT_BYTES, total_text_bytes)?;
                }
                if let Some(action_id) = &row.action_id {
                    unique(action_id.as_str(), action_ids)?;
                }
            }
            if let Some(empty_text) = empty_text {
                text(empty_text, MAX_LABEL_BYTES, total_text_bytes)?;
            }
        }
        PluginUiNode::Menu { label, .. } | PluginUiNode::Disclosure { label, .. } => {
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
        }
        PluginUiNode::Dialog {
            title,
            description,
            trigger_label,
            close_label,
            ..
        } => {
            if !policy.allow_dialogs {
                return Err(PluginUiValidationError::InvalidValue);
            }
            text(title, MAX_LABEL_BYTES, total_text_bytes)?;
            if let Some(description) = description {
                text(description, MAX_TEXT_BYTES, total_text_bytes)?;
            }
            text(trigger_label, MAX_LABEL_BYTES, total_text_bytes)?;
            text(close_label, MAX_LABEL_BYTES, total_text_bytes)?;
        }
        PluginUiNode::Tabs { label, tabs, .. } => {
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            if tabs.is_empty() || tabs.len() > 16 {
                return Err(PluginUiValidationError::LimitExceeded);
            }
            let mut ids = BTreeSet::new();
            for tab in tabs {
                ascii_token(&tab.id, 160)?;
                text(&tab.label, MAX_LABEL_BYTES, total_text_bytes)?;
                if !ids.insert(tab.id.as_str()) || tab.children.is_empty() {
                    return Err(PluginUiValidationError::InvalidValue);
                }
            }
        }
        PluginUiNode::Tree { label, items, .. } => {
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            validate_tree_items(items, action_ids, total_text_bytes, &mut BTreeSet::new(), 0)?;
        }
        PluginUiNode::Chart {
            label,
            series,
            labels,
            ..
        } => {
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            if series.is_empty() || series.len() > 8 || labels.is_empty() || labels.len() > 128 {
                return Err(PluginUiValidationError::LimitExceeded);
            }
            for label in labels {
                text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            }
            for item in series {
                text(&item.label, MAX_LABEL_BYTES, total_text_bytes)?;
                if item.values.len() != labels.len()
                    || item.values.len() > 128
                    || item.values.iter().any(|value| !value.0.is_finite())
                {
                    return Err(PluginUiValidationError::InvalidValue);
                }
            }
        }
        PluginUiNode::Editor {
            field_id,
            label,
            value,
            language,
            read_only,
            ..
        } => {
            unique(field_id.as_str(), field_ids)?;
            text(label, MAX_LABEL_BYTES, total_text_bytes)?;
            ascii_token(language, 40)?;
            optional_text(value, MAX_TEXT_BYTES, total_text_bytes)?;
            if !read_only && known_secret(value) {
                return Err(PluginUiValidationError::InvalidValue);
            }
        }
        PluginUiNode::SshSyncBrowser { profile_id, .. } => {
            if !policy.allow_ssh_sync_browser || !valid_profile_id(profile_id) {
                return Err(PluginUiValidationError::InvalidValue);
            }
        }
    }
    Ok(())
}

fn tree_has_action(
    items: &[norishell_core_api::PluginUiTreeItem],
    action: &PluginUiActionId,
) -> bool {
    items.iter().any(|item| {
        item.action_id.as_ref() == Some(action)
            || item
                .children
                .as_ref()
                .is_some_and(|children| tree_has_action(children, action))
    })
}

fn validate_tree_items(
    items: &[norishell_core_api::PluginUiTreeItem],
    action_ids: &mut BTreeSet<String>,
    total: &mut usize,
    ids: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), PluginUiValidationError> {
    if depth > MAX_UI_DEPTH || items.len() > MAX_CHILDREN {
        return Err(PluginUiValidationError::LimitExceeded);
    }
    for item in items {
        ascii_token(&item.id, 160)?;
        text(&item.label, MAX_LABEL_BYTES, total)?;
        if !ids.insert(item.id.clone()) {
            return Err(PluginUiValidationError::DuplicateIdentifier);
        }
        if let Some(action) = &item.action_id {
            unique(action.as_str(), action_ids)?;
        }
        if let Some(children) = &item.children {
            validate_tree_items(children, action_ids, total, ids, depth + 1)?;
        }
    }
    Ok(())
}

fn known_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password=",
        "api_key=",
        "secret=",
        "authorization: bearer",
        "-----begin private key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn bounded_gap(gap: u8) -> Result<(), PluginUiValidationError> {
    if gap > 64 {
        Err(PluginUiValidationError::InvalidValue)
    } else {
        Ok(())
    }
}

fn unique(value: &str, values: &mut BTreeSet<String>) -> Result<(), PluginUiValidationError> {
    if !values.insert(value.to_owned()) {
        Err(PluginUiValidationError::DuplicateIdentifier)
    } else {
        Ok(())
    }
}

fn ascii_token(value: &str, maximum_bytes: usize) -> Result<(), PluginUiValidationError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-'))
    {
        Err(PluginUiValidationError::InvalidValue)
    } else {
        Ok(())
    }
}

fn valid_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.is_ascii()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-' | b'/')
        })
}

fn icon_name(value: &str) -> Result<(), PluginUiValidationError> {
    if matches!(
        value,
        "terminal"
            | "server"
            | "folder"
            | "settings"
            | "plugin"
            | "command"
            | "copy"
            | "search"
            | "info"
            | "check"
            | "warning"
            | "close"
            | "more"
            | "play"
            | "stop"
            | "refresh"
            | "download"
            | "upload"
            | "link"
            | "shield"
            | "clock"
            | "key"
            | "lock"
            | "cloud"
            | "eye"
            | "chevronRight"
            | "history"
            | "user"
            | "logout"
            | "sparkles"
            | "code"
    ) {
        Ok(())
    } else {
        Err(PluginUiValidationError::InvalidValue)
    }
}

fn text(
    value: &str,
    maximum_bytes: usize,
    total_text_bytes: &mut usize,
) -> Result<(), PluginUiValidationError> {
    if value.trim().is_empty() {
        return Err(PluginUiValidationError::InvalidValue);
    }
    optional_text(value, maximum_bytes, total_text_bytes)
}

fn optional_text(
    value: &str,
    maximum_bytes: usize,
    total_text_bytes: &mut usize,
) -> Result<(), PluginUiValidationError> {
    if value.len() > maximum_bytes
        || value.chars().any(|character| {
            is_bidi_control(character)
                || (character.is_control() && !matches!(character, '\t' | '\n'))
        })
    {
        return Err(PluginUiValidationError::InvalidValue);
    }
    *total_text_bytes = total_text_bytes.saturating_add(value.len());
    Ok(())
}

fn is_bidi_control(character: char) -> bool {
    matches!(character, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PLUGIN_UI_SCHEMA_VERSION, PluginUiActionId, PluginUiAlign, PluginUiButtonVariant,
        PluginUiChartKind, PluginUiChartSeries, PluginUiDirection, PluginUiDocument,
        PluginUiFieldId, PluginUiFieldKind, PluginUiFieldValue, PluginUiFiniteNumber, PluginUiNode,
        PluginUiNodeId, PluginUiTab, PluginUiTextStyle, PluginUiTone, PluginUiTreeItem,
    };

    use super::{
        PluginUiValidationError, validate_plugin_dialog_document, validate_plugin_dialog_ui_action,
        validate_plugin_page_document, validate_plugin_page_ui_action, validate_plugin_ui_action,
        validate_plugin_ui_document,
    };

    fn id(value: &str) -> PluginUiNodeId {
        PluginUiNodeId::parse(value).expect("node id")
    }

    #[test]
    fn page_lifecycle_is_fieldless_and_disjoint_from_visible_actions() {
        let mut page: norishell_core_api::PluginPageContribution = serde_json::from_value(serde_json::json!({
            "pageId":"page", "title":"Page", "onOpenActionId":"status",
            "document": {"schemaVersion":1,"rootNodeId":"refresh","nodes":[
                {"kind":"button","nodeId":"refresh","actionId":"refresh","label":"Refresh","variant":"ghost","disabled":false}
            ]}
        })).unwrap();
        let status = PluginUiActionId::parse("status").unwrap();
        let refresh = PluginUiActionId::parse("refresh").unwrap();
        assert_eq!(
            super::validate_plugin_navigation(&[], &[page.clone()]),
            Ok(())
        );
        assert_eq!(
            super::plugin_page_lifecycle_action_admission(&page, &status, &[]),
            Ok(true)
        );
        assert_eq!(
            super::plugin_page_lifecycle_action_admission(&page, &refresh, &[]),
            Ok(false)
        );
        assert_eq!(
            validate_plugin_page_ui_action(&page.document, &refresh, &[]),
            Ok(super::PluginUiActionKind::Standard)
        );
        let fields = [PluginUiFieldValue {
            field_id: PluginUiFieldId::parse("password").unwrap(),
            value: "".to_owned(),
        }];
        assert_eq!(
            super::plugin_page_lifecycle_action_admission(&page, &status, &fields),
            Err(PluginUiValidationError::InvalidValue)
        );
        // A replacement document must not turn the retained hook into a click.
        if let PluginUiNode::Button {
            action_id,
            disabled,
            ..
        } = &mut page.document.nodes[0]
        {
            *action_id = status.clone();
            *disabled = true;
        }
        assert_eq!(
            super::validate_plugin_page_lifecycle(&page),
            Err(PluginUiValidationError::InvalidReference)
        );
        let tree: PluginUiDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"rootNodeId":"tree","nodes":[{"kind":"tree","nodeId":"tree","label":"Tree","items":[
                {"id":"parent","label":"Parent","children":[{"id":"child","label":"Child","actionId":"status"}]}
            ]}]
        })).unwrap();
        assert!(super::plugin_ui_document_contains_action(&tree, &status));
    }

    fn password_document(value: &str) -> PluginUiDocument {
        PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![
                PluginUiNode::Stack {
                    node_id: id("root"),
                    direction: PluginUiDirection::Vertical,
                    align: PluginUiAlign::Stretch,
                    gap: 8,
                    children: vec![id("password"), id("submit")],
                },
                PluginUiNode::TextField {
                    node_id: id("password"),
                    field_id: PluginUiFieldId::parse("password").expect("field id"),
                    label: "Password".to_owned(),
                    value: value.to_owned(),
                    placeholder: None,
                    field_kind: PluginUiFieldKind::Password,
                    required: true,
                    disabled: false,
                },
                PluginUiNode::Button {
                    node_id: id("submit"),
                    action_id: PluginUiActionId::parse("submit").expect("action id"),
                    label: "Submit".to_owned(),
                    icon: None,
                    variant: PluginUiButtonVariant::Primary,
                    disabled: false,
                },
            ],
        }
    }

    #[test]
    fn unselected_optional_choices_do_not_block_other_form_actions() {
        let mut document = password_document("");
        document.nodes[1] = PluginUiNode::Select {
            node_id: id("password"),
            field_id: PluginUiFieldId::parse("password").unwrap(),
            label: "Existing forwarding".to_owned(),
            value: None,
            options: Vec::new(),
            disabled: false,
        };
        let action = PluginUiActionId::parse("submit").unwrap();
        let field = |value: &str| {
            vec![PluginUiFieldValue {
                field_id: PluginUiFieldId::parse("password").unwrap(),
                value: value.to_owned(),
            }]
        };
        assert!(validate_plugin_ui_action(&document, &action, &field("")).is_ok());
        assert!(validate_plugin_ui_action(&document, &action, &field("revoked")).is_err());
        if let PluginUiNode::Select { value, .. } = &mut document.nodes[1] {
            *value = Some("existing".to_owned());
        }
        assert!(validate_plugin_ui_action(&document, &action, &field("")).is_err());
    }

    #[test]
    fn optional_numeric_and_url_fields_do_not_block_unrelated_actions() {
        for kind in [PluginUiFieldKind::Number, PluginUiFieldKind::Url] {
            let mut document = password_document("");
            if let PluginUiNode::TextField {
                field_kind,
                required,
                ..
            } = &mut document.nodes[1]
            {
                *field_kind = kind;
                *required = false;
            }
            let action = PluginUiActionId::parse("submit").unwrap();
            let field = |value: &str| {
                vec![PluginUiFieldValue {
                    field_id: PluginUiFieldId::parse("password").unwrap(),
                    value: value.to_owned(),
                }]
            };
            assert!(validate_plugin_ui_action(&document, &action, &field("")).is_ok());
            assert!(validate_plugin_ui_action(&document, &action, &field("invalid")).is_err());
            if let PluginUiNode::TextField { required, .. } = &mut document.nodes[1] {
                *required = true;
            }
            assert!(validate_plugin_ui_action(&document, &action, &field("")).is_err());
        }
    }

    #[test]
    fn accepts_a_bounded_reachable_document() {
        let document = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![
                PluginUiNode::Stack {
                    node_id: id("root"),
                    direction: PluginUiDirection::Vertical,
                    align: PluginUiAlign::Stretch,
                    gap: 8,
                    children: vec![id("message")],
                },
                PluginUiNode::Text {
                    node_id: id("message"),
                    text: "Safe text".to_owned(),
                    style: PluginUiTextStyle::Body,
                    tone: PluginUiTone::Neutral,
                },
            ],
        };
        assert_eq!(validate_plugin_ui_document(&document), Ok(()));
    }

    #[test]
    fn validates_optional_grid_column_weights() {
        let document = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![
                PluginUiNode::Grid {
                    node_id: id("root"),
                    columns: 2,
                    column_weights: Some(vec![3, 8]),
                    gap: 12,
                    children: vec![id("left"), id("right")],
                },
                PluginUiNode::Divider {
                    node_id: id("left"),
                },
                PluginUiNode::Divider {
                    node_id: id("right"),
                },
            ],
        };
        assert_eq!(validate_plugin_page_document(&document), Ok(()));

        let mut invalid = document;
        let PluginUiNode::Grid { column_weights, .. } = &mut invalid.nodes[0] else {
            unreachable!("fixture root is a grid")
        };
        *column_weights = Some(vec![1]);
        assert_eq!(
            validate_plugin_page_document(&invalid),
            Err(PluginUiValidationError::InvalidValue)
        );
    }

    #[test]
    fn rejects_cycles_and_unreachable_payloads() {
        let cycle = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![PluginUiNode::Stack {
                node_id: id("root"),
                direction: PluginUiDirection::Vertical,
                align: PluginUiAlign::Stretch,
                gap: 0,
                children: vec![id("root")],
            }],
        };
        assert_eq!(
            validate_plugin_ui_document(&cycle),
            Err(PluginUiValidationError::Cycle)
        );

        let unreachable = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![
                PluginUiNode::Divider {
                    node_id: id("root"),
                },
                PluginUiNode::Divider {
                    node_id: id("hidden"),
                },
            ],
        };
        assert_eq!(
            validate_plugin_ui_document(&unreachable),
            Err(PluginUiValidationError::UnreachableNode)
        );
    }

    #[test]
    fn tabs_participate_in_document_reachability_and_cycle_checks() {
        let cycle = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("tabs"),
            nodes: vec![PluginUiNode::Tabs {
                node_id: id("tabs"),
                label: "Views".to_owned(),
                tabs: vec![PluginUiTab {
                    id: "active".to_owned(),
                    label: "Active".to_owned(),
                    children: vec![id("tabs")],
                }],
            }],
        };
        assert_eq!(
            validate_plugin_ui_document(&cycle),
            Err(PluginUiValidationError::Cycle)
        );

        let unreachable = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("tabs"),
            nodes: vec![
                PluginUiNode::Tabs {
                    node_id: id("tabs"),
                    label: "Views".to_owned(),
                    tabs: vec![PluginUiTab {
                        id: "active".to_owned(),
                        label: "Active".to_owned(),
                        children: vec![id("content")],
                    }],
                },
                PluginUiNode::Divider {
                    node_id: id("content"),
                },
                PluginUiNode::Divider {
                    node_id: id("hidden"),
                },
            ],
        };
        assert_eq!(
            validate_plugin_ui_document(&unreachable),
            Err(PluginUiValidationError::UnreachableNode)
        );
    }

    #[test]
    fn chart_values_and_tree_editor_actions_remain_bounded_and_validated() {
        let mut labels = vec!["Now".to_owned()];
        labels.extend((0..128).map(|index| format!("T{index}")));
        let oversized_chart = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("chart"),
            nodes: vec![PluginUiNode::Chart {
                node_id: id("chart"),
                label: "Load".to_owned(),
                chart_kind: PluginUiChartKind::Line,
                series: vec![PluginUiChartSeries {
                    label: "CPU".to_owned(),
                    values: labels.iter().map(|_| PluginUiFiniteNumber(1.0)).collect(),
                }],
                labels,
            }],
        };
        assert_eq!(
            validate_plugin_ui_document(&oversized_chart),
            Err(PluginUiValidationError::LimitExceeded)
        );

        let non_finite = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("chart"),
            nodes: vec![PluginUiNode::Chart {
                node_id: id("chart"),
                label: "Load".to_owned(),
                chart_kind: PluginUiChartKind::Bar,
                series: vec![PluginUiChartSeries {
                    label: "CPU".to_owned(),
                    values: vec![PluginUiFiniteNumber(f64::NAN)],
                }],
                labels: vec!["Now".to_owned()],
            }],
        };
        assert_eq!(
            validate_plugin_ui_document(&non_finite),
            Err(PluginUiValidationError::InvalidValue)
        );

        let action = PluginUiActionId::parse("inspect").expect("tree action");
        let editor_field = PluginUiFieldId::parse("query").expect("editor field");
        let editable_tree = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![
                PluginUiNode::Stack {
                    node_id: id("root"),
                    direction: PluginUiDirection::Vertical,
                    align: PluginUiAlign::Stretch,
                    gap: 8,
                    children: vec![id("tree"), id("editor")],
                },
                PluginUiNode::Tree {
                    node_id: id("tree"),
                    label: "Services".to_owned(),
                    items: vec![PluginUiTreeItem {
                        id: "nginx".to_owned(),
                        label: "nginx.service".to_owned(),
                        children: None,
                        action_id: Some(action.clone()),
                    }],
                },
                PluginUiNode::Editor {
                    node_id: id("editor"),
                    field_id: editor_field.clone(),
                    label: "Filter".to_owned(),
                    value: "service=nginx".to_owned(),
                    language: "ini".to_owned(),
                    read_only: false,
                },
            ],
        };
        assert_eq!(
            validate_plugin_ui_action(
                &editable_tree,
                &action,
                &[PluginUiFieldValue {
                    field_id: editor_field,
                    value: "service=sshd".to_owned(),
                }],
            ),
            Ok(super::PluginUiActionKind::Standard)
        );
    }

    #[test]
    fn password_fields_are_page_only_and_cannot_ship_a_prefilled_secret() {
        let empty = password_document("");
        assert_eq!(
            validate_plugin_ui_document(&empty),
            Err(PluginUiValidationError::InvalidValue)
        );
        assert_eq!(validate_plugin_page_document(&empty), Ok(()));
        assert_eq!(
            validate_plugin_page_document(&password_document("embedded-secret")),
            Err(PluginUiValidationError::InvalidValue)
        );
    }

    #[test]
    fn ssh_sync_browser_is_page_only_and_traverses_its_summary_children() {
        let document = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("browser"),
            nodes: vec![
                PluginUiNode::SshSyncBrowser {
                    node_id: id("browser"),
                    profile_id: "primary".to_owned(),
                    children: vec![id("summary")],
                },
                PluginUiNode::Text {
                    node_id: id("summary"),
                    text: "Provider summary".to_owned(),
                    style: PluginUiTextStyle::Secondary,
                    tone: PluginUiTone::Neutral,
                },
            ],
        };
        assert_eq!(validate_plugin_page_document(&document), Ok(()));
        assert_eq!(
            validate_plugin_ui_document(&document),
            Err(PluginUiValidationError::InvalidValue)
        );
        assert_eq!(
            validate_plugin_dialog_document(&document),
            Err(PluginUiValidationError::InvalidValue)
        );

        let mut invalid_profile = document.clone();
        let PluginUiNode::SshSyncBrowser { profile_id, .. } = &mut invalid_profile.nodes[0] else {
            unreachable!("fixture root is an SSH sync browser")
        };
        *profile_id = "profile with spaces".to_owned();
        assert_eq!(
            validate_plugin_page_document(&invalid_profile),
            Err(PluginUiValidationError::InvalidValue)
        );

        let mut missing_child = document;
        missing_child.nodes.pop();
        assert_eq!(
            validate_plugin_page_document(&missing_child),
            Err(PluginUiValidationError::InvalidReference)
        );
    }

    #[test]
    fn plugin_page_action_accepts_an_explicit_password_but_other_surfaces_reject_it() {
        let document = password_document("");
        let action_id = PluginUiActionId::parse("submit").expect("action id");
        let fields = vec![PluginUiFieldValue {
            field_id: PluginUiFieldId::parse("password").expect("field id"),
            value: "user-entered-secret".to_owned(),
        }];
        assert!(validate_plugin_page_ui_action(&document, &action_id, &fields).is_ok());
        assert_eq!(
            validate_plugin_ui_action(&document, &action_id, &fields),
            Err(PluginUiValidationError::InvalidValue)
        );
    }

    #[test]
    fn page_dialog_actions_validate_only_their_own_form_fields() {
        let login_action = PluginUiActionId::parse("login").expect("login action");
        let register_action = PluginUiActionId::parse("register").expect("register action");
        let document = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("root"),
            nodes: vec![
                PluginUiNode::Stack {
                    node_id: id("root"),
                    direction: PluginUiDirection::Vertical,
                    align: PluginUiAlign::Stretch,
                    gap: 8,
                    children: vec![id("loginDialog"), id("registerDialog")],
                },
                PluginUiNode::Dialog {
                    node_id: id("loginDialog"),
                    title: "Sign in".to_owned(),
                    description: None,
                    trigger_label: "Sign in".to_owned(),
                    close_label: "Close".to_owned(),
                    children: vec![id("loginName"), id("loginPassword"), id("login")],
                },
                PluginUiNode::TextField {
                    node_id: id("loginName"),
                    field_id: PluginUiFieldId::parse("loginName").expect("login name"),
                    label: "Email".to_owned(),
                    value: String::new(),
                    placeholder: None,
                    field_kind: PluginUiFieldKind::Text,
                    required: true,
                    disabled: false,
                },
                PluginUiNode::TextField {
                    node_id: id("loginPassword"),
                    field_id: PluginUiFieldId::parse("loginPassword").expect("login password"),
                    label: "Password".to_owned(),
                    value: String::new(),
                    placeholder: None,
                    field_kind: PluginUiFieldKind::Password,
                    required: true,
                    disabled: false,
                },
                PluginUiNode::Button {
                    node_id: id("login"),
                    action_id: login_action.clone(),
                    label: "Sign in".to_owned(),
                    icon: None,
                    variant: PluginUiButtonVariant::Primary,
                    disabled: false,
                },
                PluginUiNode::Dialog {
                    node_id: id("registerDialog"),
                    title: "Register".to_owned(),
                    description: None,
                    trigger_label: "Register".to_owned(),
                    close_label: "Close".to_owned(),
                    children: vec![id("registerName"), id("registerPassword"), id("register")],
                },
                PluginUiNode::TextField {
                    node_id: id("registerName"),
                    field_id: PluginUiFieldId::parse("registerName").expect("register name"),
                    label: "Email".to_owned(),
                    value: String::new(),
                    placeholder: None,
                    field_kind: PluginUiFieldKind::Text,
                    required: true,
                    disabled: false,
                },
                PluginUiNode::TextField {
                    node_id: id("registerPassword"),
                    field_id: PluginUiFieldId::parse("registerPassword")
                        .expect("register password"),
                    label: "Password".to_owned(),
                    value: String::new(),
                    placeholder: None,
                    field_kind: PluginUiFieldKind::Password,
                    required: true,
                    disabled: false,
                },
                PluginUiNode::Button {
                    node_id: id("register"),
                    action_id: register_action,
                    label: "Register".to_owned(),
                    icon: None,
                    variant: PluginUiButtonVariant::Primary,
                    disabled: false,
                },
            ],
        };
        let login_fields = vec![
            PluginUiFieldValue {
                field_id: PluginUiFieldId::parse("loginName").expect("login name"),
                value: "user@example.test".to_owned(),
            },
            PluginUiFieldValue {
                field_id: PluginUiFieldId::parse("loginPassword").expect("login password"),
                value: "secret".to_owned(),
            },
        ];

        assert_eq!(
            validate_plugin_page_ui_action(&document, &login_action, &login_fields),
            Ok(super::PluginUiActionKind::Standard)
        );
        let mut leaked_registration = login_fields;
        leaked_registration.push(PluginUiFieldValue {
            field_id: PluginUiFieldId::parse("registerName").expect("register name"),
            value: "other@example.test".to_owned(),
        });
        assert_eq!(
            validate_plugin_page_ui_action(&document, &login_action, &leaked_registration),
            Err(PluginUiValidationError::InvalidValue)
        );
    }

    #[test]
    fn dialog_surface_allows_host_owned_modal_but_keeps_passwords_page_only() {
        let action_id = PluginUiActionId::parse("generate").expect("action id");
        let dialog = PluginUiDocument {
            schema_version: PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: id("dialog"),
            nodes: vec![
                PluginUiNode::Dialog {
                    node_id: id("dialog"),
                    title: "UUID generator".to_owned(),
                    description: None,
                    trigger_label: "UUID".to_owned(),
                    close_label: "Close".to_owned(),
                    children: vec![id("generate")],
                },
                PluginUiNode::Button {
                    node_id: id("generate"),
                    action_id: action_id.clone(),
                    label: "Generate".to_owned(),
                    icon: None,
                    variant: PluginUiButtonVariant::Primary,
                    disabled: false,
                },
            ],
        };
        assert_eq!(
            validate_plugin_ui_document(&dialog),
            Err(PluginUiValidationError::InvalidValue)
        );
        assert_eq!(validate_plugin_dialog_document(&dialog), Ok(()));
        assert_eq!(
            validate_plugin_dialog_ui_action(&dialog, &action_id, &[]),
            Ok(super::PluginUiActionKind::Standard)
        );
        assert_eq!(
            validate_plugin_dialog_document(&password_document("")),
            Err(PluginUiValidationError::InvalidValue)
        );
    }
}
