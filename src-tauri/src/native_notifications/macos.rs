use std::{cell::RefCell, ptr::NonNull, sync::Mutex, time::Duration};

use block2::{DynBlock, RcBlock};
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
};
use objc2_foundation::{MainThreadMarker, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::*;
use tokio::sync::oneshot;

use super::*;

// UNUserNotificationCenter.delegate is weak; retain it on the main thread until process exit.
thread_local! { static DELEGATE: RefCell<Option<Retained<NotificationDelegate>>> = const { RefCell::new(None) }; }

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = AnyThread]
    #[ivars = ClickHandler]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}
    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &DynBlock<dyn Fn()>,
        ) {
            // Handle clicks only; dismissing a notification is not activation.
            if response
                .actionIdentifier()
                .isEqualToString(unsafe { UNNotificationDefaultActionIdentifier })
            {
                deliver_click(
                    self.ivars(),
                    response.notification().request().identifier().to_string(),
                );
            }
            completion.call(());
        }
    }
);

pub struct Adapter {
    app: AppHandle,
    available: bool,
}

impl Adapter {
    pub fn new(app: AppHandle, callback: ClickHandler) -> Self {
        let available = MainThreadMarker::new().is_some_and(|_mtm| {
            // currentNotificationCenter can throw an Objective-C exception without bundle identity; fail closed first.
            let bundled = std::env::current_exe()
                .ok()
                .is_some_and(|path| path.parent().is_some_and(|p| p.ends_with("Contents/MacOS")));
            if !bundled
                || NSBundle::mainBundle()
                    .bundleIdentifier()
                    .is_none_or(|id| id.to_string() != app.config().identifier)
            {
                return false;
            }
            DELEGATE.with(|slot| {
                if slot.borrow().is_some() {
                    return false;
                }
                let allocated = NotificationDelegate::alloc().set_ivars(callback);
                // SAFETY: NSObject init returns this initialized subclass, whose ivars have already been set.
                let delegate: Retained<NotificationDelegate> =
                    unsafe { msg_send![super(allocated), init] };
                UNUserNotificationCenter::currentNotificationCenter()
                    .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                *slot.borrow_mut() = Some(delegate);
                true
            })
        });
        Self { app, available }
    }

    pub async fn permission(&self) -> Result<NotificationPermission, NotificationError> {
        if !self.available {
            return Ok(NotificationPermission::Unavailable);
        }
        let (tx, rx) = oneshot::channel();
        self.app
            .run_on_main_thread(move || {
                let tx = Mutex::new(Some(tx));
                let callback = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
                    // SAFETY: Apple guarantees settings is non-null and valid during the completion callback.
                    let settings = unsafe { settings.as_ref() };
                    let status = settings.authorizationStatus();
                    let permission = if status == UNAuthorizationStatus::NotDetermined {
                        NotificationPermission::NotDetermined
                    } else if status == UNAuthorizationStatus::Denied {
                        NotificationPermission::Denied
                    } else if status == UNAuthorizationStatus::Authorized
                        || status == UNAuthorizationStatus::Provisional
                        || status == UNAuthorizationStatus::Ephemeral
                    {
                        NotificationPermission::Granted
                    } else {
                        NotificationPermission::Unavailable
                    };
                    if let Ok(mut sender) = tx.lock()
                        && let Some(sender) = sender.take()
                    {
                        let _ = sender.send(Ok(permission));
                    }
                });
                UNUserNotificationCenter::currentNotificationCenter()
                    .getNotificationSettingsWithCompletionHandler(&callback);
            })
            .map_err(|_| NotificationError::Unavailable)?;
        receive(rx).await
    }

    pub async fn request_permission(&self) -> Result<NotificationPermission, NotificationError> {
        if !self.available {
            return Ok(NotificationPermission::Unavailable);
        }
        let (tx, rx) = oneshot::channel();
        self.app
            .run_on_main_thread(move || {
                let tx = Mutex::new(Some(tx));
                let callback = RcBlock::new(move |granted: Bool, error: *mut NSError| {
                    let result = if !error.is_null() {
                        Err(NotificationError::DispatchFailed)
                    } else {
                        Ok(if granted.as_bool() {
                            NotificationPermission::Granted
                        } else {
                            NotificationPermission::Denied
                        })
                    };
                    if let Ok(mut sender) = tx.lock()
                        && let Some(sender) = sender.take()
                    {
                        let _ = sender.send(result);
                    }
                });
                UNUserNotificationCenter::currentNotificationCenter()
                    .requestAuthorizationWithOptions_completionHandler(
                        UNAuthorizationOptions::Alert,
                        &callback,
                    );
            })
            .map_err(|_| NotificationError::Unavailable)?;
        receive(rx).await
    }

    pub async fn send(
        &self,
        id: &str,
        title: &str,
        body: &str,
        guard: DispatchGuard,
    ) -> Result<DispatchResult, NotificationError> {
        if !self.available {
            return Err(NotificationError::Unavailable);
        }
        let (id, title, body) = (id.to_owned(), title.to_owned(), body.to_owned());
        let (tx, rx) = oneshot::channel();
        self.app
            .run_on_main_thread(move || {
                if !guard() {
                    let _ = tx.send(Ok(DispatchResult::Suppressed));
                    return;
                }
                let content = UNMutableNotificationContent::new();
                content.setTitle(&NSString::from_str(&title));
                content.setBody(&NSString::from_str(&body));
                let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                    &NSString::from_str(&id),
                    &content,
                    None,
                );
                let tx = Mutex::new(Some(tx));
                let callback = RcBlock::new(move |error: *mut NSError| {
                    let result = if error.is_null() {
                        Ok(DispatchResult::Accepted)
                    } else {
                        Err(NotificationError::DispatchFailed)
                    };
                    if let Ok(mut sender) = tx.lock()
                        && let Some(sender) = sender.take()
                    {
                        let _ = sender.send(result);
                    }
                });
                UNUserNotificationCenter::currentNotificationCenter()
                    .addNotificationRequest_withCompletionHandler(&request, Some(&callback));
            })
            .map_err(|_| NotificationError::Unavailable)?;
        receive(rx).await
    }
}

async fn receive<T>(
    rx: oneshot::Receiver<Result<T, NotificationError>>,
) -> Result<T, NotificationError> {
    tokio::time::timeout(Duration::from_secs(120), rx)
        .await
        .map_err(|_| NotificationError::Timeout)?
        .map_err(|_| NotificationError::Unavailable)?
}
