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

pub fn rename_editor_window_attributes(
    chrome: &Window,
    attributes: WindowAttributes,
) -> Result<Option<WindowAttributes>, Box<dyn Error>> {
    // SAFETY: the shell retains the chrome until after its editor child is destroyed.
    Ok(Some(unsafe {
        attributes
            .with_parent_window(Some(chrome.window_handle()?.as_raw()))
            .with_decorations(false)
            .with_active(true)
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

pub fn position_rename_editor(
    _chrome: &Window,
    editor: &Window,
    position: winit::dpi::PhysicalPosition<i32>,
) {
    let Ok(editor_handle) = editor.window_handle() else {
        return;
    };
    let Some(editor) = hwnd(editor_handle.as_raw()) else {
        return;
    };
    // SAFETY: the editor HWND is a live child of the chrome HWND. A null insertion handle is
    // HWND_TOP, keeping the editor above sibling terminal panes while it owns keyboard focus.
    unsafe {
        SetWindowPos(
            editor,
            std::ptr::null_mut(),
            position.x,
            position.y,
            0,
            0,
            SWP_NOSIZE,
        );
        SetActiveWindow(editor);
        SetFocus(editor);
    }
}

fn hwnd(raw: RawWindowHandle) -> Option<HWND> {
    let RawWindowHandle::Win32(handle) = raw else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

/// Show a native popup above terminal child windows without changing pane visibility.
pub fn show_launch_menu(
    chrome: &Window,
    entries: &[vivido::shell::LaunchEntry],
    anchor: winit::dpi::PhysicalPosition<i32>,
) -> Result<Option<usize>, Box<dyn Error>> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON,
        TrackPopupMenu,
    };

    let handle = chrome.window_handle()?;
    let owner = hwnd(handle.as_raw()).ok_or("chrome has no Windows handle")?;
    // SAFETY: all handles are used on the owning event-loop thread. Labels are NUL-terminated
    // and copied by AppendMenuW. The menu is destroyed on both success and failure.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let result = (|| {
            for (index, entry) in entries.iter().enumerate() {
                let id = u32::try_from(index)?
                    .checked_add(1)
                    .ok_or("too many launch entries")?;
                let label = entry
                    .label
                    .replace('&', "&&")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect::<Vec<_>>();
                if AppendMenuW(menu, MF_STRING, id as usize, label.as_ptr()) == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            let mut point = POINT {
                x: anchor.x,
                y: anchor.y,
            };
            if ClientToScreen(owner, &mut point) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let selected = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                owner,
                std::ptr::null(),
            );
            Ok(usize::try_from(selected)
                .ok()
                .and_then(|id| id.checked_sub(1)))
        })();
        DestroyMenu(menu);
        result
    }
}
