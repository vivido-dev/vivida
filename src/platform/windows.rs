//! Win32 chrome-window integration; terminal pane hosting lives in `vivido::shell`.

use std::error::Error;

use vivido::Event;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOSIZE, SetWindowPos};
use winit::event_loop::EventLoopBuilder;
use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Icon, Window, WindowAttributes};

use super::PopupFocus;

pub fn configure_event_loop(_builder: &mut EventLoopBuilder<Event>) {}

pub fn configure_chrome_window(attributes: WindowAttributes) -> WindowAttributes {
    // The same embedded icon brands the executable and the native window/taskbar.
    let icon = Icon::from_resource(101, None).expect("Vivida Windows icon resource is missing");
    // Chrome is presented through DirectComposition. An HWND redirection bitmap would retain an
    // opaque copy of the initial client area underneath that visual, making transparency appear
    // only in regions exposed by a later resize.
    attributes
        .with_window_icon(Some(icon.clone()))
        .with_taskbar_icon(Some(icon))
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
    // A null insertion handle is HWND_TOP. Every popup must rise above terminal siblings,
    // including the initial pane created before the settings menu. SWP_NOACTIVATE keeps a
    // focus-free menu from taking the keyboard while still allowing its z-order to change.
    let flags = match focus {
        PopupFocus::Keyboard => SWP_NOSIZE,
        PopupFocus::None => SWP_NOACTIVATE | SWP_NOSIZE,
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetTopWindow, SWP_NOMOVE};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::window::WindowId;

    #[test]
    fn settings_popup_rises_above_initial_pane_without_taking_focus() {
        struct PopupTest;

        impl ApplicationHandler for PopupTest {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let chrome = event_loop
                    .create_window(Window::default_attributes().with_visible(false))
                    .unwrap();
                let child_attributes = || {
                    popup_window_attributes(
                        &chrome,
                        Window::default_attributes().with_visible(false),
                        PopupFocus::None,
                    )
                    .unwrap()
                    .unwrap()
                };
                let pane = event_loop.create_window(child_attributes()).unwrap();
                let menu = event_loop.create_window(child_attributes()).unwrap();
                let parent = hwnd(chrome.window_handle().unwrap().as_raw()).unwrap();
                let pane_handle = hwnd(pane.window_handle().unwrap().as_raw()).unwrap();
                let menu_handle = hwnd(menu.window_handle().unwrap().as_raw()).unwrap();
                // SAFETY: all three HWNDs are retained on their owning event-loop thread.
                let previous_focus = unsafe {
                    SetWindowPos(
                        pane_handle,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    );
                    assert_eq!(GetTopWindow(parent), pane_handle);
                    GetFocus()
                };
                position_popup(
                    &chrome,
                    &menu,
                    winit::dpi::PhysicalPosition::new(20, 40),
                    PopupFocus::None,
                );
                // SAFETY: the windows remain live on this thread.
                unsafe {
                    assert_eq!(GetTopWindow(parent), menu_handle);
                    assert_eq!(GetFocus(), previous_focus);
                }
                event_loop.exit();
            }

            fn window_event(
                &mut self,
                _event_loop: &ActiveEventLoop,
                _window_id: WindowId,
                _event: WindowEvent,
            ) {
            }
        }

        EventLoop::builder()
            .with_any_thread(true)
            .build()
            .unwrap()
            .run_app(&mut PopupTest)
            .unwrap();
    }
}
