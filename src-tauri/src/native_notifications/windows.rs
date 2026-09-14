use std::{collections::VecDeque, sync::Mutex};

use windows::{
    Data::Xml::Dom::XmlDocument,
    Foundation::TypedEventHandler,
    UI::Notifications::{
        NotificationSetting, ToastDismissalReason, ToastDismissedEventArgs, ToastNotification,
        ToastNotificationManager, ToastNotifier,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{HSTRING, IInspectable},
};

use super::*;

type Pending = Arc<Mutex<VecDeque<PendingToast>>>;

struct PendingToast {
    id: String,
    toast: ToastNotification,
    activated: i64,
    dismissed: i64,
    failed: i64,
}

impl Drop for PendingToast {
    fn drop(&mut self) {
        let _ = self.toast.RemoveActivated(self.activated);
        let _ = self.toast.RemoveDismissed(self.dismissed);
        let _ = self.toast.RemoveFailed(self.failed);
    }
}

pub struct Adapter {
    app_id: String,
    callback: ClickHandler,
    pending: Pending,
}

impl Adapter {
    pub fn new(app: AppHandle, callback: ClickHandler) -> Self {
        // The Tauri installer's Start Menu shortcut must use the same AUMID.
        // Do not borrow another application's identity, such as PowerShell or Terminal, or register arbitrary URL activation.
        Self {
            app_id: app.config().identifier.clone(),
            callback,
            pending: Arc::default(),
        }
    }

    pub async fn permission(&self) -> Result<NotificationPermission, NotificationError> {
        let app_id = self.app_id.clone();
        tokio::task::spawn_blocking(move || {
            let _apartment = Apartment::new()?;
            permission(&notifier(&app_id)?)
        })
        .await
        .map_err(|_| NotificationError::Unavailable)?
    }

    pub async fn request_permission(&self) -> Result<NotificationPermission, NotificationError> {
        // Win32 toasts have no macOS-style permission prompt; users must re-enable them in system settings.
        self.permission().await
    }

    pub async fn send(
        &self,
        id: &str,
        title: &str,
        body: &str,
        guard: DispatchGuard,
    ) -> Result<DispatchResult, NotificationError> {
        let (app_id, id, title, body) = (
            self.app_id.clone(),
            id.to_owned(),
            title.to_owned(),
            body.to_owned(),
        );
        let callback = self.callback.clone();
        let pending = self.pending.clone();
        tokio::task::spawn_blocking(move || {
            let _apartment = Apartment::new()?;
            let notifier = notifier(&app_id)?;
            if permission(&notifier)? != NotificationPermission::Granted { return Err(NotificationError::PermissionDenied); }
            let xml = XmlDocument::new().map_err(|_| NotificationError::DispatchFailed)?;
            xml.LoadXml(&HSTRING::from(format!("<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>", escape_xml(&title), escape_xml(&body)))).map_err(|_| NotificationError::DispatchFailed)?;
            let toast = ToastNotification::CreateToastNotification(&xml).map_err(|_| NotificationError::DispatchFailed)?;
            // Bind only the exact event ID in the host closure; do not parse untrusted activation arguments.
            let click_id = id.clone();
            let click_pending = Arc::downgrade(&pending);
            let activated = toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
                remove_pending(&click_pending, &click_id);
                deliver_click(&callback, click_id.clone());
                Ok(())
            })).map_err(|_| NotificationError::DispatchFailed)?;
            let dismiss_id = id.clone();
            let dismiss_pending = Arc::downgrade(&pending);
            let dismissed = toast.Dismissed(&TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(move |_, args| {
                // Notifications remain in the notification center after the banner expires, so retain the click handler.
                if args.as_ref().and_then(|args| args.Reason().ok()).is_some_and(|reason| reason != ToastDismissalReason::TimedOut) {
                    remove_pending(&dismiss_pending, &dismiss_id);
                }
                Ok(())
            })).map_err(|_| NotificationError::DispatchFailed)?;
            let fail_id = id.clone();
            let fail_pending = Arc::downgrade(&pending);
            let failed = toast.Failed(&TypedEventHandler::new(move |_, _| { remove_pending(&fail_pending, &fail_id); Ok(()) })).map_err(|_| NotificationError::DispatchFailed)?;
            let retired = {
                let mut active = pending.lock().map_err(|_| NotificationError::Unavailable)?;
                if active.iter().any(|entry| entry.id == id) { return Err(NotificationError::DispatchFailed); }
                // As in the service, retain only the latest 128 click handlers; old events never target a different terminal.
                let retired = if active.len() >= 128 { active.pop_front() } else { None };
                active.push_back(PendingToast { id: id.clone(), toast: toast.clone(), activated, dismissed, failed });
                retired
            };
            drop(retired);
            if !guard() {
                remove_pending(&Arc::downgrade(&pending), &id);
                return Ok(DispatchResult::Suppressed);
            }
            if notifier.Show(&toast).is_err() {
                remove_pending(&Arc::downgrade(&pending), &id);
                return Err(NotificationError::DispatchFailed);
            }
            // Successful Show means dispatch was accepted; later Failed events or Focus modes do not prove display success.
            Ok(DispatchResult::Accepted)
        }).await.map_err(|_| NotificationError::Unavailable)?
    }
}

fn remove_pending(pending: &std::sync::Weak<Mutex<VecDeque<PendingToast>>>, id: &str) {
    let removed = pending.upgrade().and_then(|pending| {
        let mut pending = pending.lock().ok()?;
        let index = pending.iter().position(|entry| entry.id == id)?;
        pending.remove(index)
    });
    // Do not hold the queue lock while unregistering system handlers.
    drop(removed);
}

fn notifier(app_id: &str) -> Result<ToastNotifier, NotificationError> {
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))
        .map_err(|_| NotificationError::Unavailable)
}

fn permission(notifier: &ToastNotifier) -> Result<NotificationPermission, NotificationError> {
    let setting = notifier
        .Setting()
        .map_err(|_| NotificationError::Unavailable)?;
    Ok(if setting == NotificationSetting::Enabled {
        NotificationPermission::Granted
    } else if setting == NotificationSetting::DisabledForApplication
        || setting == NotificationSetting::DisabledForUser
        || setting == NotificationSetting::DisabledByGroupPolicy
    {
        NotificationPermission::Denied
    } else {
        NotificationPermission::Unavailable
    })
}

struct Apartment;
impl Apartment {
    fn new() -> Result<Self, NotificationError> {
        // SAFETY: Initialize and release the WinRT apartment as a pair on the dedicated blocking worker only.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|_| NotificationError::Unavailable)?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn notification_text_cannot_inject_xml_actions() {
        assert_eq!(
            super::escape_xml("<actions>&\"'"),
            "&lt;actions&gt;&amp;&quot;&apos;"
        );
    }
}
