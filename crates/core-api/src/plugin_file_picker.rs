//! Requested local-file access. Native selection and Core approval grant the actual authority.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginFilePickerKind {
    File,
    Directory,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginFileAccessRequest {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub list: bool,
    #[serde(default)]
    pub rename: bool,
    #[serde(default)]
    pub remove: bool,
    #[serde(default)]
    pub recursive_remove: bool,
    #[serde(default)]
    pub watch: bool,
}
