//! Win32 chrome-window integration; terminal pane hosting lives in `vivido::shell`.

use std::error::Error;

use vivido::Event;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};
use winit::event_loop::EventLoopBuilder;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

use super::PopupFocus;

pub fn configure_event_loop(_builder: &mut EventLoopBuilder<Event>) {}

pub fn configure_chrome_window(attributes: WindowAttributes) -> WindowAttributes {
    // Chrome is presented through DirectComposition. An HWND redirection bitmap would retain an
    // opaque copy of the initial client area underneath that visual, making transparency appear
    // only in regions exposed by a later resize.
    attributes
        .with_decorations(false)
        .with_no_redirection_bitmap(true)
        .with_clip_children(true)
        .with_undecorated_shadow(true)
}

pub fn finalize_chrome_window(_window: &Window) {}

pub fn focus_chrome_input(window: &Window) {
    window.focus_window();
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let Some(window) = hwnd(handle.as_raw()) else {
        return;
    };
    // SAFETY: the chrome HWND is live and belongs to the event-loop thread. Native terminal panes
    // are child HWNDs on that same thread, so explicitly assigning focus to their parent is the
    // inverse of PaneHost::focus and ensures keyboard messages reach the rename editor.
    unsafe {
        SetActiveWindow(window);
        SetFocus(window);
    }
}

pub fn popup_window_attributes(
    chrome: &Window,
    attributes: WindowAttributes,
    focus: PopupFocus,
) -> Result<Option<WindowAttributes>, Box<dyn Error>> {
    // SAFETY: the shell retains the chrome until after its popup children are destroyed.
    Ok(Some(unsafe {
        attributes
            .with_parent_window(Some(chrome.window_handle()?.as_raw()))
            .with_decorations(false)
            .with_active(focus == PopupFocus::Keyboard)
    }))
}

pub fn position_popup(
    _chrome: &Window,
    popup: &Window,
    position: winit::dpi::PhysicalPosition<i32>,
    focus: PopupFocus,
) {
    let Ok(popup_handle) = popup.window_handle() else {
        return;
    };
    let Some(popup) = hwnd(popup_handle.as_raw()) else {
        return;
    };
    // A null insertion handle is HWND_TOP, keeping a focusable popup above sibling terminal panes
    // while it owns the keyboard. A focus-free popup keeps its z-order and never activates, so
    // showing it cannot make the chrome resign focus and dismiss it again.
    let flags = match focus {
        PopupFocus::Keyboard => SWP_NOSIZE,
        PopupFocus::None => SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
    };
    // SAFETY: the popup HWND is a live child of the chrome HWND.
    unsafe {
        SetWindowPos(
            popup,
            std::ptr::null_mut(),
            position.x,
            position.y,
            0,
            0,
            flags,
        );
        if focus == PopupFocus::Keyboard {
            SetActiveWindow(popup);
            SetFocus(popup);
        }
    }
}

pub fn set_popup_visible(window: &Window, visible: bool) {
    window.set_visible(visible);
}

fn hwnd(raw: RawWindowHandle) -> Option<HWND> {
    let RawWindowHandle::Win32(handle) = raw else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}
