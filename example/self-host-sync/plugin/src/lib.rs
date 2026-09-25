//! Declarative self-hosted sync page. Core owns credentials and all exchange bytes.

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
    auto_sync_enabled: bool,
    auto_sync_interval_minutes: String,
    check_on_startup: bool,
    conflict_policy: String,
    deletion_policy: String,
}

#[derive(Default)]
struct Summary {
    account: String,
    operation: String,
    difference: String,
    local_hosts: u64,
    remote_hosts: Option<u64>,
    local_desktops: u64,
    remote_desktops: Option<u64>,
    local_credentials: u64,
    remote_credentials: Option<u64>,
    error: Option<String>,
    diagnostic: Option<String>,
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
                self.record_result(&body);
                self.document_output(&request.request_id)
            }
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
            }
            self.invalid_server = !raw.trim().is_empty() && next.is_none();
            self.origin = next;
            self.auto_sync_enabled = values
                .get("autoSyncEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.auto_sync_interval_minutes = values
                .get("autoSyncIntervalMinutes")
                .and_then(Value::as_str)
                .unwrap_or("15")
                .to_owned();
            self.check_on_startup = values
                .get("checkOnStartup")
                .and_then(Value::as_bool)
                .unwrap_or(false);
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
        let request = match action {
            "sync.status" => json!({
                "action":"status", "profileId":PROFILE, "auth":self.auth()?
            }),
            "sync.pageOpened" | "sync.refresh" => json!({
                "action":"refresh", "profileId":PROFILE, "auth":self.auth()?,
                "source":{"url":self.endpoint("exchange")?, "useOauth":true}
            }),
            "sync.login" => json!({
                "action":"login", "profileId":PROFILE, "auth":self.auth()?,
                "usernameFieldId":"accountName", "passwordFieldId":"accountPassword"
            }),
            "sync.logout" => json!({
                "action":"logout", "profileId":PROFILE, "auth":self.auth()?
            }),
            "sync.run" => json!({
                "action":"sync", "profileId":PROFILE, "auth":self.auth()?,
                "source":{"url":self.endpoint("exchange")?, "useOauth":true},
                "destination":{"url":self.endpoint("exchange")?, "method":"put", "useOauth":true, "ifMatch":null},
                "conflictPolicy": if self.conflict_policy == "prompt" { "prompt" } else { "newest" },
                "deletionPolicy": if self.deletion_policy == "prompt" { "prompt" } else { "newest" }
            }),
            "sync.scope" => json!({"action":"configureScope", "profileId":PROFILE}),
            "sync.reset" => json!({
                "action":"resetRemote", "profileId":PROFILE, "auth":self.auth()?,
                "target":{"url":self.endpoint("exchange")?, "useOauth":true}
            }),
            _ => return Err(PluginError::InvalidRequest),
        };
        Ok(vec![output(request_id, "ssh.sync.request", &request)?])
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
        self.last = Some(Summary {
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
            local_hosts: result["localHostCount"].as_u64().unwrap_or(0),
            remote_hosts: result["remoteHostCount"].as_u64(),
            local_desktops: result["localDesktopProfileCount"].as_u64().unwrap_or(0),
            remote_desktops: result["remoteDesktopProfileCount"].as_u64(),
            local_credentials: result["localCredentialCount"].as_u64().unwrap_or(0),
            remote_credentials: result["remoteCredentialCount"].as_u64(),
            error: result["stableErrorCode"].as_str().map(str::to_owned),
            diagnostic: result["diagnosticCode"].as_str().map(str::to_owned),
        });
    }

    fn document_output(&self, request_id: &str) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        Ok(vec![output(
            request_id,
            "ui.document",
            &json!({
                "targetId":"app.page", "document":self.document()
            }),
        )?])
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
            json!({"kind":"dialog","nodeId":"settingsDialog","title":self.t("同步设置","Sync settings"),"description":null,"triggerLabel":self.t("设置","Settings"),"closeLabel":self.t("关闭","Close"),"children":["serverSettings","scopeRow","automationRow","policyRow","remoteRow"]}),
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
            json!({"kind":"button","nodeId":"run","actionId":"sync.run","label":self.t("立即同步","Sync now"),"icon":"refresh","variant":"primary","disabled":!ready}),
            json!({"kind":"stack","nodeId":"scopeRow","direction":"horizontal","align":"center","gap":8,"children":["scopeText","scopeButton"]}),
            json!({"kind":"text","nodeId":"scopeText","text":self.t("范围：主机 · 桌面 · 凭据 · 偏好","Scope: hosts · desktops · credentials · preferences"),"style":"secondary","tone":"neutral"}),
            json!({"kind":"button","nodeId":"scopeButton","actionId":"sync.scope","label":self.t("管理","Manage"),"icon":null,"variant":"secondary","disabled":!ready}),
            json!({"kind":"stack","nodeId":"automationRow","direction":"horizontal","align":"center","gap":8,"children":["automationText","automationButton"]}),
            json!({"kind":"text","nodeId":"automationText","text":self.automation_text(),"style":"secondary","tone":"neutral"}),
            json!({"kind":"button","nodeId":"automationButton","actionId":"norishell.openSettings:autoSyncEnabled","label":self.t("设置","Settings"),"icon":null,"variant":"secondary","disabled":false}),
            json!({"kind":"stack","nodeId":"policyRow","direction":"horizontal","align":"center","gap":8,"children":["policyText","policyButton"]}),
            json!({"kind":"text","nodeId":"policyText","text":self.policy_text(),"style":"secondary","tone":"neutral"}),
            json!({"kind":"button","nodeId":"policyButton","actionId":"norishell.openSettings:conflictPolicy","label":self.t("设置","Settings"),"icon":null,"variant":"secondary","disabled":false}),
            json!({"kind":"stack","nodeId":"remoteRow","direction":"horizontal","align":"center","gap":8,"children":["remoteText","resetDialog"]}),
            json!({"kind":"text","nodeId":"remoteText","text":self.t("远端数据：端到端加密","Remote data: end-to-end encrypted"),"style":"secondary","tone":"neutral"}),
            json!({"kind":"dialog","nodeId":"resetDialog","title":self.t("重置远端数据","Reset remote data"),"description":self.t("此操作会永久删除当前配置的远端密文，不能恢复；本机数据保留。Core 会再次请求明确确认。","This permanently deletes the remote ciphertext for this profile and cannot be undone. Local data is retained. Core asks for confirmation again."),"triggerLabel":self.t("重置…","Reset…"),"closeLabel":self.t("取消","Cancel"),"children":["resetButton"]}),
            json!({"kind":"button","nodeId":"resetButton","actionId":"sync.reset","label":self.t("继续到安全确认","Continue to secure confirmation"),"icon":null,"variant":"danger","disabled":!ready}),
            // The protected browser alone receives cloud rows, search state and selection.
            json!({"kind":"sshSyncBrowser","nodeId":"browser","profileId":PROFILE,"children":["overview"]}),
            json!({"kind":"stack","nodeId":"overview","direction":"vertical","align":"stretch","gap":16,"children":["overviewCounts","overviewSummary","browserHint"]}),
            json!({"kind":"grid","nodeId":"overviewCounts","columns":3,"gap":16,"children":["hostCountCard","credentialCountCard","desktopCountCard"]}),
            json!({"kind":"section","nodeId":"hostCountCard","title":self.t("云端主机","Cloud hosts"),"children":["hostCount"]}),
            json!({"kind":"text","nodeId":"hostCount","text":self.remote_count(|last| last.remote_hosts),"style":"heading","tone":"neutral"}),
            json!({"kind":"section","nodeId":"credentialCountCard","title":self.t("云端凭据","Cloud credentials"),"children":["credentialCount"]}),
            json!({"kind":"text","nodeId":"credentialCount","text":self.remote_count(|last| last.remote_credentials),"style":"heading","tone":"neutral"}),
            json!({"kind":"section","nodeId":"desktopCountCard","title":self.t("云端远程桌面","Cloud remote desktops"),"children":["desktopCount"]}),
            json!({"kind":"text","nodeId":"desktopCount","text":self.remote_count(|last| last.remote_desktops),"style":"heading","tone":"neutral"}),
            json!({"kind":"text","nodeId":"overviewSummary","text":self.summary_text(),"style":"secondary","tone":"neutral"}),
            json!({"kind":"text","nodeId":"browserHint","text":self.t("进入页面时自动加载云端项目，也可手动刷新。","Cloud items load when this page opens. You can also refresh manually."),"style":"secondary","tone":"neutral"}),
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
                settings["children"] =
                    json!(["scopeRow", "automationRow", "policyRow", "remoteRow"]);
            }
            nodes.push(json!({"kind":"section","nodeId":"setupNotice","title":self.t("请先配置服务器","Set up your server"),"children":["setupStatus","setupSettings"]}));
            nodes.push(json!({"kind":"status","nodeId":"setupStatus","label":if self.invalid_server { self.t("服务器地址无效：请填写 HTTP/HTTPS 地址或 IP:端口。","Invalid server address. Enter an HTTP/HTTPS URL or IP:port.") } else { self.t("填写服务器地址后即可连接并同步。","Enter your server address to connect and sync.") },"tone":"warning"}));
            nodes.push(json!({"kind":"button","nodeId":"setupSettings","actionId":"norishell.openSettings:serverUrl","label":self.t("填写服务器地址","Enter server address"),"icon":"settings","variant":"primary","disabled":false}));
        }
        if let Some(error) = self.last.as_ref().and_then(|last| last.error.as_ref()) {
            nodes.push(json!({"kind":"status","nodeId":"errorNotice","label":self.error_message(error),"tone":"warning"}));
        }
        json!({"schemaVersion":1,"rootNodeId":"root","nodes":nodes})
    }

    fn remote_count(&self, count: impl FnOnce(&Summary) -> Option<u64>) -> String {
        self.last
            .as_ref()
            .and_then(count)
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
            return self.t("操作未完成", "Action incomplete");
        }
        if last.operation == "running" {
            return self.t("正在同步", "Sync in progress");
        }
        if last.operation == "needsReview" {
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
        let remote = |value: Option<u64>| value.map_or_else(|| "—".to_owned(), |v| v.to_string());
        let review = if last.operation == "needsReview" && last.error.is_none() {
            self.t(" · 点击「立即同步」继续审查；若主窗口出现偏好待办，先处理该待办。", " · Select Sync now to continue the review; resolve any pending preferences shown in the main window first.")
        } else {
            ""
        };
        if self.locale == "en" {
            format!(
                "Account: {} · Operation: {} · Difference: {} · Hosts {} / {} · Desktops {} / {} · Credentials {} / {}{}{}",
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
                review
            )
        } else {
            format!(
                "账户：{} · 操作：{} · 差异：{} · 主机 {} / {} · 远程桌面 {} / {} · 凭据 {} / {}{}{}",
                self.account_label(&last.account),
                self.operation_label(&last.operation),
                self.difference_label(&last.difference),
                last.local_hosts,
                remote(last.remote_hosts),
                last.local_desktops,
                remote(last.remote_desktops),
                last.local_credentials,
                remote(last.remote_credentials),
                last.error
                    .as_ref()
                    .map_or_else(String::new, |e| format!(" · 错误：{}", self.error_message(e))),
                review
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
        match self.last.as_ref().and_then(|last| last.diagnostic.as_deref()) {
            Some(diagnostic) => format!("{message} [{diagnostic}]"),
            None => message.to_owned(),
        }
    }

    fn error_label(&self, code: &str) -> &str {
        match code {
            "vaultMissing" => self.t("请先创建本机 Vault。", "Create a local Vault first."),
            "vaultLocked" => self.t("请先解锁 Vault。", "Unlock the Vault first."),
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
            "accessDenied" => self.t(
                "此账号无权同步，请检查访问权限。",
                "This account cannot sync. Check its access permissions.",
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
                "同步状态发生变化，请重试；若持续出现，请更新 NoriShell。",
                "Sync state changed. Try again; if this persists, update NoriShell.",
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
            "recoveryActionExpired" => self.t(
                "输入密码期间同步状态已变化。请重新点击刷新或同步。",
                "Sync state changed while entering the password. Select refresh or sync again.",
            ),
            "localDataInvalid" => self.t("本机同步数据校验失败。下方显示失败阶段与具体原因。", "Local sync data failed validation. The failed stage and reason are shown below."),
            "preferencesUnavailable" => self.t("无法从主窗口读取应用偏好，本次同步尚未执行。请保持主窗口打开；持续出现时重新启动应用。", "Application preferences could not be read from the main window. Sync has not run. Keep the main window open; restart the app if this persists."),
            "localKeyUnavailable" => self.t("本机 Vault 中的同步密钥无法读取或导出，尚未验证远端密码。", "The sync key could not be read or exported from the local Vault. The remote password has not been checked."),
            "operationBusy" => self.t("当前账户有另一项操作正在进行，请等待该操作结束。", "Another account operation is in progress. Wait for it to finish."),
            "operationRejected" => self.t(
                "同步检查未通过，未继续执行。错误位置见下方编号。",
                "A sync check failed and stopped the operation. The diagnostic identifier locates the failed check.",
            ),
            "internal" => self.t(
                "本机安全状态保存失败，操作未完成。",
                "Local secure state could not be saved. Operation did not complete.",
            ),
            _ => self.t(
                "操作未完成，请重试。",
                "Operation did not complete. Try again.",
            ),
        }
    }

    fn automation_text(&self) -> String {
        let state = if self.auto_sync_enabled {
            self.t("已开启", "On")
        } else {
            self.t("已关闭", "Off")
        };
        let startup = if self.check_on_startup {
            self.t("开启", "On")
        } else {
            self.t("关闭", "Off")
        };
        if self.locale == "en" {
            format!(
                "Automatic: {state} · {} min · Startup: {startup}",
                self.auto_sync_interval_minutes
            )
        } else {
            format!(
                "自动：{state} · {} 分钟 · 启动：{startup}",
                self.auto_sync_interval_minutes
            )
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
    use super::canonical_origin;

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
