//! Core-owned local-process plans and stream DTOs.
//!
//! A guest never serializes [`FrozenPluginProcessPlan`]. Core creates it only after the
//! protected permission flow has fixed the executable, arguments, home directory and timeout.

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::PluginApiErrorCode;

pub const MAX_PLUGIN_PROCESS_TIMEOUT_MS: u32 = 120_000;
pub const MAX_PLUGIN_PROCESS_ARGUMENTS: usize = 128;
pub const MAX_PLUGIN_PROCESS_ARGUMENT_BYTES: usize = 4 * 1024;

/// An approved execution plan. It is deliberately not a wire type: no plugin can manufacture
/// an approved program, argument list, environment or working directory through JSON.
#[derive(Clone, PartialEq, Eq)]
pub struct FrozenPluginProcessPlan {
    program: PathBuf,
    arguments: Vec<OsString>,
    user_home: PathBuf,
    timeout_ms: u32,
}

impl fmt::Debug for FrozenPluginProcessPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenPluginProcessPlan")
            .field("argument_count", &self.arguments.len())
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

impl FrozenPluginProcessPlan {
    /// Core calls this after its protected permission decision has frozen the exact target.
    /// This checks transport-safe bounds only; it does not grant any process permission.
    pub fn new(
        program: PathBuf,
        arguments: Vec<OsString>,
        user_home: PathBuf,
        timeout_ms: u32,
    ) -> Result<Self, PluginApiErrorCode> {
        let plan = Self {
            program,
            arguments,
            user_home,
            timeout_ms,
        };
        plan.validate_bounds()?;
        Ok(plan)
    }

    pub fn validate_bounds(&self) -> Result<(), PluginApiErrorCode> {
        if !self.program.is_absolute()
            || !self.user_home.is_absolute()
            || self.timeout_ms == 0
            || self.timeout_ms > MAX_PLUGIN_PROCESS_TIMEOUT_MS
            || self.arguments.len() > MAX_PLUGIN_PROCESS_ARGUMENTS
            || self.arguments.iter().any(|argument| {
                let value = argument.to_string_lossy();
                value.len() > MAX_PLUGIN_PROCESS_ARGUMENT_BYTES || value.contains('\0')
            })
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    #[must_use]
    pub fn user_home(&self) -> &Path {
        &self.user_home
    }

    #[must_use]
    pub const fn timeout_ms(&self) -> u32 {
        self.timeout_ms
    }
}

/// Bounded stdin bytes sent to an already-approved, owner-scoped process resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProcessSendRequest {
    pub handle: String,
    pub data_base64: String,
    /// Explicit EOF for the approved process stdin. It must not carry data.
    #[serde(default)]
    pub close_stdin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginProcessOutputStream {
    Stdout,
    Stderr,
}

#[cfg(test)]
mod tests {
    use super::{FrozenPluginProcessPlan, MAX_PLUGIN_PROCESS_TIMEOUT_MS};
    use crate::PluginApiErrorCode;

    #[test]
    fn frozen_plan_requires_absolute_core_selected_paths_and_bounded_timeout() {
        assert!(matches!(
            FrozenPluginProcessPlan::new("relative".into(), vec![], "/tmp".into(), 1,),
            Err(PluginApiErrorCode::InvalidRequest)
        ));
        assert!(
            FrozenPluginProcessPlan::new(
                "/bin/cat".into(),
                vec![],
                "/tmp".into(),
                MAX_PLUGIN_PROCESS_TIMEOUT_MS,
            )
            .is_ok()
        );
    }
}
