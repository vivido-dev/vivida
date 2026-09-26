//! Wayland chrome-window integration; terminal pane hosting lives in `vivido::shell`.

use std::error::Error;

use vivido::Event;
use winit::dpi::PhysicalPosition;
use winit::event_loop::EventLoopBuilder;
use winit::window::{Window, WindowAttributes};

use super::PopupFocus;

pub fn configure_event_loop(builder: &mut EventLoopBuilder<Event>) {
    use winit::platform::wayland::EventLoopBuilderExtWayland;
    builder.with_wayland();
}
pub fn configure_chrome_window(attributes: WindowAttributes) -> WindowAttributes {
    attributes.with_decorations(false)
}
pub fn finalize_chrome_window(_window: &Window) {}

pub fn focus_chrome_input(window: &Window) {
    window.focus_window();
}
/// Wayland has no positioned child surfaces the shell can drive, so every popup falls back to an
/// overlay drawn inside the chrome.
pub fn popup_window_attributes(
    _chrome: &Window,
    _attributes: WindowAttributes,
    _focus: PopupFocus,
) -> Result<Option<WindowAttributes>, Box<dyn Error>> {
    Ok(None)
}
pub fn position_popup(
    _chrome: &Window,
    _popup: &Window,
    _position: PhysicalPosition<i32>,
    _focus: PopupFocus,
) {
}
pub fn set_popup_visible(window: &Window, visible: bool) {
    window.set_visible(visible);
}
