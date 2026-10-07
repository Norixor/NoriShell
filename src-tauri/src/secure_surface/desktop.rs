//! Desktop Secure Surfaces: native windows with the shared secure frame.

use tauri::{AppHandle, Manager as _, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::{SurfaceError, SurfaceSpec};

pub(super) fn open(app: &AppHandle, spec: SurfaceSpec) -> Result<WebviewWindow, SurfaceError> {
    let path = spec.path();
    let builder = WebviewWindowBuilder::new(app, &spec.label, WebviewUrl::App(path.clone().into()))
        .title(&spec.title);
    let mut builder =
        crate::secure_window_frame::apply_secure_window_frame(app, &spec.label, builder);
    if let Some((width, height)) = spec.size {
        builder = builder.inner_size(width, height);
    }
    if let Some((width, height)) = spec.min_size {
        builder = builder.min_inner_size(width, height);
    }
    if let Some(resizable) = spec.resizable {
        builder = builder.resizable(resizable);
    }
    if spec.center {
        builder = builder.center();
    }
    if spec.navigation_guard {
        let expected = app
            .config()
            .build
            .dev_url
            .as_ref()
            .and_then(|url| url.join(&path).ok());
        let entry = format!("/{}", spec.kind.entry());
        let query = spec.query.clone();
        builder = builder
            .on_navigation(move |url| {
                (cfg!(debug_assertions)
                    && expected.as_ref().is_some_and(|expected| expected == url))
                    || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (matches!(url.scheme(), "http" | "https")
                            && url.host_str() == Some("tauri.localhost")
                            && url.port().is_none()))
                        && url.path() == entry
                        && url.query() == query.as_deref())
            })
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    }
    builder
        .build()
        .map_err(|error| SurfaceError(error.to_string()))
}

pub(super) fn show_existing(app: &AppHandle, label: &str) -> Option<Result<(), SurfaceError>> {
    let window = app.get_webview_window(label)?;
    Some(
        crate::window_first_show::show_if_revealed(&window)
            .map(|_| ())
            .map_err(|error| SurfaceError(error.to_string())),
    )
}
