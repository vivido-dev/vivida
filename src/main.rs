//! Native workspace shell embedding Vivido panes in one process.

#![warn(rust_2018_idioms, future_incompatible)]
#![deny(clippy::all, clippy::if_not_else, clippy::enum_glob_use)]
#![cfg_attr(clippy, deny(warnings))]

mod chrome;
mod layout;
mod model;
mod platform;
mod session;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrome::{
    ChromeHitMap, ChromeLayout, ChromeRenderState, ChromeRenderer, SettingsMenuRenderer,
    SidebarMode, compute_chrome_layout, settings_menu_logical_size,
};
use clap::Parser;
use layout::{Axis, PhysicalRect, compute_rects};
use model::{
    PaneId, PaneKey, TabId, Workspace, WorkspaceId, focus_owned_pane, remove_owned_pane,
    workspace_label,
};
use platform::{
    NativePaneHost, PaneHost, RESIZE_EDGE_LOGICAL, configure_chrome_window, configure_event_loop,
    finalize_chrome_window, position_settings_menu, settings_menu_window_attributes,
};
use vivido::cli::{IpcSignalName, TerminalOptions};
use vivido::config::UiConfig;
use vivido::config::window::Decorations;
use vivido::display::renderer::EmbeddedFramePlacement;
use vivido::{Event, Processor};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
#[cfg(target_os = "linux")]
use winit::event::TouchPhase;
use winit::event::{ElementState, Event as WinitEvent, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

const CHROME_TITLE: &str = "vivida";
const INITIAL_WIDTH: f64 = 1100.0;
const INITIAL_HEIGHT: f64 = 700.0;
const PROBE_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const PROBE_FRAMES: u16 = 120;

#[derive(Debug, Parser)]
#[command(
    name = "vivida",
    version,
    about = "Native workspace shell embedding Vivido terminals"
)]
struct VividaOptions {
    /// Terminal process and arguments used for every pane.
    #[command(flatten)]
    terminal_options: TerminalOptions,
}

struct MotionProbe {
    origin: PhysicalPosition<i32>,
    frame: u16,
    next_frame: Instant,
}

struct Shell {
    config: UiConfig,
    _terminfo: vivido::tty::TerminfoGuard,
    terminal_options: TerminalOptions,
    processor: Processor,
    chrome_window: Option<Arc<Window>>,
    chrome_renderer: Option<ChromeRenderer>,
    chrome_id: Option<WindowId>,
    settings_menu_window: Option<Arc<Window>>,
    settings_menu_renderer: Option<SettingsMenuRenderer>,
    settings_menu_id: Option<WindowId>,
    settings_menu_embedded_size: Option<winit::dpi::PhysicalSize<u32>>,
    chrome_layout: ChromeLayout,
    chrome_hits: ChromeHitMap,
    cursor_position: Option<PhysicalPosition<f64>>,
    modifiers: ModifiersState,
    sidebar_mode: SidebarMode,
    hovered_workspace: Option<WorkspaceId>,
    settings_menu_open: bool,
    workspaces: Vec<Workspace>,
    active_workspace: Option<WorkspaceId>,
    next_workspace_id: u64,
    pane_index: HashMap<WindowId, PaneKey>,
    pane_rects: HashMap<WindowId, PhysicalRect>,
    closing_workspaces: HashSet<WorkspaceId>,
    launch_cwd: PathBuf,
    session_path: Option<PathBuf>,
    drag_samples: u64,
    max_drag_drift: u32,
    motion_probe: Option<MotionProbe>,
    last_title_click: Option<(Instant, PhysicalPosition<f64>)>,
    #[cfg(target_os = "linux")]
    embedded_hovered_pane: Option<WindowId>,
    #[cfg(target_os = "linux")]
    embedded_pointer_capture: Option<WindowId>,
    #[cfg(target_os = "linux")]
    embedded_touch_capture: HashMap<u64, WindowId>,
    #[cfg(target_os = "linux")]
    embedded_focused_pane: Option<WindowId>,
}

impl Shell {
    fn new(
        event_loop: &EventLoop<Event>,
        terminal_options: TerminalOptions,
    ) -> Result<Self, Box<dyn Error>> {
        // The shell owns creation and lifetime for every terminal pane.
        let mut options = vivido::cli::Options::default();
        options.daemon = true;
        let config = load_shell_config(&mut options);

        let terminfo = vivido::tty::setup_env();
        let processor = Processor::new(config.clone(), options, event_loop);
        let current_dir = std::env::current_dir()?;
        let launch_cwd = terminal_options
            .working_directory
            .as_ref()
            .map(|cwd| {
                if cwd.is_absolute() {
                    cwd.clone()
                } else {
                    current_dir.join(cwd)
                }
            })
            .unwrap_or(current_dir);
        Ok(Self {
            config,
            _terminfo: terminfo,
            terminal_options,
            processor,
            chrome_window: None,
            chrome_renderer: None,
            chrome_id: None,
            settings_menu_window: None,
            settings_menu_renderer: None,
            settings_menu_id: None,
            settings_menu_embedded_size: None,
            chrome_layout: ChromeLayout::default(),
            chrome_hits: ChromeHitMap::default(),
            cursor_position: None,
            modifiers: ModifiersState::empty(),
            sidebar_mode: SidebarMode::default(),
            hovered_workspace: None,
            settings_menu_open: false,
            workspaces: Vec::new(),
            active_workspace: None,
            next_workspace_id: 1,
            pane_index: HashMap::new(),
            pane_rects: HashMap::new(),
            closing_workspaces: HashSet::new(),
            launch_cwd,
            session_path: session::path(),
            drag_samples: 0,
            max_drag_drift: 0,
            motion_probe: None,
            last_title_click: None,
            #[cfg(target_os = "linux")]
            embedded_hovered_pane: None,
            #[cfg(target_os = "linux")]
            embedded_pointer_capture: None,
            #[cfg(target_os = "linux")]
            embedded_touch_capture: HashMap::new(),
            #[cfg(target_os = "linux")]
            embedded_focused_pane: None,
        })
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        if self.chrome_window.is_some() {
            return Ok(());
        }

        let attributes = configure_chrome_window(
            Window::default_attributes()
                .with_title(CHROME_TITLE)
                .with_position(LogicalPosition::new(80.0, 80.0))
                .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
                .with_min_inner_size(LogicalSize::new(640.0, 160.0))
                // A transparent pane must composite through the host's content region to the desktop.
                // The chrome renderer still paints the sidebar and tab strip as opaque rectangles.
                .with_transparent(self.config.window_opacity() < 1.0)
                // Pane and renderer initialization can take several seconds on a cold GPU cache.
                // Keep the unpainted native window off screen until its children are ready.
                .with_visible(false),
        );
        let chrome_window = Arc::new(event_loop.create_window(attributes)?);
        finalize_chrome_window(&chrome_window);
        let chrome_id = chrome_window.id();
        let scale_factor = chrome_window.scale_factor();
        let chrome_renderer =
            ChromeRenderer::new(Arc::clone(&chrome_window), &self.config, scale_factor)?;

        self.chrome_id = Some(chrome_id);
        self.chrome_window = Some(chrome_window);
        self.chrome_renderer = Some(chrome_renderer);
        if !self.restore_session(event_loop) {
            let launch_cwd = self.launch_cwd.clone();
            self.create_workspace(event_loop, launch_cwd)?;
        }
        self.sync_visibility_and_geometry();

        if !self.active_panes_have_native_parent() {
            return Err("a Vivido pane was not attached to the native chrome window".into());
        }

        self.initialize_settings_menu(event_loop)?;

        if let Some(chrome) = &self.chrome_window {
            chrome.set_visible(true);
        }
        self.request_chrome_redraw();
        self.focus_active_pane();

        println!("vivida ready");
        println!("process id: {}", std::process::id());
        println!("chrome window: {chrome_id:?}");
        println!("native child relationship: verified");
        println!("drag invariant: WindowEvent::Moved performs read-only drift sampling");
        println!("press P on the chrome to run the 60 Hz motion probe");
        Ok(())
    }

    fn restore_session(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(path) = &self.session_path else {
            return false;
        };
        let Ok(saved) = session::load_current_or_legacy(path) else {
            return false;
        };
        self.sidebar_mode = saved.sidebar_mode;
        for saved_workspace in saved.workspaces {
            let mut tabs = Vec::new();
            for saved_tab in saved_workspace.tabs {
                let mut panes = BTreeMap::new();
                for (pane_id, cwd) in saved_tab.pane_cwds {
                    let Ok(window_id) = self.create_pane_window(event_loop, &cwd) else {
                        continue;
                    };
                    self.pane_index.insert(
                        window_id,
                        PaneKey {
                            workspace_id: saved_workspace.id,
                            tab_id: saved_tab.id,
                            pane_id,
                        },
                    );
                    panes.insert(pane_id, window_id);
                }
                if !panes.is_empty() {
                    tabs.push(model::Tab::restore(
                        saved_tab.id,
                        saved_tab.root,
                        saved_tab.title,
                        saved_tab.focused_pane,
                        panes,
                    ));
                }
            }
            if !tabs.is_empty() {
                self.next_workspace_id = self.next_workspace_id.max(saved_workspace.id.0 + 1);
                self.workspaces.push(Workspace::restore(
                    saved_workspace.id,
                    saved_workspace.label,
                    saved_workspace.identity_cwd,
                    tabs,
                    saved_workspace.active_tab,
                ));
            }
        }
        self.active_workspace = saved
            .active_workspace
            .filter(|id| self.workspaces.iter().any(|workspace| workspace.id == *id))
            .or_else(|| self.workspaces.first().map(|workspace| workspace.id));
        !self.workspaces.is_empty()
    }

    fn save_session(&self) {
        let Some(path) = &self.session_path else {
            return;
        };
        let saved = session::Session::capture(
            self.sidebar_mode,
            self.active_workspace,
            &self.workspaces,
            |window_id| {
                self.processor
                    .window(window_id)
                    .and_then(|pane| pane.current_directory())
            },
        );
        if let Err(error) = session::save(path, &saved) {
            eprintln!("failed to save session: {error}");
        }
    }

    fn create_pane_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        cwd: &Path,
    ) -> Result<WindowId, Box<dyn Error>> {
        let chrome = Arc::clone(
            self.chrome_window
                .as_ref()
                .ok_or("chrome window is not initialized")?,
        );
        NativePaneHost::new(chrome).create_pane(
            &mut self.processor,
            event_loop,
            cwd,
            &self.terminal_options,
        )
    }

    fn create_workspace(
        &mut self,
        event_loop: &ActiveEventLoop,
        cwd: PathBuf,
    ) -> Result<WorkspaceId, Box<dyn Error>> {
        let pane_window_id = self.create_pane_window(event_loop, &cwd)?;
        let workspace_id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let workspace = Workspace::new(workspace_id, workspace_label(&cwd), cwd, pane_window_id);
        self.pane_index.insert(
            pane_window_id,
            PaneKey {
                workspace_id,
                tab_id: TabId(1),
                pane_id: PaneId(1),
            },
        );
        self.workspaces.push(workspace);
        self.active_workspace = Some(workspace_id);
        self.closing_workspaces.remove(&workspace_id);
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
        Ok(workspace_id)
    }

    fn active_workspace(&self) -> Option<&Workspace> {
        let id = self.active_workspace?;
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        let id = self.active_workspace?;
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
    }

    fn active_pane_cwd(&self) -> PathBuf {
        self.active_workspace()
            .and_then(Workspace::active_tab)
            .and_then(|tab| tab.focused_window_id())
            .and_then(|window_id| self.processor.window(window_id))
            .and_then(|pane| pane.current_directory())
            .unwrap_or_else(|| self.launch_cwd.clone())
    }

    fn switch_workspace(&mut self, workspace_id: WorkspaceId) {
        if self.active_workspace == Some(workspace_id)
            || !self
                .workspaces
                .iter()
                .any(|workspace| workspace.id == workspace_id)
            || self.closing_workspaces.contains(&workspace_id)
        {
            return;
        }
        self.active_workspace = Some(workspace_id);
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn create_tab(&mut self, event_loop: &ActiveEventLoop) {
        let cwd = self.active_pane_cwd();
        let Ok(window_id) = self.create_pane_window(event_loop, &cwd) else {
            return;
        };
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        let workspace_id = workspace.id;
        let tab_id = workspace.add_tab(window_id);
        self.pane_index.insert(
            window_id,
            PaneKey {
                workspace_id,
                tab_id,
                pane_id: PaneId(1),
            },
        );
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn switch_tab(&mut self, tab_id: TabId) {
        if self
            .active_workspace_mut()
            .is_some_and(|workspace| workspace.switch_tab(tab_id))
        {
            self.sync_visibility_and_geometry();
            self.focus_active_pane();
            self.request_chrome_redraw();
        }
    }

    fn close_active_pane(&mut self) {
        let window_id = self
            .active_workspace()
            .and_then(Workspace::active_tab)
            .and_then(|tab| tab.focused_window_id());
        if let Some(window_id) = window_id.and_then(|id| self.processor.window(id).map(|_| id))
            && let Some(pane) = self.processor.window(window_id)
        {
            let _ = pane.signal_process_group(pane_close_signal());
        }
    }

    fn close_workspace(&mut self, workspace_id: WorkspaceId) {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        if self.closing_workspaces.contains(&workspace_id) {
            return;
        }

        if self.workspaces.len() > 1 && self.active_workspace == Some(workspace_id) {
            let next_index = if index + 1 < self.workspaces.len() {
                index + 1
            } else {
                index - 1
            };
            self.active_workspace = Some(self.workspaces[next_index].id);
        }

        self.closing_workspaces.insert(workspace_id);
        let pane_ids = self
            .pane_index
            .iter()
            .filter_map(|(window_id, key)| (key.workspace_id == workspace_id).then_some(*window_id))
            .collect::<Vec<_>>();
        for window_id in pane_ids {
            if let Some(pane) = self.processor.window_mut(window_id) {
                pane.set_automation_visible(false);
                if let Err(error) = pane.signal_process_group(pane_close_signal()) {
                    eprintln!("failed to close pane {window_id:?}: {error:?}");
                }
            }
        }
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn split_active_pane(&mut self, event_loop: &ActiveEventLoop, axis: Axis) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let Some(tab) = workspace.active_tab() else {
            return;
        };
        let focused_pane = tab.focused_pane;
        let Some(focused_window) = tab.window_id(focused_pane) else {
            return;
        };
        let Some(rect) = self.pane_rects.get(&focused_window).copied() else {
            return;
        };
        let scale = self
            .chrome_window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
        let enough_room = match axis {
            Axis::Horizontal => {
                rect.width >= (chrome::MIN_PANE_WIDTH_LOGICAL * 2.0 * scale).round() as u32
            }
            Axis::Vertical => {
                rect.height >= (chrome::MIN_PANE_HEIGHT_LOGICAL * 2.0 * scale).round() as u32
            }
        };
        if !enough_room {
            return;
        }

        let cwd = self
            .processor
            .window(focused_window)
            .and_then(|pane| pane.current_directory())
            .unwrap_or_else(|| workspace.identity_cwd.clone());
        let Ok(window_id) = self.create_pane_window(event_loop, &cwd) else {
            return;
        };

        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        let workspace_id = workspace.id;
        let Some(tab) = workspace.active_tab_mut() else {
            return;
        };
        let tab_id = tab.id;
        let Some(pane_id) = tab.split(axis, window_id) else {
            if let Some(pane) = self.processor.window(window_id) {
                let _ = pane.signal_process_group(pane_close_signal());
            }
            return;
        };
        self.pane_index.insert(
            window_id,
            PaneKey {
                workspace_id,
                tab_id,
                pane_id,
            },
        );
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn set_sidebar_mode(&mut self, mode: SidebarMode) {
        if self.sidebar_mode == mode {
            return;
        }
        self.sidebar_mode = mode;
        self.sync_visibility_and_geometry();
        self.request_chrome_redraw();
    }

    fn sync_visibility_and_geometry(&mut self) {
        self.sync_pane_visibility();
        self.sync_pane_geometry();
    }

    fn sync_pane_visibility(&mut self) {
        let active = self.active_workspace;
        let closing = &self.closing_workspaces;
        let visibility = self
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace.tabs.iter().flat_map(move |tab| {
                    tab.panes.values().copied().map(move |window_id| {
                        let visible = Some(workspace.id) == active
                            && tab.id == workspace.active_tab
                            && !closing.contains(&workspace.id);
                        (window_id, visible)
                    })
                })
            })
            .collect::<Vec<_>>();
        for (window_id, visible) in visibility {
            self.set_native_pane_visibility(window_id, visible);
        }
    }

    fn set_native_pane_visibility(&mut self, window_id: WindowId, visible: bool) {
        let Some(chrome) = self.chrome_window.as_ref().map(Arc::clone) else {
            return;
        };
        NativePaneHost::new(chrome).reveal(&mut self.processor, window_id, visible);
    }

    fn sync_pane_geometry(&mut self) {
        let Some(chrome) = &self.chrome_window else {
            return;
        };
        let size = chrome.inner_size();
        let scale = chrome.scale_factor();
        self.chrome_layout = compute_chrome_layout(size, scale, self.sidebar_mode);
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let Some(tab) = workspace.active_tab() else {
            return;
        };
        let rects = compute_rects(&tab.root, self.chrome_layout.content, scale);
        let placements = tab
            .panes
            .iter()
            .filter_map(|(pane_id, window_id)| rects.get(pane_id).map(|rect| (*window_id, *rect)))
            .collect::<Vec<_>>();

        self.pane_rects.clear();
        let host = NativePaneHost::new(Arc::clone(chrome));
        for (window_id, rect) in placements {
            host.move_pane(&mut self.processor, window_id, rect);
            self.pane_rects.insert(window_id, rect);
        }
    }

    fn focus_active_pane(&mut self) {
        let window_id = self
            .active_workspace()
            .and_then(Workspace::active_tab)
            .and_then(|tab| tab.focused_window_id());
        let Some(chrome) = self.chrome_window.as_ref().map(Arc::clone) else {
            return;
        };
        if let Some(window_id) = window_id {
            NativePaneHost::new(chrome).focus(&mut self.processor, window_id);
        }
        #[cfg(target_os = "linux")]
        self.sync_embedded_focus();
    }

    fn record_focused_pane(&mut self, window_id: WindowId) -> bool {
        let Some(key) = self.pane_index.get(&window_id).copied() else {
            return false;
        };
        focus_owned_pane(&mut self.workspaces, self.active_workspace, key, window_id)
    }

    fn focus_clicked_pane(&mut self, window_id: WindowId) {
        if !self.record_focused_pane(window_id) {
            return;
        }
        self.focus_active_pane();
    }

    #[cfg(target_os = "linux")]
    fn focused_pane_id(&self) -> Option<WindowId> {
        self.active_workspace()
            .and_then(Workspace::active_tab)
            .and_then(|tab| tab.focused_window_id())
    }

    #[cfg(target_os = "linux")]
    fn embedded_pane_at(&self, position: PhysicalPosition<f64>) -> Option<WindowId> {
        if self.settings_menu_open
            && self
                .chrome_hits
                .settings_menu
                .contains(position.x, position.y)
        {
            return None;
        }
        self.pane_rects.iter().find_map(|(window_id, rect)| {
            rect.contains(position.x, position.y).then_some(*window_id)
        })
    }

    #[cfg(target_os = "linux")]
    fn sync_embedded_focus(&mut self) {
        let next = self.focused_pane_id();
        if next == self.embedded_focused_pane {
            return;
        }
        if let Some(previous) = self.embedded_focused_pane.take() {
            self.route_embedded_event(previous, WindowEvent::Focused(false));
        }
        if let Some(next) = next {
            self.route_embedded_event(next, WindowEvent::Focused(true));
            self.embedded_focused_pane = Some(next);
        }
    }

    #[cfg(target_os = "linux")]
    fn clear_embedded_focus(&mut self) {
        if let Some(previous) = self.embedded_focused_pane.take() {
            self.route_embedded_event(previous, WindowEvent::Focused(false));
        }
    }

    #[cfg(target_os = "linux")]
    fn route_embedded_event(&mut self, window_id: WindowId, event: WindowEvent) {
        self.processor
            .handle_embedded_window_event(window_id, event);
        self.sync_embedded_input_state(window_id);
        if self.processor.has_pending_embedded_redraw() {
            self.request_chrome_redraw();
        }
    }

    #[cfg(target_os = "linux")]
    fn sync_embedded_input_state(&self, window_id: WindowId) {
        let (Some(chrome), Some(state), Some(rect)) = (
            &self.chrome_window,
            self.processor.embedded_input_state(window_id),
            self.pane_rects.get(&window_id),
        ) else {
            return;
        };
        chrome.set_cursor(state.cursor);
        chrome.set_cursor_visible(state.cursor_visible);
        chrome.set_ime_allowed(state.ime_allowed);
        if let Some((position, size)) = state.ime_cursor_area {
            chrome.set_ime_cursor_area(
                PhysicalPosition::new(
                    position.x + f64::from(rect.x),
                    position.y + f64::from(rect.y),
                ),
                size,
            );
        }
    }

    fn reap_closed_panes(&mut self, event_loop: &ActiveEventLoop) {
        let closed = self
            .pane_index
            .keys()
            .copied()
            .filter(|window_id| self.processor.window(*window_id).is_none())
            .collect::<Vec<_>>();
        if closed.is_empty() {
            return;
        }

        for window_id in closed {
            self.pane_rects.remove(&window_id);
            let Some(key) = self.pane_index.remove(&window_id) else {
                continue;
            };
            remove_owned_pane(&mut self.workspaces, key);
        }
        for workspace in &mut self.workspaces {
            workspace.remove_empty_tabs();
        }

        let empty_workspaces = self
            .workspaces
            .iter()
            .filter(|workspace| workspace.pane_count() == 0)
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>();
        for workspace_id in empty_workspaces {
            self.remove_empty_workspace(workspace_id);
        }

        if self.workspaces.is_empty() {
            event_loop.exit();
            return;
        }
        if self.active_workspace.is_none_or(|active| {
            !self
                .workspaces
                .iter()
                .any(|workspace| workspace.id == active)
        }) {
            self.active_workspace = self.workspaces.first().map(|workspace| workspace.id);
        }
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn remove_empty_workspace(&mut self, workspace_id: WorkspaceId) {
        self.closing_workspaces.remove(&workspace_id);
        self.workspaces
            .retain(|workspace| workspace.id != workspace_id);
        if self.active_workspace == Some(workspace_id) {
            self.active_workspace = self.workspaces.first().map(|workspace| workspace.id);
        }
    }

    fn render_chrome(&mut self) {
        self.processor.draw_pending_embedded_windows();
        let Some(chrome) = &self.chrome_window else {
            return;
        };
        #[cfg(target_os = "linux")]
        if let Some(window_id) = self.focused_pane_id() {
            self.sync_embedded_input_state(window_id);
        }
        if self.settings_menu_open
            && self.settings_menu_window.is_none()
            && let (Some(renderer), Some(size)) = (
                &mut self.settings_menu_renderer,
                self.settings_menu_embedded_size,
            )
            && let Err(error) = renderer.render(size)
        {
            eprintln!("failed to render embedded settings menu: {error}");
        }
        let mut embedded_frames = self
            .pane_rects
            .iter()
            .filter_map(|(window_id, rect)| {
                let frame = self.processor.embedded_frame(*window_id)?;
                Some(EmbeddedFramePlacement {
                    frame,
                    origin: PhysicalPosition::new(
                        u32::try_from(rect.x).ok()?,
                        u32::try_from(rect.y).ok()?,
                    ),
                })
            })
            .collect::<Vec<_>>();
        if self.settings_menu_open
            && self.settings_menu_window.is_none()
            && let Some(frame) = self
                .settings_menu_renderer
                .as_ref()
                .and_then(SettingsMenuRenderer::embedded_frame)
        {
            let frame_width = frame.size.width;
            embedded_frames.push(EmbeddedFramePlacement {
                frame,
                origin: PhysicalPosition::new(
                    u32::try_from(
                        self.chrome_hits
                            .gear
                            .right()
                            .saturating_sub(i32::try_from(frame_width).unwrap_or(i32::MAX)),
                    )
                    .unwrap_or_default(),
                    u32::try_from(self.chrome_hits.gear.bottom()).unwrap_or_default(),
                ),
            });
        }
        let Some(renderer) = &mut self.chrome_renderer else {
            return;
        };
        match renderer.render(
            chrome.inner_size(),
            ChromeRenderState {
                sidebar_mode: self.sidebar_mode,
                workspaces: &self.workspaces,
                active_workspace: self.active_workspace,
                hovered_workspace: self.hovered_workspace,
                fullscreen: chrome.fullscreen().is_some(),
                settings_menu_open: self.settings_menu_open && self.settings_menu_window.is_none(),
                embedded_frames: &embedded_frames,
            },
        ) {
            Ok((layout, hit_map, presented)) => {
                self.chrome_layout = layout;
                let menu_moved = self.settings_menu_open
                    && self.settings_menu_window.is_none()
                    && self.chrome_hits.gear != hit_map.gear;
                self.chrome_hits = hit_map;
                if !presented {
                    self.request_chrome_redraw();
                }
                if menu_moved {
                    self.request_chrome_redraw();
                }
                self.sync_settings_menu_window();
            }
            Err(error) => eprintln!("failed to render chrome: {error}"),
        }
    }

    fn request_chrome_redraw(&self) {
        if let Some(chrome) = &self.chrome_window {
            chrome.request_redraw();
        }
    }

    fn initialize_settings_menu(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), Box<dyn Error>> {
        let Some(chrome) = &self.chrome_window else {
            return Ok(());
        };
        let (width, height) = settings_menu_logical_size();
        let attributes = Window::default_attributes()
            .with_title("vivida settings menu")
            .with_inner_size(LogicalSize::new(width, height))
            .with_resizable(false)
            .with_visible(false);
        let Some(attributes) = settings_menu_window_attributes(chrome, attributes)? else {
            let size = LogicalSize::new(width, height).to_physical(chrome.scale_factor());
            self.settings_menu_renderer = Some(SettingsMenuRenderer::new_embedded(
                size,
                &self.config,
                chrome.scale_factor(),
            )?);
            self.settings_menu_embedded_size = Some(size);
            return Ok(());
        };
        let window = Arc::new(event_loop.create_window(attributes)?);
        let renderer =
            SettingsMenuRenderer::new(Arc::clone(&window), &self.config, window.scale_factor())?;
        self.settings_menu_id = Some(window.id());
        self.settings_menu_window = Some(window);
        self.settings_menu_renderer = Some(renderer);
        Ok(())
    }

    fn set_settings_menu_open(&mut self, open: bool) {
        if self.settings_menu_open == open {
            return;
        }
        self.settings_menu_open = open;
        self.sync_settings_menu_window();
        self.request_chrome_redraw();
    }

    fn sync_settings_menu_window(&self) {
        let (Some(chrome), Some(menu)) = (&self.chrome_window, &self.settings_menu_window) else {
            return;
        };
        if self.settings_menu_open {
            let menu_width = menu.inner_size().width;
            let x = self
                .chrome_hits
                .gear
                .right()
                .saturating_sub(i32::try_from(menu_width).unwrap_or(i32::MAX))
                .max(0);
            position_settings_menu(
                chrome,
                menu,
                PhysicalPosition::new(x, self.chrome_hits.gear.bottom()),
            );
            menu.set_visible(true);
            menu.request_redraw();
        } else {
            menu.set_visible(false);
        }
    }

    fn render_settings_menu(&mut self) {
        let (Some(window), Some(renderer)) =
            (&self.settings_menu_window, &mut self.settings_menu_renderer)
        else {
            return;
        };
        match renderer.render(window.inner_size()) {
            Ok(true) => (),
            Ok(false) => window.request_redraw(),
            Err(error) => eprintln!("failed to render settings menu: {error}"),
        }
    }

    fn handle_chrome_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(cursor) = self.cursor_position else {
            return false;
        };
        if self.settings_menu_open {
            match settings_menu_click(&self.chrome_hits, cursor) {
                SettingsMenuClick::Item(_) | SettingsMenuClick::Outside => {
                    self.set_settings_menu_open(false);
                    return true;
                }
                SettingsMenuClick::MenuBackground => return true,
                SettingsMenuClick::Gear => (),
            }
        }
        if self.chrome_hits.toggle.contains(cursor.x, cursor.y) {
            self.set_sidebar_mode(self.sidebar_mode.next());
            return true;
        }
        if self
            .chrome_hits
            .split_horizontal
            .contains(cursor.x, cursor.y)
        {
            self.split_active_pane(event_loop, Axis::Horizontal);
            return true;
        }
        if self.chrome_hits.split_vertical.contains(cursor.x, cursor.y) {
            self.split_active_pane(event_loop, Axis::Vertical);
            return true;
        }
        if self.chrome_hits.gear.contains(cursor.x, cursor.y) {
            self.set_settings_menu_open(!self.settings_menu_open);
            return true;
        }
        if let Some(action) = window_frame_action(&self.chrome_hits, cursor) {
            match action {
                WindowFrameAction::Minimize => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_minimized(true);
                    }
                }
                WindowFrameAction::ToggleMaximize => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_maximized(!chrome.is_maximized());
                    }
                }
                WindowFrameAction::Close => event_loop.exit(),
            }
            return true;
        }
        if self.chrome_hits.new_tab.contains(cursor.x, cursor.y) {
            self.create_tab(event_loop);
            return true;
        }
        if let Some(tab_id) = self
            .chrome_hits
            .tab_rows
            .iter()
            .find_map(|(id, rect)| rect.contains(cursor.x, cursor.y).then_some(*id))
        {
            self.switch_tab(tab_id);
            return true;
        }
        if let Some(workspace_id) = self
            .chrome_hits
            .close_buttons
            .iter()
            .find_map(|(id, rect)| rect.contains(cursor.x, cursor.y).then_some(*id))
        {
            self.close_workspace(workspace_id);
            return true;
        }
        if self.chrome_hits.new_workspace.contains(cursor.x, cursor.y) {
            let cwd = self.active_pane_cwd();
            if let Err(error) = self.create_workspace(event_loop, cwd) {
                eprintln!("failed to create workspace: {error}");
            }
            return true;
        }
        if let Some(workspace_id) = self
            .chrome_hits
            .workspace_rows
            .iter()
            .find_map(|(id, rect)| rect.contains(cursor.x, cursor.y).then_some(*id))
        {
            self.switch_workspace(workspace_id);
            return true;
        }
        false
    }

    fn begin_chrome_drag_or_resize(&mut self) {
        let (Some(chrome), Some(cursor)) = (&self.chrome_window, self.cursor_position) else {
            return;
        };
        if let Some(direction) = chrome_resize_direction(chrome, cursor) {
            let _ = chrome.drag_resize_window(direction);
        } else if self.chrome_layout.tab_bar.contains(cursor.x, cursor.y) {
            let now = Instant::now();
            let is_double_click = self.last_title_click.is_some_and(|(at, position)| {
                now.duration_since(at) <= Duration::from_millis(500)
                    && (position.x - cursor.x).abs() <= 4.0
                    && (position.y - cursor.y).abs() <= 4.0
            });
            if is_double_click {
                self.last_title_click = None;
                chrome.set_maximized(!chrome.is_maximized());
                return;
            }
            self.last_title_click = Some((now, cursor));
            let _ = chrome.drag_window();
        }
    }

    fn update_chrome_cursor(&self, position: PhysicalPosition<f64>) {
        let Some(chrome) = &self.chrome_window else {
            return;
        };
        let icon = chrome_resize_direction(chrome, position)
            .map(resize_cursor)
            .unwrap_or(CursorIcon::Default);
        chrome.set_cursor(icon);
    }

    fn update_hovered_workspace(&mut self, position: PhysicalPosition<f64>) {
        let hovered = self
            .chrome_hits
            .workspace_rows
            .iter()
            .find_map(|(id, rect)| rect.contains(position.x, position.y).then_some(*id));
        if hovered != self.hovered_workspace {
            self.hovered_workspace = hovered;
            self.request_chrome_redraw();
        }
    }

    fn active_panes_have_native_parent(&self) -> bool {
        let Some(workspace) = self.active_workspace() else {
            return false;
        };
        let Some(tab) = workspace.active_tab() else {
            return false;
        };
        tab.panes
            .values()
            .copied()
            .all(|window_id| self.native_parent_is_chrome(window_id))
    }

    fn native_parent_is_chrome(&self, pane_id: WindowId) -> bool {
        self.chrome_window.as_ref().is_some_and(|chrome| {
            NativePaneHost::new(Arc::clone(chrome)).is_attached(&self.processor, pane_id)
        })
    }

    fn sample_drag_alignment(&mut self) {
        let Some(chrome) = &self.chrome_window else {
            return;
        };
        let Ok(chrome_position) = chrome.inner_position() else {
            return;
        };
        let placements = self
            .pane_rects
            .iter()
            .map(|(id, rect)| (*id, *rect))
            .collect::<Vec<_>>();
        if placements.is_empty() {
            return;
        }

        let mut sample_max = 0;
        for (window_id, rect) in placements {
            let Some(pane_position) = self
                .processor
                .window(window_id)
                .and_then(|pane| pane.display.window.outer_position())
            else {
                continue;
            };
            let expected_x = chrome_position.x.saturating_add(rect.x);
            let expected_y = chrome_position.y.saturating_add(rect.y);
            sample_max = sample_max
                .max(expected_x.abs_diff(pane_position.x))
                .max(expected_y.abs_diff(pane_position.y));
        }
        self.drag_samples += 1;
        self.max_drag_drift = self.max_drag_drift.max(sample_max);
        chrome.set_title(&format!(
            "{CHROME_TITLE} — {} samples, max drift {} px",
            self.drag_samples, self.max_drag_drift
        ));
    }

    fn start_motion_probe(&mut self, event_loop: &ActiveEventLoop) {
        let Some(origin) = self
            .chrome_window
            .as_ref()
            .and_then(|chrome| chrome.outer_position().ok())
        else {
            return;
        };
        self.drag_samples = 0;
        self.max_drag_drift = 0;
        self.motion_probe = Some(MotionProbe {
            origin,
            frame: 0,
            next_frame: Instant::now(),
        });
        event_loop.set_control_flow(ControlFlow::Poll);
    }

    fn advance_motion_probe(&mut self, event_loop: &ActiveEventLoop) {
        let Some(probe) = &mut self.motion_probe else {
            return;
        };
        let now = Instant::now();
        if now < probe.next_frame {
            event_loop.set_control_flow(ControlFlow::WaitUntil(probe.next_frame));
            return;
        }

        let outward_frame = if probe.frame <= PROBE_FRAMES / 2 {
            probe.frame
        } else {
            PROBE_FRAMES - probe.frame
        };
        if let Some(chrome) = &self.chrome_window {
            chrome.set_outer_position(PhysicalPosition::new(
                probe.origin.x + i32::from(outward_frame) * 5,
                probe.origin.y + i32::from(outward_frame) * 2,
            ));
        }
        probe.frame += 1;
        probe.next_frame = now + PROBE_FRAME_INTERVAL;
        if probe.frame > PROBE_FRAMES {
            self.motion_probe = None;
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(probe.next_frame));
        }
    }

    fn handle_chrome_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        if self.handle_shell_shortcut(event_loop, &event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(chrome) = &self.chrome_window {
                    finalize_chrome_window(chrome);
                }
                if let Some(renderer) = &mut self.chrome_renderer {
                    let scale = self
                        .chrome_window
                        .as_ref()
                        .map_or(1.0, |window| window.scale_factor());
                    renderer.resize(size, scale, &self.config);
                }
                self.sync_pane_geometry();
                self.request_chrome_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(chrome) = &self.chrome_window {
                    finalize_chrome_window(chrome);
                }
                if let (Some(renderer), Some(chrome)) =
                    (&mut self.chrome_renderer, &self.chrome_window)
                {
                    renderer.resize(chrome.inner_size(), scale_factor, &self.config);
                }
                self.sync_pane_geometry();
                self.request_chrome_redraw();
            }
            // Activating the top-level host gives its HWND keyboard focus. Restore focus to the
            // remembered terminal child so activation, task switching, and startup all type into
            // the pane rather than the chrome surface.
            WindowEvent::Focused(true) => {
                self.focus_active_pane();
            }
            WindowEvent::Focused(false) => {
                #[cfg(target_os = "linux")]
                self.clear_embedded_focus();
                self.set_settings_menu_open(false);
            }
            WindowEvent::RedrawRequested => self.render_chrome(),
            WindowEvent::CursorMoved {
                device_id,
                position,
            } => {
                #[cfg(not(target_os = "linux"))]
                let _ = device_id;
                self.cursor_position = Some(position);
                self.update_hovered_workspace(position);
                self.update_chrome_cursor(position);
                #[cfg(target_os = "linux")]
                {
                    let target = self
                        .embedded_pointer_capture
                        .or_else(|| self.embedded_pane_at(position));
                    if self.embedded_pointer_capture.is_none()
                        && target != self.embedded_hovered_pane
                    {
                        if let Some(previous) = self.embedded_hovered_pane {
                            self.route_embedded_event(
                                previous,
                                WindowEvent::CursorLeft { device_id },
                            );
                        }
                        if let Some(next) = target {
                            self.route_embedded_event(
                                next,
                                WindowEvent::CursorEntered { device_id },
                            );
                        }
                        self.embedded_hovered_pane = target;
                    }
                    if let Some(window_id) = target
                        && let Some(rect) = self.pane_rects.get(&window_id)
                    {
                        self.route_embedded_event(
                            window_id,
                            WindowEvent::CursorMoved {
                                device_id,
                                position: translated_pane_position(*rect, position),
                            },
                        );
                    }
                }
            }
            WindowEvent::CursorLeft { device_id } => {
                #[cfg(not(target_os = "linux"))]
                let _ = device_id;
                #[cfg(target_os = "linux")]
                if let Some(window_id) = self.embedded_hovered_pane.take() {
                    self.route_embedded_event(window_id, WindowEvent::CursorLeft { device_id });
                }
                self.cursor_position = None;
                self.hovered_workspace = None;
                self.request_chrome_redraw();
            }
            WindowEvent::MouseInput {
                device_id,
                state,
                button,
            } => {
                #[cfg(not(target_os = "linux"))]
                let _ = device_id;
                #[cfg(target_os = "linux")]
                {
                    if state == ElementState::Pressed
                        && button == MouseButton::Left
                        && self.settings_menu_open
                        && self.handle_chrome_click(event_loop)
                    {
                        return;
                    }
                    let target = self.embedded_pointer_capture.or_else(|| {
                        self.cursor_position
                            .and_then(|position| self.embedded_pane_at(position))
                    });
                    if let Some(window_id) = target {
                        if state == ElementState::Pressed {
                            self.focus_clicked_pane(window_id);
                            if button == MouseButton::Left {
                                self.embedded_pointer_capture = Some(window_id);
                            }
                        }
                        self.route_embedded_event(
                            window_id,
                            WindowEvent::MouseInput {
                                device_id,
                                state,
                                button,
                            },
                        );
                        if state == ElementState::Released && button == MouseButton::Left {
                            self.embedded_pointer_capture = None;
                        }
                        return;
                    }
                }
                if state == ElementState::Pressed
                    && button == MouseButton::Left
                    && !self.handle_chrome_click(event_loop)
                {
                    self.begin_chrome_drag_or_resize();
                }
            }
            WindowEvent::MouseWheel {
                device_id,
                delta,
                phase,
            } => {
                #[cfg(not(target_os = "linux"))]
                let _ = (device_id, delta, phase);
                #[cfg(target_os = "linux")]
                if let Some(window_id) =
                    self.embedded_pointer_capture.or(self.embedded_hovered_pane)
                {
                    self.route_embedded_event(
                        window_id,
                        WindowEvent::MouseWheel {
                            device_id,
                            delta,
                            phase,
                        },
                    );
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                #[cfg(target_os = "linux")]
                if let Some(window_id) = self.focused_pane_id() {
                    self.route_embedded_event(window_id, WindowEvent::ModifiersChanged(modifiers));
                }
            }
            WindowEvent::Ime(ime) => {
                #[cfg(not(target_os = "linux"))]
                let _ = ime;
                #[cfg(target_os = "linux")]
                if let Some(window_id) = self.focused_pane_id() {
                    self.route_embedded_event(window_id, WindowEvent::Ime(ime));
                }
            }
            WindowEvent::Touch(touch) => {
                #[cfg(not(target_os = "linux"))]
                let _ = touch;
                #[cfg(target_os = "linux")]
                {
                    let mut touch = touch;
                    let touch_id = touch.id;
                    let touch_phase = touch.phase;
                    let window_id = if touch.phase == TouchPhase::Started {
                        let window_id = self.embedded_pane_at(touch.location);
                        if let Some(window_id) = window_id {
                            self.embedded_touch_capture.insert(touch.id, window_id);
                            self.focus_clicked_pane(window_id);
                        }
                        window_id
                    } else {
                        self.embedded_touch_capture.get(&touch.id).copied()
                    };
                    if let Some(window_id) = window_id
                        && let Some(rect) = self.pane_rects.get(&window_id)
                    {
                        touch.location = translated_pane_position(*rect, touch.location);
                        self.route_embedded_event(window_id, WindowEvent::Touch(touch));
                    }
                    if matches!(touch_phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        self.embedded_touch_capture.remove(&touch_id);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                    && self.settings_menu_open =>
            {
                self.set_settings_menu_open(false);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyP) =>
            {
                self.start_motion_probe(event_loop);
            }
            WindowEvent::KeyboardInput {
                device_id,
                event,
                is_synthetic,
            } => {
                #[cfg(not(target_os = "linux"))]
                let _ = (device_id, event, is_synthetic);
                #[cfg(target_os = "linux")]
                if let Some(window_id) = self.focused_pane_id() {
                    self.route_embedded_event(
                        window_id,
                        WindowEvent::KeyboardInput {
                            device_id,
                            event,
                            is_synthetic,
                        },
                    );
                }
            }
            // Read-only verification: the native window system moves parent and children as one
            // hierarchy. No child geometry mutation is allowed here.
            WindowEvent::Moved(_) => self.sample_drag_alignment(),
            _ => (),
        }
    }

    fn handle_shell_shortcut(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) -> bool {
        let WindowEvent::KeyboardInput { event, .. } = event else {
            return false;
        };
        if event.state != ElementState::Pressed || !shell_modifier_pressed(self.modifiers) {
            return false;
        }
        match event.physical_key {
            PhysicalKey::Code(KeyCode::KeyT) => {
                self.create_tab(event_loop);
                true
            }
            PhysicalKey::Code(KeyCode::KeyD) => {
                let axis = if self.modifiers.shift_key() {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                };
                self.split_active_pane(event_loop, axis);
                true
            }
            PhysicalKey::Code(KeyCode::KeyN) if self.modifiers.shift_key() => {
                let cwd = self.active_pane_cwd();
                if let Err(error) = self.create_workspace(event_loop, cwd) {
                    eprintln!("failed to create workspace: {error}");
                }
                true
            }
            PhysicalKey::Code(KeyCode::KeyB) if self.modifiers.shift_key() => {
                self.set_sidebar_mode(self.sidebar_mode.next());
                true
            }
            PhysicalKey::Code(KeyCode::KeyW) if self.modifiers.shift_key() => {
                if let Some(workspace_id) = self.active_workspace {
                    self.close_workspace(workspace_id);
                }
                true
            }
            PhysicalKey::Code(KeyCode::KeyW) => {
                self.close_active_pane();
                true
            }
            PhysicalKey::Code(KeyCode::BracketRight) if self.modifiers.shift_key() => {
                self.cycle_tab(1);
                true
            }
            PhysicalKey::Code(KeyCode::BracketLeft) if self.modifiers.shift_key() => {
                self.cycle_tab(-1);
                true
            }
            PhysicalKey::Code(KeyCode::KeyP) if self.modifiers.shift_key() => {
                self.start_motion_probe(event_loop);
                true
            }
            PhysicalKey::Code(code) => workspace_shortcut_index(code)
                .and_then(|index| self.workspaces.get(index).map(|workspace| workspace.id))
                .is_some_and(|workspace_id| {
                    self.switch_workspace(workspace_id);
                    true
                }),
            _ => false,
        }
    }

    fn cycle_tab(&mut self, direction: isize) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let Some(index) = workspace
            .tabs
            .iter()
            .position(|tab| tab.id == workspace.active_tab)
        else {
            return;
        };
        let len = workspace.tabs.len();
        if len < 2 {
            return;
        }
        let next = (index as isize + direction).rem_euclid(len as isize) as usize;
        let tab_id = workspace.tabs[next].id;
        self.switch_tab(tab_id);
    }

    fn refresh_tab_titles(&mut self) {
        let updates = self
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace.tabs.iter().filter_map(|tab| {
                    let window = tab.window_id(tab.root.first_pane())?;
                    let title = self.processor.window(window)?.title().to_owned();
                    (title != tab.title).then_some((workspace.id, tab.id, title))
                })
            })
            .collect::<Vec<_>>();
        if updates.is_empty() {
            return;
        }
        for (workspace_id, tab_id, title) in updates {
            if let Some(tab) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == workspace_id)
                .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
            {
                tab.title = title;
            }
        }
        self.request_chrome_redraw();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowFrameAction {
    Minimize,
    ToggleMaximize,
    Close,
}

#[cfg(any(target_os = "linux", test))]
fn translated_pane_position(
    rect: PhysicalRect,
    position: PhysicalPosition<f64>,
) -> PhysicalPosition<f64> {
    PhysicalPosition::new(
        position.x - f64::from(rect.x),
        position.y - f64::from(rect.y),
    )
}

fn window_frame_action(
    hit_map: &ChromeHitMap,
    position: PhysicalPosition<f64>,
) -> Option<WindowFrameAction> {
    if hit_map.minimize.contains(position.x, position.y) {
        Some(WindowFrameAction::Minimize)
    } else if hit_map.maximize.contains(position.x, position.y) {
        Some(WindowFrameAction::ToggleMaximize)
    } else if hit_map.close.contains(position.x, position.y) {
        Some(WindowFrameAction::Close)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsMenuClick {
    Item(usize),
    Gear,
    MenuBackground,
    Outside,
}

fn settings_menu_click(
    hit_map: &ChromeHitMap,
    position: PhysicalPosition<f64>,
) -> SettingsMenuClick {
    if let Some(index) = hit_map
        .settings_items
        .iter()
        .position(|item| item.contains(position.x, position.y))
    {
        SettingsMenuClick::Item(index)
    } else if hit_map.gear.contains(position.x, position.y) {
        SettingsMenuClick::Gear
    } else if hit_map.settings_menu.contains(position.x, position.y) {
        SettingsMenuClick::MenuBackground
    } else {
        SettingsMenuClick::Outside
    }
}

fn chrome_resize_direction(
    window: &Window,
    position: PhysicalPosition<f64>,
) -> Option<ResizeDirection> {
    if cfg!(target_os = "macos") || window.is_maximized() || window.fullscreen().is_some() {
        return None;
    }
    resize_direction_at(window.inner_size(), window.scale_factor(), position)
}

fn resize_direction_at(
    size: winit::dpi::PhysicalSize<u32>,
    scale_factor: f64,
    position: PhysicalPosition<f64>,
) -> Option<ResizeDirection> {
    let edge = (RESIZE_EDGE_LOGICAL * scale_factor).max(1.0);
    let left = position.x < edge;
    let right = position.x >= f64::from(size.width) - edge;
    let top = position.y < edge;
    let bottom = position.y >= f64::from(size.height) - edge;
    match (left, right, top, bottom) {
        (true, false, true, false) => Some(ResizeDirection::NorthWest),
        (false, true, true, false) => Some(ResizeDirection::NorthEast),
        (true, false, false, true) => Some(ResizeDirection::SouthWest),
        (false, true, false, true) => Some(ResizeDirection::SouthEast),
        (true, false, false, false) => Some(ResizeDirection::West),
        (false, true, false, false) => Some(ResizeDirection::East),
        (false, false, true, false) => Some(ResizeDirection::North),
        (false, false, false, true) => Some(ResizeDirection::South),
        _ => None,
    }
}

fn resize_cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
        ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
    }
}

impl ApplicationHandler<Event> for Shell {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause == StartCause::Init
            && let Err(error) = self.initialize(event_loop)
        {
            eprintln!("failed to initialize vivida: {error}");
            event_loop.exit();
            return;
        }
        self.processor
            .handle_winit_event(event_loop, WinitEvent::NewEvents(cause));
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if Some(window_id) == self.settings_menu_id {
            match event {
                WindowEvent::CloseRequested => self.set_settings_menu_open(false),
                WindowEvent::RedrawRequested => self.render_settings_menu(),
                WindowEvent::Resized(size) => {
                    if let Some(renderer) = &mut self.settings_menu_renderer {
                        let scale = self
                            .settings_menu_window
                            .as_ref()
                            .map_or(1.0, |window| window.scale_factor());
                        renderer.resize(size, scale, &self.config);
                    }
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    if let (Some(renderer), Some(window)) =
                        (&mut self.settings_menu_renderer, &self.settings_menu_window)
                    {
                        renderer.resize(window.inner_size(), scale_factor, &self.config);
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => self.set_settings_menu_open(false),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed
                        && event.physical_key == PhysicalKey::Code(KeyCode::Escape) =>
                {
                    self.set_settings_menu_open(false);
                }
                _ => (),
            }
            return;
        }
        if Some(window_id) == self.chrome_id {
            self.handle_chrome_event(event_loop, event);
            return;
        }
        if self.processor.window(window_id).is_none() {
            return;
        }
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.modifiers = modifiers.state();
        }
        if matches!(
            &event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            }
        ) {
            self.set_settings_menu_open(false);
            self.focus_clicked_pane(window_id);
        } else if matches!(&event, WindowEvent::Focused(true)) {
            self.record_focused_pane(window_id);
        }
        if !self.handle_shell_shortcut(event_loop, &event) {
            self.processor
                .handle_winit_event(event_loop, WinitEvent::WindowEvent { window_id, event });
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        self.processor
            .handle_winit_event(event_loop, WinitEvent::UserEvent(event));
        if self.processor.has_pending_embedded_redraw() {
            self.request_chrome_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.processor
            .handle_winit_event(event_loop, WinitEvent::AboutToWait);
        if self.processor.has_pending_embedded_redraw() {
            self.request_chrome_redraw();
        }
        self.reap_closed_panes(event_loop);
        self.refresh_tab_titles();
        self.advance_motion_probe(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.save_session();
        println!(
            "drag samples: {}; maximum child drift: {} px",
            self.drag_samples, self.max_drag_drift
        );
        let pane_ids = self.pane_index.keys().copied().collect::<Vec<_>>();
        for window_id in pane_ids {
            if let Some(pane) = self.processor.window(window_id) {
                let _ = pane.signal_process_group(pane_close_signal());
            }
        }
        self.processor
            .handle_winit_event(event_loop, WinitEvent::LoopExiting);
        self.pane_index.clear();
        self.workspaces.clear();
        self.chrome_renderer = None;
        self.chrome_window = None;
        self.chrome_id = None;
    }
}

fn load_shell_config(options: &mut vivido::cli::Options) -> UiConfig {
    let mut config = vivido::config::load(options);

    // Pane windows are embedded in vivida's own chrome, so only these two window-management
    // settings are shell-owned. All visual settings, including opacity, come from the user's
    // Vivido configuration.
    config.window.decorations = Decorations::None;
    config.window.resize_increments = false;
    config
}

fn workspace_shortcut_index(key_code: KeyCode) -> Option<usize> {
    match key_code {
        KeyCode::Digit1 => Some(0),
        KeyCode::Digit2 => Some(1),
        KeyCode::Digit3 => Some(2),
        KeyCode::Digit4 => Some(3),
        KeyCode::Digit5 => Some(4),
        KeyCode::Digit6 => Some(5),
        KeyCode::Digit7 => Some(6),
        KeyCode::Digit8 => Some(7),
        KeyCode::Digit9 => Some(8),
        _ => None,
    }
}

fn shell_modifier_pressed(modifiers: ModifiersState) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.super_key()
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control_key()
    }
}

fn pane_close_signal() -> IpcSignalName {
    #[cfg(windows)]
    {
        IpcSignalName::Term
    }
    #[cfg(not(windows))]
    {
        IpcSignalName::Hup
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = VividaOptions::parse();
    let mut builder = EventLoop::<Event>::with_user_event();
    configure_event_loop(&mut builder);
    let event_loop = builder.build()?;
    let mut shell = Shell::new(&event_loop, options.terminal_options)?;
    event_loop.run_app(&mut shell)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn shell_loads_vivido_window_opacity() {
        let path = std::env::temp_dir().join(format!(
            "vivida-vivido-config-opacity-{}.toml",
            std::process::id()
        ));
        fs::write(&path, "[window]\nopacity = 0.42\n").unwrap();

        let mut options = vivido::cli::Options::default();
        options.config_file = Some(path.clone());
        let config = load_shell_config(&mut options);

        assert!((config.window_opacity() - 0.42).abs() < f32::EPSILON);
        assert_eq!(config.window.decorations, Decorations::None);
        assert!(!config.window.resize_increments);
        assert_eq!(config.config_paths, vec![path.clone()]);

        fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_shortcuts_use_control() {
        assert!(shell_modifier_pressed(ModifiersState::CONTROL));
        assert!(!shell_modifier_pressed(ModifiersState::SUPER));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_shell_shortcuts_use_command() {
        assert!(shell_modifier_pressed(ModifiersState::SUPER));
        assert!(!shell_modifier_pressed(ModifiersState::CONTROL));
    }

    #[test]
    fn pane_close_uses_the_platform_signal() {
        #[cfg(windows)]
        assert_eq!(pane_close_signal(), IpcSignalName::Term);
        #[cfg(not(windows))]
        assert_eq!(pane_close_signal(), IpcSignalName::Hup);
    }

    #[test]
    fn startup_command_uses_vivido_terminal_option_syntax() {
        let pwsh = VividaOptions::try_parse_from(["vivida", "-e", "pwsh.exe"]).unwrap();
        let command = pwsh.terminal_options.command().unwrap();
        assert_eq!(command.program(), "pwsh.exe");
        assert!(command.args().is_empty());

        let wsl = VividaOptions::try_parse_from(["vivida", "-e", "wsl.exe", "--cd", "~"]).unwrap();
        let command = wsl.terminal_options.command().unwrap();
        assert_eq!(command.program(), "wsl.exe");
        assert_eq!(command.args(), ["--cd", "~"]);
    }

    #[test]
    fn window_frame_actions_are_resolved_without_native_apis() {
        let hit_map = ChromeHitMap {
            minimize: PhysicalRect {
                x: 10,
                y: 0,
                width: 20,
                height: 35,
            },
            maximize: PhysicalRect {
                x: 30,
                y: 0,
                width: 20,
                height: 35,
            },
            close: PhysicalRect {
                x: 50,
                y: 0,
                width: 20,
                height: 35,
            },
            ..ChromeHitMap::default()
        };
        assert_eq!(
            window_frame_action(&hit_map, PhysicalPosition::new(15.0, 10.0)),
            Some(WindowFrameAction::Minimize)
        );
        assert_eq!(
            window_frame_action(&hit_map, PhysicalPosition::new(35.0, 10.0)),
            Some(WindowFrameAction::ToggleMaximize)
        );
        assert_eq!(
            window_frame_action(&hit_map, PhysicalPosition::new(55.0, 10.0)),
            Some(WindowFrameAction::Close)
        );
        assert_eq!(
            window_frame_action(&hit_map, PhysicalPosition::new(5.0, 10.0)),
            None
        );
    }

    #[test]
    fn resize_hit_regions_cover_every_edge_and_corner() {
        let size = winit::dpi::PhysicalSize::new(640, 480);
        for (position, expected) in [
            ((0.0, 0.0), ResizeDirection::NorthWest),
            ((639.0, 0.0), ResizeDirection::NorthEast),
            ((0.0, 479.0), ResizeDirection::SouthWest),
            ((639.0, 479.0), ResizeDirection::SouthEast),
            ((0.0, 240.0), ResizeDirection::West),
            ((639.0, 240.0), ResizeDirection::East),
            ((320.0, 0.0), ResizeDirection::North),
            ((320.0, 479.0), ResizeDirection::South),
        ] {
            assert_eq!(
                resize_direction_at(size, 1.0, PhysicalPosition::new(position.0, position.1)),
                Some(expected)
            );
        }
        assert_eq!(
            resize_direction_at(size, 1.0, PhysicalPosition::new(320.0, 240.0)),
            None
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resize_hit_regions_overlap_the_windows_padded_frame() {
        let size = winit::dpi::PhysicalSize::new(640, 480);
        assert_eq!(
            resize_direction_at(size, 1.0, PhysicalPosition::new(8.0, 471.0)),
            Some(ResizeDirection::SouthWest)
        );
        assert_eq!(
            resize_direction_at(size, 1.0, PhysicalPosition::new(631.0, 471.0)),
            Some(ResizeDirection::SouthEast)
        );
    }

    #[test]
    fn settings_menu_items_and_dismissal_are_deterministic() {
        let hit_map = ChromeHitMap {
            gear: PhysicalRect {
                x: 300,
                y: 0,
                width: 34,
                height: 35,
            },
            settings_menu: PhysicalRect {
                x: 144,
                y: 35,
                width: 190,
                height: 102,
            },
            settings_items: vec![
                PhysicalRect {
                    x: 144,
                    y: 35,
                    width: 190,
                    height: 34,
                },
                PhysicalRect {
                    x: 144,
                    y: 69,
                    width: 190,
                    height: 34,
                },
                PhysicalRect {
                    x: 144,
                    y: 103,
                    width: 190,
                    height: 34,
                },
            ],
            ..ChromeHitMap::default()
        };
        assert_eq!(
            settings_menu_click(&hit_map, PhysicalPosition::new(150.0, 40.0)),
            SettingsMenuClick::Item(0)
        );
        assert_eq!(
            settings_menu_click(&hit_map, PhysicalPosition::new(150.0, 75.0)),
            SettingsMenuClick::Item(1)
        );
        assert_eq!(
            settings_menu_click(&hit_map, PhysicalPosition::new(150.0, 110.0)),
            SettingsMenuClick::Item(2)
        );
        assert_eq!(
            settings_menu_click(&hit_map, PhysicalPosition::new(310.0, 10.0)),
            SettingsMenuClick::Gear
        );
        assert_eq!(
            settings_menu_click(&hit_map, PhysicalPosition::new(20.0, 200.0)),
            SettingsMenuClick::Outside
        );
    }

    #[test]
    fn embedded_pointer_coordinates_are_relative_to_the_owner_rect() {
        let rect = PhysicalRect {
            x: 220,
            y: 35,
            width: 400,
            height: 300,
        };
        assert_eq!(
            translated_pane_position(rect, PhysicalPosition::new(245.5, 80.25)),
            PhysicalPosition::new(25.5, 45.25)
        );
        // Pointer capture intentionally preserves coordinates outside the pane bounds.
        assert_eq!(
            translated_pane_position(rect, PhysicalPosition::new(700.0, 500.0)),
            PhysicalPosition::new(480.0, 465.0)
        );
    }
}
