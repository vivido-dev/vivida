//! Chrome hover cursors that stay visible while a terminal pane holds the keyboard.
//!
//! Winit realizes `set_cursor` through cursor rects, and AppKit honors a window's cursor rects
//! only while that window is key. The chrome hands the keyboard to the focused pane's child
//! window, so its rects never activate, and neither its resize band nor its dividers would ever
//! show their arrows. The shell therefore also sets the system cursor while the pointer moves
//! over the chrome, and puts the arrow back once the pointer leaves for a window whose own rects
//! will not replace that shape.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{ClassType, MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSApplication, NSCursor, NSEvent};
use winit::window::{CursorIcon, Window};

thread_local! {
    /// Whether the system cursor shows a chrome shape other than the arrow, which nothing but
    /// [`release_chrome_cursor`] will undo.
    static HOLDS_SHAPE: Cell<bool> = const { Cell::new(false) };
}

/// Show `icon` for the pointer over the chrome.
pub fn set_chrome_cursor(window: &Window, icon: CursorIcon) {
    window.set_cursor(icon);
    hover_cursor(icon).set();
    HOLDS_SHAPE.set(icon != CursorIcon::Default);
}

/// Put the arrow back once the pointer has left a chrome resize shape.
///
/// Called when the pointer leaves the chrome or enters a pane. Wherever the key window is under
/// the pointer, its own cursor rects already apply, so the cursor is left to them there.
pub fn release_chrome_cursor() {
    if !HOLDS_SHAPE.replace(false) {
        return;
    }
    let mtm = MainThreadMarker::new().expect("cursor updates run on the main thread");
    let pointer = NSEvent::mouseLocation();
    let over_key_window = NSApplication::sharedApplication(mtm)
        .keyWindow()
        .is_some_and(|window| {
            let frame = window.frame();
            pointer.x >= frame.origin.x
                && pointer.x < frame.origin.x + frame.size.width
                && pointer.y >= frame.origin.y
                && pointer.y < frame.origin.y + frame.size.height
        });
    if !over_key_window {
        NSCursor::arrowCursor().set();
    }
}

/// The system cursor for one of the chrome's hover icons.
///
/// The chrome's hover set is the arrow plus the resize arrows of its dividers and its own resize
/// band; any other icon falls back to the arrow so it cannot leave a stale shape behind.
// The public directional replacements need macOS 15, above the supported range, and these are
// the cursors winit itself maps the same icons to.
#[expect(deprecated, reason = "replacement resize cursors require macOS 15")]
pub fn hover_cursor(icon: CursorIcon) -> Retained<NSCursor> {
    match icon {
        CursorIcon::EwResize => NSCursor::resizeLeftRightCursor(),
        CursorIcon::NsResize => NSCursor::resizeUpDownCursor(),
        CursorIcon::NeswResize => private_cursor(sel!(_windowResizeNorthEastSouthWestCursor)),
        CursorIcon::NwseResize => private_cursor(sel!(_windowResizeNorthWestSouthEastCursor)),
        _ => NSCursor::arrowCursor(),
    }
}

/// Load one of AppKit's undocumented diagonal window-resize cursors, or the arrow without it.
fn private_cursor(selector: Sel) -> Retained<NSCursor> {
    let class = NSCursor::class();
    if class.class_method(selector).is_none() {
        return NSCursor::arrowCursor();
    }
    // SAFETY: `NSCursor` answers `selector`, a no-argument class method returning an
    // autoreleased cursor that the caller does not own; returning `Retained` makes `msg_send!`
    // retain it rather than take over a reference AppKit still counts as its own.
    let cursor: Option<Retained<NSCursor>> = unsafe { msg_send![class, performSelector: selector] };
    cursor.unwrap_or_else(NSCursor::arrowCursor)
}
