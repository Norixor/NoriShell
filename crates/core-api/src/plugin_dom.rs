//! Bounded host DOM observation and reversible mutation protocol.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PluginTargetContextHandle, WireSequence};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostDomAttribute {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostDomRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostDomNodeSnapshot {
    pub node_handle: String,
    pub parent_handle: Option<String>,
    pub tag_name: String,
    pub role: Option<String>,
    pub direct_text: Option<String>,
    pub attributes: Vec<PluginHostDomAttribute>,
    pub rect: PluginHostDomRect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostDomSnapshot {
    pub context_handle: PluginTargetContextHandle,
    pub snapshot_revision: WireSequence,
    pub nodes: Vec<PluginHostDomNodeSnapshot>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginHostStyleProperty {
    Color,
    BackgroundColor,
    BorderColor,
    BorderRadius,
    FontFamily,
    FontSize,
    FontWeight,
    FontStyle,
    LetterSpacing,
    LineHeight,
    TextAlign,
    TextDecoration,
    Padding,
    Margin,
    Gap,
    Width,
    MaxWidth,
    MinWidth,
    Height,
    MaxHeight,
    MinHeight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginHostDomOperation {
    SetText {
        node_handle: String,
        text: String,
    },
    SetAttribute {
        node_handle: String,
        name: String,
        value: String,
    },
    AddClass {
        node_handle: String,
        class_name: String,
    },
    RemoveClass {
        node_handle: String,
        class_name: String,
    },
    SetStyle {
        node_handle: String,
        property: PluginHostStyleProperty,
        value: String,
    },
    SetHidden {
        node_handle: String,
        hidden: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostDomOperationBatch {
    pub context_handle: PluginTargetContextHandle,
    pub snapshot_revision: WireSequence,
    pub operations: Vec<PluginHostDomOperation>,
}
