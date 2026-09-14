//! Local package integrity, optional signed catalogs, atomic installation and constrained Wasm execution.

mod catalog;
mod dom;
mod install;
mod norixor_v1;
pub mod operations;
mod package;
mod protocols;
mod runtime;
mod settings;
mod ui;
mod workflows;

pub use catalog::*;
pub use dom::*;
pub use install::*;
pub use norixor_v1::*;
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
    #[error("no production catalog trust root is configured")]
    TrustRootsUnavailable,
    #[error("catalog envelope is invalid")]
    InvalidCatalogEnvelope,
    #[error("catalog signature is invalid")]
    InvalidCatalogSignature,
    #[error("catalog payload is invalid")]
    InvalidCatalogPayload,
    #[error("catalog has expired")]
    CatalogExpired,
    #[error("catalog sequence would roll trust state back")]
    CatalogRollback,
    #[error("catalog entry is incompatible")]
    IncompatibleCatalogEntry,
    #[error("plugin package exceeds its permitted size")]
    PackageTooLarge,
    #[error("plugin package size does not match the signed catalog")]
    PackageSizeMismatch,
    #[error("plugin package hash does not match the signed catalog")]
    PackageHashMismatch,
    #[error("publisher signature is invalid")]
    InvalidPublisherSignature,
    #[error("plugin package archive is invalid")]
    InvalidArchive,
    #[error("plugin package path is forbidden")]
    RejectedArchivePath,
    #[error("plugin package exceeds extraction limits")]
    ExtractionLimitExceeded,
    #[error("plugin manifest does not exactly match the signed catalog entry")]
    ManifestMismatch,
    #[error("plugin settings schema is invalid")]
    InvalidSettingsSchema,
    #[error("plugin settings values are invalid")]
    InvalidSettingsValues,
    #[error("plugin protocol catalog is invalid")]
    InvalidProtocolCatalog,
    #[error("plugin workflow catalog is invalid")]
    InvalidWorkflowCatalog,
    #[error("Norixor NoriShell plugin wire is invalid")]
    InvalidNorixorWire,
    #[error("Norixor NoriShell plugin trust root is invalid")]
    InvalidNorixorRoot,
    #[error("Norixor NoriShell plugin catalog is invalid")]
    InvalidNorixorCatalog,
    #[error("Norixor NoriShell plugin publisher delegation is invalid")]
    InvalidNorixorPublisher,
    #[error("Norixor NoriShell plugin download response is invalid")]
    InvalidNorixorDownload,
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
