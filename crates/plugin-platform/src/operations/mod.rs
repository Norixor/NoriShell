//! Pure planning and result parsing for bounded Linux SSH operations.
//!
//! This module never opens an SSH channel or executes a command. Callers must route every
//! [`ApprovalRequirement::CoreProtectedPerExecution`] plan through a fresh Core-owned secure
//! approval before passing the command to the SSH exec transport.

mod plan;
mod result;
mod types;
mod validation;

pub use plan::plan;
pub use result::{ResultParseError, parse_result};
pub use types::*;
