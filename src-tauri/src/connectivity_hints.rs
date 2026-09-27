//! Platform changes are hints to recheck Metrics transports, not proof that a Host is reachable.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::sync::mpsc;

use crate::metrics_session_service::MetricsSessionService;

const HINT_CAPACITY: usize = 1;
const MIN_HINT_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct ConnectivityHints {
    tx: mpsc::Sender<()>,
    active: Arc<AtomicBool>,
    native: Mutex<platform::NativeWatch>,
}

impl ConnectivityHints {
    pub(crate) fn start(metrics: MetricsSessionService) -> Self {
        let (tx, mut rx) = mpsc::channel(HINT_CAPACITY);
        tauri::async_runtime::spawn(async move {
            let mut last_hint = None;
            while rx.recv().await.is_some() {
                if let Some(last) = last_hint {
                    tokio::time::sleep_until(last + MIN_HINT_INTERVAL).await;
                }
                while rx.try_recv().is_ok() {}
                metrics.connectivity_hint().await;
                last_hint = Some(tokio::time::Instant::now());
            }
        });

        let active = Arc::new(AtomicBool::new(true));
        let context = Arc::new(NativeContext {
            tx: tx.clone(),
            active: active.clone(),
        });
        let native = platform::NativeWatch::start(context);
        Self {
            tx,
            active,
            native: Mutex::new(native),
        }
    }

    pub(crate) fn notify(&self) {
        if self.active.load(Ordering::Acquire) {
            let _ = self.tx.try_send(());
        }
    }

    pub(crate) fn stop(&self) {
        self.active.store(false, Ordering::Release);
        if let Ok(mut native) = self.native.lock() {
            native.stop();
        }
    }
}

struct NativeContext {
    tx: mpsc::Sender<()>,
    active: Arc<AtomicBool>,
}

impl NativeContext {
    fn notify(&self) {
        if self.active.load(Ordering::Acquire) {
            let _ = self.tx.try_send(());
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        cell::RefCell,
        ffi::{c_char, c_void},
        ptr::{self, NonNull},
        sync::Arc,
    };

    use block2::RcBlock;
    use objc2::{rc::Retained, runtime::ProtocolObject};
    use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
    use objc2_foundation::{NSNotification, NSObjectProtocol, NSOperationQueue};

    use super::NativeContext;

    #[repr(C)]
    struct ReachabilityContext {
        version: isize,
        info: *mut c_void,
        retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
        release: Option<unsafe extern "C" fn(*const c_void)>,
        copy_description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    }

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCNetworkReachabilityCreateWithAddress(
            allocator: *const c_void,
            address: *const libc::sockaddr,
        ) -> *mut c_void;
        fn SCNetworkReachabilitySetCallback(
            target: *mut c_void,
            callback: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void)>,
            context: *mut ReachabilityContext,
        ) -> u8;
        fn SCNetworkReachabilitySetDispatchQueue(target: *mut c_void, queue: *mut c_void) -> u8;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    unsafe extern "C" {
        fn dispatch_queue_create(label: *const c_char, attribute: *const c_void) -> *mut c_void;
        fn dispatch_release(queue: *mut c_void);
    }

    thread_local! {
        static WAKE_OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
            const { RefCell::new(None) };
    }

    pub(super) struct NativeWatch {
        reachability: usize,
        queue: usize,
    }

    impl NativeWatch {
        pub(super) fn start(context: Arc<NativeContext>) -> Self {
            install_wake_observer(context.clone());

            // The default IPv4 route is a local path signal. It never verifies a remote Host.
            let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            address.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            address.sin_family = libc::AF_INET as u8;
            let reachability = unsafe {
                SCNetworkReachabilityCreateWithAddress(
                    ptr::null(),
                    (&raw const address).cast::<libc::sockaddr>(),
                )
            };
            if reachability.is_null() {
                eprintln!("metrics network change listener unavailable");
                return Self::empty();
            }
            let queue = unsafe {
                dispatch_queue_create(c"org.norishell.metrics.network".as_ptr(), ptr::null())
            };
            if queue.is_null() {
                unsafe { CFRelease(reachability) };
                eprintln!("metrics network change listener unavailable");
                return Self::empty();
            }

            // SystemConfiguration can deliver a callback after unscheduling. Keep this small
            // context alive for the process lifetime and gate callbacks when stopping.
            let info = Arc::into_raw(context) as *mut c_void;
            let mut callback_context = ReachabilityContext {
                version: 0,
                info,
                retain: None,
                release: None,
                copy_description: None,
            };
            let callback_set = unsafe {
                SCNetworkReachabilitySetCallback(
                    reachability,
                    Some(network_changed),
                    &raw mut callback_context,
                ) != 0
            };
            let scheduled = callback_set
                && unsafe { SCNetworkReachabilitySetDispatchQueue(reachability, queue) != 0 };
            if !scheduled {
                unsafe {
                    SCNetworkReachabilitySetCallback(reachability, None, ptr::null_mut());
                    CFRelease(reachability);
                    dispatch_release(queue);
                    drop(Arc::from_raw(info.cast::<NativeContext>()));
                }
                eprintln!("metrics network change listener unavailable");
                return Self::empty();
            }
            Self {
                reachability: reachability as usize,
                queue: queue as usize,
            }
        }

        fn empty() -> Self {
            Self {
                reachability: 0,
                queue: 0,
            }
        }

        pub(super) fn stop(&mut self) {
            WAKE_OBSERVER.with_borrow_mut(|observer| {
                if let Some(observer) = observer.take() {
                    let center = NSWorkspace::sharedWorkspace().notificationCenter();
                    unsafe { center.removeObserver((*observer).as_ref()) };
                }
            });
            if self.reachability != 0 {
                unsafe {
                    SCNetworkReachabilitySetDispatchQueue(
                        self.reachability as *mut c_void,
                        ptr::null_mut(),
                    );
                    SCNetworkReachabilitySetCallback(
                        self.reachability as *mut c_void,
                        None,
                        ptr::null_mut(),
                    );
                    CFRelease(self.reachability as *const c_void);
                    dispatch_release(self.queue as *mut c_void);
                }
                self.reachability = 0;
                self.queue = 0;
            }
        }
    }

    fn install_wake_observer(context: Arc<NativeContext>) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let callback = RcBlock::new(move |_: NonNull<NSNotification>| context.notify());
        // NSWorkspace notifications are delivered on AppKit's main thread.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &callback,
            )
        };
        WAKE_OBSERVER.with_borrow_mut(|slot| *slot = Some(observer));
    }

    unsafe extern "C" fn network_changed(_target: *mut c_void, _flags: u32, info: *mut c_void) {
        if let Some(context) = unsafe { info.cast::<NativeContext>().as_ref() } {
            context.notify();
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::{ffi::c_void, ptr, sync::Arc};

    use windows_sys::Win32::{
        Foundation::HANDLE,
        NetworkManagement::IpHelper::{CancelMibChangeNotify2, NotifyIpInterfaceChange},
        Networking::WinSock::AF_UNSPEC,
        System::Power::{
            DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
            PowerUnregisterSuspendResumeNotification,
        },
        UI::WindowsAndMessaging::{
            DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
        },
    };

    use super::NativeContext;

    pub(super) struct NativeWatch {
        network: usize,
        power: usize,
    }

    impl NativeWatch {
        pub(super) fn start(context: Arc<NativeContext>) -> Self {
            let mut watch = Self {
                network: 0,
                power: 0,
            };
            let network_info = Arc::into_raw(context.clone());
            let mut network_handle: HANDLE = ptr::null_mut();
            let network_result = unsafe {
                NotifyIpInterfaceChange(
                    AF_UNSPEC,
                    Some(network_changed),
                    network_info.cast(),
                    false,
                    &raw mut network_handle,
                )
            };
            if network_result == 0 {
                watch.network = network_handle as usize;
            } else {
                unsafe { drop(Arc::from_raw(network_info)) };
                eprintln!("metrics network change listener unavailable");
            }

            let power_info = Arc::into_raw(context);
            let mut parameters = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(power_changed),
                Context: power_info.cast_mut().cast(),
            };
            let mut power_handle = ptr::null_mut();
            let power_result = unsafe {
                PowerRegisterSuspendResumeNotification(
                    DEVICE_NOTIFY_CALLBACK,
                    (&raw mut parameters).cast(),
                    &raw mut power_handle,
                )
            };
            if power_result == 0 {
                watch.power = power_handle as usize;
            } else {
                unsafe { drop(Arc::from_raw(power_info)) };
                eprintln!("metrics wake listener unavailable");
            }
            watch
        }

        pub(super) fn stop(&mut self) {
            if self.network != 0 {
                unsafe { CancelMibChangeNotify2(self.network as HANDLE) };
                self.network = 0;
            }
            if self.power != 0 {
                unsafe { PowerUnregisterSuspendResumeNotification(self.power as *mut c_void) };
                self.power = 0;
            }
            // Windows may have queued a callback before unregistering it. The two tiny
            // contexts intentionally remain alive until process exit; the active gate is off.
        }
    }

    unsafe extern "system" fn network_changed(
        info: *const c_void,
        _row: *const windows_sys::Win32::NetworkManagement::IpHelper::MIB_IPINTERFACE_ROW,
        _kind: i32,
    ) {
        if let Some(context) = unsafe { info.cast::<NativeContext>().as_ref() } {
            context.notify();
        }
    }

    unsafe extern "system" fn power_changed(
        info: *const c_void,
        kind: u32,
        _setting: *const c_void,
    ) -> u32 {
        if matches!(kind, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND)
            && let Some(context) = unsafe { info.cast::<NativeContext>().as_ref() }
        {
            context.notify();
        }
        0
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use std::sync::Arc;

    use super::NativeContext;

    pub(super) struct NativeWatch;

    impl NativeWatch {
        pub(super) fn start(_context: Arc<NativeContext>) -> Self {
            Self
        }

        pub(super) fn stop(&mut self) {}
    }
}
