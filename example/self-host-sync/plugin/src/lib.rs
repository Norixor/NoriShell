//! Provider-owned synchronization over Core's protected data and network APIs.

mod api_chain;
mod browser_cache;
mod network_flow;
mod sync_flow;
mod sync_policy;

use norishell_plugin_sdk::{
    Plugin, PluginError, PluginHostMessageKind, PluginHostRequest, PluginRuntimeOutput, output,
    payload,
};
use serde_json::{Value, json};
use url::{Host, Url};

const PROFILE: &str = "primary";
const CLIENT_ID: &str = "norishell-self-host";
const SCOPE: &str = "ssh.sync";

#[derive(Default)]
struct SelfHostSync {
    origin: Option<String>,
    invalid_server: bool,
    locale: String,
    last: Option<Summary>,
    conflict_policy: String,
    deletion_policy: String,
    browser: browser_cache::BrowserCache,
    api: api_chain::ApiChain,
    flow: Option<sync_flow::Flow>,
    pending_terminal: Option<sync_flow::Transition>,
    cache_dirty: bool,
}

#[derive(Default)]
struct Summary {
    account: String,
    operation: String,
    difference: String,
    review_pending: bool,
    attention_required: bool,
    local_hosts: u64,
    remote_hosts: Option<u64>,
    local_desktops: u64,
    remote_desktops: Option<u64>,
    local_credentials: u64,
    remote_credentials: Option<u64>,
    remote_counts_stale: bool,
    error: Option<String>,
    diagnostic: Option<String>,
    http_status: Option<u16>,
    remote_updated: bool,
}

impl Plugin for SelfHostSync {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let body: Value = payload(&request)?;
        if let Some(locale) = body.get("locale").and_then(Value::as_str) {
            self.locale = locale.to_owned();
        }
        match request.kind {
            PluginHostMessageKind::Initialize => {
                self.read_settings(&body);
                self.browser
                    .restore(&body["storage"], self.origin.as_deref());
                Ok(vec![
                    output(
                        &request.request_id,
                        "ui.navigation",
                        &json!({
                            "navigationId":"selfHostSync", "label":self.t("同步", "Sync"),
                            "icon":"cloud", "pageId":"sync", "order":100
                        }),
                    )?,
                    output(
                        &request.request_id,
                        "ui.page",
                        &json!({
                            "pageId":"sync", "title":self.t("同步", "Sync"),
                            "icon":"cloud", "onOpenActionId":"sync.pageOpened",
                            "document":self.document()
                        }),
                    )?,
                ])
            }
            PluginHostMessageKind::UiAction => {
                self.read_settings(&body);
                self.action(&request.request_id, &body)
            }
            PluginHostMessageKind::SshSyncResult => {
                if matches!(
                    body["actionId"].as_str(),
                    Some("sync.pageOpened" | "sync.status" | "sync.login" | "sync.logout")
                ) {
                    self.record_account_result(&body);
                } else {
                    self.record_result(&body);
                }
                self.document_output(&request.request_id)
            }
            PluginHostMessageKind::BrokerResult => self.broker_result(&request.request_id, &body),
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

impl SelfHostSync {
    fn t<'a>(&self, zh: &'a str, en: &'a str) -> &'a str {
        if self.locale == "en" { en } else { zh }
    }

    fn read_settings(&mut self, body: &Value) {
        if let Some(values) = body.pointer("/settings/values") {
            let raw = values
                .get("serverUrl")
                .and_then(Value::as_str)
                .unwrap_or("");
            let next = if raw.trim().is_empty() {
                None
            } else {
                canonical_origin(raw)
            };
            if self.origin != next {
                self.last = None;
                self.browser = browser_cache::BrowserCache::default();
            }
            self.invalid_server = !raw.trim().is_empty() && next.is_none();
            self.origin = next;
            self.conflict_policy = match values.get("conflictPolicy").and_then(Value::as_str) {
                Some("prompt") => "prompt",
                _ => "newest",
            }
            .to_owned();
            self.deletion_policy = match values.get("deletionPolicy").and_then(Value::as_str) {
                Some("prompt") => "prompt",
                _ => "newest",
            }
            .to_owned();
        }
    }

    fn action(
        &mut self,
        request_id: &str,
        body: &Value,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let action = body
            .get("actionId")
            .and_then(Value::as_str)
            .ok_or(PluginError::InvalidRequest)?;
        if action == "sync.pageOpened" && self.origin.is_none() {
            return self.document_output(request_id);
        }
        if action == "sync.pageOpened" {
            return Ok(vec![output(
                request_id,
                "ssh.sync.request",
                &json!({"action":"status", "profileId":PROFILE, "auth":self.auth()?}),
            )?]);
        }
        if matches!(action, "sync.refresh" | "sync.run") {
            if self.flow.is_some() {
                return self.document_output(request_id);
            }
            self.api = api_chain::ApiChain::default();
            self.flow = Some(sync_flow::Flow::new(
                if action == "sync.refresh" {
                    sync_flow::Intent::Refresh
                } else {
                    sync_flow::Intent::Sync
                },
                self.endpoint("exchange")?,
                &self.conflict_policy,
                &self.deletion_policy,
            ));
            return self
                .api
                .request(request_id, sync_flow::Flow::snapshot_request());
        }
        let request = match action {
            "sync.status" => json!({
                "action":"status", "profileId":PROFILE, "auth":self.auth()?
            }),
            "sync.login" => json!({
                "action":"login", "profileId":PROFILE, "auth":self.auth()?,
                "usernameFieldId":"accountName", "passwordFieldId":"accountPassword"
            }),
            "sync.logout" => json!({
                "action":"logout", "profileId":PROFILE, "auth":self.auth()?
            }),
            _ => return Err(PluginError::InvalidRequest),
        };
        if matches!(action, "sync.login" | "sync.logout") {
            self.browser = browser_cache::BrowserCache::default();
            self.cache_dirty = true;
        }
        Ok(vec![output(request_id, "ssh.sync.request", &request)?])
    }

    fn broker_result(
        &mut self,
        request_id: &str,
        body: &Value,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let reply = self.api.reply(body)?;
        if let Some(terminal) = self.pending_terminal.take() {
            let release_succeeded = matches!(reply, api_chain::Reply::Completed(ref value) if value["kind"] == "dataRelease");
            let terminal =
                if release_succeeded || matches!(&terminal, sync_flow::Transition::Failed { .. }) {
                    terminal
                } else {
                    sync_flow::Transition::Failed {
                        code: "cleanupIncomplete".into(),
                        http_status: None,
                    }
                };
            return self.finish_flow(request_id, body, terminal);
        }
        let flow = self.flow.as_mut().ok_or(PluginError::InvalidRequest)?;
        let transition = match reply {
            api_chain::Reply::Completed(value) => flow.receive(value),
            api_chain::Reply::Failed(code) => sync_flow::Transition::Failed {
                code,
                http_status: None,
            },
        };
        match transition {
            sync_flow::Transition::Call(operation) => self.api.request(request_id, operation),
            terminal => {
                let release = flow.release_request();
                self.pending_terminal = Some(terminal);
                self.api.request(request_id, release)
            }
        }
    }

    fn finish_flow(
        &mut self,
        request_id: &str,
        body: &Value,
        terminal: sync_flow::Transition,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        match terminal {
            sync_flow::Transition::Finished { difference, review } => {
                let flow = self.flow.take().ok_or(PluginError::InvalidRequest)?;
                let remote = if flow.upload.is_some() {
                    &flow.exported
                } else {
                    &flow.remote
                };
                let remote_rows = sync_flow::display_rows(remote);
                self.browser
                    .replace(&remote_rows, body["nowUnixMs"].as_u64());
                self.cache_dirty = true;
                let local_rows = sync_flow::display_rows(if flow.upload.is_some() {
                    &flow.exported
                } else if flow.applied.is_null() {
                    &flow.local
                } else {
                    &flow.composed
                });
                let count = |rows: &[Value], category: &str| {
                    rows.iter()
                        .filter(|row| row["category"] == category)
                        .count()
                };
                let local_count = |category: &str, key: &str| {
                    if flow.upload.is_none() && flow.local["keyPending"] == true {
                        flow.local["localCounts"][key].as_u64().unwrap_or(0) as usize
                    } else {
                        count(&local_rows, category)
                    }
                };
                self.record_result(&json!({"actionId":body["actionId"],"result":{
                    "accountState":"connected","operationState":if review {"needsReview"} else {"succeeded"},
                    "differenceState":difference,"localHostCount":local_count("hosts","hostCount"),
                    "localCredentialCount":local_count("credentials","credentialCount"),"localDesktopProfileCount":local_count("desktopProfiles","desktopProfileCount"),
                    "remoteHostCount":count(&remote_rows,"hosts"),"remoteCredentialCount":count(&remote_rows,"credentials"),
                    "remoteDesktopProfileCount":count(&remote_rows,"desktopProfiles")
                }}));
                self.document_output(request_id)
            }
            sync_flow::Transition::Failed { code, http_status } => {
                let remote_updated = self.flow.as_ref().is_some_and(|flow| flow.upload.is_some());
                let authenticated = self
                    .flow
                    .as_ref()
                    .is_some_and(|flow| flow.network.started() || flow.download.is_some());
                self.flow = None;
                self.browser.stale = true;
                let account = if code == "accountNotConnected" {
                    "disconnected"
                } else if code == "authorizationExpired" {
                    "expired"
                } else if authenticated {
                    "connected"
                } else {
                    self.last
                        .as_ref()
                        .map_or("disconnected", |last| last.account.as_str())
                }
                .to_owned();
                self.record_result(&json!({"actionId":body["actionId"],"result":{"accountState":account,
                    "operationState":if remote_updated {"partial"} else {"failed"},
                    "remoteUpdated":remote_updated,
                    "differenceState":"unavailable","stableErrorCode":code,"httpStatus":http_status}}));
                self.document_output(request_id)
            }
            sync_flow::Transition::Call(_) => Err(PluginError::InvalidRequest),
        }
    }

    fn browser_hint(&self) -> String {
        let status = if self.browser.stale {
            self.t(
                "显示上次验证的离线缓存；刷新后确认当前云端状态。",
                "Showing the last verified offline cache. Refresh to confirm current cloud state.",
            )
        } else if self.browser.verified_at_unix_ms.is_some() {
            self.t(
                "显示上次验证的云端项目；点击刷新可检查最新状态。",
                "Showing the last verified cloud items. Refresh to check the latest state.",
            )
        } else {
            self.t(
                "点击“刷新远端状态”获取云端项目。",
                "Select Refresh remote status to load cloud items.",
            )
        };
        let mut text = self.browser.verified_at_utc().map_or_else(
            || status.to_owned(),
            |time| {
                format!(
                    "{} {time} · {status}",
                    self.t("上次获取：", "Last fetched:")
                )
            },
        );
        if self.browser.omitted > 0 {
            text.push_str(&if self.locale == "en" {
                format!(" {} additional items are not displayed; sync includes the complete selected data.", self.browser.omitted)
            } else {
                format!(" 另有 {} 项未在此显示；同步仍包含完整选定数据。", self.browser.omitted)
            });
        }
        text
    }

    fn endpoint(&self, path: &str) -> Result<String, PluginError> {
        Ok(format!(
            "{}/{}",
            self.origin.as_deref().ok_or(PluginError::InvalidRequest)?,
            path
        ))
    }

    fn auth(&self) -> Result<Value, PluginError> {
        let origin = self.origin.as_deref().ok_or(PluginError::InvalidRequest)?;
        Ok(json!({
            "loginUrl":self.endpoint("auth/login")?,
            "registrationUrl":self.endpoint("auth/register")?,
            "emailVerificationUrl":self.endpoint("auth/email/verify")?,
            "mfaUrl":self.endpoint("auth/mfa")?,
            "tokenUrl":self.endpoint("auth/token")?,
            "revokeUrl":self.endpoint("auth/revoke")?,
            "clientId":CLIENT_ID,
            "scopes":[SCOPE],
            "resourceOrigins":[origin]
        }))
    }

    fn record_result(&mut self, body: &Value) {
        let result = &body["result"];
        let mut next = Summary {
            account: result["accountState"]
                .as_str()
                .unwrap_or("disconnected")
                .to_owned(),
            operation: result["operationState"]
                .as_str()
                .unwrap_or("idle")
                .to_owned(),
            difference: result["differenceState"]
                .as_str()
                .unwrap_or("unavailable")
                .to_owned(),
            review_pending: result["operationState"].as_str() == Some("needsReview"),
            attention_required: false,
            local_hosts: result["localHostCount"].as_u64().unwrap_or(0),
            remote_hosts: result["remoteHostCount"].as_u64(),
            local_desktops: result["localDesktopProfileCount"].as_u64().unwrap_or(0),
            remote_desktops: result["remoteDesktopProfileCount"].as_u64(),
            local_credentials: result["localCredentialCount"].as_u64().unwrap_or(0),
            remote_credentials: result["remoteCredentialCount"].as_u64(),
            remote_counts_stale: false,
            error: result["stableErrorCode"].as_str().map(str::to_owned),
            diagnostic: result["diagnosticCode"].as_str().map(str::to_owned),
            http_status: result["httpStatus"]
                .as_u64()
                .and_then(|value| u16::try_from(value).ok()),
            remote_updated: result["remoteUpdated"].as_bool().unwrap_or(false),
        };
        if next.error.is_some()
            && body["actionId"].as_str() != Some("sync.logout")
            && let Some(previous) = &self.last
        {
            // A failed retry does not remove the work that Core asked the user
            // to review. The next explicit click will re-fetch before applying.
            next.review_pending = previous.review_pending;
            next.attention_required = previous.attention_required;
            if previous.remote_updated {
                next.remote_updated = true;
                next.operation = "partial".into();
            }
            if next.remote_hosts.is_none() {
                next.remote_hosts = previous.remote_hosts;
                next.remote_desktops = previous.remote_desktops;
                next.remote_credentials = previous.remote_credentials;
                next.remote_counts_stale = next.remote_hosts.is_some()
                    || next.remote_desktops.is_some()
                    || next.remote_credentials.is_some();
            }
        }
        if body["actionId"] == "sync.refresh"
            && let Some(previous) = &self.last
            && previous.remote_updated
        {
            next.remote_updated = true;
            next.operation = "partial".into();
            if next.error.is_none() {
                next.error = previous.error.clone();
                next.diagnostic = previous.diagnostic.clone();
                next.http_status = previous.http_status;
            }
        }
        if body["actionId"].as_str() != Some("sync.logout") && next.remote_hosts.is_none() {
            next.remote_hosts = self.browser.category_count("hosts");
            next.remote_credentials = self.browser.category_count("credentials");
            next.remote_desktops = self.browser.category_count("desktopProfiles");
            next.remote_counts_stale = next.remote_hosts.is_some();
        }
        self.last = Some(next);
    }

    fn record_account_result(&mut self, body: &Value) {
        let result = &body["result"];
        let summary = self.last.get_or_insert_with(Summary::default);
        summary.account = result["accountState"]
            .as_str()
            .unwrap_or("disconnected")
            .to_owned();
        if summary.remote_updated && body["actionId"] != "sync.logout" {
            // Account status does not settle a successful PUT followed by an
            // incomplete local commit.
            return;
        }
        summary.operation = result["operationState"]
            .as_str()
            .unwrap_or("idle")
            .to_owned();
        // Legacy account status can request attention, but its review is not
        // a resumable decision in the category-scoped data flow.
        summary.attention_required = summary.operation == "needsReview";
        summary.error = result["stableErrorCode"].as_str().map(str::to_owned);
        summary.diagnostic = result["diagnosticCode"].as_str().map(str::to_owned);
        summary.http_status = result["httpStatus"]
            .as_u64()
            .and_then(|status| u16::try_from(status).ok());
        if body["actionId"] == "sync.logout" {
            summary.remote_hosts = None;
            summary.remote_credentials = None;
            summary.remote_desktops = None;
            summary.difference = "unavailable".into();
            summary.review_pending = false;
            summary.attention_required = false;
            summary.remote_updated = false;
        }
    }

    fn document_output(
        &mut self,
        request_id: &str,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let mut outputs = vec![output(
            request_id,
            "ui.document",
            &json!({
                "targetId":"app.page", "document":self.document()
            }),
        )?];
        if self.cache_dirty
            && let Some(origin) = &self.origin
        {
            outputs.push(output(
                request_id,
                "storage.write",
                &json!({"writeToken":request_id,"valueJson":self.browser.stored(origin)}),
            )?);
            self.cache_dirty = false;
        }
        Ok(outputs)
    }

    fn document(&self) -> Value {
        let ready = self.origin.is_some();
        let insecure_http = self
            .origin
            .as_deref()
            .is_some_and(|origin| origin.starts_with("http://"));
        let connected = self
            .last
            .as_ref()
            .is_some_and(|last| last.account == "connected");
        let mut children = vec!["headingRow"];
        if !ready {
            children.push("setupNotice");
        }
        if insecure_http {
            children.push("httpWarning");
        }
        children.push("connectionBar");
        if self.last.as_ref().is_some_and(|last| last.error.is_some()) {
            children.push("errorNotice");
        }
        children.push("browser");
        let account = self
            .last
            .as_ref()
            .map_or(self.t("未连接", "Disconnected"), |last| {
                self.account_label(&last.account)
            });
        let mut nodes = vec![
            json!({"kind":"stack","nodeId":"root","direction":"vertical","align":"stretch","gap":20,"children":children}),
            json!({"kind":"stack","nodeId":"headingRow","direction":"horizontal","align":"center","gap":16,"children":["heading","settingsDialog"]}),
            json!({"kind":"text","nodeId":"heading","text":self.t("同步","Sync"),"style":"heading","tone":"neutral"}),
            json!({"kind":"dialog","nodeId":"settingsDialog","title":self.t("同步设置","Sync settings"),"description":null,"triggerLabel":self.t("设置","Settings"),"closeLabel":self.t("关闭","Close"),"children":["serverSettings","scopeRow","policyRow"]}),
            json!({"kind":"button","nodeId":"serverSettings","actionId":"norishell.openSettings:serverUrl","label":self.t("服务器地址与插件设置","Server and plugin settings"),"icon":"settings","variant":"secondary","disabled":false}),
            json!({"kind":"section","nodeId":"connectionBar","title":null,"children":["statusBar"]}),
            json!({"kind":"stack","nodeId":"statusBar","direction":"horizontal","align":"center","gap":24,"children":["accountGroup","syncState","serverInfo","actionButtons"]}),
            json!({"kind":"stack","nodeId":"accountGroup","direction":"horizontal","align":"center","gap":4,"children":["connectionStatus","accountMenu"]}),
            json!({"kind":"status","nodeId":"connectionStatus","label":account,"tone":if connected { "success" } else { "neutral" }}),
            json!({"kind":"menu","nodeId":"accountMenu","label":self.t("账户","Account"),"children":["loginDialog","logout","status"]}),
            json!({"kind":"dialog","nodeId":"loginDialog","title":self.t("连接自托管服务器","Connect to self-hosted server"),"description":self.t("密码仅交给 NoriShell 安全登录流程。","The password goes only to NoriShell's secure login flow."),"triggerLabel":self.t("连接 / 登录","Connect / Log in"),"closeLabel":self.t("关闭","Close"),"children":["accountNameField","accountPasswordField","login"]}),
            json!({"kind":"textField","nodeId":"accountNameField","fieldId":"accountName","label":self.t("账户名","Account name"),"value":"owner","placeholder":null,"fieldKind":"text","required":true,"disabled":!ready}),
            json!({"kind":"textField","nodeId":"accountPasswordField","fieldId":"accountPassword","label":self.t("访问密码","Access password"),"value":"","placeholder":null,"fieldKind":"password","required":true,"disabled":!ready}),
            json!({"kind":"button","nodeId":"login","actionId":"sync.login","label":self.t("登录","Log in"),"icon":"shield","variant":"primary","disabled":!ready}),
            json!({"kind":"button","nodeId":"logout","actionId":"sync.logout","label":self.t("断开连接","Disconnect"),"icon":null,"variant":"secondary","disabled":!ready || !connected}),
            json!({"kind":"button","nodeId":"status","actionId":"sync.status","label":self.t("检查连接状态","Check connection status"),"icon":null,"variant":"secondary","disabled":!ready}),
            json!({"kind":"status","nodeId":"syncState","label":self.sync_state_label(),"tone":self.sync_state_tone()}),
            json!({"kind":"stack","nodeId":"serverInfo","direction":"horizontal","align":"center","gap":10,"children":["serverIcon","server"]}),
            json!({"kind":"icon","nodeId":"serverIcon","icon":"server","accessibleLabel":self.t("同步服务器","Sync server"),"tone":"neutral"}),
            json!({"kind":"text","nodeId":"server","text":self.server_label(),"style":"secondary","tone":"neutral"}),
            json!({"kind":"stack","nodeId":"actionButtons","direction":"horizontal","align":"center","gap":12,"children":["refresh","run"]}),
            json!({"kind":"button","nodeId":"refresh","actionId":"sync.refresh","label":self.t("刷新远端状态","Refresh remote status"),"icon":"refresh","variant":"secondary","disabled":!ready}),
            json!({"kind":"button","nodeId":"run","actionId":"sync.run","label":if self.last.as_ref().is_some_and(|last| last.review_pending || last.attention_required) { self.t("审阅并同步","Review and sync") } else { self.t("立即同步","Sync now") },"icon":"refresh","variant":"primary","disabled":!ready}),
            json!({"kind":"stack","nodeId":"scopeRow","direction":"horizontal","align":"center","gap":8,"children":["scopeText"]}),
            json!({"kind":"text","nodeId":"scopeText","text":self.t("范围：主机 · 凭据 · 远程桌面","Scope: hosts · credentials · remote desktops"),"style":"secondary","tone":"neutral"}),
            json!({"kind":"stack","nodeId":"policyRow","direction":"horizontal","align":"center","gap":8,"children":["policyText","policyButton"]}),
            json!({"kind":"text","nodeId":"policyText","text":self.policy_text(),"style":"secondary","tone":"neutral"}),
            json!({"kind":"button","nodeId":"policyButton","actionId":"norishell.openSettings:conflictPolicy","label":self.t("设置","Settings"),"icon":null,"variant":"secondary","disabled":false}),
            json!({"kind":"stack","nodeId":"browser","direction":"vertical","align":"stretch","gap":16,"children":["overview","datahosts","datacredentials","datadesktopProfiles"]}),
            json!({"kind":"stack","nodeId":"overview","direction":"vertical","align":"stretch","gap":16,"children":["overviewCounts","overviewSummary","browserHint"]}),
            json!({"kind":"grid","nodeId":"overviewCounts","columns":3,"gap":16,"children":["hostCountCard","credentialCountCard","desktopCountCard"]}),
            json!({"kind":"section","nodeId":"hostCountCard","title":self.t("云端主机","Cloud hosts"),"children":["hostCount"]}),
            json!({"kind":"text","nodeId":"hostCount","text":self.remote_count("hosts", |last| last.remote_hosts),"style":"heading","tone":"neutral"}),
            json!({"kind":"section","nodeId":"credentialCountCard","title":self.t("云端凭据","Cloud credentials"),"children":["credentialCount"]}),
            json!({"kind":"text","nodeId":"credentialCount","text":self.remote_count("credentials", |last| last.remote_credentials),"style":"heading","tone":"neutral"}),
            json!({"kind":"section","nodeId":"desktopCountCard","title":self.t("云端远程桌面","Cloud remote desktops"),"children":["desktopCount"]}),
            json!({"kind":"text","nodeId":"desktopCount","text":self.remote_count("desktopProfiles", |last| last.remote_desktops),"style":"heading","tone":"neutral"}),
            json!({"kind":"text","nodeId":"overviewSummary","text":self.summary_text(),"style":"secondary","tone":"neutral"}),
            json!({"kind":"text","nodeId":"browserHint","text":self.browser_hint(),"style":"secondary","tone":"neutral"}),
            self.browser.table(
                "hosts",
                self.t("主机", "Hosts"),
                self.t("名称", "Name"),
                self.t("地址", "Address"),
                self.t("尚无已验证的项目", "No verified items yet"),
            ),
            self.browser.table(
                "credentials",
                self.t("凭据", "Credentials"),
                self.t("名称", "Name"),
                self.t("类型", "Type"),
                self.t("尚无已验证的项目", "No verified items yet"),
            ),
            self.browser.table(
                "desktopProfiles",
                self.t("远程桌面", "Remote desktops"),
                self.t("名称", "Name"),
                self.t("地址", "Address"),
                self.t("尚无已验证的项目", "No verified items yet"),
            ),
        ];
        if insecure_http {
            nodes.push(json!({"kind":"status","nodeId":"httpWarning","label":self.t("当前使用 HTTP，建议改用 HTTPS。","Using HTTP. HTTPS is recommended."),"tone":"warning"}));
        }
        if !ready {
            nodes.retain(|node| node["nodeId"] != "serverSettings");
            if let Some(settings) = nodes
                .iter_mut()
                .find(|node| node["nodeId"] == "settingsDialog")
            {
                settings["children"] = json!(["scopeRow", "policyRow"]);
            }
            nodes.push(json!({"kind":"section","nodeId":"setupNotice","title":self.t("请先配置服务器","Set up your server"),"children":["setupStatus","setupSettings"]}));
            nodes.push(json!({"kind":"status","nodeId":"setupStatus","label":if self.invalid_server { self.t("服务器地址无效：请填写 HTTP/HTTPS 地址或 IP:端口。","Invalid server address. Enter an HTTP/HTTPS URL or IP:port.") } else { self.t("填写服务器地址后即可连接并同步。","Enter your server address to connect and sync.") },"tone":"warning"}));
            nodes.push(json!({"kind":"button","nodeId":"setupSettings","actionId":"norishell.openSettings:serverUrl","label":self.t("填写服务器地址","Enter server address"),"icon":"settings","variant":"primary","disabled":false}));
        }
        if let Some(error) = self.last.as_ref().and_then(|last| last.error.as_ref()) {
            let remote_updated = self.last.as_ref().is_some_and(|last| last.remote_updated);
            let label = if remote_updated {
                format!(
                    "{}：{}",
                    self.t(
                        "云端已更新，本机尚未完成",
                        "Cloud updated; local completion pending"
                    ),
                    self.error_message(error)
                )
            } else {
                self.error_message(error)
            };
            nodes.push(
                json!({"kind":"status","nodeId":"errorNotice","label":label,"tone":"danger"}),
            );
        }
        json!({"schemaVersion":1,"rootNodeId":"root","nodes":nodes})
    }

    fn remote_count(&self, category: &str, count: impl FnOnce(&Summary) -> Option<u64>) -> String {
        self.last
            .as_ref()
            .and_then(count)
            .or_else(|| self.browser.category_count(category))
            .map_or_else(|| "—".to_owned(), |value| value.to_string())
    }

    fn server_label(&self) -> &str {
        self.origin
            .as_deref()
            .map(|origin| {
                origin
                    .strip_prefix("https://")
                    .or_else(|| origin.strip_prefix("http://"))
                    .unwrap_or(origin)
            })
            .unwrap_or(self.t("尚未配置服务器", "Server not configured"))
    }

    fn sync_state_label(&self) -> &str {
        let Some(last) = &self.last else {
            return self.t("远端状态需刷新", "Refresh remote status");
        };
        if last.error.is_some() {
            return if last.remote_updated {
                self.t(
                    "云端已更新，本机尚未完成",
                    "Cloud updated; local completion pending",
                )
            } else {
                self.t("操作未完成", "Action incomplete")
            };
        }
        if last.operation == "running" {
            return self.t("正在同步", "Sync in progress");
        }
        if last.review_pending || last.attention_required {
            return self.t("需要确认", "Confirmation required");
        }
        match last.difference.as_str() {
            "equal" => self.t("数据已一致", "Up to date"),
            "localOnly" | "remoteOnly" | "different" => self.t("有待同步的数据", "Changes to sync"),
            "conflict" => self.t("存在冲突", "Conflicts found"),
            _ => self.t("远端状态需刷新", "Refresh remote status"),
        }
    }

    fn sync_state_tone(&self) -> &str {
        match &self.last {
            Some(last) if last.error.is_none() && last.difference == "equal" => "success",
            Some(last) if last.operation == "running" => "info",
            _ => "warning",
        }
    }

    fn summary_text(&self) -> String {
        let Some(last) = &self.last else {
            return self.t("尚无同步结果。", "No sync result yet.").to_owned();
        };
        if last.operation == "idle" && last.difference.is_empty() {
            return if self.locale == "en" {
                format!(
                    "Account: {} · Select Refresh remote status to compare current data.",
                    self.account_label(&last.account)
                )
            } else {
                format!(
                    "账户：{} · 点击“刷新远端状态”比较当前数据。",
                    self.account_label(&last.account)
                )
            };
        }
        let remote = |value: Option<u64>| value.map_or_else(|| "—".to_owned(), |v| v.to_string());
        let review = if last.review_pending || last.attention_required {
            self.t(
                " · 点击「审阅并同步」继续审阅。",
                " · Select Review and sync to continue the review.",
            )
        } else {
            ""
        };
        let stale = if last.remote_counts_stale {
            self.t(
                " · 云端数量来自上次成功读取",
                " · Cloud counts are from the last successful read",
            )
        } else {
            ""
        };
        if self.locale == "en" {
            format!(
                "Account: {} · Operation: {} · Difference: {} · Hosts {} / {} · Desktops {} / {} · Credentials {} / {}{}{}{}",
                self.account_label(&last.account),
                self.operation_label(&last.operation),
                self.difference_label(&last.difference),
                last.local_hosts,
                remote(last.remote_hosts),
                last.local_desktops,
                remote(last.remote_desktops),
                last.local_credentials,
                remote(last.remote_credentials),
                last.error.as_ref().map_or_else(String::new, |e| format!(
                    " · Error: {}",
                    self.error_message(e)
                )),
                review,
                stale
            )
        } else {
            format!(
                "账户：{} · 操作：{} · 差异：{} · 主机 {} / {} · 远程桌面 {} / {} · 凭据 {} / {}{}{}{}",
                self.account_label(&last.account),
                self.operation_label(&last.operation),
                self.difference_label(&last.difference),
                last.local_hosts,
                remote(last.remote_hosts),
                last.local_desktops,
                remote(last.remote_desktops),
                last.local_credentials,
                remote(last.remote_credentials),
                last.error.as_ref().map_or_else(String::new, |e| format!(
                    " · 错误：{}",
                    self.error_message(e)
                )),
                review,
                stale
            )
        }
    }

    fn account_label(&self, state: &str) -> &str {
        match state {
            "disconnected" => self.t("未连接", "Disconnected"),
            "authorizing" => self.t("正在登录", "Signing in"),
            "needsMfa" => self.t("需要多因素验证", "MFA required"),
            "needsEmailVerification" => self.t("需要邮箱验证", "Email verification required"),
            "connected" => self.t("已连接", "Connected"),
            "expired" => self.t("登录已过期", "Sign-in expired"),
            _ => self.t("状态未知", "Unknown state"),
        }
    }

    fn operation_label(&self, state: &str) -> &str {
        match state {
            "idle" => self.t("空闲", "Idle"),
            "running" => self.t("进行中", "Running"),
            "needsReview" => self.t("需要审查", "Review required"),
            "succeeded" => self.t("已完成", "Completed"),
            "failed" => self.t("失败", "Failed"),
            "partial" => self.t("云端已更新，本机未完成", "Cloud updated; local incomplete"),
            _ => self.t("状态未知", "Unknown state"),
        }
    }

    fn difference_label(&self, state: &str) -> &str {
        match state {
            "unavailable" => self.t("暂无比较结果", "Not compared"),
            "equal" => self.t("一致", "Equal"),
            "localOnly" => self.t("仅本机有数据", "Local data only"),
            "remoteOnly" => self.t("仅远端有数据", "Remote data only"),
            "different" => self.t("存在差异", "Different"),
            "conflict" => self.t("冲突", "Conflict"),
            _ => self.t("状态未知", "Unknown state"),
        }
    }

    fn error_message(&self, code: &str) -> String {
        let message = self.error_label(code);
        let mut result = message.to_owned();
        if let Some(status) = self.last.as_ref().and_then(|last| last.http_status) {
            result.push_str(&format!(" (HTTP {status})"));
        }
        if let Some(diagnostic) = self
            .last
            .as_ref()
            .and_then(|last| last.diagnostic.as_deref())
        {
            result.push_str(&format!(" [{diagnostic}]"));
        }
        result
    }

    fn error_label(&self, code: &str) -> &str {
        match code {
            "vaultMissing" => self.t("请先创建本机 Vault。", "Create a local Vault first."),
            "vaultLocked" => self.t("请先解锁 Vault。", "Unlock the Vault first."),
            "vaultRequiresReload" => self.t(
                "Vault 状态需要重新加载。请重新解锁后再同步。",
                "The Vault needs to be reloaded. Unlock it again before syncing.",
            ),
            "interactionRequired" => self.t(
                "自动刷新需要你手动继续。请点击“刷新远端状态”；需要恢复密钥时才会打开密码窗口。",
                "Automatic refresh needs a manual step. Select Refresh remote status; a password window opens only if key recovery is needed.",
            ),
            "authorizationDenied" => self.t(
                "账号或密码错误，或当前无法登录。",
                "Incorrect account or password, or sign-in is unavailable.",
            ),
            "authorizationExpired" => {
                self.t("登录已过期，请重新登录。", "Sign-in expired. Log in again.")
            }
            "accountNotConnected" => self.t(
                "此插件版本尚未登录。请先连接账户；已有 Vault 数据不会因这次检查被删除。",
                "This plugin version is not signed in. Connect your account first; this check does not remove existing Vault data.",
            ),
            "accessDenied" => self.t(
                "此账号无权同步，请检查访问权限。",
                "This account cannot sync. Check its access permissions.",
            ),
            "permissionDenied" => self.t(
                "本次同步无权限，请检查插件授权与账号访问权限。",
                "Sync permission was denied. Check the plugin grant and account access.",
            ),
            "quotaExceeded" => self.t(
                "同步配额或数据大小超出限制。",
                "Sync quota or data size limit exceeded.",
            ),
            "networkUnavailable" => self.t(
                "无法连接服务器，请检查网络。",
                "Cannot reach the server. Check your network.",
            ),
            "serviceUnavailable" => self.t(
                "服务器暂时不可用，请稍后重试。",
                "The server is unavailable. Try again later.",
            ),
            "stateConflict" => self.t(
                "同步所依据的版本或操作状态发生变化。请重试；持续出现时保留错误编号。",
                "The version or operation state used for sync changed. Retry and keep the diagnostic identifier if this continues.",
            ),
            "conflict" => self.t(
                "同步状态已变化，请刷新远端状态后重试。",
                "Sync state changed. Refresh remote status and retry.",
            ),
            "localStateChanged" => self.t("本机同步数据在操作期间发生变化。请重试同步。", "Local sync data changed during the operation. Retry sync."),
            "ownerConflict" => self.t(
                "本机同步数据的归属记录无效或存在歧义。已停止同步，请保留现有数据。",
                "Local sync ownership records are invalid or ambiguous. Sync stopped; preserve the existing data.",
            ),
            "keyBindingConflict" => self.t(
                "远端同步密钥绑定与本机记录不一致。已停止同步，不会覆盖任一端数据。",
                "The remote sync key binding differs from the local record. Sync stopped without overwriting either side.",
            ),
            "revisionExhausted" => self.t(
                "远端同步修订号已达上限，重试无法解决。",
                "The remote sync revision reached its limit; retrying cannot resolve this.",
            ),
            "restoreConflict" => self.t(
                "云端数据与本机同步对象发生恢复冲突，本次写入已停止；这里没有可继续的审阅窗口。",
                "Cloud data conflicts with existing local sync objects. Restore stopped; there is no review window to continue this attempt.",
            ),
            "mergeInvalid" => self.t(
                "Core 无法生成可验证的合并结果，本次同步已停止，数据未被覆盖。",
                "Core could not produce a valid merged result. Sync stopped without overwriting data.",
            ),
            "remoteRequestRejected" => self.t(
                "服务器拒绝了本次同步请求，请检查服务器地址及服务端状态。",
                "The server rejected this sync request. Check the server address and service status.",
            ),
            "remoteDataInvalid" => self.t(
                "远端数据无法验证，同步已停止。",
                "Remote data could not be verified. Sync stopped.",
            ),
            "remoteFormatUnsupported" => self.t(
                "远端数据格式不受支持，请先核对本机数据。",
                "Remote data format is unsupported. Check your local data first.",
            ),
            "recoveryRemoteKeyAuthenticationFailed" => self.t(
                "远端同步密钥解密校验失败。请核对最初上传远端数据时使用的 Vault 密码；若密码确认正确，远端密钥可能已损坏。",
                "The remote sync key failed decryption verification. Check the Vault password used when the remote data was first uploaded. If it is correct, the remote key may be damaged.",
            ),
            "recoveryAuthenticationFailed" => self.t(
                "远端同步密钥的密码校验失败。请核对首次上传时使用的 Vault 密码。",
                "The password did not authenticate the remote sync key. Check the Vault password used for the first upload.",
            ),
            "recoveryActionExpired" => self.t(
                "输入密码期间同步状态已变化。请重新点击刷新或同步。",
                "Sync state changed while entering the password. Select refresh or sync again.",
            ),
            "localDataInvalid" => self.t("本机同步数据校验失败。下方显示失败阶段与具体原因。", "Local sync data failed validation. The failed stage and reason are shown below."),
            "localKeyUnavailable" => self.t("本机 Vault 中的同步密钥无法读取或导出，尚未验证远端密码。", "The sync key could not be read or exported from the local Vault. The remote password has not been checked."),
            "operationBusy" => self.t("当前账户有另一项操作正在进行，请等待该操作结束。", "Another account operation is in progress. Wait for it to finish."),
            "busy" => self.t(
                "当前已有同步操作正在进行，请等待完成后重试。",
                "A sync operation is already running. Wait for it to finish and retry.",
            ),
            "operationRejected" => self.t(
                "同步检查未通过，未继续执行。错误位置见下方编号。",
                "A sync check failed and stopped the operation. The diagnostic identifier locates the failed check.",
            ),
            "invalidRequest" => self.t(
                "同步请求不符合 Core 接口要求，请更新插件或检查配置。",
                "The sync request does not meet the Core API contract. Update the plugin or check its settings.",
            ),
            "unavailable" => self.t(
                "Core 当前无法完成同步，请稍后重试；持续出现时保留错误编号。",
                "Core cannot complete sync right now. Retry later and keep the diagnostic identifier if this continues.",
            ),
            "cleanupIncomplete" => self.t(
                "同步资源清理未完成。请关闭并重新打开插件后再试。",
                "Sync resource cleanup did not finish. Close and reopen the plugin before retrying.",
            ),
            "internal" => self.t(
                "Core 无法安全完成本次同步，请保留错误编号。",
                "Core could not safely complete this sync. Keep the error ID.",
            ),
            "outcomeUnknown" => self.t(
                "上传结果尚未确认。请刷新云端状态后再决定下一步；本次未标记同步完成。",
                "The upload result is unconfirmed. Refresh cloud state before continuing; this operation was not marked complete.",
            ),
            "timedOut" => self.t("请求超时，请检查网络后重试。", "The request timed out. Check the network and retry."),
            "httpFailed" | "connectFailed" | "resolveFailed" => self.t("无法连接服务器，请检查地址和网络。", "Cannot reach the server. Check its address and the network."),
            "tlsFailed" => self.t("服务器 TLS 验证失败，请检查证书。", "Server TLS validation failed. Check its certificate."),
            "invalidResponse" | "protocolFailed" => self.t("返回数据不符合接口契约，操作已停止。", "The response does not match the API contract. The operation stopped."),
            "cancelled" => self.t("操作已取消。", "The operation was cancelled."),
            "revoked" => self.t("操作授权已失效，请重新发起。", "Operation authorization expired. Start a new operation."),
            _ => self.t(
                "操作未完成，请重试。",
                "Operation did not complete. Try again.",
            ),
        }
    }

    fn policy_text(&self) -> String {
        let conflict = if self.conflict_policy == "prompt" {
            self.t("询问", "Ask")
        } else {
            self.t("较新优先", "Newest")
        };
        let deletion = if self.deletion_policy == "prompt" {
            self.t("询问", "Ask")
        } else {
            self.t("自动删除", "Auto delete")
        };
        if self.locale == "en" {
            format!("Conflicts: {conflict} · Deletions: {deletion}")
        } else {
            format!("冲突：{conflict} · 删除：{deletion}")
        }
    }
}

fn canonical_origin(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let value = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        let http_candidate = Url::parse(&format!("http://{trimmed}")).ok()?;
        let local_or_ip = matches!(http_candidate.host(), Some(Host::Ipv4(_) | Host::Ipv6(_)))
            || http_candidate.host_str() == Some("localhost");
        format!("{}://{trimmed}", if local_or_ip { "http" } else { "https" })
    };
    let parsed = Url::parse(&value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

norishell_plugin_sdk::export_plugin!(SelfHostSync);

#[cfg(test)]
mod tests {
    use super::{SelfHostSync, canonical_origin, sync_flow};
    use serde_json::json;

    #[test]
    fn failed_exchange_releases_its_handles_before_showing_the_error() {
        let mut plugin = SelfHostSync {
            origin: Some("https://example.org".into()),
            flow: Some(sync_flow::Flow::new(
                sync_flow::Intent::Sync,
                "https://example.org/exchange".into(),
                "newest",
                "newest",
            )),
            ..Default::default()
        };
        plugin
            .api
            .request("request", sync_flow::Flow::snapshot_request())
            .unwrap();
        plugin
            .broker_result(
                "request",
                &json!({"actionId":"sync.run","result":{"kind":"api","reply":{
                "callId":"self-host.1","outcome":{"kind":"failed","code":"vaultLocked"}}}}),
            )
            .unwrap();
        assert!(plugin.pending_terminal.is_some());
        assert!(plugin.last.is_none());
        plugin
            .broker_result("request", &json!({"actionId":"sync.run","result":{"kind":"api","reply":{
                "callId":"self-host.2","outcome":{"kind":"completed","value":{"kind":"dataRelease"}}}}}))
            .unwrap();
        assert!(plugin.flow.is_none());
        assert_eq!(
            plugin.last.as_ref().and_then(|last| last.error.as_deref()),
            Some("vaultLocked")
        );
    }

    #[test]
    fn current_core_failures_keep_specific_user_guidance() {
        let mut plugin = SelfHostSync {
            locale: "zh-CN".into(),
            ..Default::default()
        };
        for (code, expected) in [
            ("vaultRequiresReload", "重新解锁"),
            ("permissionDenied", "插件授权"),
            ("conflict", "刷新远端状态"),
            ("busy", "等待完成"),
            ("recoveryAuthenticationFailed", "Vault 密码"),
            ("invalidRequest", "Core 接口"),
            ("unavailable", "Core 当前"),
            ("cleanupIncomplete", "资源清理"),
        ] {
            assert!(plugin.error_label(code).contains(expected), "{code}");
        }
        plugin.locale = "en".into();
        assert!(
            plugin
                .error_label("recoveryAuthenticationFailed")
                .contains("password")
        );
        assert!(plugin.error_label("cleanupIncomplete").contains("cleanup"));
    }

    #[test]
    fn failed_fetch_keeps_last_known_counts_but_successful_absence_replaces_them() {
        let mut plugin = SelfHostSync::default();
        plugin.record_result(&json!({"actionId":"sync.refresh","result":{
            "accountState":"connected","operationState":"succeeded","differenceState":"remoteOnly",
            "remoteHostCount":2,"remoteCredentialCount":1,"remoteDesktopProfileCount":3
        }}));
        plugin.record_result(&json!({"actionId":"sync.refresh","result":{
            "accountState":"connected","operationState":"failed","differenceState":"unavailable",
            "stableErrorCode":"networkUnavailable"
        }}));
        let stale = plugin.last.as_ref().expect("failed result");
        assert_eq!(stale.remote_hosts, Some(2));
        assert_eq!(stale.remote_credentials, Some(1));
        assert_eq!(stale.remote_desktops, Some(3));
        assert!(stale.remote_counts_stale);

        plugin.record_result(&json!({"actionId":"sync.refresh","result":{
            "accountState":"connected","operationState":"succeeded","differenceState":"localOnly",
            "remoteHostCount":0,"remoteCredentialCount":0,"remoteDesktopProfileCount":0
        }}));
        let current = plugin.last.as_ref().expect("successful empty result");
        assert_eq!(current.remote_hosts, Some(0));
        assert!(!current.remote_counts_stale);
    }

    #[test]
    fn restarted_offline_page_uses_verified_cached_counts() {
        let mut plugin = SelfHostSync::default();
        plugin.browser.replace(
            &[
                json!({"category":"hosts","label":"One"}),
                json!({"category":"credentials","label":"Login"}),
                json!({"category":"desktopProfiles","label":"Desktop"}),
            ],
            Some(1_790_429_340_000),
        );
        plugin.browser.stale = true;
        plugin.record_result(&json!({"actionId":"sync.refresh","result":{
            "accountState":"connected","operationState":"failed","differenceState":"unavailable",
            "stableErrorCode":"networkUnavailable"
        }}));
        let last = plugin.last.as_ref().unwrap();
        assert_eq!(
            (
                last.remote_hosts,
                last.remote_credentials,
                last.remote_desktops
            ),
            (Some(1), Some(1), Some(1))
        );
        assert!(last.remote_counts_stale);
    }

    #[test]
    fn account_status_does_not_claim_unread_local_counts() {
        let mut plugin = SelfHostSync {
            locale: "en".into(),
            ..Default::default()
        };
        plugin.record_account_result(&json!({"actionId":"sync.pageOpened","result":{
            "accountState":"connected","operationState":"idle"
        }}));
        let summary = plugin.summary_text();
        assert!(summary.contains("Account: Connected"));
        assert!(summary.contains("Refresh remote status"));
        assert!(!summary.contains("Hosts 0 /"));
    }

    #[test]
    fn failed_review_retry_keeps_the_explicit_review_action() {
        let mut plugin = SelfHostSync {
            locale: "en".to_owned(),
            origin: Some("https://example.org".to_owned()),
            ..Default::default()
        };
        plugin.record_result(&json!({"actionId":"sync.refresh","result":{
            "accountState":"connected","operationState":"needsReview","differenceState":"conflict",
            "remoteHostCount":1,"remoteCredentialCount":1,"remoteDesktopProfileCount":1
        }}));
        plugin.record_result(&json!({"actionId":"sync.run","result":{
            "accountState":"connected","operationState":"failed","differenceState":"unavailable",
            "stableErrorCode":"stateConflict"
        }}));

        let document = plugin.document();
        let run = document["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["nodeId"] == "run")
            .expect("run action");
        assert_eq!(run["label"], "Review and sync");
        assert!(plugin.last.as_ref().expect("result").review_pending);

        plugin.record_result(&json!({"actionId":"sync.run","result":{
            "accountState":"connected","operationState":"succeeded","differenceState":"equal",
            "remoteHostCount":1,"remoteCredentialCount":1,"remoteDesktopProfileCount":1
        }}));
        let document = plugin.document();
        let run = document["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["nodeId"] == "run")
            .expect("run action");
        assert_eq!(run["label"], "Sync now");
    }

    #[test]
    fn uploaded_remote_failure_preserves_partial_fact_and_original_error() {
        let mut plugin = SelfHostSync {
            origin: Some("https://example.org".into()),
            locale: "en".into(),
            ..Default::default()
        };
        let mut flow = sync_flow::Flow::new(
            sync_flow::Intent::Sync,
            "https://example.org/exchange".into(),
            "newest",
            "newest",
        );
        flow.upload = Some(super::network_flow::Receipt {
            handle: "put-receipt".into(),
            status: 200,
            blob: None,
            etag: Some("\"v2\"".into()),
        });
        plugin.flow = Some(flow);
        plugin
            .finish_flow(
                "request",
                &json!({"actionId":"sync.run"}),
                sync_flow::Transition::Failed {
                    code: "stateConflict".into(),
                    http_status: None,
                },
            )
            .expect("partial result");
        let summary = plugin.last.as_ref().expect("summary");
        assert_eq!(summary.operation, "partial");
        assert_eq!(summary.error.as_deref(), Some("stateConflict"));
        assert!(summary.remote_updated);
        plugin.record_account_result(&json!({"actionId":"sync.status","result":{
            "accountState":"connected","operationState":"idle"
        }}));
        assert!(plugin.last.as_ref().unwrap().remote_updated);
        assert_eq!(plugin.last.as_ref().unwrap().operation, "partial");
        let document = plugin.document();
        let notice = document["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["nodeId"] == "errorNotice")
            .unwrap();
        assert!(
            notice["label"]
                .as_str()
                .unwrap()
                .contains("Cloud updated; local completion pending")
        );
    }

    #[test]
    fn legacy_status_attention_starts_a_new_category_data_flow() {
        let mut plugin = SelfHostSync {
            locale: "en".to_owned(),
            origin: Some("https://example.org".to_owned()),
            ..Default::default()
        };
        plugin.record_account_result(&json!({"actionId":"sync.pageOpened","result":{
            "accountState":"connected","operationState":"needsReview",
            "differenceState":"conflict","stableErrorCode":"vaultLocked"
        }}));

        let document = plugin.document();
        let nodes = document["nodes"].as_array().expect("nodes");
        let run = nodes
            .iter()
            .find(|node| node["nodeId"] == "run")
            .expect("run action");
        assert_eq!(run["label"], "Review and sync");
        assert!(nodes.iter().all(|node| node["nodeId"] != "reviewRemote"));
        assert!(!plugin.last.as_ref().expect("status").review_pending);

        let outputs = plugin
            .action("request", &json!({"actionId":"sync.run"}))
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert!(matches!(
            plugin.flow.as_ref().expect("new flow").phase,
            sync_flow::Phase::Snapshot
        ));
    }

    #[test]
    fn accepts_http_https_and_ip_ports_and_rejects_ambiguous_bases() {
        assert_eq!(
            canonical_origin("example.org"),
            Some("https://example.org".into())
        );
        assert_eq!(
            canonical_origin("https://[::1]:8443/"),
            Some("https://[::1]:8443".into())
        );
        assert_eq!(
            canonical_origin("192.168.1.10:8787"),
            Some("http://192.168.1.10:8787".into())
        );
        assert_eq!(
            canonical_origin("[::1]:8787"),
            Some("http://[::1]:8787".into())
        );
        assert_eq!(
            canonical_origin("http://example.org"),
            Some("http://example.org".into())
        );
        for invalid in [
            "ftp://example.org",
            "https://owner@example.org",
            "https://example.org/base",
            "https://example.org?q=1",
            "https://example.org/#fragment",
        ] {
            assert!(canonical_origin(invalid).is_none(), "accepted {invalid}");
        }
    }
}
