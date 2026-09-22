//! AppKit-owned main-window buttons aligned with the WebView header.
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
};

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSView, NSViewFrameDidChangeNotification, NSWindow, NSWindowButton,
    NSWindowDidEnterFullScreenNotification, NSWindowDidExitFullScreenNotification,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue, NSPoint,
};
use tauri::WebviewWindow;

thread_local! {
    static BRIDGE: RefCell<Option<HeaderBridge>> = const { RefCell::new(None) };
    static QUEUED: Cell<bool> = const { Cell::new(false) };
}

struct HeaderBridge {
    window: Retained<NSWindow>,
    height: f64,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    views: Vec<(Retained<NSView>, bool)>,
    frame_observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

impl Drop for HeaderBridge {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.observers {
            // SAFETY: these are the tokens returned by this notification center.
            unsafe { center.removeObserver((**observer).as_ref()) };
        }
        self.clear_view_observers();
    }
}

fn schedule_alignment() {
    if QUEUED.replace(true) {
        return;
    }
    // Defer until AppKit has finished its current layout. Frame notifications
    // caused by our own adjustment are coalesced while QUEUED remains true.
    let callback = RcBlock::new(|| {
        BRIDGE.with_borrow_mut(|bridge| {
            if let Some(bridge) = bridge {
                bridge.align();
            }
        });
        QUEUED.set(false);
    });
    // SAFETY: this capture-free block executes only on the AppKit main queue.
    unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&callback) };
}

impl HeaderBridge {
    fn clear_view_observers(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in self.frame_observers.drain(..) {
            // SAFETY: the token belongs to this notification center.
            unsafe { center.removeObserver((*observer).as_ref()) };
        }
        for (view, originally_enabled) in self.views.drain(..) {
            view.setPostsFrameChangedNotifications(originally_enabled);
        }
    }

    fn observe_current_views(&mut self, views: Vec<Retained<NSView>>) {
        if self.views.len() == views.len()
            && self
                .views
                .iter()
                .zip(&views)
                .all(|((old, _), new)| std::ptr::eq(&**old, &**new))
        {
            return;
        }
        // Fullscreen transitions may replace AppKit's titlebar hierarchy.
        // Release obsolete view observations instead of retaining every generation.
        self.clear_view_observers();
        for view in views {
            self.observe_view(view);
        }
    }

    fn observe_view(&mut self, view: Retained<NSView>) {
        if self
            .views
            .iter()
            .any(|(existing, _)| std::ptr::eq(&**existing, &*view))
        {
            return;
        }
        let originally_enabled = view.postsFrameChangedNotifications();
        view.setPostsFrameChangedNotifications(true);
        let callback = RcBlock::new(|_: NonNull<NSNotification>| schedule_alignment());
        // SAFETY: the view and observer are retained until bridge teardown;
        // AppKit view frame notifications are delivered on the main thread.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSViewFrameDidChangeNotification),
                Some(&view),
                None,
                &callback,
            )
        };
        self.frame_observers.push(observer);
        self.views.push((view, originally_enabled));
    }

    fn align(&mut self) {
        // AppKit owns fullscreen titlebar animation and auto-hide geometry.
        if self
            .window
            .styleMask()
            .contains(NSWindowStyleMask::FullScreen)
        {
            return;
        }
        let Some(close) = self
            .window
            .standardWindowButton(NSWindowButton::CloseButton)
        else {
            return;
        };
        // SAFETY: the retained button belongs to our live main-thread NSWindow.
        let Some(parent) = (unsafe { close.superview() }) else {
            return;
        };
        // SAFETY: AppKit owns this live titlebar hierarchy on the main thread.
        let Some(container) = (unsafe { parent.superview() }) else {
            return;
        };
        let mut views = vec![parent.clone(), container.clone()];
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = self.window.standardWindowButton(kind) {
                views.push(button.into_super().into_super());
            }
        }
        self.observe_current_views(views);
        // Keep AppKit's existing hierarchy and privacy indicator intact. The
        // container must cover the entire header so native hit testing works.
        let mut container_frame = container.frame();
        container_frame.origin.y += container_frame.size.height - self.height;
        container_frame.size.height = self.height;
        if container.frame() != container_frame {
            container.setFrame(container_frame);
        }
        let window_height = self
            .window
            .contentView()
            .map(|view| {
                let content = view.convertRect_toView(view.bounds(), None);
                content.origin.y + content.size.height
            })
            .unwrap_or_else(|| self.window.frame().size.height);
        let Some(minimize) = self
            .window
            .standardWindowButton(NSWindowButton::MiniaturizeButton)
        else {
            return;
        };
        let spacing = minimize.frame().origin.x - close.frame().origin.x;
        for (index, kind) in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .into_iter()
        .enumerate()
        {
            let Some(button) = self.window.standardWindowButton(kind) else {
                continue;
            };
            // SAFETY: the button is retained and accessed on the main thread.
            let Some(button_parent) = (unsafe { button.superview() }) else {
                continue;
            };
            let frame = button.frame();
            let center = button_parent.convertPoint_fromView(
                NSPoint::new(
                    9.0 + index as f64 * spacing + frame.size.width / 2.0,
                    window_height - self.height / 2.0,
                ),
                None,
            );
            let origin = NSPoint::new(
                center.x - frame.size.width / 2.0,
                center.y - frame.size.height / 2.0,
            );
            if (frame.origin.x - origin.x).abs() > 0.01 || (frame.origin.y - origin.y).abs() > 0.01
            {
                button.setFrameOrigin(origin);
            }
        }
    }
}

pub(super) fn install(window: &WebviewWindow) -> Result<(), String> {
    let _main_thread = MainThreadMarker::new().ok_or("window.native_main_thread_required")?;
    let native = window
        .ns_window()
        .map_err(|_| "window.native_handle_unavailable")?;
    // SAFETY: Tauri's live NSWindow handle is accessed and retained on main.
    let native = unsafe { Retained::retain(native.cast::<NSWindow>()) }
        .ok_or("window.native_handle_unavailable")?;
    let mut bridge = HeaderBridge {
        window: native,
        height: 56.0,
        observers: Vec::new(),
        views: Vec::new(),
        frame_observers: Vec::new(),
    };
    for name in unsafe {
        [
            NSWindowDidEnterFullScreenNotification,
            NSWindowDidExitFullScreenNotification,
        ]
    } {
        let callback = RcBlock::new(move |_: NonNull<NSNotification>| {
            schedule_alignment();
        });
        // SAFETY: AppKit delivers these window notifications on the main thread;
        // the observer and target window are retained by the bridge.
        bridge.observers.push(unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(&bridge.window),
                None,
                &callback,
            )
        });
    }
    BRIDGE.with_borrow_mut(|slot| *slot = Some(bridge));
    schedule_alignment();
    Ok(())
}

pub(super) fn set_height(height: f64) -> Result<(), String> {
    let _main_thread = MainThreadMarker::new().ok_or("window.native_main_thread_required")?;
    BRIDGE.with_borrow_mut(|bridge| {
        let bridge = bridge.as_mut().ok_or("window.native_header_unavailable")?;
        bridge.height = height;
        Ok::<_, String>(())
    })?;
    schedule_alignment();
    Ok(())
}

pub(super) fn cleanup() {
    BRIDGE.with_borrow_mut(|bridge| *bridge = None);
}
