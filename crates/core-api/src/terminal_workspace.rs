use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{HostId, RequestMeta, SshSessionEndpoint, WireSequence};

pub const TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION: u16 = 1;
pub const MAX_TERMINAL_WORKSPACE_TABS: usize = 32;
pub const MAX_TERMINAL_WORKSPACE_PANES_PER_TAB: usize = 2_048;
pub const MAX_TERMINAL_WORKSPACE_LAYOUT_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalWorkspaceLayoutNode {
    Pane {
        pane_id: String,
        terminal_id: String,
    },
    Split {
        split_id: String,
        direction: TerminalWorkspaceSplitDirection,
        ratio: f64,
        first: Box<TerminalWorkspaceLayoutNode>,
        second: Box<TerminalWorkspaceLayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TerminalWorkspaceSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalWorkspacePane {
    Plugin {
        pane_id: String,
        label: String,
        plugin_id: crate::PluginId,
        provider_id: String,
        schema_hash: String,
        #[ts(type = "Record<string, boolean | number | string>")]
        configuration: std::collections::BTreeMap<String, serde_json::Value>,
    },
    Launcher {
        pane_id: String,
        label: String,
    },
    SshHost {
        pane_id: String,
        label: String,
        host_id: HostId,
    },
    SshQuickConnect {
        pane_id: String,
        label: String,
        endpoint: SshSessionEndpoint,
    },
    Local {
        pane_id: String,
        label: String,
    },
    Telnet {
        pane_id: String,
        label: String,
        address: String,
        port: u16,
    },
}

impl TerminalWorkspacePane {
    fn pane_id(&self) -> &str {
        match self {
            Self::Launcher { pane_id, .. }
            | Self::SshHost { pane_id, .. }
            | Self::SshQuickConnect { pane_id, .. }
            | Self::Local { pane_id, .. }
            | Self::Telnet { pane_id, .. }
            | Self::Plugin { pane_id, .. } => pane_id,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Launcher { label, .. }
            | Self::SshHost { label, .. }
            | Self::SshQuickConnect { label, .. }
            | Self::Local { label, .. }
            | Self::Telnet { label, .. }
            | Self::Plugin { label, .. } => label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWorkspaceTab {
    pub tab_id: String,
    pub layout: TerminalWorkspaceLayoutNode,
    pub active_pane_id: String,
    pub panes: Vec<TerminalWorkspacePane>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWorkspaceLayout {
    pub schema_version: u16,
    pub active_tab_id: Option<String>,
    pub tabs: Vec<TerminalWorkspaceTab>,
}

impl Default for TerminalWorkspaceLayout {
    fn default() -> Self {
        Self {
            schema_version: TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION,
            active_tab_id: None,
            tabs: Vec::new(),
        }
    }
}

impl TerminalWorkspaceLayout {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION {
            return Err("terminal workspace layout schema version is unsupported");
        }
        if self.tabs.len() > MAX_TERMINAL_WORKSPACE_TABS {
            return Err("terminal workspace has too many tabs");
        }
        let mut tab_ids = HashSet::new();
        let mut global_pane_ids = HashSet::new();
        for tab in &self.tabs {
            validate_safe_text(&tab.tab_id, 128, "terminal tab id is invalid")?;
            validate_safe_text(
                &tab.active_pane_id,
                128,
                "terminal active pane id is invalid",
            )?;
            if !tab_ids.insert(tab.tab_id.as_str()) {
                return Err("terminal tab id is duplicated");
            }
            if tab.panes.is_empty() || tab.panes.len() > MAX_TERMINAL_WORKSPACE_PANES_PER_TAB {
                return Err("terminal tab pane count is invalid");
            }
            let mut layout_pane_ids = HashSet::new();
            let mut split_ids = HashSet::new();
            validate_layout_node(&tab.layout, 0, &mut layout_pane_ids, &mut split_ids)?;
            if layout_pane_ids.len() != tab.panes.len()
                || !layout_pane_ids.contains(tab.active_pane_id.as_str())
            {
                return Err("terminal tab layout does not match its panes");
            }
            let mut tab_pane_ids = HashSet::new();
            for pane in &tab.panes {
                validate_safe_text(pane.pane_id(), 128, "terminal pane id is invalid")?;
                validate_safe_text(pane.label(), 256, "terminal pane label is invalid")?;
                if !tab_pane_ids.insert(pane.pane_id())
                    || !global_pane_ids.insert(pane.pane_id())
                    || !layout_pane_ids.contains(pane.pane_id())
                {
                    return Err("terminal pane id is duplicated or absent from layout");
                }
                if let TerminalWorkspacePane::SshQuickConnect { endpoint, .. } = pane {
                    validate_safe_text(
                        &endpoint.address,
                        255,
                        "terminal endpoint address is invalid",
                    )?;
                    if endpoint.port == 0 {
                        return Err("terminal endpoint port is invalid");
                    }
                    if let Some(username) = &endpoint.username {
                        validate_optional_safe_text(
                            username,
                            255,
                            "terminal endpoint username is invalid",
                        )?;
                    }
                }
                if let TerminalWorkspacePane::Plugin {
                    provider_id,
                    schema_hash,
                    configuration,
                    ..
                } = pane
                {
                    validate_safe_text(provider_id, 64, "plugin provider id is invalid")?;
                    if schema_hash.len() != 64
                        || !schema_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                        || configuration.len() > 32
                        || serde_json::to_vec(configuration)
                            .map_or(true, |bytes| bytes.len() > 16 * 1024)
                        || configuration.iter().any(|(key, value)| {
                            key.is_empty()
                                || key.len() > 80
                                || crate::sensitive_plugin_setting_key(key)
                                || !matches!(
                                    value,
                                    serde_json::Value::Bool(_)
                                        | serde_json::Value::Number(_)
                                        | serde_json::Value::String(_)
                                )
                        })
                    {
                        return Err("plugin terminal configuration is invalid");
                    }
                }
                if let TerminalWorkspacePane::Telnet { address, port, .. } = pane {
                    validate_safe_text(address, 253, "Telnet endpoint address is invalid")?;
                    if *port == 0 {
                        return Err("Telnet endpoint port is invalid");
                    }
                }
            }
            if layout_pane_ids != tab_pane_ids {
                return Err("terminal tab layout does not match its panes");
            }
        }
        match (&self.active_tab_id, self.tabs.is_empty()) {
            (None, true) => Ok(()),
            (Some(active_tab_id), false) => {
                validate_safe_text(active_tab_id, 128, "terminal active tab id is invalid")?;
                if tab_ids.contains(active_tab_id.as_str()) {
                    Ok(())
                } else {
                    Err("terminal active tab does not exist")
                }
            }
            _ => Err("terminal active tab does not match the workspace"),
        }
    }
}

fn validate_layout_node<'a>(
    node: &'a TerminalWorkspaceLayoutNode,
    depth: usize,
    pane_ids: &mut HashSet<&'a str>,
    split_ids: &mut HashSet<&'a str>,
) -> Result<(), &'static str> {
    if depth > MAX_TERMINAL_WORKSPACE_LAYOUT_DEPTH {
        return Err("terminal layout is too deep");
    }
    match node {
        TerminalWorkspaceLayoutNode::Pane {
            pane_id,
            terminal_id,
        } => {
            validate_safe_text(pane_id, 128, "terminal layout pane id is invalid")?;
            validate_optional_safe_text(
                terminal_id,
                128,
                "terminal layout terminal id is invalid",
            )?;
            if terminal_id != pane_id {
                return Err("terminal layout terminal id must equal its pane id");
            }
            if !pane_ids.insert(pane_id) {
                return Err("terminal layout pane id is duplicated");
            }
        }
        TerminalWorkspaceLayoutNode::Split {
            split_id,
            ratio,
            first,
            second,
            ..
        } => {
            validate_safe_text(split_id, 128, "terminal split id is invalid")?;
            if !split_ids.insert(split_id) {
                return Err("terminal split id is duplicated");
            }
            if !ratio.is_finite() || *ratio <= 0.0 || *ratio >= 1.0 {
                return Err("terminal split ratio is invalid");
            }
            validate_layout_node(first, depth + 1, pane_ids, split_ids)?;
            validate_layout_node(second, depth + 1, pane_ids, split_ids)?;
        }
    }
    if pane_ids.len() > MAX_TERMINAL_WORKSPACE_PANES_PER_TAB {
        return Err("terminal layout has too many panes");
    }
    Ok(())
}

fn validate_safe_text(
    value: &str,
    max_length: usize,
    error: &'static str,
) -> Result<(), &'static str> {
    if value.is_empty() || value.chars().count() > max_length || value.chars().any(char::is_control)
    {
        return Err(error);
    }
    Ok(())
}

fn validate_optional_safe_text(
    value: &str,
    max_length: usize,
    error: &'static str,
) -> Result<(), &'static str> {
    if value.chars().count() > max_length || value.chars().any(char::is_control) {
        return Err(error);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWorkspaceLayoutSnapshot {
    pub revision: WireSequence,
    pub layout: TerminalWorkspaceLayout,
    #[ts(type = "number")]
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWorkspaceLayoutGetRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWorkspaceLayoutReplaceRequest {
    pub meta: RequestMeta,
    pub expected_revision: WireSequence,
    pub layout: TerminalWorkspaceLayout,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_layout() -> TerminalWorkspaceLayout {
        TerminalWorkspaceLayout {
            schema_version: TERMINAL_WORKSPACE_LAYOUT_SCHEMA_VERSION,
            active_tab_id: Some("tab-1".to_owned()),
            tabs: vec![TerminalWorkspaceTab {
                tab_id: "tab-1".to_owned(),
                layout: TerminalWorkspaceLayoutNode::Split {
                    split_id: "split-1".to_owned(),
                    direction: TerminalWorkspaceSplitDirection::Horizontal,
                    ratio: 0.5,
                    first: Box::new(TerminalWorkspaceLayoutNode::Pane {
                        pane_id: "pane-1".to_owned(),
                        terminal_id: "pane-1".to_owned(),
                    }),
                    second: Box::new(TerminalWorkspaceLayoutNode::Pane {
                        pane_id: "pane-2".to_owned(),
                        terminal_id: "pane-2".to_owned(),
                    }),
                },
                active_pane_id: "pane-2".to_owned(),
                panes: vec![
                    TerminalWorkspacePane::Launcher {
                        pane_id: "pane-1".to_owned(),
                        label: "New".to_owned(),
                    },
                    TerminalWorkspacePane::Local {
                        pane_id: "pane-2".to_owned(),
                        label: "zsh".to_owned(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn validates_secret_free_layout_projection() {
        assert_eq!(valid_layout().validate(), Ok(()));
    }

    #[test]
    fn rejects_mismatched_and_unsafe_layouts() {
        let mut layout = valid_layout();
        layout.tabs[0].active_pane_id = "missing".to_owned();
        assert!(layout.validate().is_err());

        let mut layout = valid_layout();
        layout.tabs[0].panes[0] = TerminalWorkspacePane::Launcher {
            pane_id: "pane-1".to_owned(),
            label: "unsafe\nlabel".to_owned(),
        };
        assert!(layout.validate().is_err());

        let mut layout = valid_layout();
        if let TerminalWorkspaceLayoutNode::Split { first, .. } = &mut layout.tabs[0].layout
            && let TerminalWorkspaceLayoutNode::Pane { terminal_id, .. } = first.as_mut()
        {
            *terminal_id = "session-like-id".to_owned();
        }
        assert!(layout.validate().is_err());
    }

    #[test]
    fn serializes_without_session_or_credential_fields() {
        let encoded = serde_json::to_string(&valid_layout()).expect("serialize layout");
        assert!(!encoded.contains("sessionId"));
        assert!(!encoded.contains("credential"));
        assert!(!encoded.contains("scrollback"));
    }
}
