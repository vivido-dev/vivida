//! Win32 chrome-window integration; terminal pane hosting lives in `vivido::shell`.

use std::error::Error;

use vivido::Event;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};
use winit::event_loop::EventLoopBuilder;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

pub fn configure_event_loop(_builder: &mut EventLoopBuilder<Event>) {}

pub fn configure_chrome_window(attributes: WindowAttributes) -> WindowAttributes {
    attributes
        .with_decorations(false)
        .with_clip_children(true)
        .with_undecorated_shadow(true)
}

pub fn finalize_chrome_window(_window: &Window) {}

pub fn settings_menu_window_attributes(
    chrome: &Window,
    attributes: WindowAttributes,
) -> Result<Option<WindowAttributes>, Box<dyn Error>> {
    // SAFETY: the shell retains the chrome until after its menu child is destroyed.
    Ok(Some(unsafe {
        attributes
            .with_parent_window(Some(chrome.window_handle()?.as_raw()))
            .with_decorations(false)
            .with_active(false)
    }))
}

pub fn position_settings_menu(
    _chrome: &Window,
    menu: &Window,
    position: winit::dpi::PhysicalPosition<i32>,
) {
    let Ok(menu_handle) = menu.window_handle() else {
        return;
    };
    let Some(menu) = hwnd(menu_handle.as_raw()) else {
        return;
    };
    // SAFETY: the menu HWND is a live child of the chrome HWND.
    unsafe {
        SetWindowPos(
            menu,
            std::ptr::null_mut(),
            position.x,
            position.y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
        );
    }
}

fn hwnd(raw: RawWindowHandle) -> Option<HWND> {
    let RawWindowHandle::Win32(handle) = raw else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}
