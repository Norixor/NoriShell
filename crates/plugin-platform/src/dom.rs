//! Validation for the high-risk, reversible host DOM broker.

use std::collections::BTreeSet;

use norishell_core_api::{
    PluginHostDomOperation, PluginHostDomOperationBatch, PluginHostDomSnapshot,
};

const MAX_DOM_NODES: usize = 2_048;
const MAX_DOM_OPERATIONS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDomValidationError {
    LimitExceeded,
    InvalidNode,
    InvalidReference,
    InvalidValue,
    DuplicateOperation,
}

pub fn validate_plugin_dom_snapshot(
    snapshot: &PluginHostDomSnapshot,
) -> Result<(), PluginDomValidationError> {
    if snapshot.nodes.is_empty() || snapshot.nodes.len() > MAX_DOM_NODES {
        return Err(PluginDomValidationError::LimitExceeded);
    }
    let mut handles = BTreeSet::new();
    for node in &snapshot.nodes {
        if !valid_handle(&node.node_handle)
            || !handles.insert(node.node_handle.as_str())
            || !valid_tag(&node.tag_name)
            || node
                .role
                .as_ref()
                .is_some_and(|value| !valid_token(value, 80))
            || node
                .direct_text
                .as_ref()
                .is_some_and(|value| !valid_text(value, 4_096, true))
            || node.attributes.len() > 8
            || node.attributes.iter().any(|attribute| {
                !matches!(
                    attribute.name.as_str(),
                    "id" | "role" | "aria-label" | "aria-current" | "data-plugin-target"
                ) || !valid_text(&attribute.value, 512, false)
            })
        {
            return Err(PluginDomValidationError::InvalidNode);
        }
    }
    for node in &snapshot.nodes {
        if node
            .parent_handle
            .as_ref()
            .is_some_and(|parent| parent == &node.node_handle || !handles.contains(parent.as_str()))
        {
            return Err(PluginDomValidationError::InvalidReference);
        }
    }
    Ok(())
}

pub fn validate_plugin_dom_operations(
    snapshot: &PluginHostDomSnapshot,
    batch: &PluginHostDomOperationBatch,
    owner_class_prefix: &str,
) -> Result<(), PluginDomValidationError> {
    validate_plugin_dom_snapshot(snapshot)?;
    if batch.context_handle != snapshot.context_handle
        || batch.snapshot_revision != snapshot.snapshot_revision
        || batch.operations.is_empty()
        || batch.operations.len() > MAX_DOM_OPERATIONS
        || !owner_class_prefix.starts_with("norishell-plugin-")
    {
        return Err(PluginDomValidationError::InvalidReference);
    }
    let handles = snapshot
        .nodes
        .iter()
        .map(|node| node.node_handle.as_str())
        .collect::<BTreeSet<_>>();
    let mut operation_keys = BTreeSet::new();
    for operation in &batch.operations {
        let (handle, key) = match operation {
            PluginHostDomOperation::SetText { node_handle, text } => {
                if !valid_text(text, 4_096, true) {
                    return Err(PluginDomValidationError::InvalidValue);
                }
                (node_handle, "text".to_owned())
            }
            PluginHostDomOperation::SetAttribute {
                node_handle,
                name,
                value,
            } => {
                if !matches!(name.as_str(), "title" | "aria-label")
                    || !valid_text(value, 512, false)
                {
                    return Err(PluginDomValidationError::InvalidValue);
                }
                (node_handle, format!("attribute:{name}"))
            }
            PluginHostDomOperation::AddClass {
                node_handle,
                class_name,
            }
            | PluginHostDomOperation::RemoveClass {
                node_handle,
                class_name,
            } => {
                if !class_name.starts_with(owner_class_prefix) || !valid_token(class_name, 160) {
                    return Err(PluginDomValidationError::InvalidValue);
                }
                (node_handle, format!("class:{class_name}"))
            }
            PluginHostDomOperation::SetStyle {
                node_handle,
                property,
                value,
            } => {
                if !valid_style_value(value) {
                    return Err(PluginDomValidationError::InvalidValue);
                }
                (node_handle, format!("style:{property:?}"))
            }
            PluginHostDomOperation::SetHidden {
                node_handle,
                hidden: _,
            } => (node_handle, "hidden".to_owned()),
        };
        if !handles.contains(handle.as_str()) || !operation_keys.insert((handle.as_str(), key)) {
            return Err(if handles.contains(handle.as_str()) {
                PluginDomValidationError::DuplicateOperation
            } else {
                PluginDomValidationError::InvalidReference
            });
        }
    }
    Ok(())
}

fn valid_handle(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= 80
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-'))
}

fn valid_tag(value: &str) -> bool {
    matches!(
        value,
        "a" | "article"
            | "aside"
            | "button"
            | "dd"
            | "details"
            | "div"
            | "dl"
            | "dt"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "header"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "output"
            | "p"
            | "pre"
            | "section"
            | "small"
            | "span"
            | "strong"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}

fn valid_token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-'))
}

fn valid_text(value: &str, maximum: usize, multiline: bool) -> bool {
    value.len() <= maximum
        && !value.chars().any(|character| {
            is_bidi_control(character)
                || (character.is_control() && !(multiline && matches!(character, '\t' | '\n')))
        })
}

fn valid_style_value(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 160
        && !value.chars().any(char::is_control)
        && !["url(", "expression(", "var(", "!important", ";", "{", "}"]
            .iter()
            .any(|forbidden| value.to_ascii_lowercase().contains(forbidden))
}

fn is_bidi_control(character: char) -> bool {
    matches!(character, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PluginHostDomNodeSnapshot, PluginHostDomOperation, PluginHostDomOperationBatch,
        PluginHostDomRect, PluginHostDomSnapshot, PluginHostStyleProperty,
        PluginTargetContextHandle, WireSequence,
    };

    use super::{validate_plugin_dom_operations, validate_plugin_dom_snapshot};

    fn snapshot() -> PluginHostDomSnapshot {
        PluginHostDomSnapshot {
            context_handle: PluginTargetContextHandle::parse(
                "019d0000-0000-4000-8000-000000000001",
            )
            .expect("context"),
            snapshot_revision: WireSequence::new(1),
            nodes: vec![PluginHostDomNodeSnapshot {
                node_handle: "node:0".to_owned(),
                parent_handle: None,
                tag_name: "div".to_owned(),
                role: None,
                direct_text: Some("Visible".to_owned()),
                attributes: vec![],
                rect: PluginHostDomRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 40,
                },
            }],
            truncated: false,
        }
    }

    #[test]
    fn accepts_scoped_cosmetic_mutation_and_rejects_fetching_css() {
        let snapshot = snapshot();
        assert!(validate_plugin_dom_snapshot(&snapshot).is_ok());
        let mut batch = PluginHostDomOperationBatch {
            context_handle: snapshot.context_handle.clone(),
            snapshot_revision: snapshot.snapshot_revision,
            operations: vec![PluginHostDomOperation::SetStyle {
                node_handle: "node:0".to_owned(),
                property: PluginHostStyleProperty::Color,
                value: "#ff8800".to_owned(),
            }],
        };
        assert!(
            validate_plugin_dom_operations(&snapshot, &batch, "norishell-plugin-fixture-").is_ok()
        );
        batch.operations = vec![PluginHostDomOperation::SetStyle {
            node_handle: "node:0".to_owned(),
            property: PluginHostStyleProperty::BackgroundColor,
            value: "url(https://example.test/pixel)".to_owned(),
        }];
        assert!(
            validate_plugin_dom_operations(&snapshot, &batch, "norishell-plugin-fixture-").is_err()
        );
    }
}
