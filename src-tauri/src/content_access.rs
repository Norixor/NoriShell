//! User-selected files across platforms.
//!
//! Desktop file dialogs return filesystem paths and every caller keeps its
//! existing path-based flow unchanged. Android's Storage Access Framework
//! returns `content://` URIs instead. Core opens those through the bridge
//! command `contentUriOpen`, receives a detached file descriptor, and reads or
//! writes it directly, so selected bytes never pass through a WebView or JSON.
//!
//! Temporary private copies (only needed when a provider hands out a pipe or a
//! flow needs a named file) live under `<app cache>/content-access/`. The tree
//! is wiped the first time it is used in a process, so copies left behind by a
//! killed process never accumulate; scoped copies are removed on drop.

use std::path::PathBuf;

use tauri_plugin_dialog::FilePath;

#[cfg(target_os = "android")]
mod android;

/// Application exit removes every private copy (`RunEvent::Exit`).
#[cfg(target_os = "android")]
pub(crate) use android::remove_all_private_copies;
#[cfg(target_os = "android")]
pub(crate) use android::{ContentUri, PrivateDir, create_document, pick_document, safe_file_name};

/// Stable failure reasons for selected-file access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentAccessError {
    /// The selection cannot be used on this platform (for example a folder
    /// picker result or a non-file URL).
    Unsupported,
    /// The URI was refused before opening (wrong scheme or an app-owned provider).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Rejected,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    NotFound,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    PermissionDenied,
    /// The platform bridge or private storage is unavailable.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Unavailable,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    TooLarge,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    ReadFailed,
    /// Nothing was written to the selected document.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    WriteFailed,
    /// Writing started but did not finish: the selected document may now be
    /// truncated or partial and the user must be told so.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    WriteIncomplete,
    /// Another Core-initiated document pick is still open.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    PickBusy,
    /// The document picker stayed open past its time bound and was closed.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    PickTimeout,
}

impl ContentAccessError {
    #[allow(
        dead_code,
        reason = "diagnostic code reserved for callers that surface it"
    )]
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "content_access.unsupported",
            Self::Rejected => "content_access.uri_rejected",
            Self::NotFound => "content_access.not_found",
            Self::PermissionDenied => "content_access.permission_denied",
            Self::Unavailable => "content_access.unavailable",
            Self::TooLarge => "content_access.too_large",
            Self::ReadFailed => "content_access.read_failed",
            Self::WriteFailed => "content_access.write_failed",
            Self::WriteIncomplete => "content_access.write_incomplete",
            Self::PickBusy => "content_access.pick_busy",
            Self::PickTimeout => "content_access.pick_timeout",
        }
    }
}

/// A file chosen by the user through a native dialog or passed back by the
/// frontend from a dialog result.
#[derive(Clone, Debug)]
pub(crate) enum Selection {
    Path(PathBuf),
    /// An Android Storage Access Framework document.
    #[cfg(target_os = "android")]
    Content(ContentUri),
}

/// Classifies a dialog result. Desktop behavior equals `FilePath::into_path`.
pub(crate) fn classify_dialog(selection: FilePath) -> Result<Selection, ContentAccessError> {
    #[cfg(target_os = "android")]
    if let FilePath::Url(url) = &selection
        && url.scheme() == "content"
    {
        return ContentUri::parse(url.as_str()).map(Selection::Content);
    }
    selection
        .into_path()
        .map(Selection::Path)
        .map_err(|_| ContentAccessError::Unsupported)
}

/// Classifies a dialog result the frontend received from
/// `@tauri-apps/plugin-dialog` and passed back as text.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn classify_text(text: String) -> Result<Selection, ContentAccessError> {
    #[cfg(target_os = "android")]
    if text.starts_with("content:") {
        return ContentUri::parse(&text).map(Selection::Content);
    }
    Ok(Selection::Path(PathBuf::from(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_dialog_paths_keep_their_path_semantics() {
        let selected = PathBuf::from("/tmp/example.zip");
        assert!(matches!(
            classify_dialog(FilePath::Path(selected.clone())),
            Ok(Selection::Path(path)) if path == selected
        ));
        assert!(matches!(
            classify_text("/tmp/example.zip".to_owned()),
            Ok(Selection::Path(path)) if path == selected
        ));
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            ContentAccessError::WriteIncomplete.code(),
            "content_access.write_incomplete"
        );
        assert_eq!(
            ContentAccessError::Rejected.code(),
            "content_access.uri_rejected"
        );
    }
}
