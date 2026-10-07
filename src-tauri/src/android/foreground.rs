//! Aggregates live Core resources every ~2 s and keeps the Android
//! `SessionForegroundService` in sync. Zero resources stops the service.
//!
//! Counting mirrors the tray status counts: a source whose snapshot fails keeps
//! its last successful value, so a transient error never drops the service
//! while resources are still alive.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use norishell_core_api::*;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_norishell_android::{
    ForegroundCounts, foreground_stop, foreground_update, log, request_notification_permission,
};

use crate::{
    sftp_session_service as sftp, ssh_session_service as ssh,
    telnet_session_service::TelnetSessionState,
};

const INTERVAL: Duration = Duration::from_secs(2);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct ForegroundWorker {
    stopped: Arc<AtomicBool>,
}

#[derive(Default)]
struct Cache {
    terminals: Option<u32>,
    local: Option<u32>,
    telnet: Option<u32>,
    desktop: Option<u32>,
    tunnels: Option<u32>,
    sftp: Option<u32>,
    transfers: Option<u32>,
}

impl Cache {
    fn counts(&self) -> ForegroundCounts {
        ForegroundCounts {
            sessions: self.terminals.unwrap_or(0)
                + self.local.unwrap_or(0)
                + self.telnet.unwrap_or(0)
                + self.desktop.unwrap_or(0),
            tunnels: self.tunnels.unwrap_or(0),
            sftp: self.sftp.unwrap_or(0),
            transfers: self.transfers.unwrap_or(0),
        }
    }
}

fn keep(slot: &mut Option<u32>, next: Option<usize>) {
    if let Some(next) = next {
        *slot = Some(u32::try_from(next).unwrap_or(u32::MAX));
    }
}

fn meta() -> RequestMeta {
    RequestMeta {
        request_id: RequestId::new(),
    }
}

async fn refresh(app: &AppHandle, cache: &mut Cache) {
    let service = app.state::<ssh::SshSessionService>();
    let telnet = app.state::<crate::telnet_session_service::TelnetSessionService>();
    let (ssh, local, telnet, sftp_sessions, transfers) = tokio::join!(
        tokio::time::timeout(SNAPSHOT_TIMEOUT, service.snapshot(RequestId::new())),
        tokio::time::timeout(SNAPSHOT_TIMEOUT, service.local_snapshot(RequestId::new())),
        tokio::time::timeout(SNAPSHOT_TIMEOUT, telnet.snapshot()),
        tokio::time::timeout(
            SNAPSHOT_TIMEOUT,
            sftp::sftp_session_snapshot(SftpSessionSnapshotRequest { meta: meta() }, app.state())
        ),
        tokio::time::timeout(
            SNAPSHOT_TIMEOUT,
            sftp::sftp_transfer_intent_snapshot(
                SftpTransferIntentSnapshotRequest { meta: meta() },
                app.state()
            )
        )
    );
    keep(
        &mut cache.terminals,
        ssh.ok().and_then(Result::ok).map(|snapshot| {
            snapshot
                .sessions
                .iter()
                .filter(|s| !matches!(s.state, SshSessionState::Closed | SshSessionState::Failed))
                .count()
        }),
    );
    keep(
        &mut cache.local,
        local.ok().and_then(Result::ok).map(|snapshot| {
            snapshot
                .sessions
                .iter()
                .filter(|s| {
                    !matches!(
                        s.state,
                        LocalSessionState::Closed
                            | LocalSessionState::Exited
                            | LocalSessionState::Failed
                    )
                })
                .count()
        }),
    );
    keep(
        &mut cache.telnet,
        telnet.ok().and_then(Result::ok).map(|sessions| {
            sessions
                .iter()
                .filter(|s| {
                    !matches!(
                        s.state,
                        TelnetSessionState::Closed | TelnetSessionState::Failed
                    )
                })
                .count()
        }),
    );
    keep(
        &mut cache.sftp,
        sftp_sessions.ok().and_then(Result::ok).map(|snapshot| {
            snapshot
                .sessions
                .iter()
                .filter(|s| !matches!(s.state, SftpSessionState::Closed | SftpSessionState::Failed))
                .count()
        }),
    );
    keep(
        &mut cache.transfers,
        transfers.ok().and_then(Result::ok).map(|snapshot| {
            snapshot
                .transfers
                .iter()
                .filter(|t| {
                    !matches!(
                        t.state,
                        SftpTransferState::Completed
                            | SftpTransferState::Cancelled
                            | SftpTransferState::Failed
                    )
                })
                .count()
        }),
    );
    keep(
        &mut cache.tunnels,
        app.state::<crate::forward_session_service::ForwardSessionService>()
            .wire_summaries()
            .ok()
            .map(|summaries| {
                summaries
                    .iter()
                    .filter(|s| {
                        !matches!(
                            s.state,
                            ForwardSessionState::Stopped | ForwardSessionState::Failed
                        )
                    })
                    .count()
            }),
    );
    keep(
        &mut cache.desktop,
        Some(
            app.state::<crate::desktop_service::DesktopService>()
                .snapshot()
                .iter()
                .filter(|s| {
                    !matches!(
                        s.state,
                        DesktopSessionState::Closed | DesktopSessionState::Failed
                    )
                })
                .count(),
        ),
    );
}

/// App language preference for the notification text: `en`, `zh-CN`, or
/// `system` (device language) when unset or unreadable. Read every tick so a
/// language change in Settings reaches the notification within one interval.
fn app_locale(app: &AppHandle) -> &'static str {
    let locale = app
        .try_state::<crate::host_service::HostService>()
        .and_then(|hosts| {
            hosts
                .with_desktop_repository(|repository| {
                    repository
                        .get_application_preferences(ApplicationPreferenceGroupId::Application)
                })
                .ok()
        })
        .and_then(|snapshot| snapshot.value)
        .and_then(|value| {
            value
                .get("locale")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    match locale.as_deref() {
        Some("en") => "en",
        Some("zh-CN") => "zh-CN",
        _ => "system",
    }
}

/// Starts the worker once; later calls are ignored.
pub(super) fn start(app: AppHandle) {
    if app.try_state::<ForegroundWorker>().is_some() {
        return;
    }
    let worker = ForegroundWorker::default();
    let stopped = worker.stopped.clone();
    app.manage(worker);
    tauri::async_runtime::spawn(async move {
        let mut cache = Cache::default();
        // Last counts and language pushed to Kotlin; None while the service is stopped.
        let shown: Arc<Mutex<Option<(ForegroundCounts, &'static str)>>> = Arc::default();
        let mut permission_requested = false;
        while !stopped.load(Ordering::Acquire) {
            refresh(&app, &mut cache).await;
            let counts = cache.counts();
            let locale = app_locale(&app);
            let previous = *shown.lock().unwrap_or_else(|poison| poison.into_inner());
            // The system permission dialog would cover a visible Secure Surface
            // (host key, Vault, credential prompt) and take its input: wait until
            // every surface closed, without spending the one-time request.
            let permission_due = counts.total() > 0
                && !permission_requested
                && !crate::secure_surface::android::any_open();
            if permission_due
                || (previous != Some((counts, locale))
                    && !(previous.is_none() && counts.total() == 0))
            {
                let request_permission = permission_due;
                permission_requested |= request_permission;
                let shown = shown.clone();
                let stopped = stopped.clone();
                // Bridge calls block on the Android UI thread; keep them off the runtime.
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    if stopped.load(Ordering::Acquire) {
                        return;
                    }
                    if request_permission && request_notification_permission().is_err() {
                        log::warn("notification permission request failed");
                    }
                    let result = if counts.total() == 0 {
                        foreground_stop().map(|()| None)
                    } else {
                        foreground_update(counts, locale).map(|()| Some((counts, locale)))
                    };
                    match result {
                        Ok(next) => {
                            *shown.lock().unwrap_or_else(|poison| poison.into_inner()) = next;
                        }
                        Err(error) => {
                            log::warn(&format!("foreground service sync failed: {}", error.code()))
                        }
                    }
                })
                .await;
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// Stops the worker. The service is not stopped explicitly: the process is
/// exiting, the service is `START_NOT_STICKY`, and a blocking bridge call
/// during teardown would race the runtime shutdown.
pub(super) fn stop<R: Runtime>(app: &AppHandle<R>) {
    if let Some(worker) = app.try_state::<ForegroundWorker>() {
        worker.stopped.store(true, Ordering::Release);
    }
}
