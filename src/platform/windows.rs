//! Native Win32 child-window pane host.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use vivido::cli::TerminalOptions;
use vivido::{Event, LoopHandle, ParentWindowHandle, Processor, WindowOptions};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetParent, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
};
use winit::event_loop::{ActiveEventLoop, EventLoopBuilder};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes, WindowId};

use super::PaneHost;
use crate::layout::PhysicalRect;

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
    // SAFETY: Shell owns the chrome window until after the menu child is destroyed.
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
            SWP_NOACTIVATE | SWP_NOSIZE,
        );
    }
}

#[derive(Clone)]
pub struct NativePaneHost {
    chrome: Arc<Window>,
}

impl NativePaneHost {
    pub fn new(chrome: Arc<Window>) -> Self {
        Self { chrome }
    }

    fn native_windows(&self, processor: &Processor, pane: WindowId) -> Option<(HWND, HWND)> {
        Some((
            hwnd(self.chrome.window_handle().ok()?.as_raw())?,
            hwnd(processor.window(pane)?.display.window.raw_window_handle()?)?,
        ))
    }
}

impl PaneHost for NativePaneHost {
    fn create_pane(
        &self,
        processor: &mut Processor,
        event_loop: &ActiveEventLoop,
        cwd: &Path,
        terminal_options: &TerminalOptions,
    ) -> Result<WindowId, Box<dyn Error>> {
        // SAFETY: NativePaneHost retains the parent for the lifetime of every created child.
        let parent = unsafe { ParentWindowHandle::new(self.chrome.window_handle()?.as_raw()) };
        let mut options = WindowOptions::default();
        options.terminal_options = terminal_options.clone();
        options.no_activate = true;
        options.parent_window = Some(parent);
        options.terminal_options.working_directory = Some(cwd.to_owned());
        Ok(WindowId::from(
            processor.create_window(LoopHandle::Winit(event_loop), options)?,
        ))
    }

    fn move_pane(&self, processor: &mut Processor, pane: WindowId, rect: PhysicalRect) {
        let Some((_, pane)) = self.native_windows(processor, pane) else {
            return;
        };
        let (Ok(width), Ok(height)) = (i32::try_from(rect.width), i32::try_from(rect.height))
        else {
            return;
        };
        // SAFETY: both HWNDs are live on the event-loop thread. WS_CHILD coordinates are relative
        // to the parent's client area, and this combines position and size into one mutation.
        unsafe {
            SetWindowPos(
                pane,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }

    fn reveal(&self, processor: &mut Processor, pane: WindowId, visible: bool) {
        if let Some(pane) = processor.window_mut(pane) {
            pane.set_automation_visible(visible);
        }
    }

    fn focus(&self, processor: &mut Processor, pane: WindowId) {
        let Some((chrome, pane)) = self.native_windows(processor, pane) else {
            return;
        };
        // SAFETY: both HWNDs belong to this event-loop thread. A WS_CHILD cannot itself become
        // the foreground window, so activate its top-level chrome and then assign keyboard focus
        // directly to the pane.
        unsafe {
            SetForegroundWindow(chrome);
            SetActiveWindow(chrome);
            SetFocus(pane);
        }
    }

    fn is_attached(&self, processor: &Processor, pane: WindowId) -> bool {
        self.native_windows(processor, pane)
            .is_some_and(|(chrome, pane)| unsafe { GetParent(pane) == chrome })
    }
}

fn hwnd(raw: RawWindowHandle) -> Option<HWND> {
    let RawWindowHandle::Win32(handle) = raw else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}
