// Native window chrome on macOS (the shipped "rounded frame" approach): the
// shell keeps the OS-drawn window frame — rounded corners, drop shadow, and
// the traffic-light controls — but hides the titlebar so the custom Slint
// title bar can sit in its place. The window stays an ordinary titled
// `NSWindow`; we only flip it to `fullSizeContentView` with a transparent,
// title-less, separator-less titlebar. On every other platform the shell is
// fully frameless (`no-frame` in app.slint) and this is a no-op.

use slint::winit_030::winit;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSTitlebarSeparatorStyle, NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

// Resolve the `NSWindow` hosting the given winit window through its raw
// `NSView` handle. `None` if the handle isn't an AppKit one (e.g. before the
// window is shown).
#[cfg(target_os = "macos")]
fn ns_window_of(window: &winit::window::Window) -> Option<Retained<NSWindow>> {
    let raw = window.window_handle().ok()?.as_raw();
    let RawWindowHandle::AppKit(handle) = raw else {
        return None;
    };
    // SAFETY: `handle.ns_view` is documented by raw-window-handle to be a
    // live, retained `NSView *` for the duration the WindowHandle is borrowed
    // from winit. We retain it before reading `-window` so the returned
    // `NSWindow` is independently owned.
    unsafe {
        let view_ptr: *mut NSView = handle.ns_view.as_ptr().cast();
        let view: Retained<NSView> = Retained::retain(view_ptr)?;
        view.window()
    }
}

/// Hide the titlebar while keeping the native (rounded, shadowed) frame and
/// the traffic-light controls. Idempotent — safe to call more than once.
#[cfg(target_os = "macos")]
pub fn apply_native_chrome(window: &winit::window::Window) {
    let Some(ns_window) = ns_window_of(window) else {
        return;
    };
    // Let content fill the full height (under the now-hidden titlebar) and
    // strip the titlebar's chrome so the custom title bar shows through.
    let mask = ns_window.styleMask() | NSWindowStyleMask::FullSizeContentView;
    ns_window.setStyleMask(mask);
    ns_window.setTitlebarAppearsTransparent(true);
    ns_window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    // Drop the hairline under the (invisible) titlebar so the title bar reads
    // as one continuous surface.
    ns_window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
}

/// No-op off macOS: those platforms keep the fully frameless shell.
#[cfg(not(target_os = "macos"))]
pub fn apply_native_chrome(_window: &winit::window::Window) {}
