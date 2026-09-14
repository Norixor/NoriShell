use std::{collections::HashMap, time::Duration};

use norishell_core_api::*;
use tauri::{AppHandle, Manager};

use super::{Action, meta};
use crate::{
    native_notification_service::{NativeNotificationService, NotificationPauseStatus},
    sftp_session_service as sftp, ssh_session_service as ssh,
    telnet_session_service::{TelnetSessionState as TelnetState, TelnetSessionSummary},
};

const RESOURCE_LIMIT: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub label: String,
    /// Native menus still use label; the panel may use an unnumbered, privacy-filtered name.
    pub panel_label: Option<String>,
    pub action: Option<Action>,
    pub role: Option<NativeTrayPanelRole>,
    pub children: Vec<Row>,
}
impl Row {
    pub(super) fn leaf(label: impl Into<String>, action: Option<Action>) -> Self {
        Self {
            label: label.into(),
            panel_label: None,
            action,
            role: None,
            children: vec![],
        }
    }
    fn group(label: impl Into<String>, mut children: Vec<Row>, locale: NativeTrayLocale) -> Self {
        if children.is_empty() {
            children.push(Self::leaf(text(locale, "empty"), None));
        }
        Self {
            label: label.into(),
            panel_label: None,
            action: None,
            role: None,
            children,
        }
    }
    pub(super) fn with_role(mut self, role: NativeTrayPanelRole) -> Self {
        self.role = Some(role);
        self
    }
    pub(super) fn with_panel_label(mut self, label: impl Into<String>) -> Self {
        self.panel_label = Some(label.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Projection {
    pub rows: Vec<Row>,
    pub tooltip: String,
    pub preferences_revision: Option<WireSequence>,
    pub locale: NativeTrayLocale,
    pub stats: Vec<NativeTrayPanelStat>,
    pub error_summary: Option<String>,
    pub notification_state: NativeTrayPanelNotificationState,
}
impl Projection {
    pub fn initial(locale: NativeTrayLocale) -> Self {
        Self {
            rows: vec![
                Row::leaf(text(locale, "show"), Some(Action::Show)),
                Row::leaf(text(locale, "unavailable"), None),
                Row::leaf(text(locale, "quit"), Some(Action::Quit)),
            ],
            tooltip: "NoriShell".into(),
            preferences_revision: None,
            locale,
            stats: vec![],
            error_summary: None,
            notification_state: NativeTrayPanelNotificationState::Unavailable,
        }
    }
    pub fn item_count(&self) -> usize {
        self.rows.iter().map(|row| 1 + row.children.len()).sum()
    }
}

/// Each source retains its last successful snapshot; failures must not turn real active resources into zeros.
#[derive(Default)]
pub(super) struct Cache {
    hosts: Option<Vec<HostCatalogEntry>>,
    ssh: Option<Vec<SshSessionSummary>>,
    local: Option<Vec<LocalSessionSummary>>,
    telnet: Option<Vec<TelnetSessionSummary>>,
    desktop: Option<Vec<DesktopSessionSummary>>,
    forwards: Option<Vec<ForwardSessionSummary>>,
    sftp: Option<Vec<SftpSessionSummary>>,
    transfers: Option<Vec<SftpTransferIntentSummary>>,
}

fn retain_snapshot<T>(old: &mut Option<T>, next: Option<T>) -> bool {
    if let Some(next) = next {
        *old = Some(next);
        true
    } else {
        false
    }
}

impl Cache {
    pub async fn refresh(&mut self, app: &AppHandle, locale: NativeTrayLocale) -> Projection {
        let preferences_snapshot = app
            .state::<crate::desktop_preferences::DesktopPreferencesService>()
            .snapshot()
            .ok();
        let preferences_revision = preferences_snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision);
        let preferences = preferences_snapshot.map(|snapshot| snapshot.preferences);
        // Hide all private labels when persisted settings cannot be read, and expose the unavailable state.
        let show_names = preferences.as_ref().is_some_and(|p| p.tray_show_host_names);
        let show_status = preferences.as_ref().is_none_or(|p| p.tray_show_status);
        let recent_limit = preferences
            .as_ref()
            .map_or(0, |p| usize::from(p.tray_recent_limit));
        let hosts = app
            .state::<crate::host_service::HostService>()
            .with_desktop_repository(|repo| {
                repo.list_host_catalog(HostCatalogSort::RecentlyConnected)
            })
            .ok();
        let mut available = retain_snapshot(&mut self.hosts, hosts) && preferences.is_some();
        let service = app.state::<ssh::SshSessionService>();
        let telnet = app.state::<crate::telnet_session_service::TelnetSessionService>();
        let (ssh, local, telnet, sftp, transfers) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(2), service.snapshot(RequestId::new())),
            tokio::time::timeout(
                Duration::from_secs(2),
                service.local_snapshot(RequestId::new())
            ),
            tokio::time::timeout(Duration::from_secs(2), telnet.snapshot()),
            tokio::time::timeout(
                Duration::from_secs(2),
                sftp::sftp_session_snapshot(
                    SftpSessionSnapshotRequest { meta: meta() },
                    app.state()
                )
            ),
            tokio::time::timeout(
                Duration::from_secs(2),
                sftp::sftp_transfer_intent_snapshot(
                    SftpTransferIntentSnapshotRequest { meta: meta() },
                    app.state()
                )
            )
        );
        available &= retain_snapshot(
            &mut self.ssh,
            ssh.ok().and_then(Result::ok).map(|s| s.sessions),
        );
        available &= retain_snapshot(
            &mut self.local,
            local.ok().and_then(Result::ok).map(|s| s.sessions),
        );
        available &= retain_snapshot(&mut self.telnet, telnet.ok().and_then(Result::ok));
        available &= retain_snapshot(
            &mut self.sftp,
            sftp.ok().and_then(Result::ok).map(|s| s.sessions),
        );
        available &= retain_snapshot(
            &mut self.transfers,
            transfers.ok().and_then(Result::ok).map(|s| s.transfers),
        );
        available &= retain_snapshot(
            &mut self.forwards,
            app.state::<crate::forward_session_service::ForwardSessionService>()
                .wire_summaries()
                .ok(),
        );
        retain_snapshot(
            &mut self.desktop,
            Some(
                app.state::<crate::desktop_service::DesktopService>()
                    .snapshot(),
            ),
        );
        let names: HashMap<_, _> = self
            .hosts
            .iter()
            .flatten()
            .map(|entry| (entry.host.host_id.clone(), entry.host.label.clone()))
            .collect();

        let mut rows = vec![
            Row::leaf(text(locale, "show"), Some(Action::Show)),
            frontend(text(locale, "new"), NativeTrayAction::NewTerminal),
            frontend(text(locale, "local"), NativeTrayAction::NewLocalTerminal),
            frontend(text(locale, "quick"), NativeTrayAction::QuickConnect),
            frontend(text(locale, "settings"), NativeTrayAction::Settings),
        ];
        let counts = self.status_counts();
        let summary = counts.summary(locale);
        if show_status {
            rows.push(Row::leaf(summary.clone(), None).with_role(NativeTrayPanelRole::Summary));
        }
        if !available {
            rows.push(Row::leaf(text(locale, "unavailable"), None));
        }
        if recent_limit > 0 {
            let recent = self
                .hosts
                .iter()
                .flatten()
                .filter(|entry| entry.recent_connection.is_some())
                .take(recent_limit)
                .enumerate()
                .map(|(index, entry)| {
                    frontend(
                        private_label(show_names, &entry.host.label, text(locale, "host"), index),
                        NativeTrayAction::OpenHost {
                            host_id: entry.host.host_id.clone(),
                        },
                    )
                    .with_role(NativeTrayPanelRole::RecentHost)
                    .with_panel_label(private_panel_label(
                        show_names,
                        &entry.host.label,
                        text(locale, "host"),
                    ))
                })
                .collect();
            rows.push(
                Row::group(text(locale, "recent"), recent, locale)
                    .with_role(NativeTrayPanelRole::Recent),
            );
        }

        let mut terminals = Vec::new();
        let mut ssh_sessions: Vec<_> = self
            .ssh
            .iter()
            .flatten()
            .filter(|s| s.state != SshSessionState::Closed)
            .collect();
        ssh_sessions.sort_by(|a, b| a.session_id.as_str().cmp(b.session_id.as_str()));
        for (index, session) in ssh_sessions.into_iter().take(RESOURCE_LIMIT).enumerate() {
            let name = match &session.target {
                SshSessionTarget::Host { host_id, .. } => {
                    names.get(host_id).cloned().unwrap_or_default()
                }
                SshSessionTarget::QuickConnect { endpoint } => endpoint.address.clone(),
            };
            let label = resource_label(
                locale,
                show_status,
                "SSH",
                &private_label(show_names, &name, text(locale, "session"), index),
                session.state == SshSessionState::Running,
                session.state == SshSessionState::Failed,
            );
            let scope = if let Some(channel_id) = &session.channel_id {
                tokio::time::timeout(
                    Duration::from_millis(500),
                    ssh::ssh_terminal_get(
                        SshSessionGetRequest {
                            meta: meta(),
                            session_id: session.session_id.clone(),
                        },
                        app.state(),
                    ),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|details| {
                    if details.session.generation != session.generation
                        || details.session.channel_id.as_ref() != Some(channel_id)
                    {
                        return None;
                    }
                    details
                        .attachments
                        .into_iter()
                        .find(|a| {
                            a.generation == session.generation
                                && a.channel_id.as_ref() == Some(channel_id)
                        })
                        .map(|a| NativeTerminalSessionScope::Ssh {
                            session_id: session.session_id.clone(),
                            generation: session.generation,
                            channel_id: channel_id.clone(),
                            pane_id: a.view_id,
                        })
                })
            } else {
                None
            };
            terminals.push(Row::leaf(
                label,
                Some(Action::Frontend(scope.map_or_else(
                    || NativeTrayAction::FocusSshSession {
                        session_id: session.session_id.clone(),
                        generation: session.generation,
                    },
                    |scope| NativeTrayAction::FocusTerminal { scope },
                ))),
            ));
        }
        let mut locals: Vec<_> = self
            .local
            .iter()
            .flatten()
            .filter(|s| {
                !matches!(
                    s.state,
                    LocalSessionState::Closed | LocalSessionState::Exited
                )
            })
            .collect();
        locals.sort_by(|a, b| a.session_id.as_str().cmp(b.session_id.as_str()));
        for (index, session) in locals.into_iter().take(RESOURCE_LIMIT).enumerate() {
            let label = resource_label(
                locale,
                show_status,
                text(locale, "localKind"),
                &private_label(
                    show_names,
                    &session.shell_name,
                    text(locale, "session"),
                    index,
                ),
                session.state == LocalSessionState::Running,
                session.state == LocalSessionState::Failed,
            );
            let scope = if let Some(pty_id) = &session.pty_id {
                tokio::time::timeout(
                    Duration::from_millis(500),
                    ssh::local_terminal_get(
                        LocalSessionGetRequest {
                            meta: meta(),
                            session_id: session.session_id.clone(),
                        },
                        app.state(),
                    ),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|details| {
                    if details.session.generation != session.generation
                        || details.session.pty_id.as_ref() != Some(pty_id)
                    {
                        return None;
                    }
                    details
                        .attachments
                        .into_iter()
                        .find(|a| {
                            a.generation == session.generation && a.pty_id.as_ref() == Some(pty_id)
                        })
                        .map(|a| NativeTerminalSessionScope::Local {
                            session_id: session.session_id.clone(),
                            generation: session.generation,
                            pty_id: pty_id.clone(),
                            pane_id: a.view_id,
                        })
                })
            } else {
                None
            };
            terminals.push(Row::leaf(
                label,
                Some(Action::Frontend(scope.map_or_else(
                    || NativeTrayAction::FocusLocalSession {
                        session_id: session.session_id.clone(),
                        generation: session.generation,
                    },
                    |scope| NativeTrayAction::FocusTerminal { scope },
                ))),
            ));
        }
        let mut telnets: Vec<_> = self
            .telnet
            .iter()
            .flatten()
            .filter(|s| s.state != TelnetState::Closed)
            .collect();
        telnets.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        for (index, session) in telnets.into_iter().take(RESOURCE_LIMIT).enumerate() {
            let label = resource_label(
                locale,
                show_status,
                "Telnet",
                &private_label(show_names, &session.address, text(locale, "session"), index),
                session.state == TelnetState::Running,
                session.state == TelnetState::Failed,
            );
            terminals.push(Row::leaf(
                label,
                Some(Action::Frontend(NativeTrayAction::FocusTelnet {
                    session_id: session.session_id.clone(),
                    generation: WireSequence::new(session.generation),
                    socket_id: session.socket_id.clone(),
                })),
            ));
        }
        let mut desktops: Vec<_> = self
            .desktop
            .iter()
            .flatten()
            .filter(|s| s.state != DesktopSessionState::Closed)
            .collect();
        desktops.sort_by(|a, b| a.id.cmp(&b.id));
        for (index, session) in desktops.into_iter().take(RESOURCE_LIMIT).enumerate() {
            terminals.push(frontend(
                resource_label(
                    locale,
                    show_status,
                    text(locale, "desktop"),
                    &private_label(
                        show_names,
                        &session.profile.label,
                        text(locale, "session"),
                        index,
                    ),
                    session.state == DesktopSessionState::Running,
                    session.state == DesktopSessionState::Failed,
                ),
                NativeTrayAction::FocusDesktop {
                    session_id: session.id.clone(),
                    generation: session.generation,
                },
            ));
        }
        rows.push(
            Row::group(text(locale, "sessions"), terminals, locale)
                .with_role(NativeTrayPanelRole::Sessions),
        );

        let mut tunnels = vec![frontend(
            text(locale, "manageTunnels"),
            NativeTrayAction::OpenTunnels {
                session_id: None,
                generation: None,
            },
        )];
        let mut forwards: Vec<_> = self
            .forwards
            .iter()
            .flatten()
            .filter(|s| s.state != ForwardSessionState::Stopped)
            .collect();
        forwards.sort_by(|a, b| a.session_id.as_str().cmp(b.session_id.as_str()));
        for (index, session) in forwards.into_iter().take(RESOURCE_LIMIT).enumerate() {
            let name = names.get(&session.host_id).map_or("", String::as_str);
            tunnels.push(frontend(
                resource_label(
                    locale,
                    show_status,
                    text(locale, "tunnel"),
                    &private_label(show_names, name, text(locale, "tunnel"), index),
                    session.state == ForwardSessionState::Running,
                    session.state == ForwardSessionState::Failed,
                ),
                NativeTrayAction::OpenTunnels {
                    session_id: Some(session.session_id.clone()),
                    generation: Some(session.generation),
                },
            ));
        }
        rows.push(
            Row::group(text(locale, "tunnels"), tunnels, locale)
                .with_role(NativeTrayPanelRole::Tunnels),
        );

        let mut transfers = vec![frontend(
            text(locale, "manageTransfers"),
            NativeTrayAction::OpenTransfers {
                transfer_id: None,
                state_revision: None,
                source_fence: None,
                target_fence: None,
            },
        )];
        let mut sessions: Vec<_> = self
            .sftp
            .iter()
            .flatten()
            .filter(|s| s.state != SftpSessionState::Closed)
            .collect();
        sessions.sort_by(|a, b| a.session_id.as_str().cmp(b.session_id.as_str()));
        for (index, session) in sessions.into_iter().take(RESOURCE_LIMIT).enumerate() {
            let name = session
                .host_id
                .as_ref()
                .and_then(|id| names.get(id))
                .map_or("", String::as_str);
            transfers.push(frontend(
                resource_label(
                    locale,
                    show_status,
                    "SFTP",
                    &private_label(show_names, name, text(locale, "session"), index),
                    session.state == SftpSessionState::Ready,
                    session.state == SftpSessionState::Failed,
                ),
                NativeTrayAction::OpenSftp {
                    session_id: session.session_id.clone(),
                    generation: session.generation,
                },
            ));
        }
        let mut intents: Vec<_> = self
            .transfers
            .iter()
            .flatten()
            .filter(|t| {
                !matches!(
                    t.state,
                    SftpTransferState::Completed | SftpTransferState::Cancelled
                )
            })
            .collect();
        intents.sort_by(|a, b| a.transfer_id.as_str().cmp(b.transfer_id.as_str()));
        for (index, transfer) in intents.into_iter().take(RESOURCE_LIMIT).enumerate() {
            // File paths and names never enter the system tray, even when host labels are allowed.
            let mut label = resource_label(
                locale,
                show_status,
                text(locale, "transfer"),
                &format!("{} {}", text(locale, "transfer"), index + 1),
                !matches!(
                    transfer.state,
                    SftpTransferState::Failed | SftpTransferState::PausedByDisconnect
                ),
                matches!(
                    transfer.state,
                    SftpTransferState::Failed | SftpTransferState::PausedByDisconnect
                ),
            );
            if show_status && transfer.expected_bytes > 0 {
                let percent = (u128::from(transfer.transferred_bytes) * 100
                    / u128::from(transfer.expected_bytes))
                .min(100);
                label.push_str(&format!(" · {percent}%"));
            }
            transfers.push(Row::leaf(
                label,
                Some(Action::Frontend(NativeTrayAction::OpenTransfers {
                    transfer_id: Some(transfer.transfer_id.clone()),
                    state_revision: Some(transfer.state_revision),
                    source_fence: Some(transfer.source_fence.clone()),
                    target_fence: Some(transfer.target_fence.clone()),
                })),
            ));
        }
        rows.push(
            Row::group(text(locale, "transfers"), transfers, locale)
                .with_role(NativeTrayPanelRole::Transfers),
        );
        let vault = app
            .state::<crate::vault_service::VaultService>()
            .status()
            .state;
        rows.push(
            (match vault {
                VaultState::Unlocked => {
                    Row::leaf(text(locale, "lockVault"), Some(Action::LockVault))
                }
                VaultState::Missing => frontend(
                    text(locale, "createVault"),
                    NativeTrayAction::Vault {
                        expected_state: vault,
                    },
                ),
                VaultState::Locked => frontend(
                    text(locale, "unlockVault"),
                    NativeTrayAction::Vault {
                        expected_state: vault,
                    },
                ),
                VaultState::RequiresReload => frontend(
                    text(locale, "reloadVault"),
                    NativeTrayAction::Vault {
                        expected_state: vault,
                    },
                ),
            })
            .with_role(NativeTrayPanelRole::Vault),
        );
        let notifications = app.state::<NativeNotificationService>();
        let mut pause = vec![
            Row::leaf(text(locale, "pause30"), Some(Action::Pause(30 * 60))),
            Row::leaf(text(locale, "pause60"), Some(Action::Pause(60 * 60))),
        ];
        let (label, notification_state) = match notifications.pause_status() {
            Ok(NotificationPauseStatus::Active) => (
                text(locale, "notifications").to_owned(),
                NativeTrayPanelNotificationState::Active,
            ),
            Ok(NotificationPauseStatus::Paused { remaining }) => {
                pause.push(
                    Row::leaf(text(locale, "resume"), Some(Action::Resume))
                        .with_role(NativeTrayPanelRole::Notifications),
                );
                (
                    format!(
                        "{} · {} {}",
                        text(locale, "paused"),
                        remaining.as_secs().div_ceil(60),
                        text(locale, "minutes")
                    ),
                    NativeTrayPanelNotificationState::Paused,
                )
            }
            Err(_) => {
                pause.clear();
                pause.push(Row::leaf(text(locale, "unavailable"), None));
                (
                    text(locale, "notifications").to_owned(),
                    NativeTrayPanelNotificationState::Unavailable,
                )
            }
        };
        rows.push(Row::group(label, pause, locale).with_role(NativeTrayPanelRole::Notifications));
        rows.push(Row::leaf(text(locale, "quit"), Some(Action::Quit)));
        let tooltip = if show_status {
            format!(
                "NoriShell · {}{}",
                summary,
                if available {
                    ""
                } else {
                    text(locale, "unavailableSuffix")
                }
            )
        } else {
            "NoriShell".into()
        };
        Projection {
            rows,
            tooltip,
            preferences_revision,
            locale,
            stats: counts.stats(show_status),
            error_summary: counts.error_summary(show_status, locale),
            notification_state,
        }
    }

    fn status_counts(&self) -> StatusCounts {
        let terminal_count = self
            .ssh
            .iter()
            .flatten()
            .filter(|s| !matches!(s.state, SshSessionState::Closed | SshSessionState::Failed))
            .count()
            + self
                .local
                .iter()
                .flatten()
                .filter(|s| {
                    !matches!(
                        s.state,
                        LocalSessionState::Closed
                            | LocalSessionState::Exited
                            | LocalSessionState::Failed
                    )
                })
                .count()
            + self
                .telnet
                .iter()
                .flatten()
                .filter(|s| !matches!(s.state, TelnetState::Closed | TelnetState::Failed))
                .count()
            + self
                .desktop
                .iter()
                .flatten()
                .filter(|s| {
                    !matches!(
                        s.state,
                        DesktopSessionState::Closed | DesktopSessionState::Failed
                    )
                })
                .count();
        let tunnel_count = self
            .forwards
            .iter()
            .flatten()
            .filter(|s| {
                !matches!(
                    s.state,
                    ForwardSessionState::Stopped | ForwardSessionState::Failed
                )
            })
            .count();
        let sftp_count = self
            .sftp
            .iter()
            .flatten()
            .filter(|s| !matches!(s.state, SftpSessionState::Closed | SftpSessionState::Failed))
            .count();
        let transfer_count = self
            .transfers
            .iter()
            .flatten()
            .filter(|t| {
                !matches!(
                    t.state,
                    SftpTransferState::Completed
                        | SftpTransferState::Cancelled
                        | SftpTransferState::Failed
                )
            })
            .count();
        let errors = self
            .ssh
            .iter()
            .flatten()
            .filter(|s| s.state == SshSessionState::Failed)
            .count()
            + self
                .local
                .iter()
                .flatten()
                .filter(|s| s.state == LocalSessionState::Failed)
                .count()
            + self
                .telnet
                .iter()
                .flatten()
                .filter(|s| s.state == TelnetState::Failed)
                .count()
            + self
                .desktop
                .iter()
                .flatten()
                .filter(|s| s.state == DesktopSessionState::Failed)
                .count()
            + self
                .forwards
                .iter()
                .flatten()
                .filter(|s| s.state == ForwardSessionState::Failed)
                .count()
            + self
                .sftp
                .iter()
                .flatten()
                .filter(|s| s.state == SftpSessionState::Failed)
                .count()
            + self
                .transfers
                .iter()
                .flatten()
                .filter(|s| {
                    matches!(
                        s.state,
                        SftpTransferState::Failed | SftpTransferState::PausedByDisconnect
                    )
                })
                .count();
        StatusCounts {
            sessions: count_option(
                self.ssh.is_some()
                    && self.local.is_some()
                    && self.telnet.is_some()
                    && self.desktop.is_some(),
                terminal_count,
            ),
            tunnels: count_option(self.forwards.is_some(), tunnel_count),
            sftp: count_option(self.sftp.is_some(), sftp_count),
            transfers: count_option(self.transfers.is_some(), transfer_count),
            errors,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatusCounts {
    sessions: Option<usize>,
    tunnels: Option<usize>,
    sftp: Option<usize>,
    transfers: Option<usize>,
    errors: usize,
}
impl StatusCounts {
    fn summary(self, locale: NativeTrayLocale) -> String {
        let mut summary = format!(
            "{} {} · {} {} · SFTP {} · {} {}",
            text(locale, "sessions"),
            count_label(self.sessions),
            text(locale, "tunnels"),
            count_label(self.tunnels),
            count_label(self.sftp),
            text(locale, "transfers"),
            count_label(self.transfers)
        );
        if self.errors > 0 {
            summary.push_str(&format!(" · ⚠ {} {}", text(locale, "errors"), self.errors));
        }
        summary
    }
    fn stats(self, show_status: bool) -> Vec<NativeTrayPanelStat> {
        if !show_status {
            return vec![];
        }
        [
            (NativeTrayPanelStatKind::Sessions, self.sessions),
            (NativeTrayPanelStatKind::Tunnels, self.tunnels),
            (NativeTrayPanelStatKind::Sftp, self.sftp),
            (NativeTrayPanelStatKind::Transfers, self.transfers),
        ]
        .into_iter()
        .map(|(kind, count)| NativeTrayPanelStat { kind, count })
        .collect()
    }
    fn error_summary(self, show_status: bool, locale: NativeTrayLocale) -> Option<String> {
        (show_status && self.errors > 0)
            .then(|| format!("{} {}", text(locale, "errors"), self.errors))
    }
}

fn count_option(known: bool, count: usize) -> Option<usize> {
    known.then_some(count)
}
fn count_label(count: Option<usize>) -> String {
    count.map_or_else(|| "—".into(), |count| count.to_string())
}

fn frontend(label: impl Into<String>, action: NativeTrayAction) -> Row {
    Row::leaf(label, Some(Action::Frontend(action)))
}
fn private_label(show: bool, name: &str, generic: &str, index: usize) -> String {
    if show && !name.trim().is_empty() {
        // Reserve room for status and numbering; long host labels must not displace error indicators.
        let name: String = name.chars().filter(|c| !c.is_control()).take(36).collect();
        format!("{name} · {}", index + 1)
    } else {
        format!("{generic} {}", index + 1)
    }
}
fn private_panel_label(show: bool, name: &str, generic: &str) -> String {
    if show && !name.trim().is_empty() {
        name.into()
    } else {
        generic.into()
    }
}
fn resource_label(
    locale: NativeTrayLocale,
    show_status: bool,
    kind: &str,
    name: &str,
    running: bool,
    failed: bool,
) -> String {
    if !show_status {
        return format!("{kind} · {name}");
    }
    format!(
        "{} · {} · {}",
        kind,
        text(
            locale,
            if failed {
                "failed"
            } else if running {
                "running"
            } else {
                "pending"
            }
        ),
        name,
    )
}

/// System menus reject control characters, newlines, and platform accelerator syntax; character counts are also bounded.
pub(super) fn safe_label(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars().filter(|c| !c.is_control() && !matches!(c,'\u{2028}'|'\u{2029}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).take(80){
        if character=='&'{out.push_str("&&");}else{out.push(character);}
    }
    out
}

fn text(locale: NativeTrayLocale, key: &str) -> &'static str {
    let zh = locale == NativeTrayLocale::ZhCn;
    match key {
        "show" => {
            if zh {
                "显示 NoriShell"
            } else {
                "Show NoriShell"
            }
        }
        "new" => {
            if zh {
                "新建终端"
            } else {
                "New Terminal"
            }
        }
        "local" => {
            if zh {
                "新建本地终端"
            } else {
                "New Local Terminal"
            }
        }
        "quick" => {
            if zh {
                "快速连接…"
            } else {
                "Quick Connect…"
            }
        }
        "settings" => {
            if zh {
                "设置"
            } else {
                "Settings"
            }
        }
        "quit" => {
            if zh {
                "退出 NoriShell…"
            } else {
                "Quit NoriShell…"
            }
        }
        "unavailable" => {
            if zh {
                "状态暂不可用（保留上次快照）"
            } else {
                "Status unavailable (last snapshot retained)"
            }
        }
        "unavailableSuffix" => {
            if zh {
                " · 状态暂不可用"
            } else {
                " · Status unavailable"
            }
        }
        "empty" => {
            if zh {
                "无项目"
            } else {
                "None"
            }
        }
        "host" => {
            if zh {
                "主机"
            } else {
                "Host"
            }
        }
        "session" => {
            if zh {
                "会话"
            } else {
                "Session"
            }
        }
        "recent" => {
            if zh {
                "最近连接"
            } else {
                "Recent Connections"
            }
        }
        "localKind" => {
            if zh {
                "本地"
            } else {
                "Local"
            }
        }
        "desktop" => {
            if zh {
                "远程桌面"
            } else {
                "Desktop"
            }
        }
        "sessions" => {
            if zh {
                "会话"
            } else {
                "Sessions"
            }
        }
        "manageTunnels" => {
            if zh {
                "管理隧道…"
            } else {
                "Manage Tunnels…"
            }
        }
        "tunnel" => {
            if zh {
                "隧道"
            } else {
                "Tunnel"
            }
        }
        "tunnels" => {
            if zh {
                "隧道"
            } else {
                "Tunnels"
            }
        }
        "manageTransfers" => {
            if zh {
                "传输活动…"
            } else {
                "Transfer Activity…"
            }
        }
        "transfer" => {
            if zh {
                "传输"
            } else {
                "Transfer"
            }
        }
        "transfers" => {
            if zh {
                "传输"
            } else {
                "Transfers"
            }
        }
        "lockVault" => {
            if zh {
                "Vault 已解锁 · 锁定"
            } else {
                "Vault Unlocked · Lock"
            }
        }
        "createVault" => {
            if zh {
                "Vault 未创建 · 创建…"
            } else {
                "Vault Missing · Create…"
            }
        }
        "unlockVault" => {
            if zh {
                "Vault 已锁定 · 解锁…"
            } else {
                "Vault Locked · Unlock…"
            }
        }
        "reloadVault" => {
            if zh {
                "Vault 需要重新加载 · 解锁…"
            } else {
                "Vault Requires Reload · Unlock…"
            }
        }
        "notifications" => {
            if zh {
                "暂停通知"
            } else {
                "Pause Notifications"
            }
        }
        "paused" => {
            if zh {
                "通知已暂停"
            } else {
                "Notifications Paused"
            }
        }
        "pause30" => {
            if zh {
                "暂停 30 分钟"
            } else {
                "Pause for 30 Minutes"
            }
        }
        "pause60" => {
            if zh {
                "暂停 1 小时"
            } else {
                "Pause for 1 Hour"
            }
        }
        "resume" => {
            if zh {
                "立即恢复通知"
            } else {
                "Resume Notifications"
            }
        }
        "minutes" => {
            if zh {
                "分钟"
            } else {
                "min"
            }
        }
        "running" => {
            if zh {
                "运行中"
            } else {
                "Running"
            }
        }
        "pending" => {
            if zh {
                "处理中"
            } else {
                "Pending"
            }
        }
        "failed" => {
            if zh {
                "⚠ 异常"
            } else {
                "⚠ Failed"
            }
        }
        "errors" => {
            if zh {
                "异常"
            } else {
                "Failures"
            }
        }
        _ => "NoriShell",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_source_does_not_erase_last_successful_baseline() {
        let mut cache = Some(vec![7, 9]);
        assert!(!retain_snapshot(&mut cache, None));
        assert_eq!(cache, Some(vec![7, 9]));
        assert!(retain_snapshot(&mut cache, Some(vec![])));
        assert_eq!(cache, Some(vec![]));
    }
    #[test]
    fn missing_initial_snapshots_are_not_reported_as_zero() {
        let counts = Cache::default().status_counts();
        let summary = counts.summary(NativeTrayLocale::En);
        assert_eq!(summary, "Sessions — · Tunnels — · SFTP — · Transfers —");
        assert_eq!(
            counts.stats(true),
            vec![
                NativeTrayPanelStat {
                    kind: NativeTrayPanelStatKind::Sessions,
                    count: None,
                },
                NativeTrayPanelStat {
                    kind: NativeTrayPanelStatKind::Tunnels,
                    count: None,
                },
                NativeTrayPanelStat {
                    kind: NativeTrayPanelStatKind::Sftp,
                    count: None,
                },
                NativeTrayPanelStat {
                    kind: NativeTrayPanelStatKind::Transfers,
                    count: None,
                },
            ]
        );
        assert!(counts.stats(false).is_empty());
    }
    #[test]
    fn private_labels_never_contain_remote_metadata() {
        for name in [
            "secret-user@production.example:22",
            "socks://admin:token@private:1080",
            "/home/private/file",
        ] {
            assert_eq!(private_label(false, name, "Session", 3), "Session 4");
        }
    }
    #[test]
    fn panel_recent_label_keeps_the_original_private_name_without_menu_suffix() {
        let name = "工程主机 · 1\n保留";
        assert_eq!(private_label(true, name, "Host", 2), "工程主机 · 1保留 · 3");
        assert_eq!(private_panel_label(true, name, "Host"), name);
        assert_eq!(private_panel_label(false, name, "Host"), "Host");
    }
    #[test]
    fn error_summary_requires_the_status_preference() {
        let counts = StatusCounts {
            sessions: Some(1),
            tunnels: Some(0),
            sftp: Some(0),
            transfers: Some(0),
            errors: 2,
        };
        assert_eq!(
            counts.error_summary(true, NativeTrayLocale::En),
            Some("Failures 2".into())
        );
        assert_eq!(counts.error_summary(false, NativeTrayLocale::En), None);
    }
    #[test]
    fn menu_labels_are_bounded_and_strip_control_and_accelerators() {
        assert_eq!(safe_label("A\nB\t&C\u{202e}"), "AB&&C");
        assert_eq!(safe_label(&"x".repeat(500)).len(), 80);
    }

    #[test]
    fn status_preference_hides_issue_indicators_and_long_names_cannot_hide_them() {
        let name = private_label(true, &"host".repeat(100), "Host", 1);
        let hidden = resource_label(NativeTrayLocale::En, false, "SSH", &name, false, true);
        assert!(!hidden.contains("Failed") && !hidden.contains('⚠'));
        let visible = safe_label(&resource_label(
            NativeTrayLocale::En,
            true,
            "SSH",
            &name,
            false,
            true,
        ));
        assert!(visible.contains("⚠ Failed"));
        assert!(visible.chars().count() <= 80);
    }
}
