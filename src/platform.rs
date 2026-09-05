//! Platform boundary for native pane hosting and event-loop integration.

#[cfg(target_os = "macos")]
use std::error::Error;
#[cfg(target_os = "macos")]
use std::path::Path;

#[cfg(target_os = "macos")]
use vivido::Processor;
#[cfg(target_os = "macos")]
use vivido::cli::{TerminalOptions, WindowOptions};
#[cfg(target_os = "macos")]
use winit::event_loop::ActiveEventLoop;
#[cfg(target_os = "macos")]
use winit::window::WindowId;

#[cfg(target_os = "macos")]
use crate::layout::PhysicalRect;

#[cfg(target_os = "windows")]
pub const RESIZE_EDGE_LOGICAL: f64 = 10.0;
#[cfg(not(target_os = "windows"))]
pub const RESIZE_EDGE_LOGICAL: f64 = 6.0;

pub fn pane_bottom_resize_gutter(scale_factor: f64) -> u32 {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        (RESIZE_EDGE_LOGICAL * scale_factor).round() as u32
    } else {
        0
    }
}

/// Width of the side strips panes leave uncovered so the chrome's resize border stays reachable.
///
/// On macOS a pane is a child NSWindow floating above the chrome, so it would otherwise cover
/// the chrome's native resize border along its trailing edge (and its leading edge when the
/// sidebar is hidden). Windows performs side resizing through client-area hit testing, which
/// the shell already owns, so it needs no side gutter.
pub fn pane_side_resize_gutter(scale_factor: f64) -> u32 {
    if cfg!(target_os = "macos") {
        (RESIZE_EDGE_LOGICAL * scale_factor).round() as u32
    } else {
        0
    }
}

#[cfg(target_os = "macos")]
pub trait PaneHost {
    fn create_pane(
        &self,
        processor: &mut Processor,
        event_loop: &ActiveEventLoop,
        cwd: &Path,
        terminal_options: &TerminalOptions,
    ) -> Result<WindowId, Box<dyn Error>>;
    fn create_pane_with_options(
        &self,
        processor: &mut Processor,
        event_loop: &ActiveEventLoop,
        mut options: WindowOptions,
    ) -> Result<WindowId, Box<dyn Error>> {
        let cwd = options
            .terminal_options
            .working_directory
            .take()
            .unwrap_or_default();
        self.create_pane(processor, event_loop, &cwd, &options.terminal_options)
    }
    fn move_pane(&self, processor: &mut Processor, pane: WindowId, rect: PhysicalRect);
    fn reveal(&self, processor: &mut Processor, pane: WindowId, visible: bool);
    fn focus(&self, processor: &mut Processor, pane: WindowId);
    fn is_attached(&self, processor: &Processor, pane: WindowId) -> bool;
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use vivido::shell::{NativePaneHost, PaneHost};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::configure_chrome_window;
#[cfg(target_os = "macos")]
pub use macos::{
    NativePaneHost, configure_event_loop, finalize_chrome_window, focus_chrome_input,
    position_rename_editor, position_settings_menu, rename_editor_window_attributes,
    settings_menu_window_attributes,
};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::configure_chrome_window;
#[cfg(target_os = "windows")]
pub use windows::show_launch_menu;
#[cfg(target_os = "windows")]
pub use windows::{
    configure_event_loop, finalize_chrome_window, focus_chrome_input, position_rename_editor,
    position_settings_menu, rename_editor_window_attributes, settings_menu_window_attributes,
};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::configure_chrome_window;
#[cfg(target_os = "linux")]
pub use linux::{
    configure_event_loop, finalize_chrome_window, focus_chrome_input, position_rename_editor,
    position_settings_menu, rename_editor_window_attributes, settings_menu_window_attributes,
};
