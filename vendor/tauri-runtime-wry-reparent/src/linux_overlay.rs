// NoriShell: GTK layout for multiwebview windows on Linux.
//
// wry honours child bounds only inside a `gtk::Fixed`; a `gtk::Box` merely packs
// webviews so every child would fill the window and cover the shell webview.
// The window content webview therefore lives in a `gtk::Overlay`, and child
// webviews live in a pass-through `gtk::Fixed` layered above it. Empty areas of
// the Fixed forward pointer events to the content webview underneath.

use gtk::prelude::*;
use wry::WebViewExtUnix;

/// Returns the overlay that hosts the window content, creating it on first use.
pub(crate) fn content_overlay(vbox: &gtk::Box) -> gtk::Overlay {
  if let Some(overlay) = vbox
    .children()
    .into_iter()
    .find_map(|child| child.downcast::<gtk::Overlay>().ok())
  {
    return overlay;
  }
  let overlay = gtk::Overlay::new();
  vbox.pack_start(&overlay, true, true, 0);
  overlay.show();
  overlay
}

/// Returns the pass-through container that positions child webviews by bounds.
pub(crate) fn child_fixed(vbox: &gtk::Box) -> gtk::Fixed {
  let overlay = content_overlay(vbox);
  if let Some(fixed) = overlay
    .children()
    .into_iter()
    .find_map(|child| child.downcast::<gtk::Fixed>().ok())
  {
    return fixed;
  }
  let fixed = gtk::Fixed::new();
  overlay.add_overlay(&fixed);
  overlay.set_overlay_pass_through(&fixed, true);
  fixed.show();
  fixed
}

/// Applies bounds to a webview hosted in a `gtk::Fixed`.
///
/// wry only calls `size_allocate` for Fixed parents, which GTK discards on the
/// next layout pass, so the position and size request are set explicitly.
/// Returns `false` when the webview is not inside a Fixed (window content).
pub(crate) fn place(webview: &wry::WebView, bounds: wry::Rect) -> bool {
  let widget = webview.webview();
  let Some(fixed) = widget
    .parent()
    .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
  else {
    return false;
  };
  let scale_factor = widget.scale_factor() as f64;
  let (width, height): (i32, i32) = bounds.size.to_logical::<i32>(scale_factor).into();
  let (x, y): (i32, i32) = bounds.position.to_logical::<i32>(scale_factor).into();
  widget.set_size_request(width, height);
  fixed.move_(&widget, x, y);
  // SAFETY: the key is private to this module and only ever holds this tuple type.
  unsafe { widget.set_data::<(i32, i32, i32, i32)>(BOUNDS_KEY, (x, y, width, height)) };
  true
}

const BOUNDS_KEY: &str = "norishell-bounds";

/// Re-applies the last known bounds and forces a fresh layout pass.
///
/// A webview hidden while the window was resized shows its old size when it becomes visible again, so
/// showing it must not rely on the geometry GTK computed while it was hidden.
pub(crate) fn refresh(webview: &wry::WebView) {
  let widget = webview.webview();
  // SAFETY: only `place` writes this key, always with the same tuple type.
  let Some((x, y, width, height)) = (unsafe {
    widget
      .data::<(i32, i32, i32, i32)>(BOUNDS_KEY)
      .map(|bounds| *bounds.as_ref())
  }) else {
    return;
  };
  let Some(fixed) = widget
    .parent()
    .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
  else {
    return;
  };
  widget.set_size_request(width, height);
  fixed.move_(&widget, x, y);
  widget.queue_resize();
  fixed.queue_resize();
}

/// Whether the primary pointer button is currently held over the window.
/// Unknown state counts as pressed so a drag is never lost to a missing device.
pub(crate) fn primary_button_down(window: &gtk::ApplicationWindow) -> bool {
  let pointer = gtk::gdk::Display::default()
    .and_then(|display| display.default_seat())
    .and_then(|seat| seat.pointer());
  match (window.window(), pointer) {
    (Some(surface), Some(pointer)) => surface
      .device_position(&pointer)
      .3
      .contains(gtk::gdk::ModifierType::BUTTON1_MASK),
    _ => true,
  }
}
