//! Wayland chrome-window integration; terminal pane hosting lives in `vivido::shell`.

use std::error::Error;

use vivido::Event;
use winit::dpi::PhysicalPosition;
use winit::event_loop::EventLoopBuilder;
use winit::window::{Window, WindowAttributes};

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
pub fn settings_menu_window_attributes(
    _chrome: &Window,
    _attributes: WindowAttributes,
) -> Result<Option<WindowAttributes>, Box<dyn Error>> {
    Ok(None)
}
pub fn position_settings_menu(_chrome: &Window, _menu: &Window, _position: PhysicalPosition<i32>) {}
