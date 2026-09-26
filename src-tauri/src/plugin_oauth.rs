//! Core-owned, plugin-scoped native account session.
//!
//! A plugin supplies bounded credential endpoints and decides when login or
//! synchronization is needed. Core resolves only host-owned Page field
//! references, performs the credential exchange, owns refresh-token storage,
//! and exposes non-secret profile status. Profiles are isolated by plugin,
//! signer, provider ID and configuration digest.

use std::{
    collections::{BTreeSet, VecDeque},
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use norishell_core_api::{PluginOperationId, SecretRefId};
use norishell_secret_vault::SecretKind;
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use thiserror::Error;
use tokio::{sync::Mutex as AsyncMutex, time::Instant};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::vault_service::{VaultSecretInsert, VaultService, VaultServiceError};

const STATE_SCHEMA_VERSION: u16 = 2;
const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 256 * 1024;
const MAX_TRANSFER_SIGNERS: usize = 4096;
const MAX_TRANSFER_MARKERS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeAccountState {
    Disconnected,
    Authorizing,
    NeedsMfa,
    NeedsEmailVerification,
    Connected,
    Expired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeAuthStatus {
    pub(crate) account_state: NativeAccountState,
    pub(crate) stable_error_code: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginOAuthConfiguration {
    pub(crate) plugin_id: String,
    pub(crate) signer_fingerprint_sha256: String,
    pub(crate) profile_id: String,
    pub(crate) authorization_url: Url,
    pub(crate) login_url: Option<Url>,
    pub(crate) registration_url: Option<Url>,
    pub(crate) email_verification_url: Option<Url>,
    pub(crate) mfa_url: Option<Url>,
    pub(crate) token_url: Url,
    pub(crate) revoke_url: Url,
    pub(crate) client_id: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) resource_origins: Vec<String>,
}

#[derive(Debug, Error)]
pub(crate) enum NativeAuthError {
    #[error("the local Vault must be unlocked")]
    VaultLocked,
    #[error("the local authorization state is unavailable")]
    StateUnavailable,
    #[error("this account is not signed in")]
    SessionMissing,
    #[error("another account operation is already running")]
    OperationInProgress,
    #[error("the saved account session has expired")]
    RefreshExpired,
    #[error("authorization was denied")]
    AccessDenied,
    #[error("the authorization server response was invalid")]
    Protocol,
    #[error("the authorization service is unavailable")]
    Network,
    #[error("the encrypted local session could not be updated")]
    LocalCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistStateError {
    BeforeCommit,
    CommitStateUnknown,
}

impl From<PersistStateError> for NativeAuthError {
    fn from(_: PersistStateError) -> Self {
        Self::LocalCommit
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthState {
    schema_version: u16,
    owner_plugin_id: String,
    owner_signer_fingerprint_sha256: String,
    profile_id: String,
    configuration_sha256: String,
    refresh_token_ref: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    retired_secret_refs: Vec<String>,
}

impl Default for StoredAuthState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            owner_plugin_id: String::new(),
            owner_signer_fingerprint_sha256: String::new(),
            profile_id: String::new(),
            configuration_sha256: String::new(),
            refresh_token_ref: None,
            session_id: None,
            retired_secret_refs: Vec::new(),
        }
    }
}

struct AccessToken {
    value: Zeroizing<String>,
    expires_at: Instant,
}

struct RuntimeState {
    stored: Option<StoredAuthState>,
    access_token: Option<AccessToken>,
    refresh_expired: bool,
    authorizing: bool,
    pending_challenge: Option<PendingAuthChallenge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAuthChallengeKind {
    Mfa,
    EmailVerification,
}

struct PendingAuthChallenge {
    kind: PendingAuthChallengeKind,
    token: Zeroizing<String>,
}

#[derive(Clone)]
pub(crate) struct PluginOAuthService {
    configuration: Arc<PluginOAuthConfiguration>,
    http: Client,
    state_path: Arc<PathBuf>,
    vault: VaultService,
    runtime: Arc<Mutex<RuntimeState>>,
    operation: Arc<AsyncMutex<()>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
    nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialChallengeResponse {
    challenge_type: String,
    challenge_token: String,
}

#[derive(Serialize)]
struct CredentialAuthForm<'a> {
    client_id: &'a str,
    username: &'a str,
    password: &'a str,
    scope: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
}

#[derive(Serialize)]
struct CredentialChallengeForm<'a> {
    client_id: &'a str,
    challenge_token: &'a str,
    code: &'a str,
}

enum CredentialAuthResponse {
    Token(OAuthTokenResponse),
    Challenge(PendingAuthChallenge),
}

#[derive(Clone, Copy)]
enum TokenAcceptance {
    NewSession,
    Refresh,
}

impl PluginOAuthService {
    pub(crate) fn start(
        app_data_directory: impl AsRef<Path>,
        vault: VaultService,
        configuration: PluginOAuthConfiguration,
    ) -> Result<Self, NativeAuthError> {
        validate_configuration(&configuration)?;
        let configuration_sha256 = configuration_sha256(&configuration);
        let profile_hash = sha256_text(&format!(
            "{}\0{}\0{}\0{}",
            configuration.plugin_id,
            configuration.signer_fingerprint_sha256,
            configuration.profile_id,
            configuration_sha256,
        ));
        let state_path = app_data_directory
            .as_ref()
            .join("plugins")
            .join("oauth")
            .join(sha256_text(&configuration.plugin_id))
            .join(format!("{profile_hash}.json"));
        transfer_matching_state(app_data_directory.as_ref(), &configuration, &state_path)?;
        let stored = load_state(&state_path).ok().and_then(|mut state| {
            // An empty account record owns no authorization material, so an
            // updated plugin/provider configuration can safely rebind it. A
            // record containing current or retired token refs remains bound to
            // its exact original configuration and fails closed on mismatch.
            if state.refresh_token_ref.is_none() && state.retired_secret_refs.is_empty() {
                state.owner_plugin_id = configuration.plugin_id.clone();
                state.owner_signer_fingerprint_sha256 =
                    configuration.signer_fingerprint_sha256.clone();
                state.profile_id = configuration.profile_id.clone();
                state.configuration_sha256 = configuration_sha256.clone();
            }
            (state.owner_plugin_id == configuration.plugin_id
                && state.owner_signer_fingerprint_sha256 == configuration.signer_fingerprint_sha256
                && state.profile_id == configuration.profile_id
                && state.configuration_sha256 == configuration_sha256)
                .then_some(state)
        });
        Ok(Self {
            configuration: Arc::new(configuration),
            http: Client::builder()
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("NoriShell/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("plugin OAuth HTTP client configuration must be valid"),
            state_path: Arc::new(state_path),
            vault,
            runtime: Arc::new(Mutex::new(RuntimeState {
                stored,
                access_token: None,
                refresh_expired: false,
                authorizing: false,
                pending_challenge: None,
            })),
            operation: Arc::new(AsyncMutex::new(())),
        })
    }

    pub(crate) fn status(&self) -> NativeAuthStatus {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stored) = runtime.stored.as_ref() else {
            return NativeAuthStatus {
                account_state: NativeAccountState::Unavailable,
                stable_error_code: Some("stateUnavailable"),
            };
        };
        let account_state = if runtime.authorizing {
            NativeAccountState::Authorizing
        } else if let Some(challenge) = runtime.pending_challenge.as_ref() {
            match challenge.kind {
                PendingAuthChallengeKind::Mfa => NativeAccountState::NeedsMfa,
                PendingAuthChallengeKind::EmailVerification => {
                    NativeAccountState::NeedsEmailVerification
                }
            }
        } else if runtime
            .access_token
            .as_ref()
            .is_some_and(|token| token.expires_at > Instant::now())
        {
            NativeAccountState::Connected
        } else if runtime.refresh_expired {
            NativeAccountState::Expired
        } else if stored.refresh_token_ref.is_some() {
            // A persisted refresh token is an active account session from the
            // user's perspective. Core refreshes the short-lived access token
            // lazily when the next protected network operation starts.
            NativeAccountState::Connected
        } else {
            NativeAccountState::Disconnected
        };
        NativeAuthStatus {
            account_state,
            stable_error_code: None,
        }
    }

    pub(crate) fn configuration_digest(&self) -> String {
        configuration_sha256(&self.configuration)
    }

    pub(crate) fn session_id(&self) -> Option<String> {
        if !self.vault.is_unlocked() {
            return None;
        }
        let (session_id, refresh_token_ref) = {
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if runtime.refresh_expired {
                return None;
            }
            let stored = runtime.stored.as_ref()?;
            (
                stored.session_id.clone()?,
                stored.refresh_token_ref.clone()?,
            )
        };
        let secret_ref = SecretRefId::parse(&refresh_token_ref).ok()?;
        let token = self
            .vault
            .read_secret(&secret_ref, SecretKind::OAuthRefreshToken)
            .ok()?;
        if token.expose().is_empty() || token.expose().len() > MAX_TOKEN_BYTES {
            return None;
        }
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stored = runtime.stored.as_ref()?;
        (!runtime.refresh_expired
            && stored.session_id.as_deref() == Some(&session_id)
            && stored.refresh_token_ref.as_deref() == Some(&refresh_token_ref))
        .then_some(session_id)
    }

    pub(crate) fn matches_configuration(&self, configuration: &PluginOAuthConfiguration) -> bool {
        self.configuration.as_ref() == configuration
    }

    #[cfg(test)]
    pub(crate) fn shares_runtime_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.runtime, &other.runtime)
    }

    pub(crate) fn allows_resource_url(&self, url: &Url) -> bool {
        let candidate = canonical_resource_origin(url);
        self.configuration.resource_origins.iter().any(|allowed| {
            allowed.len() == candidate.len()
                && bool::from(allowed.as_bytes().ct_eq(candidate.as_bytes()))
        })
    }

    pub(crate) fn clear_runtime(&self) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime.access_token = None;
        runtime.authorizing = false;
        runtime.pending_challenge = None;
    }

    pub(crate) async fn login(
        &self,
        username: Zeroizing<String>,
        password: Zeroizing<String>,
    ) -> Result<NativeAuthStatus, NativeAuthError> {
        let endpoint = self
            .configuration
            .login_url
            .clone()
            .ok_or(NativeAuthError::Protocol)?;
        self.credential_auth(endpoint, username, password, None)
            .await
    }

    pub(crate) async fn register(
        &self,
        username: Zeroizing<String>,
        password: Zeroizing<String>,
        display_name: Option<Zeroizing<String>>,
    ) -> Result<NativeAuthStatus, NativeAuthError> {
        let endpoint = self
            .configuration
            .registration_url
            .clone()
            .ok_or(NativeAuthError::Protocol)?;
        self.credential_auth(endpoint, username, password, display_name)
            .await
    }

    pub(crate) async fn complete_mfa(
        &self,
        code: Zeroizing<String>,
    ) -> Result<NativeAuthStatus, NativeAuthError> {
        let endpoint = self
            .configuration
            .mfa_url
            .clone()
            .ok_or(NativeAuthError::Protocol)?;
        self.complete_challenge(PendingAuthChallengeKind::Mfa, endpoint, code)
            .await
    }

    pub(crate) async fn verify_email(
        &self,
        code: Zeroizing<String>,
    ) -> Result<NativeAuthStatus, NativeAuthError> {
        let endpoint = self
            .configuration
            .email_verification_url
            .clone()
            .ok_or(NativeAuthError::Protocol)?;
        self.complete_challenge(PendingAuthChallengeKind::EmailVerification, endpoint, code)
            .await
    }

    async fn credential_auth(
        &self,
        endpoint: Url,
        username: Zeroizing<String>,
        password: Zeroizing<String>,
        display_name: Option<Zeroizing<String>>,
    ) -> Result<NativeAuthStatus, NativeAuthError> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| NativeAuthError::OperationInProgress)?;
        if !self.vault.is_unlocked() {
            return Err(NativeAuthError::VaultLocked);
        }
        self.stored_state()?;
        self.set_authorizing(true);
        let scope = self.configuration.scopes.join(" ");
        let form = CredentialAuthForm {
            client_id: self.configuration.client_id.as_str(),
            username: username.as_str(),
            password: password.as_str(),
            scope: &scope,
            display_name: display_name.as_deref().map(String::as_str),
        };
        let response = match self.http.post(endpoint).form(&form).send().await {
            Ok(response) => parse_credential_auth_response(response).await,
            Err(_) => Err(NativeAuthError::Network),
        };
        self.set_authorizing(false);
        match response? {
            CredentialAuthResponse::Token(token) => {
                self.persist_token_response(token, TokenAcceptance::NewSession)?;
                self.runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pending_challenge = None;
            }
            CredentialAuthResponse::Challenge(challenge) => {
                self.runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pending_challenge = Some(challenge);
            }
        }
        Ok(self.status())
    }

    async fn complete_challenge(
        &self,
        expected_kind: PendingAuthChallengeKind,
        endpoint: Url,
        code: Zeroizing<String>,
    ) -> Result<NativeAuthStatus, NativeAuthError> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| NativeAuthError::OperationInProgress)?;
        if !self.vault.is_unlocked() {
            return Err(NativeAuthError::VaultLocked);
        }
        let challenge_token = {
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let challenge = runtime
                .pending_challenge
                .as_ref()
                .filter(|challenge| challenge.kind == expected_kind)
                .ok_or(NativeAuthError::StateUnavailable)?;
            Zeroizing::new(challenge.token.to_string())
        };
        self.set_authorizing(true);
        let form = CredentialChallengeForm {
            client_id: self.configuration.client_id.as_str(),
            challenge_token: challenge_token.as_str(),
            code: code.as_str(),
        };
        let response = match self.http.post(endpoint).form(&form).send().await {
            Ok(response) => parse_credential_auth_response(response).await,
            Err(_) => Err(NativeAuthError::Network),
        };
        self.set_authorizing(false);
        match response? {
            CredentialAuthResponse::Token(token) => {
                self.persist_token_response(token, TokenAcceptance::NewSession)?;
                self.runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pending_challenge = None;
            }
            CredentialAuthResponse::Challenge(challenge) => {
                self.runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pending_challenge = Some(challenge);
            }
        }
        Ok(self.status())
    }

    pub(crate) async fn access_token(&self) -> Result<Zeroizing<String>, NativeAuthError> {
        if let Some(token) = self.current_access_token() {
            return Ok(token);
        }
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| NativeAuthError::OperationInProgress)?;
        if let Some(token) = self.current_access_token() {
            return Ok(token);
        }
        if !self.vault.is_unlocked() {
            return Err(NativeAuthError::VaultLocked);
        }
        let refresh_token = self.read_refresh_token()?;
        let response = self
            .http
            .post(self.configuration.token_url.clone())
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.configuration.client_id.as_str()),
                ("refresh_token", refresh_token.as_str()),
            ])
            .send()
            .await
            .map_err(|_| NativeAuthError::Network)?;
        let response = match parse_token_response(response).await {
            Ok(response) => response,
            Err(NativeAuthError::RefreshExpired) => {
                let mut runtime = self
                    .runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                runtime.access_token = None;
                runtime.refresh_expired = true;
                drop(runtime);
                self.invalidate_session_id()?;
                return Err(NativeAuthError::RefreshExpired);
            }
            Err(error) => return Err(error),
        };
        let access = Zeroizing::new(response.access_token.clone());
        self.persist_token_response(response, TokenAcceptance::Refresh)?;
        Ok(access)
    }

    pub(crate) async fn logout(&self) -> Result<NativeAuthStatus, NativeAuthError> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| NativeAuthError::OperationInProgress)?;
        if !self.vault.is_unlocked() {
            return Err(NativeAuthError::VaultLocked);
        }
        let refresh_token = self.read_refresh_token()?;
        let response = self
            .http
            .post(self.configuration.revoke_url.clone())
            .form(&[
                ("client_id", self.configuration.client_id.as_str()),
                ("token", refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|_| NativeAuthError::Network)?;
        if response.status() != StatusCode::OK
            || response.bytes().await.map_or(true, |body| !body.is_empty())
        {
            return Err(NativeAuthError::Protocol);
        }
        self.clear_persisted_session()?;
        Ok(self.status())
    }

    fn current_access_token(&self) -> Option<Zeroizing<String>> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .access_token
            .as_ref()
            .filter(|token| token.expires_at > Instant::now() + Duration::from_secs(30))
            .map(|token| Zeroizing::new(token.value.to_string()))
    }

    fn persist_token_response(
        &self,
        mut response: OAuthTokenResponse,
        acceptance: TokenAcceptance,
    ) -> Result<(), NativeAuthError> {
        validate_token_response(&response, &self.configuration.scopes)?;
        let new_ref = SecretRefId::new();
        let refresh_value = Zeroizing::new(response.refresh_token.as_bytes().to_vec());
        self.vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: new_ref.clone(),
                kind: SecretKind::OAuthRefreshToken,
                value: refresh_value,
            }])
            .map_err(map_vault_error)?;

        let mut next = self.stored_state()?;
        next.configuration_sha256 = configuration_sha256(&self.configuration);
        if matches!(acceptance, TokenAcceptance::NewSession)
            || (next.session_id.is_none() && next.refresh_token_ref.is_some())
        {
            next.session_id = Some(Uuid::new_v4().to_string());
        }
        if let Some(previous) = next.refresh_token_ref.replace(new_ref.as_str().to_owned())
            && previous != new_ref.as_str()
            && !next.retired_secret_refs.contains(&previous)
        {
            next.retired_secret_refs.push(previous);
        }
        if let Err(error) = persist_state(&self.state_path, &next) {
            if error == PersistStateError::BeforeCommit {
                let _ = self.vault.delete_secrets(std::slice::from_ref(&new_ref));
            }
            self.mark_state_unavailable();
            response.access_token.zeroize();
            response.refresh_token.zeroize();
            return Err(error.into());
        }
        {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime.stored = Some(next);
            runtime.refresh_expired = false;
            runtime.access_token = Some(AccessToken {
                value: Zeroizing::new(std::mem::take(&mut response.access_token)),
                expires_at: Instant::now()
                    + Duration::from_secs(response.expires_in.saturating_sub(30).max(1)),
            });
        }
        response.refresh_token.zeroize();
        self.cleanup_retired_secrets()
    }

    fn read_refresh_token(&self) -> Result<Zeroizing<String>, NativeAuthError> {
        let stored = self.stored_state()?;
        let secret_ref = stored
            .refresh_token_ref
            .as_deref()
            .ok_or(NativeAuthError::SessionMissing)
            .and_then(|value| {
                SecretRefId::parse(value).map_err(|_| NativeAuthError::StateUnavailable)
            })?;
        let value = self
            .vault
            .read_secret(&secret_ref, SecretKind::OAuthRefreshToken)
            .map_err(map_vault_error)?;
        String::from_utf8(value.expose().to_vec())
            .map(Zeroizing::new)
            .map_err(|_| NativeAuthError::StateUnavailable)
    }

    fn clear_persisted_session(&self) -> Result<(), NativeAuthError> {
        let mut next = self.stored_state()?;
        next.session_id = None;
        if let Some(current) = next.refresh_token_ref.take()
            && !next.retired_secret_refs.contains(&current)
        {
            next.retired_secret_refs.push(current);
        }
        if let Err(error) = persist_state(&self.state_path, &next) {
            self.mark_state_unavailable();
            return Err(error.into());
        }
        {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime.stored = Some(next);
            runtime.access_token = None;
            runtime.refresh_expired = false;
        }
        self.cleanup_retired_secrets()
    }

    fn invalidate_session_id(&self) -> Result<(), NativeAuthError> {
        let mut next = self.stored_state()?;
        if next.session_id.take().is_none() {
            return Ok(());
        }
        if let Err(error) = persist_state(&self.state_path, &next) {
            self.mark_state_unavailable();
            return Err(error.into());
        }
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stored = Some(next);
        Ok(())
    }

    fn cleanup_retired_secrets(&self) -> Result<(), NativeAuthError> {
        let mut next = self.stored_state()?;
        let refs = next
            .retired_secret_refs
            .iter()
            .map(|value| SecretRefId::parse(value).map_err(|_| NativeAuthError::StateUnavailable))
            .collect::<Result<Vec<_>, _>>()?;
        if refs.is_empty() {
            return Ok(());
        }
        self.vault.delete_secrets(&refs).map_err(map_vault_error)?;
        next.retired_secret_refs.clear();
        if let Err(error) = persist_state(&self.state_path, &next) {
            self.mark_state_unavailable();
            return Err(error.into());
        }
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stored = Some(next);
        Ok(())
    }

    fn stored_state(&self) -> Result<StoredAuthState, NativeAuthError> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime
            .stored
            .clone()
            .ok_or(NativeAuthError::StateUnavailable)
    }

    fn mark_state_unavailable(&self) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime.stored = None;
        runtime.access_token = None;
    }

    fn set_authorizing(&self, authorizing: bool) {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .authorizing = authorizing;
    }
}

fn transfer_marker_path(app_data_directory: &Path, plugin_id: &str, new_signer: &str) -> PathBuf {
    app_data_directory
        .join("plugins")
        .join("oauth")
        .join(sha256_text(plugin_id))
        .join(format!(".transfer-{new_signer}"))
}

fn transfer_rollback_path(
    app_data_directory: &Path,
    plugin_id: &str,
    operation_id: &PluginOperationId,
) -> PathBuf {
    transfer_marker_path(app_data_directory, plugin_id, "").with_file_name(format!(
        ".oauth-transfer-rollback-{}",
        operation_id.as_str()
    ))
}

fn read_transfer_signers(path: &Path) -> Result<Vec<String>, NativeAuthError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| NativeAuthError::StateUnavailable)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > (MAX_TRANSFER_SIGNERS * 65) as u64
    {
        return Err(NativeAuthError::StateUnavailable);
    }
    let value = fs::read_to_string(path).map_err(|_| NativeAuthError::StateUnavailable)?;
    let signers = value.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut unique = BTreeSet::new();
    if signers.is_empty()
        || signers.len() > MAX_TRANSFER_SIGNERS
        || signers
            .iter()
            .any(|signer| !valid_sha256(signer) || !unique.insert(signer))
    {
        return Err(NativeAuthError::StateUnavailable);
    }
    Ok(signers)
}

fn write_transfer_signers(path: &Path, signers: &[String]) -> Result<(), NativeAuthError> {
    let parent = path.parent().ok_or(NativeAuthError::LocalCommit)?;
    fs::create_dir_all(parent).map_err(|_| NativeAuthError::LocalCommit)?;
    let temporary = parent.join(format!(".oauth-transfer-write-{}", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(signers.join("\n").as_bytes())?;
        file.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(NativeAuthError::LocalCommit);
    }
    if replace_file(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(NativeAuthError::LocalCommit);
    }
    sync_parent_directory(parent).map_err(|_| NativeAuthError::LocalCommit)
}

pub(crate) fn record_oauth_signer_transfer(
    app_data_directory: &Path,
    plugin_id: &str,
    old_signer: &str,
    new_signer: &str,
    operation_id: &PluginOperationId,
) -> Result<(), NativeAuthError> {
    if !valid_identifier(plugin_id, 160)
        || !valid_sha256(old_signer)
        || !valid_sha256(new_signer)
        || old_signer == new_signer
    {
        return Err(NativeAuthError::StateUnavailable);
    }
    let path = transfer_marker_path(app_data_directory, plugin_id, new_signer);
    let mut signers = match fs::symlink_metadata(&path) {
        Ok(_) => read_transfer_signers(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err(NativeAuthError::StateUnavailable),
    };
    let rollback = transfer_rollback_path(app_data_directory, plugin_id, operation_id);
    let parent = rollback.parent().ok_or(NativeAuthError::LocalCommit)?;
    fs::create_dir_all(parent).map_err(|_| NativeAuthError::LocalCommit)?;
    if fs::symlink_metadata(&rollback).is_ok() {
        return Err(NativeAuthError::StateUnavailable);
    }
    let temporary = parent.join(format!(".oauth-transfer-write-{}", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let snapshot_text = format!("{new_signer}\n{}", signers.join("\n"));
    let write_result = (|| -> std::io::Result<()> {
        let mut snapshot = options.open(&temporary)?;
        snapshot.write_all(snapshot_text.as_bytes())?;
        snapshot.sync_all()
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(NativeAuthError::LocalCommit);
    }
    if fs::rename(&temporary, &rollback).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(NativeAuthError::LocalCommit);
    }
    sync_parent_directory(parent).map_err(|_| NativeAuthError::LocalCommit)?;
    if signers.first().is_some_and(|signer| signer == old_signer) {
        return Ok(());
    }
    signers.retain(|signer| signer != old_signer);
    signers.insert(0, old_signer.to_owned());
    if signers.len() > MAX_TRANSFER_SIGNERS {
        return Err(NativeAuthError::StateUnavailable);
    }
    write_transfer_signers(&path, &signers)
}

pub(crate) fn discard_oauth_signer_transfer(
    app_data_directory: &Path,
    plugin_id: &str,
    operation_id: &PluginOperationId,
) -> Result<(), NativeAuthError> {
    let rollback = transfer_rollback_path(app_data_directory, plugin_id, operation_id);
    let (new_signer, original) = match read_transfer_rollback(&rollback)? {
        Some(snapshot) => snapshot,
        None => return Ok(()),
    };
    let marker = transfer_marker_path(app_data_directory, plugin_id, &new_signer);
    if original.is_empty() {
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_file() => {
                fs::remove_file(&marker).map_err(|_| NativeAuthError::LocalCommit)?;
            }
            Ok(_) => return Err(NativeAuthError::StateUnavailable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(NativeAuthError::LocalCommit),
        }
    } else {
        write_transfer_signers(&marker, &original)?;
    }
    finalize_oauth_signer_transfer(app_data_directory, plugin_id, operation_id)
}

fn read_transfer_rollback(path: &Path) -> Result<Option<(String, Vec<String>)>, NativeAuthError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(NativeAuthError::StateUnavailable),
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > ((MAX_TRANSFER_SIGNERS + 1) * 65) as u64
    {
        return Err(NativeAuthError::StateUnavailable);
    }
    let value = fs::read_to_string(path).map_err(|_| NativeAuthError::StateUnavailable)?;
    let (new_signer, previous) = value
        .split_once('\n')
        .ok_or(NativeAuthError::StateUnavailable)?;
    if !valid_sha256(new_signer) {
        return Err(NativeAuthError::StateUnavailable);
    }
    let signers = if previous.is_empty() {
        Vec::new()
    } else {
        previous.lines().map(str::to_owned).collect::<Vec<_>>()
    };
    let mut unique = BTreeSet::new();
    if signers.len() > MAX_TRANSFER_SIGNERS
        || signers
            .iter()
            .any(|signer| !valid_sha256(signer) || !unique.insert(signer))
    {
        return Err(NativeAuthError::StateUnavailable);
    }
    Ok(Some((new_signer.to_owned(), signers)))
}

pub(crate) fn finalize_oauth_signer_transfer(
    app_data_directory: &Path,
    plugin_id: &str,
    operation_id: &PluginOperationId,
) -> Result<(), NativeAuthError> {
    let rollback = transfer_rollback_path(app_data_directory, plugin_id, operation_id);
    match fs::symlink_metadata(&rollback) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::remove_file(&rollback).map_err(|_| NativeAuthError::LocalCommit)?;
            sync_parent_directory(rollback.parent().ok_or(NativeAuthError::LocalCommit)?)
                .map_err(|_| NativeAuthError::LocalCommit)
        }
        Ok(_) => Err(NativeAuthError::StateUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(NativeAuthError::LocalCommit),
    }
}

fn transfer_matching_state(
    app_data_directory: &Path,
    configuration: &PluginOAuthConfiguration,
    destination: &Path,
) -> Result<(), NativeAuthError> {
    let mut queue = VecDeque::from([configuration.signer_fingerprint_sha256.clone()]);
    let mut visited = BTreeSet::new();
    while let Some(signer) = queue.pop_front() {
        if !visited.insert(signer.clone()) {
            continue;
        }
        if visited.len() > MAX_TRANSFER_MARKERS {
            return Err(NativeAuthError::StateUnavailable);
        }
        let marker = transfer_marker_path(app_data_directory, &configuration.plugin_id, &signer);
        let old_signers = match fs::symlink_metadata(&marker) {
            Ok(_) => read_transfer_signers(&marker)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(NativeAuthError::StateUnavailable),
        };
        for old_signer in old_signers {
            if visited.contains(&old_signer) {
                continue;
            }
            let mut previous = configuration.clone();
            previous.signer_fingerprint_sha256 = old_signer.clone();
            let previous_digest = configuration_sha256(&previous);
            let previous_path = destination
                .parent()
                .ok_or(NativeAuthError::StateUnavailable)?
                .join(format!(
                    "{}.json",
                    sha256_text(&format!(
                        "{}\0{}\0{}\0{}",
                        previous.plugin_id, old_signer, previous.profile_id, previous_digest
                    ))
                ));
            if previous_path.exists() {
                let mut state = load_state(&previous_path)?;
                if state.owner_plugin_id != configuration.plugin_id
                    || state.owner_signer_fingerprint_sha256 != old_signer
                    || state.profile_id != configuration.profile_id
                    || state.configuration_sha256 != previous_digest
                {
                    continue;
                }
                if destination.exists() {
                    let current = load_state(destination)?;
                    if current.owner_plugin_id == configuration.plugin_id
                        && current.owner_signer_fingerprint_sha256
                            == configuration.signer_fingerprint_sha256
                        && current.profile_id == configuration.profile_id
                        && current.configuration_sha256 == configuration_sha256(configuration)
                        && current.refresh_token_ref == state.refresh_token_ref
                        && current.session_id == state.session_id
                    {
                        fs::remove_file(&previous_path)
                            .map_err(|_| NativeAuthError::LocalCommit)?;
                        sync_parent_directory(
                            destination.parent().ok_or(NativeAuthError::LocalCommit)?,
                        )
                        .map_err(|_| NativeAuthError::LocalCommit)?;
                    }
                    return Ok(());
                }
                state.owner_signer_fingerprint_sha256 =
                    configuration.signer_fingerprint_sha256.clone();
                state.configuration_sha256 = configuration_sha256(configuration);
                persist_state(destination, &state).map_err(NativeAuthError::from)?;
                fs::remove_file(&previous_path).map_err(|_| NativeAuthError::LocalCommit)?;
                sync_parent_directory(destination.parent().ok_or(NativeAuthError::LocalCommit)?)
                    .map_err(|_| NativeAuthError::LocalCommit)?;
                return Ok(());
            }
            queue.push_back(old_signer);
        }
    }
    Ok(())
}

pub(crate) fn delete_plugin_oauth_data(
    app_data_directory: &Path,
    vault: &VaultService,
    plugin_id: &str,
) -> Result<(), NativeAuthError> {
    if !valid_identifier(plugin_id, 160) {
        return Err(NativeAuthError::StateUnavailable);
    }
    let directory = app_data_directory
        .join("plugins")
        .join("oauth")
        .join(sha256_text(plugin_id));
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(NativeAuthError::LocalCommit),
    };
    let mut matching = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_TRANSFER_MARKERS + 256 + 64 {
            return Err(NativeAuthError::StateUnavailable);
        }
        let entry = entry.map_err(|_| NativeAuthError::LocalCommit)?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| NativeAuthError::LocalCommit)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(NativeAuthError::StateUnavailable);
        }
        let state = load_state(&path)?;
        if state.owner_plugin_id == plugin_id {
            matching.push((path, state));
            if matching.len() > 256 {
                return Err(NativeAuthError::StateUnavailable);
            }
        }
    }
    for (path, state) in matching {
        delete_state_data(&path, vault, state)?;
    }
    for entry in fs::read_dir(&directory).map_err(|_| NativeAuthError::LocalCommit)? {
        let entry = entry.map_err(|_| NativeAuthError::LocalCommit)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".transfer-")
            || name.starts_with(".oauth-transfer-write-")
            || name.starts_with(".oauth-transfer-rollback-")
        {
            let metadata =
                fs::symlink_metadata(entry.path()).map_err(|_| NativeAuthError::LocalCommit)?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(NativeAuthError::StateUnavailable);
            }
            fs::remove_file(entry.path()).map_err(|_| NativeAuthError::LocalCommit)?;
        }
    }
    fs::remove_dir(&directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeAuthError::StateUnavailable
        } else {
            NativeAuthError::LocalCommit
        }
    })?;
    if let Some(parent) = directory.parent() {
        sync_parent_directory(parent).map_err(|_| NativeAuthError::LocalCommit)?;
    }
    Ok(())
}

fn delete_state_data(
    path: &Path,
    vault: &VaultService,
    mut state: StoredAuthState,
) -> Result<(), NativeAuthError> {
    if let Some(current) = state.refresh_token_ref.take()
        && !state.retired_secret_refs.contains(&current)
    {
        state.retired_secret_refs.push(current);
    }
    state.session_id = None;
    persist_state(path, &state).map_err(NativeAuthError::from)?;
    let refs = state
        .retired_secret_refs
        .iter()
        .map(|value| SecretRefId::parse(value).map_err(|_| NativeAuthError::StateUnavailable))
        .collect::<Result<Vec<_>, _>>()?;
    if !refs.is_empty() {
        vault.delete_secrets(&refs).map_err(map_vault_error)?;
    }
    state.retired_secret_refs.clear();
    persist_state(path, &state).map_err(NativeAuthError::from)?;
    fs::remove_file(path).map_err(|_| NativeAuthError::LocalCommit)?;
    if let Some(parent) = path.parent() {
        sync_parent_directory(parent).map_err(|_| NativeAuthError::LocalCommit)?;
    }
    Ok(())
}

fn map_vault_error(error: VaultServiceError) -> NativeAuthError {
    match error {
        VaultServiceError::NotUnlocked => NativeAuthError::VaultLocked,
        _ => NativeAuthError::LocalCommit,
    }
}

fn validate_configuration(configuration: &PluginOAuthConfiguration) -> Result<(), NativeAuthError> {
    let direct_endpoints = [
        configuration.login_url.as_ref(),
        configuration.registration_url.as_ref(),
        configuration.email_verification_url.as_ref(),
        configuration.mfa_url.as_ref(),
    ];
    let direct_mode = direct_endpoints.iter().all(|endpoint| endpoint.is_some());
    let mixed_direct_mode =
        direct_endpoints.iter().any(|endpoint| endpoint.is_some()) && !direct_mode;
    if !valid_identifier(&configuration.plugin_id, 160)
        || !valid_sha256(&configuration.signer_fingerprint_sha256)
        || !valid_identifier(&configuration.profile_id, 160)
        || !valid_identifier(&configuration.client_id, 256)
        || configuration.scopes.is_empty()
        || configuration.scopes.len() > 32
        || configuration
            .scopes
            .iter()
            .any(|scope| !valid_identifier(scope, 160))
        || !valid_oauth_url(&configuration.authorization_url)
        || mixed_direct_mode
        || direct_endpoints
            .iter()
            .flatten()
            .any(|endpoint| !valid_oauth_url(endpoint))
        || !valid_oauth_url(&configuration.token_url)
        || !valid_oauth_url(&configuration.revoke_url)
        || configuration.resource_origins.is_empty()
        || configuration.resource_origins.len() > 16
        || configuration.resource_origins.iter().any(|origin| {
            Url::parse(origin).map_or(true, |url| {
                !valid_resource_origin_url(&url) || canonical_resource_origin(&url) != *origin
            })
        })
        || configuration
            .resource_origins
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != configuration.resource_origins.len()
    {
        return Err(NativeAuthError::Protocol);
    }
    Ok(())
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/')
        })
}

fn valid_oauth_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn valid_resource_origin_url(url: &Url) -> bool {
    valid_oauth_url(url) && matches!(url.path(), "" | "/")
}

fn canonical_resource_origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn configuration_sha256(configuration: &PluginOAuthConfiguration) -> String {
    sha256_text(&format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        configuration.plugin_id,
        configuration.signer_fingerprint_sha256,
        configuration.profile_id,
        configuration.authorization_url,
        configuration.login_url.as_ref().map_or("", Url::as_str),
        configuration
            .registration_url
            .as_ref()
            .map_or("", Url::as_str),
        configuration
            .email_verification_url
            .as_ref()
            .map_or("", Url::as_str),
        configuration.mfa_url.as_ref().map_or("", Url::as_str),
        configuration.token_url,
        configuration.revoke_url,
        configuration.client_id,
        configuration.scopes.join(" "),
        configuration.resource_origins.join(" ")
    ))
}

fn sha256_text(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn parse_token_response(
    response: reqwest::Response,
) -> Result<OAuthTokenResponse, NativeAuthError> {
    if !response.status().is_success() {
        return Err(token_response_error(response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| NativeAuthError::Network)?;
    if bytes.len() > MAX_TOKEN_BYTES {
        return Err(NativeAuthError::Protocol);
    }
    serde_json::from_slice(&bytes).map_err(|_| NativeAuthError::Protocol)
}

fn token_response_error(status: StatusCode) -> NativeAuthError {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED => NativeAuthError::RefreshExpired,
        _ => NativeAuthError::Network,
    }
}

async fn parse_credential_auth_response(
    response: reqwest::Response,
) -> Result<CredentialAuthResponse, NativeAuthError> {
    if !response.status().is_success() {
        return Err(
            if matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) {
                NativeAuthError::AccessDenied
            } else {
                NativeAuthError::Network
            },
        );
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| NativeAuthError::Network)?;
    if bytes.len() > MAX_TOKEN_BYTES {
        return Err(NativeAuthError::Protocol);
    }
    if let Ok(token) = serde_json::from_slice::<OAuthTokenResponse>(&bytes) {
        return Ok(CredentialAuthResponse::Token(token));
    }
    let challenge = serde_json::from_slice::<CredentialChallengeResponse>(&bytes)
        .map_err(|_| NativeAuthError::Protocol)?;
    let kind = match challenge.challenge_type.as_str() {
        "mfa" => PendingAuthChallengeKind::Mfa,
        "emailVerification" => PendingAuthChallengeKind::EmailVerification,
        _ => return Err(NativeAuthError::Protocol),
    };
    if !(32..=4_096).contains(&challenge.challenge_token.len())
        || !challenge.challenge_token.is_ascii()
        || challenge
            .challenge_token
            .bytes()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(NativeAuthError::Protocol);
    }
    Ok(CredentialAuthResponse::Challenge(PendingAuthChallenge {
        kind,
        token: Zeroizing::new(challenge.challenge_token),
    }))
}

fn validate_token_response(
    response: &OAuthTokenResponse,
    expected_scopes: &[String],
) -> Result<(), NativeAuthError> {
    let expected_scope = expected_scopes.join(" ");
    let scope = response.scope.as_deref().unwrap_or(&expected_scope);
    if response.access_token.is_empty()
        || response.refresh_token.is_empty()
        || response.access_token.len() > MAX_TOKEN_BYTES
        || response.refresh_token.len() > MAX_TOKEN_BYTES
        || !response.token_type.eq_ignore_ascii_case("bearer")
        || !(60..=24 * 60 * 60).contains(&response.expires_in)
        || scope
            .split_ascii_whitespace()
            .collect::<std::collections::BTreeSet<_>>()
            != expected_scopes
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
        || response
            .nonce
            .as_ref()
            .is_some_and(|nonce| nonce.is_empty() || nonce.len() > 256)
    {
        return Err(NativeAuthError::Protocol);
    }
    Ok(())
}

fn load_state(path: &Path) -> Result<StoredAuthState, NativeAuthError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoredAuthState::default());
        }
        Err(_) => return Err(NativeAuthError::StateUnavailable),
    };
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(NativeAuthError::StateUnavailable);
    }
    let state: StoredAuthState =
        serde_json::from_slice(&bytes).map_err(|_| NativeAuthError::StateUnavailable)?;
    if state.schema_version != STATE_SCHEMA_VERSION
        || (!state.owner_plugin_id.is_empty() && !valid_identifier(&state.owner_plugin_id, 160))
        || (!state.owner_signer_fingerprint_sha256.is_empty()
            && !valid_sha256(&state.owner_signer_fingerprint_sha256))
        || (!state.profile_id.is_empty() && !valid_identifier(&state.profile_id, 160))
        || (!state.configuration_sha256.is_empty() && !valid_sha256(&state.configuration_sha256))
        || state.retired_secret_refs.len() > 32
        || state
            .session_id
            .as_deref()
            .is_some_and(|value| Uuid::parse_str(value).is_err())
        || state
            .refresh_token_ref
            .iter()
            .chain(state.retired_secret_refs.iter())
            .any(|value| SecretRefId::parse(value).is_err())
    {
        return Err(NativeAuthError::StateUnavailable);
    }
    Ok(state)
}

fn persist_state(path: &Path, state: &StoredAuthState) -> Result<(), PersistStateError> {
    let parent = path.parent().ok_or(PersistStateError::BeforeCommit)?;
    fs::create_dir_all(parent).map_err(|_| PersistStateError::BeforeCommit)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|_| PersistStateError::BeforeCommit)?;
    }
    let encoded = serde_json::to_vec(state).map_err(|_| PersistStateError::BeforeCommit)?;
    if encoded.len() as u64 > MAX_STATE_BYTES {
        return Err(PersistStateError::BeforeCommit);
    }
    let temporary = parent.join(format!(".native-auth-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(PersistStateError::BeforeCommit);
    }
    if replace_file(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(PersistStateError::BeforeCommit);
    }
    sync_parent_directory(parent).map_err(|_| PersistStateError::CommitStateUnknown)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_configuration() -> PluginOAuthConfiguration {
        PluginOAuthConfiguration {
            plugin_id: "org.example.sync".to_owned(),
            signer_fingerprint_sha256: "a".repeat(64),
            profile_id: "primary".to_owned(),
            authorization_url: Url::parse("https://auth.example.test/authorize").unwrap(),
            login_url: None,
            registration_url: None,
            email_verification_url: None,
            mfa_url: None,
            token_url: Url::parse("https://auth.example.test/token").unwrap(),
            revoke_url: Url::parse("https://auth.example.test/revoke").unwrap(),
            client_id: "norishell-native".to_owned(),
            scopes: vec!["ssh-sync:read".to_owned(), "ssh-sync:write".to_owned()],
            resource_origins: vec!["https://sync.example.test".to_owned()],
        }
    }

    fn test_token_response(refresh_token: &str) -> OAuthTokenResponse {
        OAuthTokenResponse {
            access_token: "test-access-token".to_owned(),
            refresh_token: refresh_token.to_owned(),
            token_type: "Bearer".to_owned(),
            expires_in: 3600,
            scope: None,
            nonce: None,
        }
    }

    #[test]
    fn absent_session_is_distinct_from_corrupt_authorization_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = PluginOAuthService::start(
            directory.path(),
            VaultService::start(directory.path()),
            test_configuration(),
        )
        .expect("start account service");
        assert!(matches!(
            service.read_refresh_token(),
            Err(NativeAuthError::SessionMissing)
        ));
    }

    #[test]
    fn rejected_refresh_is_expired_while_server_failure_is_transient() {
        assert!(matches!(
            token_response_error(StatusCode::UNAUTHORIZED),
            NativeAuthError::RefreshExpired
        ));
        assert!(matches!(
            token_response_error(StatusCode::BAD_REQUEST),
            NativeAuthError::RefreshExpired
        ));
        assert!(matches!(
            token_response_error(StatusCode::SERVICE_UNAVAILABLE),
            NativeAuthError::Network
        ));
    }

    #[test]
    fn stored_state_rejects_unknown_fields_and_invalid_secret_refs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("state.json");
        let state = StoredAuthState::default();
        persist_state(&path, &state).expect("persist");
        assert_eq!(load_state(&path).expect("load").schema_version, 2);
        fs::write(
            &path,
            br#"{"schema_version":2,"owner_plugin_id":"org.example.sync","owner_signer_fingerprint_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","profile_id":"primary","configuration_sha256":"","refresh_token_ref":"bad","retired_secret_refs":[]}"#,
        )
        .expect("write invalid fixture");
        assert!(load_state(&path).is_err());
    }

    #[test]
    fn session_id_is_persisted_rotated_only_by_new_login_and_cleared_on_logout() {
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"test vault password")
            .expect("create vault");
        let configuration = test_configuration();
        let service =
            PluginOAuthService::start(directory.path(), vault.clone(), configuration.clone())
                .expect("start account service");
        assert_eq!(service.session_id(), None);

        service
            .persist_token_response(
                test_token_response("first-refresh"),
                TokenAcceptance::NewSession,
            )
            .expect("accept login token");
        let first_id = service.session_id().expect("session ID after login");
        assert_eq!(
            load_state(&service.state_path).unwrap().session_id,
            Some(first_id.clone())
        );

        service
            .persist_token_response(
                test_token_response("rotated-refresh"),
                TokenAcceptance::Refresh,
            )
            .expect("rotate refresh token");
        assert_eq!(service.session_id(), Some(first_id.clone()));

        let restored = PluginOAuthService::start(directory.path(), vault.clone(), configuration)
            .expect("restore account service");
        assert_eq!(restored.session_id(), Some(first_id.clone()));

        service.clear_persisted_session().expect("clear session");
        assert_eq!(service.session_id(), None);
        assert_eq!(restored.session_id(), None);
        assert_eq!(load_state(&service.state_path).unwrap().session_id, None);

        service
            .persist_token_response(
                test_token_response("next-login"),
                TokenAcceptance::NewSession,
            )
            .expect("accept another login token");
        assert_ne!(service.session_id(), Some(first_id));
        vault.lock_for_tests().expect("lock vault");
        assert_eq!(service.session_id(), None);
    }

    #[test]
    fn legacy_session_without_id_becomes_identified_only_after_successful_refresh() {
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"test vault password")
            .expect("create vault");
        let configuration = test_configuration();
        let service =
            PluginOAuthService::start(directory.path(), vault.clone(), configuration.clone())
                .expect("start account service");
        service
            .persist_token_response(
                test_token_response("legacy-refresh"),
                TokenAcceptance::NewSession,
            )
            .expect("accept token");
        let mut legacy = load_state(&service.state_path).expect("load state");
        legacy.session_id = None;
        persist_state(&service.state_path, &legacy).expect("persist legacy state");
        let mut legacy_json: serde_json::Value =
            serde_json::from_slice(&fs::read(service.state_path.as_path()).unwrap()).unwrap();
        legacy_json.as_object_mut().unwrap().remove("session_id");
        fs::write(
            service.state_path.as_path(),
            serde_json::to_vec(&legacy_json).unwrap(),
        )
        .expect("write legacy record without session ID");
        let restored = PluginOAuthService::start(directory.path(), vault, configuration)
            .expect("restore legacy state");
        assert_eq!(restored.session_id(), None);
        restored
            .persist_token_response(
                test_token_response("legacy-rotated"),
                TokenAcceptance::Refresh,
            )
            .expect("refresh legacy token");
        let refreshed_id = restored.session_id().expect("new ID after refresh");
        assert_eq!(
            load_state(&restored.state_path).unwrap().session_id,
            Some(refreshed_id)
        );
    }

    #[test]
    fn session_id_requires_a_present_refresh_secret_and_is_invalidated_on_rejection() {
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"test vault password")
            .expect("create vault");
        let configuration = test_configuration();
        let service =
            PluginOAuthService::start(directory.path(), vault.clone(), configuration.clone())
                .expect("start account service");
        service
            .persist_token_response(test_token_response("refresh"), TokenAcceptance::NewSession)
            .expect("accept token");
        let stored = service.stored_state().expect("stored state");
        let secret_ref = SecretRefId::parse(stored.refresh_token_ref.as_deref().unwrap()).unwrap();
        vault.delete_secrets(&[secret_ref]).expect("remove secret");
        assert_eq!(service.session_id(), None);

        service
            .persist_token_response(
                test_token_response("replacement"),
                TokenAcceptance::NewSession,
            )
            .expect("accept replacement token");
        service.invalidate_session_id().expect("invalidate session");
        assert_eq!(service.session_id(), None);
        let restored = PluginOAuthService::start(directory.path(), vault, configuration)
            .expect("restore invalidated state");
        assert_eq!(restored.session_id(), None);
    }

    #[test]
    fn oauth_configuration_binds_signer_and_canonical_resource_origins() {
        let configuration = test_configuration();
        assert!(validate_configuration(&configuration).is_ok());
        assert_eq!(
            canonical_resource_origin(&Url::parse("https://sync.example.test/path").unwrap()),
            "https://sync.example.test"
        );

        let mut different_signer = configuration.clone();
        different_signer.signer_fingerprint_sha256 = "b".repeat(64);
        assert_ne!(
            configuration_sha256(&configuration),
            configuration_sha256(&different_signer)
        );

        let mut different_origin = configuration.clone();
        different_origin.resource_origins = vec!["https://other.example.test".to_owned()];
        assert_ne!(
            configuration_sha256(&configuration),
            configuration_sha256(&different_origin)
        );
    }

    #[test]
    fn same_version_signer_transfer_retains_login_only_for_identical_account_config() {
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"test vault password")
            .expect("vault");
        let old_configuration = test_configuration();
        let old =
            PluginOAuthService::start(directory.path(), vault.clone(), old_configuration.clone())
                .expect("old account");
        old.persist_token_response(test_token_response("refresh"), TokenAcceptance::NewSession)
            .expect("save refresh token");
        let old_path = old.state_path.as_path().to_path_buf();
        let old_session = old.session_id();
        let mut updated = old_configuration.clone();
        updated.signer_fingerprint_sha256 = "b".repeat(64);
        record_oauth_signer_transfer(
            directory.path(),
            &updated.plugin_id,
            &old_configuration.signer_fingerprint_sha256,
            &updated.signer_fingerprint_sha256,
            &PluginOperationId::new(),
        )
        .expect("record authorized replacement");
        let transferred =
            PluginOAuthService::start(directory.path(), vault.clone(), updated.clone())
                .expect("new account");
        assert_eq!(
            transferred.status().account_state,
            NativeAccountState::Connected
        );
        assert_eq!(transferred.session_id(), old_session);
        assert!(transferred.read_refresh_token().is_ok());
        assert!(!old_path.exists());

        let mut changed = updated;
        changed.resource_origins = vec!["https://different.example.test".to_owned()];
        let separate =
            PluginOAuthService::start(directory.path(), vault, changed).expect("changed config");
        assert_eq!(
            separate.status().account_state,
            NativeAccountState::Disconnected
        );
    }

    #[test]
    fn signer_transfer_cycle_finds_original_account_without_reusing_old_marker() {
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"test vault password")
            .expect("vault");
        let a = test_configuration();
        let service = PluginOAuthService::start(directory.path(), vault.clone(), a.clone())
            .expect("original account");
        service
            .persist_token_response(test_token_response("refresh"), TokenAcceptance::NewSession)
            .expect("save account");
        let session = service.session_id();
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let first = PluginOperationId::new();
        record_oauth_signer_transfer(
            directory.path(),
            &a.plugin_id,
            &a.signer_fingerprint_sha256,
            &b,
            &first,
        )
        .expect("A to B");
        finalize_oauth_signer_transfer(directory.path(), &a.plugin_id, &first).expect("finalize B");
        let second = PluginOperationId::new();
        record_oauth_signer_transfer(directory.path(), &a.plugin_id, &b, &c, &second)
            .expect("B to C");
        finalize_oauth_signer_transfer(directory.path(), &a.plugin_id, &second)
            .expect("finalize C");
        let failed = PluginOperationId::new();
        record_oauth_signer_transfer(directory.path(), &a.plugin_id, &c, &b, &failed)
            .expect("C to B");
        discard_oauth_signer_transfer(directory.path(), &a.plugin_id, &failed)
            .expect("rollback C to B");
        assert_eq!(
            read_transfer_signers(&transfer_marker_path(directory.path(), &a.plugin_id, &b))
                .expect("remaining history"),
            vec![a.signer_fingerprint_sha256.clone()],
        );
        record_oauth_signer_transfer(
            directory.path(),
            &a.plugin_id,
            &c,
            &b,
            &PluginOperationId::new(),
        )
        .expect("retry C to B");
        let mut returned = a;
        returned.signer_fingerprint_sha256 = b;
        let restored =
            PluginOAuthService::start(directory.path(), vault, returned).expect("return to B");
        assert_eq!(
            restored.status().account_state,
            NativeAccountState::Connected
        );
        assert_eq!(restored.session_id(), session);
        assert!(restored.read_refresh_token().is_ok());
    }

    #[test]
    fn failed_cycle_restores_marker_even_when_record_did_not_change_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let a = test_configuration();
        let b = "b".repeat(64);
        let first = PluginOperationId::new();
        record_oauth_signer_transfer(
            directory.path(),
            &a.plugin_id,
            &a.signer_fingerprint_sha256,
            &b,
            &first,
        )
        .expect("A to B");
        finalize_oauth_signer_transfer(directory.path(), &a.plugin_id, &first).expect("commit B");
        let second = PluginOperationId::new();
        record_oauth_signer_transfer(
            directory.path(),
            &a.plugin_id,
            &b,
            &a.signer_fingerprint_sha256,
            &second,
        )
        .expect("B to A");
        finalize_oauth_signer_transfer(directory.path(), &a.plugin_id, &second).expect("commit A");
        let marker = transfer_marker_path(directory.path(), &a.plugin_id, &b);
        let original = read_transfer_signers(&marker).expect("original history");
        let failed = PluginOperationId::new();
        record_oauth_signer_transfer(
            directory.path(),
            &a.plugin_id,
            &a.signer_fingerprint_sha256,
            &b,
            &failed,
        )
        .expect("repeat A to B");
        discard_oauth_signer_transfer(directory.path(), &a.plugin_id, &failed)
            .expect("rollback repeat");
        assert_eq!(
            read_transfer_signers(&marker).expect("restored history"),
            original
        );
    }

    #[test]
    fn incomplete_unpublished_snapshot_cannot_block_recovery() {
        let directory = tempfile::tempdir().expect("tempdir");
        let configuration = test_configuration();
        let next = "b".repeat(64);
        let operation = PluginOperationId::new();
        let marker = transfer_marker_path(directory.path(), &configuration.plugin_id, &next);
        let parent = marker.parent().expect("oauth directory");
        fs::create_dir_all(parent).expect("oauth directory");
        let incomplete = parent.join(format!(".oauth-transfer-write-{}", Uuid::new_v4()));
        fs::write(&incomplete, b"half-written snapshot").expect("interrupted temp write");
        discard_oauth_signer_transfer(directory.path(), &configuration.plugin_id, &operation)
            .expect("no published snapshot to recover");
        assert!(!marker.exists());
        record_oauth_signer_transfer(
            directory.path(),
            &configuration.plugin_id,
            &configuration.signer_fingerprint_sha256,
            &next,
            &operation,
        )
        .expect("record independent of interrupted temp");
        assert!(
            read_transfer_rollback(&transfer_rollback_path(
                directory.path(),
                &configuration.plugin_id,
                &operation,
            ))
            .expect("read complete snapshot")
            .is_some()
        );
        discard_oauth_signer_transfer(directory.path(), &configuration.plugin_id, &operation)
            .expect("restore original absence");
        assert!(!marker.exists());
    }

    #[test]
    fn long_update_chain_preserves_unstarted_account() {
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"test vault password")
            .expect("vault");
        let original = test_configuration();
        let service = PluginOAuthService::start(directory.path(), vault.clone(), original.clone())
            .expect("original account");
        service
            .persist_token_response(test_token_response("refresh"), TokenAcceptance::NewSession)
            .expect("save account");
        let session = service.session_id();
        let mut previous = original.signer_fingerprint_sha256.clone();
        for number in 1..=40 {
            let next = format!("{number:064x}");
            let operation = PluginOperationId::new();
            record_oauth_signer_transfer(
                directory.path(),
                &original.plugin_id,
                &previous,
                &next,
                &operation,
            )
            .expect("record update");
            finalize_oauth_signer_transfer(directory.path(), &original.plugin_id, &operation)
                .expect("commit update");
            previous = next;
        }
        let mut latest = original;
        latest.signer_fingerprint_sha256 = previous;
        let restored = PluginOAuthService::start(directory.path(), vault, latest)
            .expect("account after long update chain");
        assert_eq!(
            restored.status().account_state,
            NativeAccountState::Connected
        );
        assert_eq!(restored.session_id(), session);
        assert!(restored.read_refresh_token().is_ok());
    }

    #[test]
    fn oauth_configuration_rejects_reserved_query_and_non_origin_resources() {
        let mut configuration = test_configuration();
        configuration.authorization_url =
            Url::parse("https://auth.example.test/authorize?state=attacker").unwrap();
        assert!(validate_configuration(&configuration).is_err());

        let mut configuration = test_configuration();
        configuration.resource_origins = vec!["https://sync.example.test/provider/path".to_owned()];
        assert!(validate_configuration(&configuration).is_err());
    }

    #[test]
    fn oauth_configuration_accepts_http_and_binds_exact_resource_origin() {
        let mut configuration = test_configuration();
        configuration.authorization_url = Url::parse("http://127.0.0.1:8080/login").unwrap();
        configuration.token_url = Url::parse("http://127.0.0.1:8080/token").unwrap();
        configuration.revoke_url = Url::parse("http://127.0.0.1:8080/revoke").unwrap();
        configuration.resource_origins = vec!["http://127.0.0.1:8080".to_owned()];
        assert!(validate_configuration(&configuration).is_ok());
        let directory = tempfile::tempdir().unwrap();
        let service = PluginOAuthService::start(
            directory.path(),
            VaultService::start(directory.path()),
            configuration.clone(),
        )
        .unwrap();
        assert!(
            service.allows_resource_url(&Url::parse("http://127.0.0.1:8080/exchange").unwrap())
        );
        assert!(
            !service.allows_resource_url(&Url::parse("https://127.0.0.1:8080/exchange").unwrap())
        );
        assert!(
            !service.allows_resource_url(&Url::parse("http://127.0.0.1:8081/exchange").unwrap())
        );
        configuration.token_url = Url::parse("http://user@127.0.0.1:8080/token").unwrap();
        assert!(validate_configuration(&configuration).is_err());
    }

    #[test]
    fn persisted_refresh_token_restores_a_connected_account_without_password_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        let configuration = test_configuration();
        let service = PluginOAuthService::start(
            directory.path(),
            VaultService::start(directory.path()),
            configuration.clone(),
        )
        .expect("start account service");
        let mut state = service.stored_state().expect("initialized state");
        state.refresh_token_ref = Some(SecretRefId::new().as_str().to_owned());
        persist_state(&service.state_path, &state).expect("persist refresh token pointer");

        let restored = PluginOAuthService::start(
            directory.path(),
            VaultService::start(directory.path()),
            configuration,
        )
        .expect("restore account service");
        assert_eq!(
            restored.status().account_state,
            NativeAccountState::Connected
        );
    }

    #[test]
    fn empty_account_state_rebinds_after_a_provider_configuration_update() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut updated = test_configuration();
        let login_url = Url::parse("https://auth.example.test/native/login").unwrap();
        updated.authorization_url = login_url.clone();
        updated.login_url = Some(login_url);
        updated.registration_url =
            Some(Url::parse("https://auth.example.test/native/register").unwrap());
        updated.email_verification_url =
            Some(Url::parse("https://auth.example.test/native/verify-email").unwrap());
        updated.mfa_url = Some(Url::parse("https://auth.example.test/native/mfa").unwrap());
        let initial = PluginOAuthService::start(
            directory.path(),
            VaultService::start(directory.path()),
            updated.clone(),
        )
        .expect("start account service");
        let mut stale = initial.stored_state().expect("empty state");
        stale.configuration_sha256 = "b".repeat(64);
        persist_state(&initial.state_path, &stale).expect("persist stale empty state");

        let rebound = PluginOAuthService::start(
            directory.path(),
            VaultService::start(directory.path()),
            updated.clone(),
        )
        .expect("rebind empty account state");

        assert_eq!(
            rebound.status().account_state,
            NativeAccountState::Disconnected
        );
        assert_eq!(
            rebound
                .stored_state()
                .expect("rebound state")
                .configuration_sha256,
            configuration_sha256(&updated)
        );
    }

    #[test]
    fn plugin_data_delete_removes_all_owned_pointer_files_without_a_live_runtime() {
        let directory = tempfile::tempdir().expect("tempdir");
        let plugin_id = "org.example.sync";
        let oauth_directory = directory
            .path()
            .join("plugins")
            .join("oauth")
            .join(sha256_text(plugin_id));
        let state_path = oauth_directory.join("profile.json");
        let state = StoredAuthState {
            schema_version: STATE_SCHEMA_VERSION,
            owner_plugin_id: plugin_id.to_owned(),
            owner_signer_fingerprint_sha256: "a".repeat(64),
            profile_id: "primary".to_owned(),
            configuration_sha256: "b".repeat(64),
            refresh_token_ref: None,
            session_id: None,
            retired_secret_refs: Vec::new(),
        };
        persist_state(&state_path, &state).expect("persist pointer");
        for number in 0..260 {
            let marker = oauth_directory.join(format!(".transfer-{number:064x}"));
            write_transfer_signers(&marker, &["a".repeat(64)]).expect("history marker");
        }
        let vault = VaultService::start(directory.path());
        delete_plugin_oauth_data(directory.path(), &vault, plugin_id).expect("delete plugin data");
        assert!(!oauth_directory.exists());
    }
}
