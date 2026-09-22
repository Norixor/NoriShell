#[cfg(target_os = "macos")]
#[path = "window_frame_macos.rs"]
mod macos;

use serde::Deserialize;
use tauri::WebviewWindow;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub struct WindowsCaptionHitRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalCaptionHitRegion {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(any(windows, test))]
fn validate_caption_hit_region(
    region: WindowsCaptionHitRegion,
    client_width: u32,
    client_height: u32,
) -> Option<PhysicalCaptionHitRegion> {
    if region.width == 0 || region.height == 0 {
        return None;
    }

    let right = region.x.checked_add(region.width)?;
    let bottom = region.y.checked_add(region.height)?;
    if right > client_width || bottom > client_height {
        return None;
    }

    Some(PhysicalCaptionHitRegion {
        left: i32::try_from(region.x).ok()?,
        top: i32::try_from(region.y).ok()?,
        right: i32::try_from(right).ok()?,
        bottom: i32::try_from(bottom).ok()?,
    })
}

#[cfg(any(windows, test))]
impl PhysicalCaptionHitRegion {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug)]
struct NativeControlFrame {
    left: f64,
    width: f64,
}

#[cfg(any(target_os = "macos", test))]
fn native_controls_leading_inset(
    close_button: NativeControlFrame,
    zoom_button: NativeControlFrame,
) -> Option<f64> {
    let left_gap = close_button.left;
    let right_edge = zoom_button.left + zoom_button.width;
    if !left_gap.is_finite()
        || left_gap < 0.0
        || !zoom_button.left.is_finite()
        || zoom_button.left < 0.0
        || !zoom_button.width.is_finite()
        || zoom_button.width <= 0.0
        || !right_edge.is_finite()
    {
        return None;
    }

    // Reuse the measured distance from the window's leading content edge to
    // the first native button after the last button. This keeps both sides of
    // the AppKit-owned traffic-light group optically balanced without a
    // guessed CSS or Rust clearance constant.
    Some(right_edge + left_gap)
}

/// Returns the leading edge after the native macOS traffic-light group in
/// logical points. Other platforms and unavailable native handles return
/// `None`, allowing the UI token's conservative fallback to remain in force.
#[tauri::command]
pub fn window_native_controls_inset(window: WebviewWindow) -> Option<f64> {
    native_controls_inset(&window)
}

/// Updates the physical Windows maximize-button overlay used by the native
/// `HTMAXBUTTON` bridge. The Vue component owns visual layout; Win32 owns the
/// non-client hit-test semantics required by Windows 11 Snap Layout.
#[tauri::command]
pub fn window_set_windows_maximize_hit_region(
    window: WebviewWindow,
    region: Option<WindowsCaptionHitRegion>,
) -> Result<(), String> {
    set_windows_maximize_hit_region(&window, region)
}

#[cfg(target_os = "macos")]
fn native_controls_inset(window: &WebviewWindow) -> Option<f64> {
    use objc2_app_kit::{NSView, NSWindow, NSWindowButton};

    let native_window = window.ns_window().ok()?;
    // SAFETY: Tauri owns this NSWindow for at least the duration of the command,
    // and AppKit window commands are dispatched on the main thread by Tauri.
    let native_window = unsafe { &*native_window.cast::<NSWindow>() };

    let frame_for = |kind| {
        native_window.standardWindowButton(kind).map(|button| {
            let frame = NSView::frame(&button);
            NativeControlFrame {
                left: frame.origin.x,
                width: frame.size.width,
            }
        })
    };
    let close_button = frame_for(NSWindowButton::CloseButton)?;
    let zoom_button = frame_for(NSWindowButton::ZoomButton)?;

    native_controls_leading_inset(close_button, zoom_button)
}

#[cfg(not(target_os = "macos"))]
fn native_controls_inset(_window: &WebviewWindow) -> Option<f64> {
    None
}

#[cfg(windows)]
mod windows_caption_bridge {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, OnceLock},
    };

    use tauri::WebviewWindow;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::ScreenToClient,
        UI::{
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{GetClientRect, HTMAXBUTTON, WM_NCDESTROY, WM_NCHITTEST},
        },
    };

    use super::{PhysicalCaptionHitRegion, WindowsCaptionHitRegion, validate_caption_hit_region};

    type SharedRegion = Arc<Mutex<Option<PhysicalCaptionHitRegion>>>;

    const CAPTION_SUBCLASS_ID: usize = 0x4e_56_58_43;
    static CAPTION_REGIONS: OnceLock<Mutex<HashMap<isize, SharedRegion>>> = OnceLock::new();

    fn registry() -> &'static Mutex<HashMap<isize, SharedRegion>> {
        CAPTION_REGIONS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn native_hwnd(window: &WebviewWindow) -> Result<HWND, String> {
        window
            .hwnd()
            .map(|handle| handle.0 as HWND)
            .map_err(|_| "window.caption_native_handle_unavailable".to_owned())
    }

    pub fn install(window: &WebviewWindow) -> Result<(), String> {
        let hwnd = native_hwnd(window)?;
        let key = hwnd as isize;
        let mut registered = registry()
            .lock()
            .map_err(|_| "window.caption_bridge_unavailable".to_owned())?;
        if registered.contains_key(&key) {
            return Ok(());
        }

        let region = Arc::new(Mutex::new(None));
        let callback_region = Arc::into_raw(Arc::clone(&region)) as usize;
        // SAFETY: this runs during Tauri setup on the HWND-owning UI thread.
        // The raw Arc is reclaimed on WM_NCDESTROY or immediately on failure.
        let installed = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(caption_subclass_proc),
                CAPTION_SUBCLASS_ID,
                callback_region,
            )
        };
        if installed == 0 {
            // SAFETY: SetWindowSubclass did not retain the callback data.
            unsafe {
                drop(Arc::from_raw(
                    callback_region as *const Mutex<Option<PhysicalCaptionHitRegion>>,
                ))
            };
            return Err("window.caption_bridge_install_failed".to_owned());
        }

        registered.insert(key, region);
        Ok(())
    }

    pub fn update(
        window: &WebviewWindow,
        region: Option<WindowsCaptionHitRegion>,
    ) -> Result<(), String> {
        let hwnd = native_hwnd(window)?;
        let key = hwnd as isize;
        let shared = registry()
            .lock()
            .map_err(|_| "window.caption_bridge_unavailable".to_owned())?
            .get(&key)
            .cloned()
            .ok_or_else(|| "window.caption_bridge_unavailable".to_owned())?;

        let validated = match region {
            Some(region) => {
                let mut client_rect = RECT::default();
                // SAFETY: hwnd belongs to the live Tauri window and the output
                // pointer is valid for this call.
                if unsafe { GetClientRect(hwnd, &mut client_rect) } == 0 {
                    return Err("window.caption_client_bounds_unavailable".to_owned());
                }
                let width = u32::try_from(client_rect.right - client_rect.left)
                    .map_err(|_| "window.caption_hit_region_invalid".to_owned())?;
                let height = u32::try_from(client_rect.bottom - client_rect.top)
                    .map_err(|_| "window.caption_hit_region_invalid".to_owned())?;
                Some(
                    validate_caption_hit_region(region, width, height)
                        .ok_or_else(|| "window.caption_hit_region_invalid".to_owned())?,
                )
            }
            None => None,
        };

        *shared
            .lock()
            .map_err(|_| "window.caption_bridge_unavailable".to_owned())? = validated;
        Ok(())
    }

    unsafe extern "system" fn caption_subclass_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        callback_region: usize,
    ) -> LRESULT {
        let region_pointer = callback_region as *const Mutex<Option<PhysicalCaptionHitRegion>>;

        if message == WM_NCHITTEST {
            let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
            let mut point = POINT { x, y };
            // SAFETY: the subclass is attached only to a live HWND and point is
            // valid writable storage.
            if unsafe { ScreenToClient(hwnd, &mut point) } != 0 {
                // SAFETY: callback_region is an Arc allocation retained until
                // WM_NCDESTROY below.
                let hit = unsafe { &*region_pointer }
                    .lock()
                    .ok()
                    .and_then(|region| *region)
                    .is_some_and(|region| region.contains(point.x, point.y));
                if hit {
                    return HTMAXBUTTON as LRESULT;
                }
            }
        }

        // SAFETY: forwarding unhandled messages is required by the common
        // controls subclass contract.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        if message == WM_NCDESTROY {
            if let Ok(mut registered) = registry().lock() {
                registered.remove(&(hwnd as isize));
            }
            // SAFETY: this is the final message for the subclass and balances
            // the Arc::into_raw performed by install.
            unsafe { drop(Arc::from_raw(region_pointer)) };
        }
        result
    }
}

#[cfg(windows)]
pub fn install_windows_caption_bridge(window: &WebviewWindow) -> Result<(), String> {
    windows_caption_bridge::install(window)
}

#[cfg(windows)]
fn set_windows_maximize_hit_region(
    window: &WebviewWindow,
    region: Option<WindowsCaptionHitRegion>,
) -> Result<(), String> {
    windows_caption_bridge::update(window, region)
}

#[cfg(not(windows))]
fn set_windows_maximize_hit_region(
    _window: &WebviewWindow,
    region: Option<WindowsCaptionHitRegion>,
) -> Result<(), String> {
    if region.is_none() {
        Ok(())
    } else {
        Err("window.caption_hit_test_unsupported".to_owned())
    }
}

/// Updates the native main-window header using logical AppKit points.
#[tauri::command]
pub async fn window_set_native_header_height(
    window: WebviewWindow,
    height: f64,
) -> Result<(), String> {
    if window.label() != "main" || !height.is_finite() || !(32.0..=160.0).contains(&height) {
        return Err("window.native_header_invalid".to_owned());
    }
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let _ = sender.send(macos::set_height(height));
            })
            .map_err(|_| "window.native_header_unavailable".to_owned())?;
        receiver
            .await
            .map_err(|_| "window.native_header_unavailable".to_owned())?
    }
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn install_macos_header_bridge(window: &WebviewWindow) -> Result<(), String> {
    macos::install(window)
}

#[cfg(target_os = "macos")]
pub fn cleanup_macos_header_bridge() {
    macos::cleanup();
}

#[cfg(test)]
mod tests {
    use super::{
        NativeControlFrame, WindowsCaptionHitRegion, native_controls_leading_inset,
        validate_caption_hit_region,
    };

    #[test]
    fn mirrors_the_measured_left_gap_after_the_native_button_group() {
        let inset = native_controls_leading_inset(
            NativeControlFrame {
                left: 14.0,
                width: 14.0,
            },
            NativeControlFrame {
                left: 54.0,
                width: 14.0,
            },
        );

        assert_eq!(inset, Some(82.0));
    }

    #[test]
    fn uses_the_actual_frame_values_instead_of_a_clearance_constant() {
        let inset = native_controls_leading_inset(
            NativeControlFrame {
                left: 15.0,
                width: 13.0,
            },
            NativeControlFrame {
                left: 57.0,
                width: 13.0,
            },
        );

        assert_eq!(inset, Some(85.0));
    }

    #[test]
    fn ignores_invalid_frames_and_preserves_the_frontend_fallback_without_measurements() {
        let inset = native_controls_leading_inset(
            NativeControlFrame {
                left: f64::NAN,
                width: 14.0,
            },
            NativeControlFrame {
                left: 14.0,
                width: 14.0,
            },
        );

        assert_eq!(inset, None);

        let inset = native_controls_leading_inset(
            NativeControlFrame {
                left: 14.0,
                width: 14.0,
            },
            NativeControlFrame {
                left: 54.0,
                width: 0.0,
            },
        );

        assert_eq!(inset, None);
    }

    #[test]
    fn validates_a_bounded_physical_maximize_hit_region() {
        let region = validate_caption_hit_region(
            WindowsCaptionHitRegion {
                x: 1100,
                y: 0,
                width: 46,
                height: 56,
            },
            1280,
            800,
        )
        .expect("bounded caption region");

        assert!(region.contains(1100, 0));
        assert!(region.contains(1145, 55));
        assert!(!region.contains(1146, 55));
        assert!(!region.contains(1145, 56));
    }

    #[test]
    fn rejects_empty_overflowing_or_out_of_client_caption_regions() {
        assert!(
            validate_caption_hit_region(
                WindowsCaptionHitRegion {
                    x: 1100,
                    y: 0,
                    width: 0,
                    height: 56,
                },
                1280,
                800,
            )
            .is_none()
        );
        assert!(
            validate_caption_hit_region(
                WindowsCaptionHitRegion {
                    x: u32::MAX,
                    y: 0,
                    width: 46,
                    height: 56,
                },
                1280,
                800,
            )
            .is_none()
        );
        assert!(
            validate_caption_hit_region(
                WindowsCaptionHitRegion {
                    x: 1240,
                    y: 0,
                    width: 46,
                    height: 56,
                },
                1280,
                800,
            )
            .is_none()
        );
    }
}
