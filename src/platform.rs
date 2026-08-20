//! Platform boundary for native pane hosting and event-loop integration.

use std::error::Error;
use std::path::Path;

use vivido::Processor;
use vivido::cli::TerminalOptions;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::layout::PhysicalRect;

#[cfg(target_os = "windows")]
pub const RESIZE_EDGE_LOGICAL: f64 = 10.0;
#[cfg(not(target_os = "windows"))]
pub const RESIZE_EDGE_LOGICAL: f64 = 6.0;

pub fn pane_bottom_resize_gutter(scale_factor: f64) -> u32 {
    if cfg!(target_os = "windows") {
        (RESIZE_EDGE_LOGICAL * scale_factor).round() as u32
    } else {
        0
    }
}

pub trait PaneHost {
    fn create_pane(
        &self,
        processor: &mut Processor,
        event_loop: &ActiveEventLoop,
        cwd: &Path,
        terminal_options: &TerminalOptions,
    ) -> Result<WindowId, Box<dyn Error>>;
    fn move_pane(&self, processor: &mut Processor, pane: WindowId, rect: PhysicalRect);
    fn reveal(&self, processor: &mut Processor, pane: WindowId, visible: bool);
    fn focus(&self, processor: &mut Processor, pane: WindowId);
    fn is_attached(&self, processor: &Processor, pane: WindowId) -> bool;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::configure_chrome_window;
#[cfg(target_os = "macos")]
pub use macos::{
    NativePaneHost, configure_event_loop, finalize_chrome_window, position_settings_menu,
    settings_menu_window_attributes,
};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::configure_chrome_window;
#[cfg(target_os = "windows")]
pub use windows::{
    NativePaneHost, configure_event_loop, finalize_chrome_window, position_settings_menu,
    settings_menu_window_attributes,
};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::configure_chrome_window;
#[cfg(target_os = "linux")]
pub use linux::{
    NativePaneHost, configure_event_loop, finalize_chrome_window, position_settings_menu,
    settings_menu_window_attributes,
};
