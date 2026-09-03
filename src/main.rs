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
    ChromeHitMap, ChromeLayout, ChromeRenderState, ChromeRenderer, ContextMenuRenderState,
    RenameEditorRenderState, RenameEditorRenderer, SettingsMenuRenderer, SidebarMode,
    compute_chrome_layout, rename_editor_logical_size, settings_menu_logical_size,
};
use clap::{Args, Parser, Subcommand};
use layout::{Axis, PhysicalRect, compute_rects, resize_pane};
use model::{
    PaneId, PaneKey, TabId, Workspace, WorkspaceId, focus_owned_pane, names_equal, normalize_name,
    remove_owned_pane, unique_name, workspace_label,
};
use platform::{
    NativePaneHost, PaneHost, RESIZE_EDGE_LOGICAL, configure_chrome_window, configure_event_loop,
    finalize_chrome_window, focus_chrome_input, position_rename_editor, position_settings_menu,
    rename_editor_window_attributes, settings_menu_window_attributes,
};
use vivido::cli::{
    IpcSignalName, ListOptions, MessageOptions, SocketMessage, TerminalOptions, WindowOptions,
};
use vivido::config::UiConfig;
use vivido::config::window::Decorations;
use vivido::display::renderer::EmbeddedFramePlacement;
use vivido::host::{IoListener, MethodCapability, MethodClass, RegistryGuard, SessionPaths};
use vivido::shell::ShellAction;
use vivido::{Event, Processor};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
#[cfg(target_os = "linux")]
use winit::event::TouchPhase;
use winit::event::{ElementState, Event as WinitEvent, Ime, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, ResizeDirection, Window, WindowId};

const CHROME_TITLE: &str = "vivida";
const INITIAL_WIDTH: f64 = 1100.0;
const INITIAL_HEIGHT: f64 = 700.0;

#[derive(Debug, Parser)]
#[command(
    name = "vivida",
    version,
    about = "Native workspace shell embedding Vivido terminals"
)]
struct VividaOptions {
    #[command(subcommand)]
    command: Option<VividaCommand>,

    /// Terminal process and arguments used for every pane.
    #[command(flatten)]
    terminal_options: TerminalOptions,
}

#[derive(Debug, Subcommand)]
enum VividaCommand {
    /// Control a running Vivida instance.
    Msg(Box<VividaMessageOptions>),

    /// List discoverable Vivido and Vivida instances.
    List(ListOptions),
}

#[derive(Args, Debug)]
struct VividaMessageOptions {
    /// IPC endpoint override (a filesystem path on Unix, a named pipe on Windows).
    #[arg(short, long)]
    socket: Option<PathBuf>,

    /// Exact registered Vivida instance name.
    #[arg(short = 't', long, conflicts_with = "socket")]
    target: Option<String>,

    #[command(subcommand)]
    message: VividaMessage,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum VividaMessage {
    /// Use any standard Vivido terminal automation command.
    #[command(flatten)]
    Vivido(SocketMessage),

    /// Return the complete Vivida workspace, tab, split, and pane layout.
    Layout(LayoutOptions),

    /// Navigate between panes in a workspace and tab.
    ResolvePane(ResolvePaneOptions),

    /// Select and reveal a pane without requesting operating-system focus.
    ActivatePane(WindowTarget),

    /// Create a workspace and its initial terminal pane.
    CreateWorkspace(CreateWorkspaceOptions),

    /// Create a tab in the caller's workspace, or the sole workspace.
    CreateTab(CreateTabOptions),

    /// Split a terminal pane.
    SplitPane(SplitPaneOptions),

    /// Close one terminal pane.
    ClosePane(WindowTarget),

    /// Close a tab and all of its terminal panes.
    CloseTab(TabTarget),

    /// Close a workspace and all of its terminal panes.
    CloseWorkspace(WorkspaceTarget),

    /// Rename a workspace.
    RenameWorkspace(RenameWorkspaceOptions),

    /// Pin a tab to a custom title.
    RenameTab(RenameTabOptions),

    /// Resume context-driven naming for a tab.
    ResetTabTitle(TabTarget),
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct LayoutOptions {
    /// Identify this pane in the returned layout.
    #[arg(long, env = "VIVIDO_WINDOW_ID")]
    window_id: Option<u64>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct ResolvePaneOptions {
    /// Calling pane used to supply the default workspace and tab.
    #[arg(
        long = "from-window-id",
        visible_alias = "window-id",
        env = "VIVIDO_WINDOW_ID"
    )]
    from_window_id: Option<u64>,

    /// One-based workspace position in the sidebar.
    #[arg(long, conflicts_with = "workspace_id", value_parser = clap::value_parser!(u64).range(1..))]
    workspace: Option<u64>,

    /// Stable workspace ID instead of its displayed position.
    #[arg(long)]
    workspace_id: Option<u64>,

    /// Case-insensitive workspace name.
    #[arg(long, conflicts_with_all = ["workspace", "workspace_id"])]
    workspace_name: Option<String>,

    /// One-based tab position in the selected workspace.
    #[arg(long, conflicts_with = "tab_id", value_parser = clap::value_parser!(u64).range(1..))]
    tab: Option<u64>,

    /// Stable tab ID instead of its displayed position.
    #[arg(long)]
    tab_id: Option<u64>,

    /// Case-insensitive tab name in the selected workspace.
    #[arg(long, conflicts_with_all = ["tab", "tab_id"])]
    tab_name: Option<String>,

    /// Stable pane ID to resolve directly.
    #[arg(long, conflicts_with_all = ["from_pane_id", "path"])]
    pane_id: Option<u64>,

    /// Stable pane ID to start from instead of the caller or focused pane.
    #[arg(long)]
    from_pane_id: Option<u64>,

    /// Comma-separated directional route from the starting pane.
    #[arg(long, value_enum, value_delimiter = ',')]
    path: Vec<PaneDirection>,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct CreateWorkspaceOptions {
    /// Persistent workspace name. Defaults to a unique directory basename.
    #[arg(long)]
    name: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    options: WindowOptions,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct CreateTabOptions {
    #[arg(long, conflicts_with = "workspace_name")]
    workspace_id: Option<u64>,
    #[arg(long, conflicts_with = "workspace_id")]
    workspace_name: Option<String>,
    #[arg(long = "from-window-id", env = "VIVIDO_WINDOW_ID")]
    from_window_id: Option<u64>,
    /// Persistent tab name. Without this, the terminal context controls the title.
    #[arg(long)]
    name: Option<String>,
    #[command(flatten)]
    options: WindowOptions,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct SplitPaneOptions {
    #[arg(long, env = "VIVIDO_WINDOW_ID")]
    window_id: u64,
    #[arg(long, value_enum)]
    axis: SplitAxis,
    #[command(flatten)]
    options: WindowOptions,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct WindowTarget {
    #[arg(long, env = "VIVIDO_WINDOW_ID")]
    window_id: u64,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct TabTarget {
    #[arg(long, conflicts_with = "workspace_name")]
    workspace_id: Option<u64>,
    #[arg(long, conflicts_with = "workspace_id")]
    workspace_name: Option<String>,
    #[arg(long, conflicts_with = "tab_name")]
    tab_id: Option<u64>,
    #[arg(long, conflicts_with = "tab_id")]
    tab_name: Option<String>,
    #[arg(long = "from-window-id", env = "VIVIDO_WINDOW_ID")]
    from_window_id: Option<u64>,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct WorkspaceTarget {
    #[arg(long, conflicts_with = "workspace_name")]
    workspace_id: Option<u64>,
    #[arg(long, conflicts_with = "workspace_id")]
    workspace_name: Option<String>,
    #[arg(long = "from-window-id", env = "VIVIDO_WINDOW_ID")]
    from_window_id: Option<u64>,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct RenameWorkspaceOptions {
    #[command(flatten)]
    #[serde(flatten)]
    target: WorkspaceTarget,
    #[arg(long)]
    name: String,
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
struct RenameTabOptions {
    #[command(flatten)]
    #[serde(flatten)]
    target: TabTarget,
    #[arg(long)]
    name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameTarget {
    Workspace(WorkspaceId),
    Tab {
        workspace_id: WorkspaceId,
        tab_id: TabId,
    },
}

#[derive(Clone, Copy, Debug)]
struct NameContextMenu {
    target: NameTarget,
    anchor: PhysicalPosition<f64>,
}

#[derive(Clone, Copy, Debug)]
struct RecoveryMenu {
    pane: WindowId,
    anchor: PhysicalPosition<f64>,
}

#[derive(Clone, Debug)]
struct NameEditor {
    target: NameTarget,
    text: String,
    cursor: usize,
    preedit: String,
    select_all: bool,
    error: Option<String>,
}

impl NameEditor {
    fn new(target: NameTarget, text: String) -> Self {
        let cursor = text.len();
        Self {
            target,
            text,
            cursor,
            preedit: String::new(),
            select_all: true,
            error: None,
        }
    }

    fn display_value(&self) -> String {
        let mut display = self.text.clone();
        if self.select_all {
            display = format!("[{display}]");
            display.push('|');
        } else {
            display.insert_str(self.cursor, &self.preedit);
            display.insert(self.cursor + self.preedit.len(), '|');
        }
        display
    }

    fn insert(&mut self, value: &str) {
        if self.select_all {
            self.text.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        let remaining = model::MAX_NAME_CHARS.saturating_sub(self.text.chars().count());
        let value = value
            .chars()
            .filter(|character| !character.is_control())
            .take(remaining)
            .collect::<String>();
        self.text.insert_str(self.cursor, &value);
        self.cursor += value.len();
        self.error = None;
    }

    fn backspace(&mut self) {
        if self.select_all {
            self.text.clear();
            self.cursor = 0;
            self.select_all = false;
        } else if self.cursor > 0 {
            let previous = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index);
            self.text.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
        self.error = None;
    }

    fn delete(&mut self) {
        if self.select_all {
            self.text.clear();
            self.cursor = 0;
            self.select_all = false;
        } else if let Some(character) = self.text[self.cursor..].chars().next() {
            self.text
                .replace_range(self.cursor..self.cursor + character.len_utf8(), "");
        }
        self.error = None;
    }

    fn move_left(&mut self) {
        if self.select_all {
            self.select_all = false;
            self.cursor = 0;
        } else if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index);
        }
    }

    fn move_right(&mut self) {
        if self.select_all {
            self.select_all = false;
            self.cursor = self.text.len();
        } else if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }
}

struct Shell {
    config: UiConfig,
    _terminfo: vivido::tty::TerminfoGuard,
    terminal_options: TerminalOptions,
    processor: Processor,
    _automation_registry: RegistryGuard,
    chrome_window: Option<Arc<Window>>,
    chrome_renderer: Option<ChromeRenderer>,
    chrome_id: Option<WindowId>,
    settings_menu_window: Option<Arc<Window>>,
    settings_menu_renderer: Option<SettingsMenuRenderer>,
    settings_menu_id: Option<WindowId>,
    settings_menu_embedded_size: Option<winit::dpi::PhysicalSize<u32>>,
    rename_editor_window: Option<Arc<Window>>,
    rename_editor_renderer: Option<RenameEditorRenderer>,
    rename_editor_id: Option<WindowId>,
    chrome_layout: ChromeLayout,
    chrome_hits: ChromeHitMap,
    cursor_position: Option<PhysicalPosition<f64>>,
    modifiers: ModifiersState,
    sidebar_mode: SidebarMode,
    hovered_workspace: Option<WorkspaceId>,
    settings_menu_open: bool,
    name_context_menu: Option<NameContextMenu>,
    recovery_menu: Option<RecoveryMenu>,
    name_editor: Option<NameEditor>,
    workspaces: Vec<Workspace>,
    active_workspace: Option<WorkspaceId>,
    next_workspace_id: u64,
    pane_index: HashMap<WindowId, PaneKey>,
    pane_rects: HashMap<WindowId, PhysicalRect>,
    closing_workspaces: HashSet<WorkspaceId>,
    launch_cwd: PathBuf,
    session_path: Option<PathBuf>,
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
        let mut config = load_shell_config(&mut options);
        config.ipc_socket = Some(true);

        let automation_name = format!("vivida-{}", std::process::id());
        let automation_paths = SessionPaths::for_session(&automation_name)?;
        automation_paths.prepare_endpoint(&automation_name)?;
        options.automation_name = Some(automation_name.clone());
        options.socket = Some(automation_paths.socket.clone());

        let terminfo = vivido::tty::setup_env();
        let _listener = IoListener::spawn(
            &config,
            &options,
            vivido::EventSink::Winit(event_loop.create_proxy()),
        )?;
        let automation_registry = automation_paths.register(&automation_name, (1, 1), false)?;
        let mut processor = Processor::new(config.clone(), options, event_loop);
        processor.claim_ipc_method_capabilities(&[
            MethodCapability::host("create_window", MethodClass::Window, true),
            MethodCapability::host("vivida_layout", MethodClass::Observe, false),
            MethodCapability::host("vivida_resolve_pane", MethodClass::Observe, false),
            MethodCapability::host("vivida_activate_pane", MethodClass::Window, true),
            MethodCapability::host("vivida_create_workspace", MethodClass::Window, true),
            MethodCapability::host("vivida_create_tab", MethodClass::Window, true),
            MethodCapability::host("vivida_split_pane", MethodClass::Window, true),
            MethodCapability::host("vivida_close_pane", MethodClass::Window, true),
            MethodCapability::host("vivida_close_tab", MethodClass::Window, true),
            MethodCapability::host("vivida_close_workspace", MethodClass::Window, true),
            MethodCapability::host("vivida_rename_workspace", MethodClass::Window, true),
            MethodCapability::host("vivida_rename_tab", MethodClass::Window, true),
            MethodCapability::host("vivida_reset_tab_title", MethodClass::Window, true),
        ]);
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
            _automation_registry: automation_registry,
            chrome_window: None,
            chrome_renderer: None,
            chrome_id: None,
            settings_menu_window: None,
            settings_menu_renderer: None,
            settings_menu_id: None,
            settings_menu_embedded_size: None,
            rename_editor_window: None,
            rename_editor_renderer: None,
            rename_editor_id: None,
            chrome_layout: ChromeLayout::default(),
            chrome_hits: ChromeHitMap::default(),
            cursor_position: None,
            modifiers: ModifiersState::empty(),
            sidebar_mode: SidebarMode::default(),
            hovered_workspace: None,
            settings_menu_open: false,
            name_context_menu: None,
            recovery_menu: None,
            name_editor: None,
            workspaces: Vec::new(),
            active_workspace: None,
            next_workspace_id: 1,
            pane_index: HashMap::new(),
            pane_rects: HashMap::new(),
            closing_workspaces: HashSet::new(),
            launch_cwd,
            session_path: session::path(),
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
        self.initialize_rename_editor(event_loop)?;

        if let Some(chrome) = &self.chrome_window {
            chrome.set_visible(true);
        }
        self.request_chrome_redraw();
        self.focus_active_pane();
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
                        saved_tab.custom_title,
                        saved_tab.focused_pane,
                        panes,
                    ));
                }
            }
            if !tabs.is_empty() {
                self.next_workspace_id = self.next_workspace_id.max(saved_workspace.id.0 + 1);
                let label = unique_name(
                    &model::automatic_name(&saved_workspace.label, "workspace"),
                    self.workspaces
                        .iter()
                        .map(|workspace| workspace.label.as_str()),
                );
                let mut workspace = Workspace::restore(
                    saved_workspace.id,
                    label,
                    saved_workspace.identity_cwd,
                    tabs,
                    saved_workspace.active_tab,
                );
                workspace.refresh_tab_display_titles();
                self.workspaces.push(workspace);
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

    fn create_pane_window_with_options(
        &mut self,
        event_loop: &ActiveEventLoop,
        options: WindowOptions,
    ) -> Result<WindowId, Box<dyn Error>> {
        let chrome = Arc::clone(
            self.chrome_window
                .as_ref()
                .ok_or("chrome window is not initialized")?,
        );
        NativePaneHost::new(chrome).create_pane_with_options(
            &mut self.processor,
            event_loop,
            options,
        )
    }

    fn platform_window_id(&self, ipc_window_id: u64) -> Option<WindowId> {
        self.pane_index.keys().copied().find(|window_id| {
            self.processor
                .window(*window_id)
                .is_some_and(|pane| pane.ipc_window_id() == ipc_window_id)
        })
    }

    fn caller_key(
        &self,
        ipc_window_id: Option<u64>,
    ) -> Result<Option<PaneKey>, vivido::host::IpcError> {
        let Some(ipc_window_id) = ipc_window_id else {
            return Ok(None);
        };
        self.platform_window_id(ipc_window_id)
            .and_then(|window_id| self.pane_index.get(&window_id).copied())
            .map(Some)
            .ok_or_else(|| {
                vivido::host::IpcError::new(
                    "window_not_found",
                    "calling pane is not part of this Vivida instance",
                )
            })
    }

    fn resolve_workspace_id(
        &self,
        workspace_id: Option<u64>,
        workspace_name: Option<&str>,
        workspace_ordinal: Option<u64>,
        caller: Option<PaneKey>,
    ) -> Result<WorkspaceId, vivido::host::IpcError> {
        if [
            workspace_id.is_some(),
            workspace_name.is_some(),
            workspace_ordinal.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count()
            > 1
        {
            return Err(vivido::host::IpcError::new(
                "invalid_params",
                "workspace selectors are mutually exclusive",
            ));
        }
        let matched = if let Some(id) = workspace_id {
            self.workspaces
                .iter()
                .find(|workspace| workspace.id == WorkspaceId(id))
        } else if let Some(name) = workspace_name {
            let name = normalize_name(name).map_err(|error| {
                vivido::host::IpcError::new("invalid_params", error.to_string())
            })?;
            self.workspaces
                .iter()
                .find(|workspace| names_equal(&workspace.label, &name))
        } else if let Some(ordinal) = workspace_ordinal {
            ordinal_index(ordinal).and_then(|index| self.workspaces.get(index))
        } else if let Some(caller) = caller {
            self.workspaces
                .iter()
                .find(|workspace| workspace.id == caller.workspace_id)
        } else if self.workspaces.len() == 1 {
            self.workspaces.first()
        } else {
            return Err(vivido::host::IpcError::new(
                "ambiguous_selector",
                "workspace must be specified when more than one workspace exists",
            ));
        };
        matched.map(|workspace| workspace.id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "workspace selector did not match")
        })
    }

    fn resolve_tab_id(
        &self,
        workspace_id: WorkspaceId,
        tab_id: Option<u64>,
        tab_name: Option<&str>,
        tab_ordinal: Option<u64>,
        caller: Option<PaneKey>,
        use_active_default: bool,
    ) -> Result<TabId, vivido::host::IpcError> {
        if [tab_id.is_some(), tab_name.is_some(), tab_ordinal.is_some()]
            .into_iter()
            .filter(|selected| *selected)
            .count()
            > 1
        {
            return Err(vivido::host::IpcError::new(
                "invalid_params",
                "tab selectors are mutually exclusive",
            ));
        }
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        let matched = if let Some(id) = tab_id {
            workspace.tabs.iter().find(|tab| tab.id == TabId(id))
        } else if let Some(name) = tab_name {
            let name = normalize_name(name).map_err(|error| {
                vivido::host::IpcError::new("invalid_params", error.to_string())
            })?;
            workspace
                .tabs
                .iter()
                .find(|tab| names_equal(&tab.title, &name))
        } else if let Some(ordinal) = tab_ordinal {
            ordinal_index(ordinal).and_then(|index| workspace.tabs.get(index))
        } else if let Some(caller) = caller.filter(|caller| caller.workspace_id == workspace_id) {
            workspace.tabs.iter().find(|tab| tab.id == caller.tab_id)
        } else if workspace.tabs.len() == 1 {
            workspace.tabs.first()
        } else if use_active_default {
            workspace
                .tabs
                .iter()
                .find(|tab| tab.id == workspace.active_tab)
        } else {
            return Err(vivido::host::IpcError::new(
                "ambiguous_selector",
                "tab must be specified when more than one tab exists",
            ));
        };
        matched.map(|tab| tab.id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "tab selector did not match")
        })
    }

    fn validate_unique_workspace_name(
        &self,
        name: &str,
        except: Option<WorkspaceId>,
    ) -> Result<String, vivido::host::IpcError> {
        let name = normalize_name(name)
            .map_err(|error| vivido::host::IpcError::new("invalid_params", error.to_string()))?;
        if self
            .workspaces
            .iter()
            .any(|workspace| Some(workspace.id) != except && names_equal(&workspace.label, &name))
        {
            return Err(vivido::host::IpcError::new(
                "name_conflict",
                "workspace name is already in use",
            ));
        }
        Ok(name)
    }

    fn validate_unique_tab_name(
        &self,
        workspace_id: WorkspaceId,
        name: &str,
        except: Option<TabId>,
    ) -> Result<String, vivido::host::IpcError> {
        let name = normalize_name(name)
            .map_err(|error| vivido::host::IpcError::new("invalid_params", error.to_string()))?;
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        if workspace
            .tabs
            .iter()
            .any(|tab| Some(tab.id) != except && names_equal(&tab.title, &name))
        {
            return Err(vivido::host::IpcError::new(
                "name_conflict",
                "tab name is already in use in this workspace",
            ));
        }
        Ok(name)
    }

    fn create_workspace(
        &mut self,
        event_loop: &ActiveEventLoop,
        cwd: PathBuf,
    ) -> Result<WorkspaceId, Box<dyn Error>> {
        let pane_window_id = self.create_pane_window(event_loop, &cwd)?;
        let workspace_id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let label = unique_name(
            &workspace_label(&cwd),
            self.workspaces
                .iter()
                .map(|workspace| workspace.label.as_str()),
        );
        let workspace = Workspace::new(workspace_id, label, cwd, pane_window_id);
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
        workspace.refresh_tab_display_titles();
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

    fn creation_result(
        &self,
        window_id: WindowId,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let key = self.pane_index.get(&window_id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "new pane disappeared")
        })?;
        let ipc_window_id = self
            .processor
            .window(window_id)
            .map(|pane| pane.ipc_window_id())
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "new pane disappeared")
            })?;
        Ok(serde_json::json!({
            "workspace_id": key.workspace_id.0,
            "tab_id": key.tab_id.0,
            "pane_id": key.pane_id.0,
            "window_id": ipc_window_id,
        }))
    }

    fn host_create_workspace(
        &mut self,
        event_loop: &ActiveEventLoop,
        params: CreateWorkspaceOptions,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let CreateWorkspaceOptions { name, mut options } = params;
        let cwd = options
            .terminal_options
            .working_directory
            .clone()
            .unwrap_or_else(|| self.active_pane_cwd());
        options.terminal_options.working_directory = Some(cwd.clone());
        let label = if let Some(name) = name {
            self.validate_unique_workspace_name(&name, None)?
        } else {
            unique_name(
                &workspace_label(&cwd),
                self.workspaces
                    .iter()
                    .map(|workspace| workspace.label.as_str()),
            )
        };
        let window_id = self
            .create_pane_window_with_options(event_loop, options)
            .map_err(|error| vivido::host::IpcError::new("invalid_params", error.to_string()))?;
        let workspace_id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id = self.next_workspace_id.saturating_add(1);
        self.workspaces
            .push(Workspace::new(workspace_id, label, cwd, window_id));
        self.active_workspace = Some(workspace_id);
        self.closing_workspaces.remove(&workspace_id);
        self.pane_index.insert(
            window_id,
            PaneKey {
                workspace_id,
                tab_id: TabId(1),
                pane_id: PaneId(1),
            },
        );
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
        self.creation_result(window_id)
    }

    fn host_create_tab(
        &mut self,
        event_loop: &ActiveEventLoop,
        params: CreateTabOptions,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let CreateTabOptions {
            workspace_id,
            workspace_name,
            from_window_id,
            name,
            mut options,
        } = params;
        let caller = self.caller_key(from_window_id)?;
        let workspace_id =
            self.resolve_workspace_id(workspace_id, workspace_name.as_deref(), None, caller)?;
        let custom_title = name
            .as_deref()
            .map(|name| self.validate_unique_tab_name(workspace_id, name, None))
            .transpose()?;
        let default_cwd = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.identity_cwd.clone())
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        if options.terminal_options.working_directory.is_none() {
            options.terminal_options.working_directory = Some(default_cwd);
        }
        let window_id = self
            .create_pane_window_with_options(event_loop, options)
            .map_err(|error| vivido::host::IpcError::new("invalid_params", error.to_string()))?;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace disappeared")
            })?;
        let tab_id = workspace.add_tab(window_id);
        if let Some(title) = custom_title
            && let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id)
        {
            tab.set_custom_title(title);
        }
        workspace.refresh_tab_display_titles();
        self.active_workspace = Some(workspace_id);
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
        self.creation_result(window_id)
    }

    fn host_split_pane(
        &mut self,
        event_loop: &ActiveEventLoop,
        params: SplitPaneOptions,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let target = self.platform_window_id(params.window_id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "target pane not found")
        })?;
        let key = *self.pane_index.get(&target).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "target pane not found")
        })?;
        let cwd = self
            .processor
            .window(target)
            .and_then(|pane| pane.current_directory())
            .unwrap_or_else(|| self.launch_cwd.clone());
        let mut options = params.options;
        if options.terminal_options.working_directory.is_none() {
            options.terminal_options.working_directory = Some(cwd);
        }
        let window_id = self
            .create_pane_window_with_options(event_loop, options)
            .map_err(|error| vivido::host::IpcError::new("invalid_params", error.to_string()))?;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == key.workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace disappeared")
            })?;
        workspace.active_tab = key.tab_id;
        let tab = workspace
            .tabs
            .iter_mut()
            .find(|tab| tab.id == key.tab_id)
            .ok_or_else(|| vivido::host::IpcError::new("window_not_found", "tab disappeared"))?;
        tab.focused_pane = key.pane_id;
        let axis = match params.axis {
            SplitAxis::Horizontal => Axis::Horizontal,
            SplitAxis::Vertical => Axis::Vertical,
        };
        let pane_id = tab.split(axis, window_id).ok_or_else(|| {
            vivido::host::IpcError::new("invalid_state", "target pane cannot be split")
        })?;
        self.active_workspace = Some(key.workspace_id);
        self.pane_index.insert(
            window_id,
            PaneKey {
                workspace_id: key.workspace_id,
                tab_id: key.tab_id,
                pane_id,
            },
        );
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        self.request_chrome_redraw();
        self.creation_result(window_id)
    }

    fn request_close_panes(&mut self, panes: &[WindowId]) -> Result<(), vivido::host::IpcError> {
        for window_id in panes {
            let pane = self.processor.window_mut(*window_id).ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "target pane not found")
            })?;
            pane.set_automation_visible(false);
            pane.signal_process_group(pane_close_signal())
                .map_err(|error| {
                    vivido::host::IpcError::new(
                        "invalid_state",
                        format!("failed to close pane: {error:?}"),
                    )
                })?;
        }
        Ok(())
    }

    fn host_close_pane(
        &mut self,
        ipc_window_id: u64,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let window_id = self.platform_window_id(ipc_window_id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "target pane not found")
        })?;
        self.request_close_panes(&[window_id])?;
        Ok(serde_json::json!({"accepted": true, "window_id": ipc_window_id}))
    }

    fn host_close_tab(
        &mut self,
        target: TabTarget,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let caller = self.caller_key(target.from_window_id)?;
        let workspace_id = self.resolve_workspace_id(
            target.workspace_id,
            target.workspace_name.as_deref(),
            None,
            caller,
        )?;
        let tab_id = self.resolve_tab_id(
            workspace_id,
            target.tab_id,
            target.tab_name.as_deref(),
            None,
            caller,
            false,
        )?;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        let panes = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.panes.values().copied().collect::<Vec<_>>())
            .ok_or_else(|| vivido::host::IpcError::new("window_not_found", "tab not found"))?;
        if workspace.active_tab == tab_id
            && let Some(next) = workspace.tabs.iter().find(|tab| tab.id != tab_id)
        {
            workspace.active_tab = next.id;
        }
        self.request_close_panes(&panes)?;
        self.sync_visibility_and_geometry();
        self.focus_active_pane();
        Ok(
            serde_json::json!({"accepted": true, "workspace_id": workspace_id.0, "tab_id": tab_id.0}),
        )
    }

    fn host_rename_workspace(
        &mut self,
        params: RenameWorkspaceOptions,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let caller = self.caller_key(params.target.from_window_id)?;
        let workspace_id = self.resolve_workspace_id(
            params.target.workspace_id,
            params.target.workspace_name.as_deref(),
            None,
            caller,
        )?;
        let name = self.validate_unique_workspace_name(&params.name, Some(workspace_id))?;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        workspace.label.clone_from(&name);
        self.request_chrome_redraw();
        Ok(serde_json::json!({"workspace_id": workspace_id.0, "workspace_name": name}))
    }

    fn host_rename_tab(
        &mut self,
        params: RenameTabOptions,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let caller = self.caller_key(params.target.from_window_id)?;
        let workspace_id = self.resolve_workspace_id(
            params.target.workspace_id,
            params.target.workspace_name.as_deref(),
            None,
            caller,
        )?;
        let tab_id = self.resolve_tab_id(
            workspace_id,
            params.target.tab_id,
            params.target.tab_name.as_deref(),
            None,
            caller,
            false,
        )?;
        let name = self.validate_unique_tab_name(workspace_id, &params.name, Some(tab_id))?;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        let tab = workspace
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| vivido::host::IpcError::new("window_not_found", "tab not found"))?;
        tab.set_custom_title(name.clone());
        workspace.refresh_tab_display_titles();
        self.request_chrome_redraw();
        Ok(serde_json::json!({
            "workspace_id": workspace_id.0,
            "tab_id": tab_id.0,
            "tab_name": name,
            "custom_title": true,
        }))
    }

    fn host_reset_tab_title(
        &mut self,
        target: TabTarget,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        let caller = self.caller_key(target.from_window_id)?;
        let workspace_id = self.resolve_workspace_id(
            target.workspace_id,
            target.workspace_name.as_deref(),
            None,
            caller,
        )?;
        let tab_id = self.resolve_tab_id(
            workspace_id,
            target.tab_id,
            target.tab_name.as_deref(),
            None,
            caller,
            false,
        )?;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        let tab = workspace
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| vivido::host::IpcError::new("window_not_found", "tab not found"))?;
        tab.reset_title();
        workspace.refresh_tab_display_titles();
        let title = workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.title.clone())
            .unwrap_or_default();
        self.request_chrome_redraw();
        Ok(serde_json::json!({
            "workspace_id": workspace_id.0,
            "tab_id": tab_id.0,
            "tab_name": title,
            "custom_title": false,
        }))
    }

    fn host_resolve_pane(
        &self,
        params: ResolvePaneOptions,
    ) -> Result<serde_json::Value, vivido::host::IpcError> {
        if params.pane_id.is_some() && (params.from_pane_id.is_some() || !params.path.is_empty()) {
            return Err(vivido::host::IpcError::new(
                "invalid_params",
                "pane_id cannot be combined with from_pane_id or path",
            ));
        }
        let caller = self.caller_key(params.from_window_id)?;
        let workspace_id = self.resolve_workspace_id(
            params.workspace_id,
            params.workspace_name.as_deref(),
            params.workspace,
            caller,
        )?;
        let tab_id = self.resolve_tab_id(
            workspace_id,
            params.tab_id,
            params.tab_name.as_deref(),
            params.tab,
            caller,
            true,
        )?;
        let (workspace_index, workspace) = self
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, workspace)| workspace.id == workspace_id)
            .ok_or_else(|| {
                vivido::host::IpcError::new("window_not_found", "workspace not found")
            })?;
        let (tab_index, tab) = workspace
            .tabs
            .iter()
            .enumerate()
            .find(|(_, tab)| tab.id == tab_id)
            .ok_or_else(|| vivido::host::IpcError::new("window_not_found", "tab not found"))?;

        let scale = self
            .chrome_window
            .as_ref()
            .map_or(1.0, |chrome| chrome.scale_factor());
        let rects = compute_rects(&tab.root, self.chrome_layout.content, scale);
        let source_pane_id = if let Some(pane_id) = params.pane_id {
            let pane_id = PaneId(pane_id);
            tab.panes.contains_key(&pane_id).then_some(pane_id)
        } else if let Some(pane_id) = params.from_pane_id {
            let pane_id = PaneId(pane_id);
            tab.panes.contains_key(&pane_id).then_some(pane_id)
        } else {
            caller
                .filter(|caller| caller.workspace_id == workspace.id && caller.tab_id == tab.id)
                .map(|caller| caller.pane_id)
                .or(Some(tab.focused_pane))
        }
        .ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "starting pane is not in the tab")
        })?;
        let mut pane_id = source_pane_id;
        let mut steps = Vec::with_capacity(params.path.len());
        for direction in &params.path {
            let next = directional_neighbor(tab, &rects, pane_id, *direction).ok_or_else(|| {
                vivido::host::IpcError::new(
                    "window_not_found",
                    format!("no pane exists {:?} of pane {}", direction, pane_id.0),
                )
            })?;
            steps.push(serde_json::json!({
                "direction": direction,
                "from_pane_id": pane_id.0,
                "to_pane_id": next.0,
            }));
            pane_id = next;
        }
        let window_id = tab.window_id(pane_id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "resolved pane disappeared")
        })?;
        let pane = self.processor.window(window_id).ok_or_else(|| {
            vivido::host::IpcError::new("window_not_found", "resolved pane disappeared")
        })?;
        let rect = rects.get(&pane_id).copied().unwrap_or_default();
        let target = serde_json::json!({
            "pane_id": pane_id.0,
            "locator": {
                "workspace_name": workspace.label,
                "tab_name": tab.title,
                "pane_id": pane_id.0,
            },
            "split_path": tab.root.pane_path(pane_id),
            "window_id": pane.ipc_window_id(),
            "title": pane.title(),
            "working_directory": pane.current_directory(),
            "rect": pane_rect_json(rect),
            "neighbors": pane_neighbors_json(tab, &rects, pane_id, &self.processor),
        });
        Ok(serde_json::json!({
            "schema_version": 1,
            "selector": {
                "workspace": params.workspace,
                "workspace_id": params.workspace_id,
                "workspace_name": params.workspace_name,
                "tab": params.tab,
                "tab_id": params.tab_id,
                "tab_name": params.tab_name,
                "pane_id": params.pane_id,
                "from_pane_id": params.from_pane_id,
                "path": params.path,
            },
            "scope": {
                "workspace_index": workspace_index + 1,
                "workspace_id": workspace.id.0,
                "workspace_label": workspace.label,
                "tab_index": tab_index + 1,
                "tab_id": tab.id.0,
                "tab_title": tab.title,
            },
            "source_pane_id": source_pane_id.0,
            "steps": steps,
            "target": target,
        }))
    }

    fn layout_json(&self, caller_window_id: Option<u64>) -> serde_json::Value {
        let scale = self
            .chrome_window
            .as_ref()
            .map_or(1.0, |chrome| chrome.scale_factor());
        let chrome_visible = self
            .chrome_window
            .as_ref()
            .is_some_and(|chrome| chrome.is_visible() != Some(false));
        let workspaces = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(workspace_index, workspace)| {
                let tabs = workspace
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(tab_index, tab)| {
                        let rects = compute_rects(&tab.root, self.chrome_layout.content, scale);
                        let panes = tab
                            .panes
                            .iter()
                            .filter_map(|(pane_id, window_id)| {
                                let pane = self.processor.window(*window_id)?;
                                let rect = rects.get(pane_id).copied().unwrap_or_default();
                                let hierarchy_visible = self.active_workspace == Some(workspace.id)
                                    && workspace.active_tab == tab.id
                                    && !self.closing_workspaces.contains(&workspace.id);
                                Some(serde_json::json!({
                                    "pane_id": pane_id.0,
                                    "locator": {
                                        "workspace_name": workspace.label,
                                        "tab_name": tab.title,
                                        "pane_id": pane_id.0,
                                    },
                                    "split_path": tab.root.pane_path(*pane_id),
                                    "window_id": pane.ipc_window_id(),
                                    "is_caller": caller_window_id == Some(pane.ipc_window_id()),
                                    "title": pane.title(),
                                    "working_directory": pane.current_directory(),
                                    "focused": pane.is_focused(),
                                    "os_focused": pane.is_focused(),
                                    "selected_in_host": hierarchy_visible
                                        && tab.focused_pane == *pane_id,
                                    "visible": hierarchy_visible && chrome_visible
                                        && pane.display.window.is_visible() != Some(false),
                                    "rect": pane_rect_json(rect),
                                    "neighbors": pane_neighbors_json(
                                        tab,
                                        &rects,
                                        *pane_id,
                                        &self.processor,
                                    ),
                                }))
                            })
                            .collect::<Vec<_>>();
                        serde_json::json!({
                        "tab_id": tab.id.0,
                            "tab_index": tab_index + 1,
                        "title": tab.title,
                        "custom_title": tab.is_title_custom(),
                            "active": workspace.active_tab == tab.id,
                            "focused_pane_id": tab.focused_pane.0,
                            "layout": layout_node_json(&tab.root, tab, &self.processor),
                            "panes": panes,
                        })
                    })
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "workspace_id": workspace.id.0,
                    "workspace_index": workspace_index + 1,
                    "label": workspace.label,
                    "working_directory": workspace.identity_cwd,
                    "active": self.active_workspace == Some(workspace.id),
                    "active_tab_id": workspace.active_tab.0,
                    "closing": self.closing_workspaces.contains(&workspace.id),
                    "tabs": tabs,
                })
            })
            .collect::<Vec<_>>();
        let caller = caller_window_id.map(|ipc_window_id| {
            let located = self
                .platform_window_id(ipc_window_id)
                .and_then(|window_id| {
                    let key = self.pane_index.get(&window_id)?;
                    let workspace_index = self
                        .workspaces
                        .iter()
                        .position(|workspace| workspace.id == key.workspace_id)?;
                    let workspace = &self.workspaces[workspace_index];
                    let tab_index = workspace.tabs.iter().position(|tab| tab.id == key.tab_id)?;
                    let tab = &workspace.tabs[tab_index];
                    let rects = compute_rects(&tab.root, self.chrome_layout.content, scale);
                    let rect = rects.get(&key.pane_id).copied().unwrap_or_default();
                    Some(serde_json::json!({
                        "located": true,
                        "window_id": ipc_window_id,
                        "workspace_index": workspace_index + 1,
                        "workspace_id": key.workspace_id.0,
                        "workspace_name": workspace.label,
                        "tab_index": tab_index + 1,
                        "tab_id": key.tab_id.0,
                        "tab_name": tab.title,
                        "pane_id": key.pane_id.0,
                        "split_path": tab.root.pane_path(key.pane_id),
                        "selected_in_host": self.active_workspace == Some(key.workspace_id)
                            && workspace.active_tab == key.tab_id
                            && tab.focused_pane == key.pane_id,
                        "visible": self.active_workspace == Some(key.workspace_id)
                            && workspace.active_tab == key.tab_id
                            && chrome_visible,
                        "rect": pane_rect_json(rect),
                        "neighbors": pane_neighbors_json(
                            tab,
                            &rects,
                            key.pane_id,
                            &self.processor,
                        ),
                    }))
                });
            located.unwrap_or_else(|| {
                serde_json::json!({
                    "located": false,
                    "window_id": ipc_window_id,
                })
            })
        });
        serde_json::json!({
            "schema_version": 1,
            "caller": caller,
            "active_workspace_id": self.active_workspace.map(|id| id.0),
            "workspaces": workspaces,
        })
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

    fn resize_active_pane_layout(
        &mut self,
        window_id: WindowId,
        width: u32,
        height: u32,
    ) -> Option<PhysicalRect> {
        let key = self.pane_index.get(&window_id).copied()?;
        let scale = self.chrome_window.as_ref()?.scale_factor();
        let content = self.chrome_layout.content;
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == key.workspace_id)?;
        let tab = workspace.tabs.iter_mut().find(|tab| tab.id == key.tab_id)?;
        resize_pane(&mut tab.root, key.pane_id, width, height, content, scale)?;
        self.sync_pane_geometry();
        self.pane_rects.get(&window_id).copied()
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

    fn select_pane(&mut self, window_id: WindowId) -> bool {
        let Some(key) = self.pane_index.get(&window_id).copied() else {
            return false;
        };
        let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == key.workspace_id)
        else {
            return false;
        };
        let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == key.tab_id) else {
            return false;
        };
        if tab.window_id(key.pane_id) != Some(window_id) {
            return false;
        }
        self.active_workspace = Some(key.workspace_id);
        workspace.active_tab = key.tab_id;
        tab.focused_pane = key.pane_id;
        self.sync_visibility_and_geometry();
        self.request_chrome_redraw();
        true
    }

    fn activate_pane(&mut self, window_id: WindowId) -> bool {
        if !self.select_pane(window_id) {
            return false;
        }
        self.focus_active_pane();
        true
    }

    fn alternative_tab_pane(&self, source: WindowId) -> Option<WindowId> {
        let source_key = self.pane_index.get(&source)?;
        self.workspaces.iter().find_map(|workspace| {
            workspace.tabs.iter().find_map(|tab| {
                (workspace.id != source_key.workspace_id || tab.id != source_key.tab_id)
                    .then(|| tab.focused_window_id())
                    .flatten()
            })
        })
    }

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
        if ((self.name_context_menu.is_some() || self.recovery_menu.is_some())
            && self
                .chrome_hits
                .context_menu
                .contains(position.x, position.y))
            || (self.name_editor.is_some()
                && self
                    .chrome_hits
                    .rename_editor
                    .contains(position.x, position.y))
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
        let context_menu = self
            .recovery_menu
            .map(|menu| ContextMenuRenderState {
                anchor: menu.anchor,
                automatic_title_action: false,
                terminal_actions: false,
                recovery_actions: true,
            })
            .or_else(|| {
                self.name_context_menu.map(|menu| ContextMenuRenderState {
                    anchor: menu.anchor,
                    automatic_title_action: match menu.target {
                        NameTarget::Workspace(_) => false,
                        NameTarget::Tab {
                            workspace_id,
                            tab_id,
                        } => self
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.id == workspace_id)
                            .and_then(|workspace| {
                                workspace.tabs.iter().find(|tab| tab.id == tab_id)
                            })
                            .is_some_and(model::Tab::is_title_custom),
                    },
                    terminal_actions: matches!(menu.target, NameTarget::Tab { .. }),
                    recovery_actions: false,
                })
            });
        let editor_display = self.name_editor.as_ref().map(NameEditor::display_value);
        let rename_editor = self
            .name_editor
            .as_ref()
            .zip(editor_display.as_deref())
            .filter(|_| self.rename_editor_window.is_none())
            .map(|(editor, display_value)| RenameEditorRenderState {
                label: match editor.target {
                    NameTarget::Workspace(_) => "Rename Workspace",
                    NameTarget::Tab { .. } => "Rename Tab",
                },
                display_value,
                error: editor.error.as_deref(),
            });
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
                context_menu,
                rename_editor,
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

    fn initialize_rename_editor(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), Box<dyn Error>> {
        let Some(chrome) = &self.chrome_window else {
            return Ok(());
        };
        let (width, height) = rename_editor_logical_size();
        let attributes = Window::default_attributes()
            .with_title("vivida rename")
            .with_inner_size(LogicalSize::new(width, height))
            .with_resizable(false)
            .with_visible(false);
        let Some(attributes) = rename_editor_window_attributes(chrome, attributes)? else {
            return Ok(());
        };
        let window = Arc::new(event_loop.create_window(attributes)?);
        let renderer =
            RenameEditorRenderer::new(Arc::clone(&window), &self.config, window.scale_factor())?;
        self.rename_editor_id = Some(window.id());
        self.rename_editor_window = Some(window);
        self.rename_editor_renderer = Some(renderer);
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

    fn name_target_at(&self, position: PhysicalPosition<f64>) -> Option<NameTarget> {
        if let Some(tab_id) = self
            .chrome_hits
            .tab_rows
            .iter()
            .find_map(|(id, rect)| rect.contains(position.x, position.y).then_some(*id))
            && let Some(workspace_id) = self.active_workspace
        {
            return Some(NameTarget::Tab {
                workspace_id,
                tab_id,
            });
        }
        self.chrome_hits
            .workspace_rows
            .iter()
            .find_map(|(id, rect)| {
                rect.contains(position.x, position.y)
                    .then_some(NameTarget::Workspace(*id))
            })
    }

    fn open_name_context_menu(&mut self, target: NameTarget, anchor: PhysicalPosition<f64>) {
        self.close_recovery_menu();
        self.set_settings_menu_open(false);
        self.name_editor = None;
        // Native terminal children sit above the chrome content surface. Keep a tab's primary
        // Rename row wholly inside the tab strip so its click always reaches the chrome, including
        // when the target tab is inactive.
        let anchor = name_context_anchor(target, anchor);
        self.name_context_menu = Some(NameContextMenu { target, anchor });
        self.request_chrome_redraw();
    }

    fn open_recovery_menu(&mut self, pane: WindowId) {
        self.close_recovery_menu();
        self.set_settings_menu_open(false);
        self.name_context_menu = None;
        self.name_editor = None;
        let anchor =
            self.chrome_window
                .as_ref()
                .map_or(PhysicalPosition::new(0.0, 0.0), |window| {
                    let size = window.inner_size();
                    PhysicalPosition::new(f64::from(size.width) / 2.0, f64::from(size.height) / 3.0)
                });
        self.recovery_menu = Some(RecoveryMenu { pane, anchor });
        self.set_native_pane_visibility(pane, false);
        self.request_chrome_redraw();
    }

    fn close_recovery_menu(&mut self) {
        if self.recovery_menu.take().is_some() {
            self.sync_pane_visibility();
            self.focus_active_pane();
            self.request_chrome_redraw();
        }
    }

    fn recover_pane(&mut self, pane: WindowId, restart: bool) {
        let Some(ipc_window_id) = self
            .processor
            .window(pane)
            .map(|window| window.ipc_window_id())
        else {
            self.close_recovery_menu();
            return;
        };
        let result = if restart {
            self.processor.restart_terminal(ipc_window_id)
        } else {
            self.processor.reset_terminal(ipc_window_id).map(|_| ())
        };
        if let Err(error) = result {
            eprintln!("terminal recovery failed: {}", error.message);
        }
        self.close_recovery_menu();
    }

    fn focused_pane_in_tab(&self, workspace_id: WorkspaceId, tab_id: TabId) -> Option<WindowId> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .and_then(|workspace| workspace.tabs.iter().find(|tab| tab.id == tab_id))
            .and_then(|tab| tab.window_id(tab.focused_pane))
    }

    fn start_name_editor(&mut self, target: NameTarget) {
        let current = match target {
            NameTarget::Workspace(workspace_id) => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .map(|workspace| workspace.label.clone()),
            NameTarget::Tab {
                workspace_id,
                tab_id,
            } => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .and_then(|workspace| workspace.tabs.iter().find(|tab| tab.id == tab_id))
                .map(|tab| tab.title.clone()),
        };
        let Some(current) = current else {
            self.name_context_menu = None;
            return;
        };
        self.name_context_menu = None;
        self.name_editor = Some(NameEditor::new(target, current));
        #[cfg(target_os = "linux")]
        self.clear_embedded_focus();
        if let (Some(chrome), Some(editor)) = (&self.chrome_window, &self.rename_editor_window) {
            editor.set_ime_allowed(true);
            editor.set_visible(true);
            let chrome_size = chrome.inner_size();
            let editor_size = editor.inner_size();
            let x = chrome_size.width.saturating_sub(editor_size.width) / 2;
            let y = chrome_size.height.saturating_sub(editor_size.height) / 3;
            position_rename_editor(
                chrome,
                editor,
                PhysicalPosition::new(
                    i32::try_from(x).unwrap_or_default(),
                    i32::try_from(y).unwrap_or_default(),
                ),
            );
            editor.request_redraw();
            editor.focus_window();
        } else if let Some(chrome) = &self.chrome_window {
            chrome.set_ime_allowed(true);
            focus_chrome_input(chrome);
        }
        self.request_chrome_redraw();
    }

    fn close_name_editor(&mut self) {
        if self.name_editor.take().is_none() {
            return;
        }
        if let Some(editor) = &self.rename_editor_window {
            editor.set_ime_allowed(false);
            editor.set_visible(false);
        }
        if let Some(chrome) = &self.chrome_window {
            chrome.set_ime_allowed(false);
        }
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn commit_name_editor(&mut self) {
        let Some(editor) = &self.name_editor else {
            return;
        };
        let target = editor.target;
        let name = editor.text.clone();
        let result = match target {
            NameTarget::Workspace(workspace_id) => self
                .host_rename_workspace(RenameWorkspaceOptions {
                    target: WorkspaceTarget {
                        workspace_id: Some(workspace_id.0),
                        workspace_name: None,
                        from_window_id: None,
                    },
                    name,
                })
                .map(|_| ()),
            NameTarget::Tab {
                workspace_id,
                tab_id,
            } => self
                .host_rename_tab(RenameTabOptions {
                    target: TabTarget {
                        workspace_id: Some(workspace_id.0),
                        workspace_name: None,
                        tab_id: Some(tab_id.0),
                        tab_name: None,
                        from_window_id: None,
                    },
                    name,
                })
                .map(|_| ()),
        };
        match result {
            Ok(()) => self.close_name_editor(),
            Err(error) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.error = Some(error.message);
                }
                self.request_rename_editor_redraw();
                self.request_chrome_redraw();
            }
        }
    }

    fn reset_context_tab_title(&mut self, workspace_id: WorkspaceId, tab_id: TabId) {
        let _ = self.host_reset_tab_title(TabTarget {
            workspace_id: Some(workspace_id.0),
            workspace_name: None,
            tab_id: Some(tab_id.0),
            tab_name: None,
            from_window_id: None,
        });
        self.name_context_menu = None;
        self.focus_active_pane();
        self.request_chrome_redraw();
    }

    fn handle_name_editor_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        if self.name_editor.is_none() || event.state != ElementState::Pressed {
            return false;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => self.commit_name_editor(),
            Key::Named(NamedKey::Escape) => self.close_name_editor(),
            Key::Named(NamedKey::Backspace) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.backspace();
                }
            }
            Key::Named(NamedKey::Delete) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.delete();
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.move_left();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.move_right();
                }
            }
            Key::Named(NamedKey::Home) => {
                let editor = self.name_editor.as_mut().unwrap();
                editor.select_all = false;
                editor.cursor = 0;
            }
            Key::Named(NamedKey::End) => {
                let editor = self.name_editor.as_mut().unwrap();
                editor.select_all = false;
                editor.cursor = editor.text.len();
            }
            Key::Named(NamedKey::Space) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.insert(" ");
                }
            }
            Key::Character(text)
                if shell_modifier_pressed(self.modifiers) && text.eq_ignore_ascii_case("a") =>
            {
                if let Some(editor) = &mut self.name_editor {
                    editor.select_all = true;
                }
            }
            Key::Character(text) if !shell_modifier_pressed(self.modifiers) => {
                if let Some(editor) = &mut self.name_editor {
                    editor.insert(text);
                }
            }
            _ => return true,
        }
        self.request_chrome_redraw();
        self.request_rename_editor_redraw();
        true
    }

    fn handle_name_editor_ime(&mut self, ime: Ime) -> bool {
        let Some(editor) = &mut self.name_editor else {
            return false;
        };
        match ime {
            Ime::Preedit(text, _) => editor.preedit = text,
            Ime::Commit(text) => {
                editor.preedit.clear();
                editor.insert(&text);
            }
            Ime::Enabled | Ime::Disabled => editor.preedit.clear(),
        }
        self.request_chrome_redraw();
        self.request_rename_editor_redraw();
        true
    }

    fn request_rename_editor_redraw(&self) {
        if let Some(window) = &self.rename_editor_window {
            window.request_redraw();
        }
    }

    fn render_rename_editor(&mut self) {
        let (Some(window), Some(renderer), Some(editor)) = (
            &self.rename_editor_window,
            &mut self.rename_editor_renderer,
            &self.name_editor,
        ) else {
            return;
        };
        let display_value = editor.display_value();
        let state = RenameEditorRenderState {
            label: match editor.target {
                NameTarget::Workspace(_) => "Rename Workspace",
                NameTarget::Tab { .. } => "Rename Tab",
            },
            display_value: &display_value,
            error: editor.error.as_deref(),
        };
        match renderer.render(window.inner_size(), state) {
            Ok(true) => (),
            Ok(false) => window.request_redraw(),
            Err(error) => eprintln!("failed to render rename editor: {error}"),
        }
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

    fn drain_host_requests(&mut self, event_loop: &ActiveEventLoop) {
        for request in self.processor.take_host_requests() {
            let result = match request.method.as_str() {
                "create_window" => serde_json::from_value::<WindowOptions>(request.params.clone())
                    .map_err(|error| {
                        vivido::host::IpcError::new("invalid_params", error.to_string())
                    })
                    .and_then(|options| {
                        self.host_create_tab(
                            event_loop,
                            CreateTabOptions {
                                workspace_id: self.active_workspace.map(|id| id.0),
                                workspace_name: None,
                                from_window_id: None,
                                name: None,
                                options,
                            },
                        )
                    }),
                "vivida_layout" => serde_json::from_value::<LayoutOptions>(request.params.clone())
                    .map_err(|error| {
                        vivido::host::IpcError::new("invalid_params", error.to_string())
                    })
                    .map(|params| self.layout_json(params.window_id)),
                "vivida_resolve_pane" => {
                    serde_json::from_value::<ResolvePaneOptions>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|params| self.host_resolve_pane(params))
                }
                "vivida_activate_pane" => {
                    serde_json::from_value::<WindowTarget>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|target| {
                            let window_id =
                                self.platform_window_id(target.window_id).ok_or_else(|| {
                                    vivido::host::IpcError::new(
                                        "window_not_found",
                                        "target pane not found",
                                    )
                                })?;
                            if !self.select_pane(window_id) {
                                return Err(vivido::host::IpcError::new(
                                    "window_not_found",
                                    "target pane not found",
                                ));
                            }
                            Ok(serde_json::json!({
                                "window_id": target.window_id,
                                "selected_in_host": true,
                                "visible": true,
                                "os_focus_requested": false,
                            }))
                        })
                }
                "vivida_create_workspace" => {
                    serde_json::from_value::<CreateWorkspaceOptions>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|options| self.host_create_workspace(event_loop, options))
                }
                "vivida_create_tab" => {
                    serde_json::from_value::<CreateTabOptions>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|params| self.host_create_tab(event_loop, params))
                }
                "vivida_split_pane" => {
                    serde_json::from_value::<SplitPaneOptions>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|params| self.host_split_pane(event_loop, params))
                }
                "vivida_close_pane" => {
                    serde_json::from_value::<WindowTarget>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|target| self.host_close_pane(target.window_id))
                }
                "vivida_close_tab" => serde_json::from_value::<TabTarget>(request.params.clone())
                    .map_err(|error| {
                        vivido::host::IpcError::new("invalid_params", error.to_string())
                    })
                    .and_then(|target| self.host_close_tab(target)),
                "vivida_close_workspace" => serde_json::from_value::<WorkspaceTarget>(
                    request.params.clone(),
                )
                .map_err(|error| vivido::host::IpcError::new("invalid_params", error.to_string()))
                .and_then(|target| {
                    let caller = self.caller_key(target.from_window_id)?;
                    let workspace_id = self.resolve_workspace_id(
                        target.workspace_id,
                        target.workspace_name.as_deref(),
                        None,
                        caller,
                    )?;
                    self.close_workspace(workspace_id);
                    Ok(serde_json::json!({"accepted": true, "workspace_id": workspace_id.0}))
                }),
                "vivida_rename_workspace" => {
                    serde_json::from_value::<RenameWorkspaceOptions>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|params| self.host_rename_workspace(params))
                }
                "vivida_rename_tab" => {
                    serde_json::from_value::<RenameTabOptions>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|params| self.host_rename_tab(params))
                }
                "vivida_reset_tab_title" => {
                    serde_json::from_value::<TabTarget>(request.params.clone())
                        .map_err(|error| {
                            vivido::host::IpcError::new("invalid_params", error.to_string())
                        })
                        .and_then(|target| self.host_reset_tab_title(target))
                }
                unknown => Err(vivido::host::IpcError::new(
                    "unsupported",
                    format!("Vivida does not answer {unknown}"),
                )),
            };
            match result {
                Ok(result) => request.connection.reply(request.id, result),
                Err(error) => request.connection.error(request.id, error),
            }
        }
    }

    fn drain_shell_actions(&mut self, event_loop: &ActiveEventLoop) {
        for request in self.processor.take_shell_actions() {
            match request.action {
                ShellAction::CreateTab(options) => {
                    if let Some(key) = self.pane_index.get(&request.source).copied() {
                        let _ = self.host_create_tab(
                            event_loop,
                            CreateTabOptions {
                                workspace_id: Some(key.workspace_id.0),
                                workspace_name: None,
                                from_window_id: None,
                                name: None,
                                options: *options,
                            },
                        );
                    }
                }
                action @ (ShellAction::SelectNextTab | ShellAction::SelectPreviousTab) => {
                    if self.activate_pane(request.source) {
                        let direction = if matches!(action, ShellAction::SelectNextTab) {
                            1
                        } else {
                            -1
                        };
                        self.cycle_tab(direction);
                    }
                }
                ShellAction::SelectTab(index) => {
                    if self.activate_pane(request.source)
                        && let Some(tab_id) = self
                            .active_workspace()
                            .and_then(|workspace| workspace.tabs.get(index))
                            .map(|tab| tab.id)
                    {
                        self.switch_tab(tab_id);
                    }
                }
                ShellAction::SelectLastTab => {
                    if self.activate_pane(request.source)
                        && let Some(tab_id) = self
                            .active_workspace()
                            .and_then(|workspace| workspace.tabs.last())
                            .map(|tab| tab.id)
                    {
                        self.switch_tab(tab_id);
                    }
                }
                ShellAction::Minimize => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_minimized(true);
                    }
                }
                ShellAction::ToggleMaximized => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_maximized(!chrome.is_maximized());
                    }
                }
                ShellAction::ToggleFullscreen => {
                    if let Some(chrome) = &self.chrome_window {
                        let fullscreen = chrome
                            .fullscreen()
                            .is_none()
                            .then_some(Fullscreen::Borderless(None));
                        chrome.set_fullscreen(fullscreen);
                    }
                }
                ShellAction::Hide => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_visible(false);
                    }
                }
                ShellAction::Activate => {
                    self.activate_pane(request.source);
                }
                ShellAction::Resize { width, height } => {
                    self.activate_pane(request.source);
                    if let Some(rect) =
                        self.resize_active_pane_layout(request.source, width, height)
                        && let Some(chrome) = &self.chrome_window
                    {
                        let current = chrome.inner_size();
                        let requested = winit::dpi::PhysicalSize::new(
                            current
                                .width
                                .saturating_add(width)
                                .saturating_sub(rect.width),
                            current
                                .height
                                .saturating_add(height)
                                .saturating_sub(rect.height),
                        );
                        let _ = chrome.request_inner_size(requested);
                    }
                }
                ShellAction::SetPosition { x, y } => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                    }
                }
                ShellAction::SetVisible(visible) => {
                    if visible {
                        self.activate_pane(request.source);
                    } else if let Some(alternative) = self.alternative_tab_pane(request.source) {
                        self.activate_pane(alternative);
                    }
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_visible(
                            visible || self.alternative_tab_pane(request.source).is_some(),
                        );
                    }
                }
                ShellAction::SetLevel(level) => {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_window_level(level);
                    }
                }
            }
        }
    }

    fn handle_chrome_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(cursor) = self.cursor_position else {
            return false;
        };
        if self.name_editor.is_some() {
            if !self.chrome_hits.rename_editor.contains(cursor.x, cursor.y) {
                self.close_name_editor();
            }
            return true;
        }
        if let Some(menu) = self.recovery_menu {
            let item = self
                .chrome_hits
                .context_items
                .iter()
                .position(|rect| rect.contains(cursor.x, cursor.y));
            match item {
                Some(0) => self.recover_pane(menu.pane, false),
                Some(1) => self.recover_pane(menu.pane, true),
                _ => self.close_recovery_menu(),
            }
            return true;
        }
        if let Some(menu) = self.name_context_menu {
            let item = self
                .chrome_hits
                .context_items
                .iter()
                .position(|rect| rect.contains(cursor.x, cursor.y));
            let automatic_title_action = match menu.target {
                NameTarget::Workspace(_) => false,
                NameTarget::Tab {
                    workspace_id,
                    tab_id,
                } => self
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .and_then(|workspace| workspace.tabs.iter().find(|tab| tab.id == tab_id))
                    .is_some_and(model::Tab::is_title_custom),
            };
            match (menu.target, item) {
                (target, Some(0)) => self.start_name_editor(target),
                (
                    NameTarget::Tab {
                        workspace_id,
                        tab_id,
                    },
                    Some(1),
                ) if automatic_title_action => {
                    self.reset_context_tab_title(workspace_id, tab_id);
                }
                (
                    NameTarget::Tab {
                        workspace_id,
                        tab_id,
                    },
                    Some(index),
                ) => {
                    let reset_index = 1 + usize::from(automatic_title_action);
                    if let Some(pane) = self.focused_pane_in_tab(workspace_id, tab_id) {
                        if index == reset_index {
                            self.recover_pane(pane, false);
                        } else if index == reset_index + 1 {
                            self.recover_pane(pane, true);
                        }
                    }
                    self.name_context_menu = None;
                }
                _ => {
                    self.name_context_menu = None;
                    self.request_chrome_redraw();
                }
            }
            return true;
        }
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

    fn handle_chrome_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        if self.name_editor.is_none() && self.handle_shell_shortcut(event_loop, &event) {
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
            WindowEvent::Focused(true) if self.name_editor.is_none() => {
                self.focus_active_pane();
            }
            WindowEvent::Focused(false) => {
                #[cfg(target_os = "linux")]
                self.clear_embedded_focus();
                self.set_settings_menu_open(false);
                self.name_context_menu = None;
                if self.recovery_menu.take().is_some() {
                    self.sync_pane_visibility();
                }
                if self.rename_editor_window.is_none() && self.name_editor.take().is_some() {
                    if let Some(chrome) = &self.chrome_window {
                        chrome.set_ime_allowed(false);
                    }
                    self.request_chrome_redraw();
                }
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
                if state == ElementState::Pressed && button == MouseButton::Right {
                    if let Some(position) = self.cursor_position
                        && let Some(target) = self.name_target_at(position)
                    {
                        self.open_name_context_menu(target, position);
                        return;
                    }
                    let closed_recovery = self.recovery_menu.is_some();
                    if closed_recovery {
                        self.close_recovery_menu();
                    }
                    if self.name_context_menu.take().is_some()
                        || closed_recovery
                        || self.name_editor.is_some()
                    {
                        self.close_name_editor();
                        self.request_chrome_redraw();
                        return;
                    }
                }
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
                if self.handle_name_editor_ime(ime.clone()) {
                    return;
                }
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
            WindowEvent::KeyboardInput { event, .. } if self.handle_name_editor_key(&event) => {}
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                    && self.settings_menu_open =>
            {
                self.set_settings_menu_open(false);
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
            _ => (),
        }
    }

    fn handle_shell_shortcut(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) -> bool {
        let WindowEvent::KeyboardInput { event, .. } = event else {
            return false;
        };
        if event.state != ElementState::Pressed {
            return false;
        }
        if event.physical_key == PhysicalKey::Code(KeyCode::F12)
            && self.modifiers.control_key()
            && self.modifiers.shift_key()
        {
            if let Some(pane) = self.focused_pane_id() {
                self.open_recovery_menu(pane);
            }
            return true;
        }
        if !shell_modifier_pressed(self.modifiers) {
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
                    if tab.is_title_custom() {
                        return None;
                    }
                    let window = tab.window_id(tab.root.first_pane())?;
                    let title = self.processor.window(window)?.title().to_owned();
                    Some((workspace.id, tab.id, title))
                })
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        for (workspace_id, tab_id, title) in updates {
            if let Some(tab) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == workspace_id)
                .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
            {
                changed |= tab.set_context_title(&title);
            }
        }
        for workspace in &mut self.workspaces {
            changed |= workspace.refresh_tab_display_titles();
        }
        if changed {
            self.request_chrome_redraw();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowFrameAction {
    Minimize,
    ToggleMaximize,
    Close,
}

fn name_context_anchor(
    target: NameTarget,
    requested: PhysicalPosition<f64>,
) -> PhysicalPosition<f64> {
    match target {
        NameTarget::Tab { .. } => PhysicalPosition::new(requested.x, 0.0),
        NameTarget::Workspace(_) => requested,
    }
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
        if Some(window_id) == self.rename_editor_id {
            match event {
                WindowEvent::CloseRequested | WindowEvent::Focused(false) => {
                    self.close_name_editor();
                }
                WindowEvent::RedrawRequested => self.render_rename_editor(),
                WindowEvent::Resized(size) => {
                    if let Some(renderer) = &mut self.rename_editor_renderer {
                        let scale = self
                            .rename_editor_window
                            .as_ref()
                            .map_or(1.0, |window| window.scale_factor());
                        renderer.resize(size, scale, &self.config);
                    }
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    if let (Some(renderer), Some(window)) =
                        (&mut self.rename_editor_renderer, &self.rename_editor_window)
                    {
                        renderer.resize(window.inner_size(), scale_factor, &self.config);
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
                WindowEvent::Ime(ime) => {
                    self.handle_name_editor_ime(ime);
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    self.handle_name_editor_key(&event);
                }
                _ => (),
            }
            return;
        }
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
        self.drain_shell_actions(event_loop);
        self.drain_host_requests(event_loop);
        if self.processor.has_pending_embedded_redraw() {
            self.request_chrome_redraw();
        }
        self.reap_closed_panes(event_loop);
        self.refresh_tab_titles();
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.save_session();
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

fn ordinal_index(ordinal: u64) -> Option<usize> {
    usize::try_from(ordinal.checked_sub(1)?).ok()
}

fn pane_rect_json(rect: PhysicalRect) -> serde_json::Value {
    serde_json::json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}

fn directional_neighbor(
    tab: &model::Tab,
    rects: &HashMap<PaneId, PhysicalRect>,
    source: PaneId,
    direction: PaneDirection,
) -> Option<PaneId> {
    let source_rect = rects.get(&source)?;
    tab.panes
        .keys()
        .filter(|pane_id| **pane_id != source)
        .filter_map(|pane_id| {
            let rect = rects.get(pane_id)?;
            directional_score(source_rect, rect, direction).map(|score| (score, *pane_id))
        })
        .min_by_key(|(score, pane_id)| (*score, *pane_id))
        .map(|(_, pane_id)| pane_id)
}

fn directional_score(
    source: &PhysicalRect,
    candidate: &PhysicalRect,
    direction: PaneDirection,
) -> Option<(bool, i64, i64, i64)> {
    let (primary_gap, source_orthogonal, candidate_orthogonal) = match direction {
        PaneDirection::Left if rect_right(candidate) <= i64::from(source.x) => (
            i64::from(source.x) - rect_right(candidate),
            (i64::from(source.y), rect_bottom(source)),
            (i64::from(candidate.y), rect_bottom(candidate)),
        ),
        PaneDirection::Right if i64::from(candidate.x) >= rect_right(source) => (
            i64::from(candidate.x) - rect_right(source),
            (i64::from(source.y), rect_bottom(source)),
            (i64::from(candidate.y), rect_bottom(candidate)),
        ),
        PaneDirection::Up if rect_bottom(candidate) <= i64::from(source.y) => (
            i64::from(source.y) - rect_bottom(candidate),
            (i64::from(source.x), rect_right(source)),
            (i64::from(candidate.x), rect_right(candidate)),
        ),
        PaneDirection::Down if i64::from(candidate.y) >= rect_bottom(source) => (
            i64::from(candidate.y) - rect_bottom(source),
            (i64::from(source.x), rect_right(source)),
            (i64::from(candidate.x), rect_right(candidate)),
        ),
        _ => return None,
    };
    let overlap = source_orthogonal.1.min(candidate_orthogonal.1)
        - source_orthogonal.0.max(candidate_orthogonal.0);
    let center_distance = (source_orthogonal.0 + source_orthogonal.1
        - candidate_orthogonal.0
        - candidate_orthogonal.1)
        .abs();
    Some((overlap <= 0, primary_gap, center_distance, -overlap.max(0)))
}

fn pane_neighbors_json(
    tab: &model::Tab,
    rects: &HashMap<PaneId, PhysicalRect>,
    pane_id: PaneId,
    processor: &Processor,
) -> serde_json::Value {
    let neighbor = |direction| {
        directional_neighbor(tab, rects, pane_id, direction).and_then(|neighbor_id| {
            let window_id = tab.window_id(neighbor_id)?;
            let pane = processor.window(window_id)?;
            Some(serde_json::json!({
                "pane_id": neighbor_id.0,
                "window_id": pane.ipc_window_id(),
            }))
        })
    };
    serde_json::json!({
        "left": neighbor(PaneDirection::Left),
        "right": neighbor(PaneDirection::Right),
        "up": neighbor(PaneDirection::Up),
        "down": neighbor(PaneDirection::Down),
    })
}

fn rect_right(rect: &PhysicalRect) -> i64 {
    i64::from(rect.x) + i64::from(rect.width)
}

fn rect_bottom(rect: &PhysicalRect) -> i64 {
    i64::from(rect.y) + i64::from(rect.height)
}

fn layout_node_json(
    node: &layout::Node,
    tab: &model::Tab,
    processor: &Processor,
) -> serde_json::Value {
    match node {
        layout::Node::Leaf(pane_id) => {
            let window_id = tab
                .window_id(*pane_id)
                .and_then(|window_id| processor.window(window_id))
                .map(|pane| pane.ipc_window_id());
            serde_json::json!({
                "kind": "pane",
                "pane_id": pane_id.0,
                "window_id": window_id,
            })
        }
        layout::Node::Split {
            axis,
            children,
            sizes,
        } => {
            let axis = match axis {
                Axis::Horizontal => "horizontal",
                Axis::Vertical => "vertical",
            };
            serde_json::json!({
                "kind": "split",
                "axis": axis,
                "sizes": sizes,
                "children": children
                    .iter()
                    .map(|child| layout_node_json(child, tab, processor))
                    .collect::<Vec<_>>(),
            })
        }
    }
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

fn send_vivida_message(options: VividaMessageOptions) -> Result<(), Box<dyn Error>> {
    let VividaMessageOptions {
        socket,
        target,
        message,
    } = options;
    if let VividaMessage::Vivido(message) = message {
        return vivido::host::send_message(MessageOptions {
            socket,
            target,
            message,
        })
        .map_err(Into::into);
    }

    let (method, params) = match message {
        VividaMessage::Layout(options) => ("vivida_layout", serde_json::to_value(options)?),
        VividaMessage::ResolvePane(options) => {
            ("vivida_resolve_pane", serde_json::to_value(options)?)
        }
        VividaMessage::ActivatePane(target) => {
            ("vivida_activate_pane", serde_json::to_value(target)?)
        }
        VividaMessage::CreateWorkspace(options) => {
            ("vivida_create_workspace", serde_json::to_value(options)?)
        }
        VividaMessage::CreateTab(options) => ("vivida_create_tab", serde_json::to_value(options)?),
        VividaMessage::SplitPane(options) => ("vivida_split_pane", serde_json::to_value(options)?),
        VividaMessage::ClosePane(target) => ("vivida_close_pane", serde_json::to_value(target)?),
        VividaMessage::CloseTab(target) => ("vivida_close_tab", serde_json::to_value(target)?),
        VividaMessage::CloseWorkspace(target) => {
            ("vivida_close_workspace", serde_json::to_value(target)?)
        }
        VividaMessage::RenameWorkspace(options) => {
            ("vivida_rename_workspace", serde_json::to_value(options)?)
        }
        VividaMessage::RenameTab(options) => ("vivida_rename_tab", serde_json::to_value(options)?),
        VividaMessage::ResetTabTitle(target) => {
            ("vivida_reset_tab_title", serde_json::to_value(target)?)
        }
        VividaMessage::Vivido(_) => unreachable!("standard messages returned above"),
    };
    let (_, result) = vivido::host::request_method(socket, target.as_deref(), method, params)?;
    serde_json::to_writer(std::io::stdout().lock(), &result)?;
    println!();
    Ok(())
}

fn list_instances(options: ListOptions) -> Result<(), Box<dyn Error>> {
    let instances = vivido::host::list_registries()?
        .into_iter()
        .filter(|instance| options.all || instance.headless)
        .collect::<Vec<_>>();
    if options.json {
        serde_json::to_writer(
            std::io::stdout().lock(),
            &serde_json::json!({"schema_version": 1, "instances": instances}),
        )?;
        println!();
    } else {
        for instance in instances {
            println!(
                "{}\tpid {}\t{}x{}\t{}",
                instance.name,
                instance.pid,
                instance.columns,
                instance.lines,
                instance.socket.display()
            );
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = VividaOptions::parse();
    match options.command {
        Some(VividaCommand::Msg(options)) => return send_vivida_message(*options),
        Some(VividaCommand::List(options)) => return list_instances(options),
        None => {}
    }
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

    #[test]
    fn msg_exposes_standard_vivido_and_vivida_layout_commands() {
        let standard = VividaOptions::try_parse_from([
            "vivida",
            "msg",
            "typing",
            "hello",
            "--window-id",
            "42",
        ])
        .unwrap();
        assert!(matches!(
            standard.command.as_ref(),
            Some(VividaCommand::Msg(options))
                if matches!(options.message, VividaMessage::Vivido(SocketMessage::Typing(_)))
        ));

        let plan =
            VividaOptions::try_parse_from(["vivida", "msg", "run-plan", "--file", "plan.json"])
                .unwrap();
        assert!(matches!(
            plan.command.as_ref(),
            Some(VividaCommand::Msg(options))
                if matches!(options.message, VividaMessage::Vivido(SocketMessage::RunPlan(_)))
        ));

        let layout = VividaOptions::try_parse_from(["vivida", "msg", "layout"]).unwrap();
        assert!(matches!(
            layout.command.as_ref(),
            Some(VividaCommand::Msg(options))
                if matches!(options.message, VividaMessage::Layout(_))
        ));

        let resolve = VividaOptions::try_parse_from([
            "vivida",
            "msg",
            "resolve-pane",
            "--from-window-id",
            "42",
            "--tab",
            "2",
            "--path",
            "left,down",
        ])
        .unwrap();
        assert!(matches!(
            resolve.command.as_ref(),
            Some(VividaCommand::Msg(options))
                if matches!(
                    options.message,
                    VividaMessage::ResolvePane(ResolvePaneOptions {
                        from_window_id: Some(42),
                        tab: Some(2),
                        ref path,
                        ..
                    }) if path == &[PaneDirection::Left, PaneDirection::Down]
                )
        ));

        let named = VividaOptions::try_parse_from([
            "vivida",
            "msg",
            "resolve-pane",
            "--workspace-name",
            "Project A",
            "--tab-name",
            "Logs",
            "--pane-id",
            "3",
        ])
        .unwrap();
        assert!(matches!(
            named.command.as_ref(),
            Some(VividaCommand::Msg(options))
                if matches!(
                    &options.message,
                    VividaMessage::ResolvePane(ResolvePaneOptions {
                        workspace_name: Some(workspace_name),
                        tab_name: Some(tab_name),
                        pane_id: Some(3),
                        ..
                    }) if workspace_name == "Project A" && tab_name == "Logs"
                )
        ));

        for command in ["rename-workspace", "rename-tab", "reset-tab-title"] {
            let mut arguments = vec!["vivida", "msg", command, "--workspace-name", "Project A"];
            if command != "rename-workspace" {
                arguments.extend(["--tab-name", "Logs"]);
            }
            if command != "reset-tab-title" {
                arguments.extend(["--name", "Server"]);
            }
            VividaOptions::try_parse_from(arguments).unwrap();
        }

        let activate =
            VividaOptions::try_parse_from(["vivida", "msg", "activate-pane", "--window-id", "42"])
                .unwrap();
        assert!(matches!(
            activate.command.as_ref(),
            Some(VividaCommand::Msg(options))
                if matches!(
                    options.message,
                    VividaMessage::ActivatePane(WindowTarget { window_id: 42 })
                )
        ));

        let list = VividaOptions::try_parse_from(["vivida", "list", "--all", "--json"]).unwrap();
        assert!(matches!(
            list.command,
            Some(VividaCommand::List(ListOptions {
                all: true,
                json: true
            }))
        ));
    }

    #[test]
    fn split_axis_uses_stable_snake_case_wire_values() {
        assert_eq!(
            serde_json::to_value(SplitAxis::Horizontal).unwrap(),
            "horizontal"
        );
        assert_eq!(
            serde_json::to_value(SplitAxis::Vertical).unwrap(),
            "vertical"
        );
    }

    #[test]
    fn directional_paths_navigate_deeply_nested_split_trees() {
        let mut tab = model::Tab::new(TabId(1), WindowId::from(10));
        tab.split(Axis::Horizontal, WindowId::from(20)).unwrap();
        tab.split(Axis::Vertical, WindowId::from(30)).unwrap();
        tab.focused_pane = PaneId(1);
        tab.split(Axis::Vertical, WindowId::from(40)).unwrap();
        let rects = compute_rects(
            &tab.root,
            PhysicalRect {
                x: 0,
                y: 0,
                width: 404,
                height: 404,
            },
            1.0,
        );

        assert_eq!(
            directional_neighbor(&tab, &rects, PaneId(1), PaneDirection::Right),
            Some(PaneId(2))
        );
        assert_eq!(
            directional_neighbor(&tab, &rects, PaneId(1), PaneDirection::Down),
            Some(PaneId(4))
        );
        let right = directional_neighbor(&tab, &rects, PaneId(1), PaneDirection::Right).unwrap();
        assert_eq!(
            directional_neighbor(&tab, &rects, right, PaneDirection::Down),
            Some(PaneId(3))
        );
        assert_eq!(
            directional_neighbor(&tab, &rects, PaneId(3), PaneDirection::Left),
            Some(PaneId(4))
        );
    }

    #[test]
    fn name_editor_replaces_selection_and_edits_unicode_by_character() {
        let mut editor = NameEditor::new(NameTarget::Workspace(WorkspaceId(1)), "Old Name".into());
        editor.insert("界a");
        assert_eq!(editor.text, "界a");
        editor.backspace();
        assert_eq!(editor.text, "界");
        editor.move_left();
        editor.delete();
        assert!(editor.text.is_empty());
    }

    #[test]
    fn inactive_tab_rename_action_stays_in_the_chrome_tab_strip() {
        let requested = PhysicalPosition::new(640.0, 28.0);
        let tab_anchor = name_context_anchor(
            NameTarget::Tab {
                workspace_id: WorkspaceId(1),
                tab_id: TabId(2),
            },
            requested,
        );
        assert_eq!(tab_anchor, PhysicalPosition::new(640.0, 0.0));
        assert_eq!(
            name_context_anchor(NameTarget::Workspace(WorkspaceId(1)), requested),
            requested
        );
    }

    #[test]
    fn one_based_ordinals_are_checked_before_indexing() {
        assert_eq!(ordinal_index(1), Some(0));
        assert_eq!(ordinal_index(2), Some(1));
        assert_eq!(ordinal_index(0), None);
        assert_eq!(ordinal_index(u64::MAX), usize::try_from(u64::MAX - 1).ok());
    }
}
