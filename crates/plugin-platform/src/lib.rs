//! Local package integrity, atomic installation and constrained Wasm execution.

mod dom;
mod install;
pub mod operations;
mod package;
mod protocols;
mod runtime;
mod settings;
mod ui;
mod workflows;

pub use dom::*;
pub use install::*;
pub use package::*;
pub use protocols::*;
pub use runtime::*;
pub use settings::*;
pub use ui::*;
pub use workflows::*;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, PluginPlatformError>;

#[derive(Debug, Error)]
pub enum PluginPlatformError {
    #[error("plugin package exceeds its permitted size")]
    PackageTooLarge,
    #[error("plugin package hash changed after local preparation")]
    PackageHashMismatch,
    #[error("plugin package archive is invalid")]
    InvalidArchive,
    #[error("plugin package path is forbidden")]
    RejectedArchivePath,
    #[error("plugin package exceeds extraction limits")]
    ExtractionLimitExceeded,
    #[error("plugin manifest is invalid or incompatible")]
    ManifestMismatch,
    #[error("plugin requires an incompatible Core API version")]
    CoreApiIncompatible,
    #[error("plugin requires a newer application version")]
    AppVersionIncompatible,
    #[error("plugin settings schema is invalid")]
    InvalidSettingsSchema,
    #[error("plugin settings values are invalid")]
    InvalidSettingsValues,
    #[error("plugin protocol catalog is invalid")]
    InvalidProtocolCatalog,
    #[error("plugin workflow catalog is invalid")]
    InvalidWorkflowCatalog,
    #[error("plugin install compare-and-swap conflict")]
    InstallConflict,
    #[error("plugin install commit state is uncertain and requires reconciliation")]
    InstallCommitUncertain,
    #[error("plugin Wasm ABI is invalid")]
    InvalidWasmAbi,
    #[error("plugin Wasm execution exceeded a quota")]
    RuntimeQuotaExceeded,
    #[error("plugin Wasm execution timed out")]
    RuntimeTimedOut,
    #[error("plugin Wasm instance was poisoned and must be rebuilt")]
    RuntimePoisoned,
    #[error("plugin output was rejected")]
    InvalidRuntimeOutput,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}
