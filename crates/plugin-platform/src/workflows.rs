//! Validation for the fixed `assets/workflows.json` package asset.
//!
//! The catalog names bounded, localized workflow and step identifiers. It is
//! intentionally not a declarative execution engine: the resident Wasm
//! instance emits at most one typed API call for a fixed step only after Core
//! delivers the preceding event.

use std::collections::BTreeSet;

use norishell_core_api::{
    MAX_PLUGIN_WORKFLOW_ID_BYTES, MAX_PLUGIN_WORKFLOW_STEPS, MAX_PLUGIN_WORKFLOWS,
    PLUGIN_WORKFLOW_CATALOG_SCHEMA_VERSION, PluginSettingLabel, PluginWorkflowCatalog,
    PluginWorkflowDefinition, valid_plugin_workflow_id,
};
use sha2::{Digest, Sha256};

use crate::{PluginPlatformError, Result, package::lower_hex};

pub const PLUGIN_WORKFLOWS_ASSET_PATH: &str = "assets/workflows.json";
pub const MAX_PLUGIN_WORKFLOW_CATALOG_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedPluginWorkflows {
    pub catalog: PluginWorkflowCatalog,
    pub catalog_json: String,
    pub catalog_sha256: String,
}

impl InspectedPluginWorkflows {
    #[must_use]
    pub fn workflow(&self, workflow_id: &str) -> Option<&PluginWorkflowDefinition> {
        self.catalog
            .workflows
            .iter()
            .find(|workflow| workflow.id == workflow_id)
    }
}

pub fn inspect_plugin_workflows(bytes: &[u8]) -> Result<InspectedPluginWorkflows> {
    if bytes.is_empty() || bytes.len() > MAX_PLUGIN_WORKFLOW_CATALOG_BYTES {
        return Err(PluginPlatformError::InvalidWorkflowCatalog);
    }
    let catalog: PluginWorkflowCatalog =
        serde_json::from_slice(bytes).map_err(|_| PluginPlatformError::InvalidWorkflowCatalog)?;
    validate_plugin_workflow_catalog(&catalog)?;
    let catalog_json =
        serde_json::to_string(&catalog).map_err(|_| PluginPlatformError::InvalidWorkflowCatalog)?;
    if catalog_json.len() > MAX_PLUGIN_WORKFLOW_CATALOG_BYTES {
        return Err(PluginPlatformError::InvalidWorkflowCatalog);
    }
    Ok(InspectedPluginWorkflows {
        catalog,
        catalog_sha256: lower_hex(&Sha256::digest(catalog_json.as_bytes())),
        catalog_json,
    })
}

pub fn validate_plugin_workflow_catalog(catalog: &PluginWorkflowCatalog) -> Result<()> {
    if catalog.schema_version != PLUGIN_WORKFLOW_CATALOG_SCHEMA_VERSION
        || catalog.workflows.len() > MAX_PLUGIN_WORKFLOWS
    {
        return Err(PluginPlatformError::InvalidWorkflowCatalog);
    }
    let mut workflow_ids = BTreeSet::new();
    for workflow in &catalog.workflows {
        if !valid_plugin_workflow_id(&workflow.id) || !workflow_ids.insert(workflow.id.as_str()) {
            return Err(PluginPlatformError::InvalidWorkflowCatalog);
        }
        validate_label(&workflow.label)?;
        if workflow.steps.is_empty() || workflow.steps.len() > MAX_PLUGIN_WORKFLOW_STEPS {
            return Err(PluginPlatformError::InvalidWorkflowCatalog);
        }
        let mut step_ids = BTreeSet::new();
        for step in &workflow.steps {
            if !valid_plugin_workflow_id(&step.id)
                || step.id.len() > MAX_PLUGIN_WORKFLOW_ID_BYTES
                || !step_ids.insert(step.id.as_str())
            {
                return Err(PluginPlatformError::InvalidWorkflowCatalog);
            }
            validate_label(&step.label)?;
        }
    }
    Ok(())
}

fn validate_label(label: &PluginSettingLabel) -> Result<()> {
    for value in [&label.en, &label.zh_cn] {
        if value.trim().is_empty()
            || value.len() > 512
            || value.chars().any(|character| {
                character.is_control()
                    || matches!(
                        character,
                        '\u{061C}'
                            | '\u{200E}'
                            | '\u{200F}'
                            | '\u{202A}'..='\u{202E}'
                            | '\u{2066}'..='\u{2069}'
                    )
            })
        {
            return Err(PluginPlatformError::InvalidWorkflowCatalog);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_catalog_is_strict_bounded_and_not_an_execution_plan() {
        let inspected = inspect_plugin_workflows(
            r#"{"schemaVersion":1,"workflows":[{"id":"backup.daily","label":{"en":"Daily backup","zh-CN":"每日备份"},"steps":[{"id":"prepare","label":{"en":"Prepare","zh-CN":"准备"}},{"id":"publish","label":{"en":"Publish","zh-CN":"发布"}}]}]}"#
                .as_bytes(),
        )
        .expect("catalog");
        assert!(inspected.workflow("backup.daily").is_some());
        assert!(inspect_plugin_workflows(
            r#"{"schemaVersion":1,"workflows":[{"id":"same","label":{"en":"A","zh-CN":"甲"},"steps":[{"id":"x","label":{"en":"X","zh-CN":"甲"}}]},{"id":"same","label":{"en":"B","zh-CN":"乙"},"steps":[{"id":"x","label":{"en":"X","zh-CN":"乙"}}]}]}"#
                .as_bytes(),
        )
        .is_err());
        assert!(inspect_plugin_workflows(
            r#"{"schemaVersion":1,"workflows":[{"id":"bad","label":{"en":"A","zh-CN":"甲"},"steps":[]}],"call":{"kind":"networkStart"}}"#
                .as_bytes(),
        )
        .is_err());
    }
}
