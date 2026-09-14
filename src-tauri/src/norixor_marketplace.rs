use std::{io::Read as _, sync::OnceLock, time::Duration};

#[cfg(test)]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::PluginId;
use norishell_plugin_platform::NorixorV1EmbeddedRoot;
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH},
    redirect::Policy,
};

pub(crate) const PRODUCTION_API_BASE: &str = "https://api.norixor.org";
const PRODUCTION_ROOT_KEY_ID: &str = "norishell-root-20260901-11f625b9cf29";
const PRODUCTION_ROOT_PUBLIC_KEY: [u8; 32] = [
    108, 95, 181, 76, 99, 163, 113, 161, 131, 26, 58, 216, 143, 219, 203, 211, 198, 131, 165, 139,
    5, 39, 212, 247, 4, 26, 199, 22, 219, 42, 170, 48,
];
const USER_AGENT: &str = concat!("NoriShell/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarketplaceFetchError {
    Unavailable,
    Rejected,
    TooLarge,
}

/// A bounded response from the fixed Norixor icon endpoint. The caller still
/// validates image bytes and dimensions before caching or returning them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginIconFetch {
    Image {
        mime_type: String,
        etag: String,
        bytes: Vec<u8>,
    },
    NotModified,
    NotFound,
}

#[derive(Clone)]
pub(crate) struct NorixorMarketplaceClient {
    client: &'static Client,
    api_base: String,
}

impl NorixorMarketplaceClient {
    pub(crate) fn production() -> Result<Self, MarketplaceFetchError> {
        Self::new(PRODUCTION_API_BASE)
    }

    fn new(api_base: &str) -> Result<Self, MarketplaceFetchError> {
        if !api_base.starts_with("https://") || api_base.ends_with('/') {
            return Err(MarketplaceFetchError::Rejected);
        }
        static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
        let client = HTTP_CLIENT.get_or_init(|| {
            std::thread::Builder::new()
                .name("norishell-marketplace-http-init".to_owned())
                .spawn(|| {
                    Client::builder()
                        .https_only(true)
                        .redirect(Policy::none())
                        .connect_timeout(Duration::from_secs(10))
                        .timeout(Duration::from_secs(30))
                        .user_agent(USER_AGENT)
                        .build()
                        .expect("the fixed Norixor marketplace HTTP client must be constructible")
                })
                .expect("the marketplace HTTP initializer thread must start")
                .join()
                .expect("the marketplace HTTP initializer thread must finish")
        });
        Ok(Self {
            client,
            api_base: api_base.to_owned(),
        })
    }

    pub(crate) fn get_api_path(
        &self,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, MarketplaceFetchError> {
        if !valid_api_path(path) {
            return Err(MarketplaceFetchError::Rejected);
        }
        self.get_url(&format!("{}{path}", self.api_base), maximum_bytes)
    }

    pub(crate) fn get_download_url(
        &self,
        url: &str,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, MarketplaceFetchError> {
        if !url.starts_with("https://") {
            return Err(MarketplaceFetchError::Rejected);
        }
        self.get_url(url, maximum_bytes)
    }

    pub(crate) fn get_plugin_icon(
        &self,
        plugin_id: &PluginId,
        if_none_match: Option<&str>,
        maximum_bytes: usize,
    ) -> Result<PluginIconFetch, MarketplaceFetchError> {
        if maximum_bytes == 0 {
            return Err(MarketplaceFetchError::TooLarge);
        }
        let path = format!("/apps/norishell/plugins/{}/icon", plugin_id.as_str());
        if !valid_api_path(&path) {
            return Err(MarketplaceFetchError::Rejected);
        }
        let mut request = self.client.get(format!("{}{path}", self.api_base));
        if let Some(etag) = if_none_match {
            if !valid_etag(etag) {
                return Err(MarketplaceFetchError::Rejected);
            }
            request = request.header(IF_NONE_MATCH, etag);
        }
        let mut response = request
            .send()
            .map_err(|_| MarketplaceFetchError::Unavailable)?;
        match response.status() {
            StatusCode::OK => {
                let mime_type = icon_mime_type(&response)?;
                let etag = response
                    .headers()
                    .get(ETAG)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| valid_etag(value))
                    .ok_or(MarketplaceFetchError::Rejected)?
                    .to_owned();
                if response
                    .content_length()
                    .is_some_and(|length| length > maximum_bytes as u64)
                {
                    return Err(MarketplaceFetchError::TooLarge);
                }
                let mut bytes = Vec::with_capacity(
                    response
                        .content_length()
                        .and_then(|length| usize::try_from(length).ok())
                        .unwrap_or(0)
                        .min(maximum_bytes),
                );
                response
                    .by_ref()
                    .take(maximum_bytes.saturating_add(1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| MarketplaceFetchError::Unavailable)?;
                if bytes.len() > maximum_bytes {
                    return Err(MarketplaceFetchError::TooLarge);
                }
                Ok(PluginIconFetch::Image {
                    mime_type,
                    etag,
                    bytes,
                })
            }
            StatusCode::NOT_MODIFIED => Ok(PluginIconFetch::NotModified),
            StatusCode::NOT_FOUND => {
                // The response contract also uses `Cache-Control: no-store`,
                // but the state must still evict a stale icon if an intermediary
                // strips that advisory header.
                Ok(PluginIconFetch::NotFound)
            }
            status if status.is_server_error() => Err(MarketplaceFetchError::Unavailable),
            _ => Err(MarketplaceFetchError::Rejected),
        }
    }

    fn get_url(&self, url: &str, maximum_bytes: usize) -> Result<Vec<u8>, MarketplaceFetchError> {
        if maximum_bytes == 0 {
            return Err(MarketplaceFetchError::TooLarge);
        }
        let mut response = self
            .client
            .get(url)
            .send()
            .map_err(|_| MarketplaceFetchError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(if response.status().is_server_error() {
                MarketplaceFetchError::Unavailable
            } else {
                MarketplaceFetchError::Rejected
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(MarketplaceFetchError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or(0)
                .min(maximum_bytes),
        );
        response
            .by_ref()
            .take(maximum_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| MarketplaceFetchError::Unavailable)?;
        if bytes.len() > maximum_bytes {
            return Err(MarketplaceFetchError::TooLarge);
        }
        Ok(bytes)
    }
}

fn icon_mime_type(response: &reqwest::blocking::Response) -> Result<String, MarketplaceFetchError> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| matches!(*value, "image/png" | "image/jpeg" | "image/webp"))
        .map(str::to_owned)
        .ok_or(MarketplaceFetchError::Rejected)
}

fn valid_etag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

pub(crate) fn production_embedded_root() -> Option<NorixorV1EmbeddedRoot> {
    NorixorV1EmbeddedRoot::from_embedded_bytes(PRODUCTION_ROOT_KEY_ID, PRODUCTION_ROOT_PUBLIC_KEY)
        .ok()
}

#[cfg(test)]
fn embedded_root_from_values(
    key_id: Option<&'static str>,
    public_key_base64: Option<&'static str>,
) -> Option<NorixorV1EmbeddedRoot> {
    let key_id = key_id?;
    let decoded: [u8; 32] = BASE64.decode(public_key_base64?).ok()?.try_into().ok()?;
    NorixorV1EmbeddedRoot::from_embedded_bytes(key_id, decoded).ok()
}

fn valid_api_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("..")
        && !path.contains('\r')
        && !path.contains('\n')
        && path.is_ascii()
        && path.len() <= 2_048
}

#[cfg(test)]
mod tests {
    use super::{embedded_root_from_values, valid_api_path, valid_etag};

    #[test]
    fn api_paths_are_origin_relative_and_cannot_escape() {
        assert!(valid_api_path("/apps/norishell/plugin-catalog"));
        assert!(!valid_api_path("https://attacker.invalid/catalog"));
        assert!(!valid_api_path("//attacker.invalid/catalog"));
        assert!(!valid_api_path("/apps/../admin"));
        assert!(!valid_api_path("/apps/catalog\r\nX-Test: injected"));
    }

    #[test]
    fn icon_etag_is_opaque_but_bounded() {
        assert!(valid_etag("\"sha256:fixture\""));
        assert!(valid_etag("W/\"sha256:fixture\""));
        assert!(!valid_etag("\r\ninjected"));
    }

    #[test]
    fn embedded_root_never_accepts_missing_or_malformed_build_values() {
        assert!(embedded_root_from_values(None, None).is_none());
        assert!(embedded_root_from_values(Some("fixture"), Some("not-base64")).is_none());
        assert!(
            embedded_root_from_values(
                Some("fixture"),
                Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            )
            .is_some()
        );
    }
}
