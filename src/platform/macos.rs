use std::error::Error;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSView, NSWindowButton, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use vivido::cli::TerminalOptions;
use vivido::{Event, LoopHandle, ParentWindowHandle, Processor, WindowOptions};
use winit::dpi::PhysicalPosition;
use winit::event_loop::{ActiveEventLoop, EventLoopBuilder};
use winit::platform::macos::{
    ActivationPolicy, EventLoopBuilderExtMacOS, WindowAttributesExtMacOS,
};
use winit::window::{Window, WindowAttributes, WindowId};

use super::{PaneHost, PopupFocus};
use crate::layout::PhysicalRect;

pub fn configure_event_loop(builder: &mut EventLoopBuilder<Event>) {
    builder
        .with_activation_policy(ActivationPolicy::Regular)
        .with_activate_ignoring_other_apps(true);
}

pub fn configure_chrome_window(attributes: WindowAttributes) -> WindowAttributes {
    attributes
        .with_title_hidden(true)
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
}

pub fn finalize_chrome_window(window: &Window) {
    // macOS manages a separate fullscreen titlebar; forcing its geometry causes visual glitches.
    if window.fullscreen().is_some() {
        return;
    }
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let Some(window) = ns_window(handle.as_raw()) else {
        return;
    };
    let Some(close) = window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: standard window buttons and their titlebar views are live on the main thread.
    let Some(titlebar) = (unsafe { close.superview() }) else {
        return;
    };
    let Some(container) = (unsafe { titlebar.superview() }) else {
        return;
    };

    const HEIGHT: f64 = 35.0;
    const BUTTON_SIZE: f64 = 14.0;
    const LEFT_MARGIN: f64 = 12.0;
    const BUTTON_SPACING: f64 = 6.0;
    let mut container_frame = container.frame();
    container_frame.origin.y = window.frame().size.height - HEIGHT;
    container_frame.size.height = HEIGHT;
    container.setFrame(container_frame);
    titlebar.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(container_frame.size.width, HEIGHT),
    ));

    for (index, kind) in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(button) = window.standardWindowButton(kind) {
            let x = LEFT_MARGIN + index as f64 * (BUTTON_SIZE + BUTTON_SPACING);
            let y = (HEIGHT - BUTTON_SIZE) / 2.0 + 1.0;
            button.setFrame(NSRect::new(
                NSPoint::new(x, y),
                NSSize::new(BUTTON_SIZE, BUTTON_SIZE),
            ));
        }
    }
}

pub fn focus_chrome_input(window: &Window) {
    window.focus_window();
}

pub fn popup_window_attributes(
    _chrome: &Window,
    attributes: WindowAttributes,
    focus: PopupFocus,
) -> Result<Option<WindowAttributes>, Box<dyn Error>> {
    Ok(Some(detached_popup_window_attributes(attributes, focus)))
}

fn detached_popup_window_attributes(
    attributes: WindowAttributes,
    focus: PopupFocus,
) -> WindowAttributes {
    // Winit implements a parent window on macOS by calling `addChildWindow` while constructing
    // the NSWindow. AppKit can order that child with its parent even when the requested initial
    // visibility is false, exposing an unpainted white popup when the chrome first appears.
    // Keep auxiliary windows detached while hidden; `position_popup` attaches them immediately
    // before they are shown.
    attributes
        .with_decorations(false)
        .with_active(focus == PopupFocus::Keyboard)
}

pub fn position_popup(
    chrome: &Window,
    popup: &Window,
    position: PhysicalPosition<i32>,
    focus: PopupFocus,
) {
    let Ok(origin) = chrome.inner_position() else {
        return;
    };
    popup.set_outer_position(PhysicalPosition::new(
        origin.x.saturating_add(position.x),
        origin.y.saturating_add(position.y),
    ));
    let (Ok(chrome_handle), Ok(popup_handle)) = (chrome.window_handle(), popup.window_handle())
    else {
        return;
    };
    if let (Some(chrome), Some(child)) = (
        ns_window(chrome_handle.as_raw()),
        ns_window(popup_handle.as_raw()),
    ) {
        // SAFETY: both windows are live on the main event-loop thread.
        unsafe { chrome.addChildWindow_ordered(&child, NSWindowOrderingMode::Above) };
    }
    if focus == PopupFocus::Keyboard {
        popup.focus_window();
    }
}

pub fn set_popup_visible(window: &Window, visible: bool) {
    // Winit's `set_visible(true)` is `makeKeyAndOrderFront:`, which takes the keyboard away from
    // the chrome. The chrome dismisses its menus when it resigns key, so showing a popup that way
    // would close it again within the same click. Order it in without touching key status and let
    // `position_popup` hand over the keyboard only where a popup asked for it.
    let popup = window
        .window_handle()
        .ok()
        .and_then(|handle| ns_window(handle.as_raw()));
    let Some(popup) = popup else {
        window.set_visible(visible);
        return;
    };
    if visible {
        // `orderFrontRegardless` also works while another application is active, which
        // `orderFront:` does not.
        popup.orderFrontRegardless();
    } else {
        popup.orderOut(None);
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

    fn native_windows(
        &self,
        processor: &Processor,
        pane: WindowId,
    ) -> Option<(
        objc2::rc::Retained<objc2_app_kit::NSWindow>,
        objc2::rc::Retained<objc2_app_kit::NSWindow>,
    )> {
        Some((
            ns_window(self.chrome.window_handle().ok()?.as_raw())?,
            ns_window(processor.window(pane)?.display.window.raw_window_handle()?)?,
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
        let pane_id = processor.create_hosted_pane(LoopHandle::Winit(event_loop), options)?;
        // The shell owns pane geometry. A child NSWindow floats above the chrome and carries
        // its own resize border, so leaving it user-resizable lets an edge drag live-resize
        // the pane out from under the chrome's layout instead of resizing the chrome.
        if let Some(pane) = processor.window_mut(pane_id) {
            pane.display.window.set_resizable(false);
        }
        Ok(pane_id)
    }

    fn create_pane_with_options(
        &self,
        processor: &mut Processor,
        event_loop: &ActiveEventLoop,
        mut options: WindowOptions,
    ) -> Result<WindowId, Box<dyn Error>> {
        // SAFETY: NativePaneHost retains the parent for the lifetime of every created child.
        let parent = unsafe { ParentWindowHandle::new(self.chrome.window_handle()?.as_raw()) };
        options.no_activate = true;
        options.parent_window = Some(parent);
        let pane_id = processor.create_hosted_pane(LoopHandle::Winit(event_loop), options)?;
        if let Some(pane) = processor.window_mut(pane_id) {
            pane.display.window.set_resizable(false);
        }
        Ok(pane_id)
    }

    fn move_pane(&self, processor: &mut Processor, pane_id: WindowId, rect: PhysicalRect) {
        let Ok(origin) = self.chrome.inner_position() else {
            return;
        };
        if let Some(pane) = processor.window_mut(pane_id) {
            pane.display
                .window
                .set_outer_position(PhysicalPosition::new(
                    origin.x.saturating_add(rect.x),
                    origin.y.saturating_add(rect.y),
                ));
            pane.display
                .window
                .request_inner_size(winit::dpi::PhysicalSize::new(rect.width, rect.height));
        }
    }

    fn reveal(&self, processor: &mut Processor, pane_id: WindowId, visible: bool) {
        if !visible
            && let Some((chrome, pane)) = self.native_windows(processor, pane_id)
            && pane
                .parentWindow()
                .is_some_and(|parent| ptr::eq(&*parent, &*chrome))
        {
            chrome.removeChildWindow(&pane);
        }
        if let Some(pane) = processor.window_mut(pane_id) {
            pane.set_automation_visible(visible);
            if visible {
                pane.display.window.order_front_without_focus();
            }
        }
        if visible && let Some((chrome, pane)) = self.native_windows(processor, pane_id) {
            // SAFETY: both retained NSWindows are live on the main event-loop thread.
            unsafe { chrome.addChildWindow_ordered(&pane, NSWindowOrderingMode::Above) };
        }
    }

    fn focus(&self, processor: &mut Processor, pane: WindowId) {
        if let Some(pane) = processor.window_mut(pane) {
            pane.display.window.focus_window();
        }
    }

    fn is_attached(&self, processor: &Processor, pane: WindowId) -> bool {
        self.native_windows(processor, pane)
            .is_some_and(|(chrome, pane)| {
                pane.parentWindow()
                    .is_some_and(|parent| ptr::eq(&*parent, &*chrome))
            })
    }
}

fn ns_window(raw: RawWindowHandle) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    assert!(MainThreadMarker::new().is_some());
    let RawWindowHandle::AppKit(handle) = raw else {
        return None;
    };
    // SAFETY: winit supplied this live view; calls are confined to the main event loop.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_popup_starts_detached_from_the_chrome() {
        for focus in [PopupFocus::Keyboard, PopupFocus::None] {
            let attributes = detached_popup_window_attributes(
                Window::default_attributes().with_visible(false),
                focus,
            );

            assert!(!attributes.visible);
            assert!(!attributes.decorations);
            assert!(attributes.parent_window().is_none());
            assert_eq!(attributes.active, focus == PopupFocus::Keyboard);
        }
    }
}
