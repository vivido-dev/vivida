use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Circle, Rect, Shape, Stroke};
use vello::peniko::{Color, Fill};
use vivido::config::UiConfig;
use vivido::config::font::FontSize;
use vivido::display::color::Rgb;
use vivido::display::rects::{RenderRect, paint_rects};
use vivido::display::renderer::EmbeddedFramePlacement;
use vivido::display::renderer::SceneRenderer;
use vivido::display::text::TextSystem;
use vivido::display::window::RenderSource;
use vivido::shell::LaunchEntry;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::Window;

use crate::layout::PhysicalRect;
use crate::model::{TabId, Workspace, WorkspaceId};
use crate::platform::{pane_bottom_resize_gutter, pane_side_resize_gutter};
use crate::shortcuts;

pub const EXPANDED_SIDEBAR_LOGICAL: f64 = 220.0;
pub const COMPACT_SIDEBAR_LOGICAL: f64 = 44.0;
pub const TAB_BAR_LOGICAL: f64 = 35.0;
pub const MIN_PANE_WIDTH_LOGICAL: f64 = 160.0;
pub const MIN_PANE_HEIGHT_LOGICAL: f64 = 80.0;
const CHROME_CONTROL_LOGICAL: f64 = 34.0;
const NEW_TAB_LOGICAL: f64 = 36.0;
const MACOS_TRAFFIC_LIGHTS_LOGICAL: f64 = 64.0;
const GEAR_TEETH: usize = 8;
const GEAR_TIP_LOGICAL: f64 = 8.0;
const GEAR_ROOT_LOGICAL: f64 = 5.8;
const GEAR_HUB_LOGICAL: f64 = 2.6;
const SETTINGS_MENU_WIDTH_LOGICAL: f64 = 190.0;
const SETTINGS_MENU_ROW_LOGICAL: f64 = 34.0;
const CONTEXT_MENU_WIDTH_LOGICAL: f64 = 210.0;
const CONTEXT_MENU_ROW_LOGICAL: f64 = 34.0;
const SHORTCUTS_WIDTH_LOGICAL: f64 = 460.0;
const SHORTCUTS_MAX_HEIGHT_LOGICAL: f64 = 560.0;
const SHORTCUTS_HEADER_LOGICAL: f64 = 38.0;
const SHORTCUTS_SECTION_LOGICAL: f64 = 34.0;
const SHORTCUTS_ROW_LOGICAL: f64 = 26.0;
const SHORTCUTS_PADDING_LOGICAL: f64 = 14.0;
const RENAME_EDITOR_WIDTH_LOGICAL: f64 = 380.0;
const RENAME_EDITOR_HEIGHT_LOGICAL: f64 = 112.0;

/// Rows of the gear menu, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsMenuItem {
    Settings,
    Shortcuts,
    Documentation,
}

impl SettingsMenuItem {
    pub const ALL: [Self; 3] = [Self::Settings, Self::Shortcuts, Self::Documentation];

    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Shortcuts => "Shortcuts",
            Self::Documentation => "Documentation",
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

const BACKGROUND: Rgb = Rgb::new(24, 24, 29);
const SIDEBAR: Rgb = Rgb::new(30, 30, 37);
const ACTIVE: Rgb = Rgb::new(53, 53, 65);
const HOVER: Rgb = Rgb::new(43, 43, 53);
const BORDER: Rgb = Rgb::new(68, 68, 82);
const TEXT: Rgb = Rgb::new(232, 232, 238);
const MUTED: Rgb = Rgb::new(155, 155, 170);
const ACCENT: Rgb = Rgb::new(129, 140, 248);
const DANGER: Rgb = Rgb::new(243, 139, 168);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarMode {
    #[default]
    Expanded,
    Compact,
    Hidden,
}

impl SidebarMode {
    pub fn next(self) -> Self {
        match self {
            Self::Expanded => Self::Compact,
            Self::Compact => Self::Hidden,
            Self::Hidden => Self::Expanded,
        }
    }

    fn logical_width(self) -> f64 {
        match self {
            Self::Expanded => EXPANDED_SIDEBAR_LOGICAL,
            Self::Compact => COMPACT_SIDEBAR_LOGICAL,
            Self::Hidden => 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChromeHitMap {
    pub toggle: PhysicalRect,
    pub new_workspace: PhysicalRect,
    pub workspace_rows: Vec<(WorkspaceId, PhysicalRect)>,
    pub close_buttons: Vec<(WorkspaceId, PhysicalRect)>,
    pub tab_rows: Vec<(TabId, PhysicalRect)>,
    pub new_tab: PhysicalRect,
    pub split_horizontal: PhysicalRect,
    pub split_vertical: PhysicalRect,
    pub gear: PhysicalRect,
    pub minimize: PhysicalRect,
    pub maximize: PhysicalRect,
    pub close: PhysicalRect,
    pub settings_menu: PhysicalRect,
    pub settings_items: Vec<PhysicalRect>,
    pub shortcuts: PhysicalRect,
    pub shortcuts_close: PhysicalRect,
    pub context_menu: PhysicalRect,
    pub context_items: Vec<PhysicalRect>,
    pub rename_editor: PhysicalRect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChromeLayout {
    pub sidebar: PhysicalRect,
    pub tab_bar: PhysicalRect,
    pub content: PhysicalRect,
}

pub fn compute_chrome_layout(
    size: PhysicalSize<u32>,
    scale_factor: f64,
    sidebar_mode: SidebarMode,
) -> ChromeLayout {
    let min_content_width = (MIN_PANE_WIDTH_LOGICAL * scale_factor).round() as u32;
    let requested_sidebar = (sidebar_mode.logical_width() * scale_factor).round() as u32;
    let sidebar_width = requested_sidebar.min(size.width.saturating_sub(min_content_width));
    let bottom_resize_gutter = pane_bottom_resize_gutter(scale_factor).min(size.height);
    let available_height = size.height.saturating_sub(bottom_resize_gutter);
    let tab_height = ((TAB_BAR_LOGICAL * scale_factor).round() as u32).min(
        available_height.saturating_sub((MIN_PANE_HEIGHT_LOGICAL * scale_factor).round() as u32),
    );
    // Panes must never cover the chrome's resize border: they would swallow the drags that
    // resize the shell. Keep a trailing strip uncovered, and a leading one where the sidebar
    // leaves the leading edge exposed.
    let side_resize_gutter = pane_side_resize_gutter(scale_factor).min(size.width);
    let content_x = sidebar_width.max(side_resize_gutter);
    let main_width = size
        .width
        .saturating_sub(content_x)
        .saturating_sub(side_resize_gutter);
    let content_height = available_height.saturating_sub(tab_height);

    ChromeLayout {
        sidebar: PhysicalRect {
            x: 0,
            y: 0,
            width: sidebar_width,
            height: size.height,
        },
        tab_bar: PhysicalRect {
            x: 0,
            y: 0,
            width: size.width,
            height: tab_height,
        },
        content: PhysicalRect {
            x: content_x as i32,
            y: tab_height as i32,
            width: main_width,
            height: content_height,
        },
    }
}

pub struct ChromeRenderer {
    renderer: SceneRenderer,
    text: TextSystem,
    scale_factor: f64,
    transparent_content: bool,
    pane_background: Rgb,
    pane_opacity: f32,
}

pub struct ChromeRenderState<'a> {
    pub sidebar_mode: SidebarMode,
    pub workspaces: &'a [Workspace],
    pub active_workspace: Option<WorkspaceId>,
    pub hovered_workspace: Option<WorkspaceId>,
    pub fullscreen: bool,
    pub settings_menu_open: bool,
    pub settings_menu_hover: Option<SettingsMenuItem>,
    pub shortcuts: Option<ShortcutsRenderState>,
    pub context_menu: Option<ContextMenuRenderState<'a>>,
    pub rename_editor: Option<RenameEditorRenderState<'a>>,
    pub embedded_frames: &'a [EmbeddedFramePlacement<'a>],
}

#[derive(Clone, Copy, Debug)]
pub struct ContextMenuRenderState<'a> {
    pub anchor: PhysicalPosition<f64>,
    pub automatic_title_action: bool,
    pub terminal_actions: bool,
    pub recovery_actions: bool,
    pub launch_entries: Option<&'a [LaunchEntry]>,
    pub selected: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct RenameEditorRenderState<'a> {
    pub label: &'a str,
    pub display_value: &'a str,
    pub error: Option<&'a str>,
}

pub struct SettingsMenuRenderer {
    renderer: SceneRenderer,
    text: TextSystem,
    scale_factor: f64,
}

pub struct RenameEditorRenderer {
    renderer: SceneRenderer,
    text: TextSystem,
    scale_factor: f64,
}

impl RenameEditorRenderer {
    pub fn new(
        window: Arc<Window>,
        config: &UiConfig,
        scale_factor: f64,
    ) -> Result<Self, vivido::display::renderer::Error> {
        let size = window.inner_size();
        Ok(Self {
            renderer: SceneRenderer::new(RenderSource::Surface(window), size, false)?,
            text: text_system(config, scale_factor),
            scale_factor,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64, config: &UiConfig) {
        self.renderer.resize(size);
        if (scale_factor - self.scale_factor).abs() > f64::EPSILON {
            self.scale_factor = scale_factor;
            self.text = text_system(config, scale_factor);
        }
    }

    pub fn render(
        &mut self,
        size: PhysicalSize<u32>,
        state: RenameEditorRenderState<'_>,
    ) -> Result<bool, vivido::display::renderer::Error> {
        let mut scene = Scene::new();
        paint_rename_editor(
            &mut self.text,
            self.scale_factor,
            &mut scene,
            size,
            state,
            &mut PhysicalRect::default(),
        );
        self.renderer.render(
            &scene,
            Color::from_rgba8(SIDEBAR.r, SIDEBAR.g, SIDEBAR.b, u8::MAX),
        )
    }
}

pub fn rename_editor_logical_size() -> (f64, f64) {
    (RENAME_EDITOR_WIDTH_LOGICAL, RENAME_EDITOR_HEIGHT_LOGICAL)
}

impl SettingsMenuRenderer {
    pub fn new(
        window: Arc<Window>,
        config: &UiConfig,
        scale_factor: f64,
    ) -> Result<Self, vivido::display::renderer::Error> {
        let size = window.inner_size();
        Ok(Self {
            renderer: SceneRenderer::new(RenderSource::Surface(window), size, false)?,
            text: text_system(config, scale_factor),
            scale_factor,
        })
    }

    pub fn new_embedded(
        size: PhysicalSize<u32>,
        config: &UiConfig,
        scale_factor: f64,
    ) -> Result<Self, vivido::display::renderer::Error> {
        Ok(Self {
            renderer: SceneRenderer::new(RenderSource::Embedded, size, false)?,
            text: text_system(config, scale_factor),
            scale_factor,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64, config: &UiConfig) {
        self.renderer.resize(size);
        if (scale_factor - self.scale_factor).abs() > f64::EPSILON {
            self.scale_factor = scale_factor;
            self.text = text_system(config, scale_factor);
        }
    }

    pub fn render(
        &mut self,
        size: PhysicalSize<u32>,
        hovered: Option<SettingsMenuItem>,
    ) -> Result<bool, vivido::display::renderer::Error> {
        let scale = self.scale_factor;
        let row_height = (SETTINGS_MENU_ROW_LOGICAL * scale).round() as u32;
        let mut scene = Scene::new();
        paint_rects(
            &mut scene,
            [
                RenderRect::new(
                    0.0,
                    0.0,
                    size.width as f32,
                    size.height as f32,
                    SIDEBAR,
                    1.0,
                ),
                RenderRect::new(
                    0.0,
                    0.0,
                    size.width as f32,
                    scale.max(1.0) as f32,
                    BORDER,
                    1.0,
                ),
            ],
        );
        for (index, item) in SettingsMenuItem::ALL.into_iter().enumerate() {
            let top = (index as u32 * row_height) as f32;
            if hovered == Some(item) {
                paint_rects(
                    &mut scene,
                    [RenderRect::new(
                        0.0,
                        top,
                        size.width as f32,
                        row_height as f32,
                        HOVER,
                        1.0,
                    )],
                );
            }
            self.text.paint_text(
                &mut scene,
                item.label(),
                (12.0 * scale as f32, top + 9.0 * scale as f32),
                TEXT,
                false,
            );
        }
        self.renderer.render(
            &scene,
            Color::from_rgba8(SIDEBAR.r, SIDEBAR.g, SIDEBAR.b, u8::MAX),
        )
    }

    pub fn embedded_frame(&self) -> Option<vivido::display::renderer::EmbeddedFrame<'_>> {
        self.renderer.embedded_frame()
    }
}

/// Row under `position` within a detached settings-menu window, in that window's own coordinates.
pub fn settings_menu_item_at(
    size: PhysicalSize<u32>,
    scale_factor: f64,
    position: PhysicalPosition<f64>,
) -> Option<SettingsMenuItem> {
    if position.x < 0.0
        || position.y < 0.0
        || position.x >= f64::from(size.width)
        || position.y >= f64::from(size.height)
    {
        return None;
    }
    let row_height = (SETTINGS_MENU_ROW_LOGICAL * scale_factor).round().max(1.0);
    SettingsMenuItem::from_index((position.y / row_height) as usize)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ShortcutsRenderState {
    pub scroll: f64,
    pub hovered_close: bool,
}

pub struct ShortcutsRenderer {
    renderer: SceneRenderer,
    text: TextSystem,
    scale_factor: f64,
}

impl ShortcutsRenderer {
    pub fn new(
        window: Arc<Window>,
        config: &UiConfig,
        scale_factor: f64,
    ) -> Result<Self, vivido::display::renderer::Error> {
        let size = window.inner_size();
        Ok(Self {
            renderer: SceneRenderer::new(RenderSource::Surface(window), size, false)?,
            text: text_system(config, scale_factor),
            scale_factor,
        })
    }

    pub fn new_embedded(
        size: PhysicalSize<u32>,
        config: &UiConfig,
        scale_factor: f64,
    ) -> Result<Self, vivido::display::renderer::Error> {
        Ok(Self {
            renderer: SceneRenderer::new(RenderSource::Embedded, size, false)?,
            text: text_system(config, scale_factor),
            scale_factor,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64, config: &UiConfig) {
        self.renderer.resize(size);
        if (scale_factor - self.scale_factor).abs() > f64::EPSILON {
            self.scale_factor = scale_factor;
            self.text = text_system(config, scale_factor);
        }
    }

    pub fn render(
        &mut self,
        size: PhysicalSize<u32>,
        state: ShortcutsRenderState,
    ) -> Result<bool, vivido::display::renderer::Error> {
        let mut scene = Scene::new();
        paint_shortcuts_panel(
            &mut self.text,
            self.scale_factor,
            &mut scene,
            PhysicalRect {
                x: 0,
                y: 0,
                width: size.width,
                height: size.height,
            },
            state,
        );
        self.renderer.render(
            &scene,
            Color::from_rgba8(SIDEBAR.r, SIDEBAR.g, SIDEBAR.b, u8::MAX),
        )
    }

    pub fn embedded_frame(&self) -> Option<vivido::display::renderer::EmbeddedFrame<'_>> {
        self.renderer.embedded_frame()
    }
}

/// Height of one shortcut row, which is also the wheel and arrow-key scroll step.
pub fn shortcuts_row_height(scale_factor: f64) -> f64 {
    (SHORTCUTS_ROW_LOGICAL * scale_factor).round()
}

/// Height of the shortcuts panel's fixed header, above the scrolling list.
pub fn shortcuts_header_height(scale_factor: f64) -> f64 {
    (SHORTCUTS_HEADER_LOGICAL * scale_factor).round()
}

pub fn shortcuts_logical_size() -> (f64, f64) {
    let content = SHORTCUTS_HEADER_LOGICAL + shortcuts_content_height(1.0);
    (
        SHORTCUTS_WIDTH_LOGICAL,
        content.min(SHORTCUTS_MAX_HEIGHT_LOGICAL),
    )
}

/// Height of the scrolling list, excluding the fixed header.
pub fn shortcuts_content_height(scale_factor: f64) -> f64 {
    let sections = shortcuts::sections();
    let rows: usize = sections.iter().map(|section| section.rows.len()).sum();
    let logical = sections.len() as f64 * SHORTCUTS_SECTION_LOGICAL
        + rows as f64 * SHORTCUTS_ROW_LOGICAL
        + SHORTCUTS_PADDING_LOGICAL;
    logical * scale_factor
}

/// Close button of a shortcuts panel occupying `panel`.
pub fn shortcuts_close_rect(panel: PhysicalRect, scale_factor: f64) -> PhysicalRect {
    let header = (SHORTCUTS_HEADER_LOGICAL * scale_factor).round() as u32;
    let size = header.min(panel.height);
    PhysicalRect {
        x: panel.right() - i32::try_from(size.min(panel.width)).unwrap_or_default(),
        y: panel.y,
        width: size.min(panel.width),
        height: size,
    }
}

fn paint_shortcuts_panel(
    text: &mut TextSystem,
    scale: f64,
    scene: &mut Scene,
    panel: PhysicalRect,
    state: ShortcutsRenderState,
) {
    let header = ((SHORTCUTS_HEADER_LOGICAL * scale).round() as u32).min(panel.height);
    let padding = (SHORTCUTS_PADDING_LOGICAL * scale).round() as f32;
    paint_rects(
        scene,
        [
            rect(panel, SIDEBAR),
            RenderRect::new(
                panel.x as f32,
                (panel.y + i32::try_from(header).unwrap_or_default()) as f32,
                panel.width as f32,
                scale.max(1.0) as f32,
                BORDER,
                1.0,
            ),
        ],
    );
    text.paint_text(
        scene,
        "Keyboard Shortcuts",
        (
            panel.x as f32 + padding,
            panel.y as f32 + 11.0 * scale as f32,
        ),
        TEXT,
        true,
    );

    let close = shortcuts_close_rect(panel, scale);
    if state.hovered_close {
        paint_rects(scene, [rect(close, HOVER)]);
    }
    let center = (
        f64::from(close.x) + f64::from(close.width) / 2.0,
        f64::from(close.y) + f64::from(close.height) / 2.0,
    );
    let arm = 4.5 * scale;
    paint_line(
        scene,
        (center.0 - arm, center.1 - arm),
        (center.0 + arm, center.1 + arm),
        scale,
    );
    paint_line(
        scene,
        (center.0 + arm, center.1 - arm),
        (center.0 - arm, center.1 + arm),
        scale,
    );

    let list = PhysicalRect {
        x: panel.x,
        y: panel.y + i32::try_from(header).unwrap_or_default(),
        width: panel.width,
        height: panel.height.saturating_sub(header),
    };
    if list.height == 0 {
        return;
    }
    scene.push_clip_layer(
        Fill::NonZero,
        Affine::IDENTITY,
        &Rect::new(
            f64::from(list.x),
            f64::from(list.y),
            f64::from(list.right()),
            f64::from(list.bottom()),
        ),
    );
    let section_height = (SHORTCUTS_SECTION_LOGICAL * scale).round() as f32;
    let row_height = (SHORTCUTS_ROW_LOGICAL * scale).round() as f32;
    let mut y = list.y as f32 + padding / 2.0 - state.scroll as f32;
    for section in shortcuts::sections() {
        text.paint_text(
            scene,
            section.title,
            (panel.x as f32 + padding, y + 12.0 * scale as f32),
            MUTED,
            true,
        );
        y += section_height;
        for row in section.rows {
            // Rows scrolled out of view still cost a layout pass, so skip them outright.
            if y + row_height >= list.y as f32 && y <= list.bottom() as f32 {
                text.paint_text(
                    scene,
                    row.description,
                    (panel.x as f32 + padding, y + 5.0 * scale as f32),
                    TEXT,
                    false,
                );
                let keys_width = text.measure_text(row.keys, false);
                text.paint_text(
                    scene,
                    row.keys,
                    (
                        panel.right() as f32 - padding - keys_width,
                        y + 5.0 * scale as f32,
                    ),
                    MUTED,
                    false,
                );
            }
            y += row_height;
        }
    }
    scene.pop_layer();

    let content = shortcuts_content_height(scale);
    let viewport = f64::from(list.height);
    if content > viewport {
        let track = viewport;
        let thumb = (track * viewport / content).max(f64::from((16.0 * scale) as u32));
        let travel = track - thumb;
        let offset = travel * (state.scroll / (content - viewport)).clamp(0.0, 1.0);
        let width = (3.0 * scale).round().max(1.0) as f32;
        paint_rects(
            scene,
            [RenderRect::new(
                panel.right() as f32 - width - (3.0 * scale) as f32,
                list.y as f32 + offset as f32,
                width,
                thumb as f32,
                BORDER,
                1.0,
            )],
        );
    }
}

pub fn settings_menu_logical_size() -> (f64, f64) {
    (
        SETTINGS_MENU_WIDTH_LOGICAL,
        SETTINGS_MENU_ROW_LOGICAL * SettingsMenuItem::ALL.len() as f64,
    )
}

impl ChromeRenderer {
    pub fn new(
        window: Arc<Window>,
        config: &UiConfig,
        scale_factor: f64,
    ) -> Result<Self, vivido::display::renderer::Error> {
        let size = window.inner_size();
        let transparent_content = config.window_opacity() < 1.0;
        let renderer =
            SceneRenderer::new(RenderSource::Surface(window), size, transparent_content)?;
        let text = text_system(config, scale_factor);
        Ok(Self {
            renderer,
            text,
            scale_factor,
            transparent_content,
            pane_background: config.colors.primary.background,
            pane_opacity: config.window_opacity(),
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64, config: &UiConfig) {
        self.renderer.resize(size);
        self.pane_background = config.colors.primary.background;
        self.pane_opacity = config.window_opacity();
        if (scale_factor - self.scale_factor).abs() > f64::EPSILON {
            self.scale_factor = scale_factor;
            self.text = text_system(config, scale_factor);
        }
    }

    pub fn render(
        &mut self,
        size: PhysicalSize<u32>,
        state: ChromeRenderState<'_>,
    ) -> Result<(ChromeLayout, ChromeHitMap, bool), vivido::display::renderer::Error> {
        let scale = self.scale_factor;
        let layout = compute_chrome_layout(size, scale, state.sidebar_mode);
        let mut scene = Scene::new();
        let mut hit_map = ChromeHitMap::default();
        let mut sidebar_label_x = 0;

        paint_rects(
            &mut scene,
            [
                rect(layout.tab_bar, BACKGROUND),
                rect(layout.sidebar, SIDEBAR),
                RenderRect::new(
                    layout.sidebar.width.saturating_sub(1) as f32,
                    0.0,
                    scale.max(1.0) as f32,
                    size.height as f32,
                    BORDER,
                    1.0,
                ),
                RenderRect::new(
                    layout.tab_bar.x as f32,
                    layout.tab_bar.height.saturating_sub(1) as f32,
                    layout.tab_bar.width as f32,
                    scale.max(1.0) as f32,
                    BORDER,
                    1.0,
                ),
            ],
        );

        let gutters = resize_gutter_rects(size, layout);
        if !gutters.is_empty() {
            let pane_background = self.pane_background;
            let pane_opacity = self.pane_opacity;
            paint_rects(
                &mut scene,
                gutters.into_iter().map(|gutter| {
                    RenderRect::new(
                        gutter.x as f32,
                        gutter.y as f32,
                        gutter.width as f32,
                        gutter.height as f32,
                        pane_background,
                        pane_opacity,
                    )
                }),
            );
        }

        if layout.tab_bar.height > 0 {
            let (tabs_area, label_x) =
                self.paint_top_controls(&mut scene, layout, state.fullscreen, &mut hit_map);
            sidebar_label_x = label_x;
            let workspace = state
                .workspaces
                .iter()
                .find(|workspace| Some(workspace.id) == state.active_workspace);
            if let Some(workspace) = workspace {
                let tab_width = (150.0 * scale).round() as u32;
                let new_tab_width = (NEW_TAB_LOGICAL * scale).round() as u32;
                let tabs_width = tabs_area.width.saturating_sub(new_tab_width);
                for (index, tab) in workspace.tabs.iter().enumerate() {
                    let tab_rect = PhysicalRect {
                        x: tabs_area.x + (index as u32 * tab_width) as i32,
                        y: 0,
                        width: tab_width.min(tabs_width.saturating_sub(index as u32 * tab_width)),
                        height: layout.tab_bar.height,
                    };
                    if tab_rect.width == 0 {
                        break;
                    }
                    hit_map.tab_rows.push((tab.id, tab_rect));
                    if tab.id == workspace.active_tab {
                        paint_rects(&mut scene, [rect(tab_rect, ACTIVE)]);
                    }
                    if let Some(title_clip) = tab_title_clip(tab_rect, scale) {
                        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &title_clip);
                        self.text.paint_text(
                            &mut scene,
                            &tab.title,
                            (
                                tab_rect.x as f32 + (12.0 * scale) as f32,
                                (8.0 * scale) as f32,
                            ),
                            if tab.id == workspace.active_tab {
                                TEXT
                            } else {
                                MUTED
                            },
                            tab.id == workspace.active_tab,
                        );
                        scene.pop_layer();
                    }
                }
                let x =
                    tabs_area.x + (workspace.tabs.len() as u32 * tab_width).min(tabs_width) as i32;
                hit_map.new_tab = PhysicalRect {
                    x,
                    y: 0,
                    width: new_tab_width.min(
                        u32::try_from(tabs_area.right().saturating_sub(x)).unwrap_or_default(),
                    ),
                    height: layout.tab_bar.height,
                };
                self.text.paint_text(
                    &mut scene,
                    "+",
                    (x as f32 + (10.0 * scale) as f32, (8.0 * scale) as f32),
                    ACCENT,
                    true,
                );
            }
        }

        if state.sidebar_mode != SidebarMode::Hidden && layout.sidebar.width > 0 {
            self.paint_sidebar(
                &mut scene,
                layout.sidebar,
                state.sidebar_mode,
                state.workspaces,
                state.active_workspace,
                state.hovered_workspace,
                sidebar_label_x,
                &mut hit_map,
            );
        }

        if state.settings_menu_open {
            self.paint_settings_menu(&mut scene, size, state.settings_menu_hover, &mut hit_map);
        }
        if let Some(shortcuts) = state.shortcuts {
            self.paint_shortcuts(&mut scene, size, shortcuts, &mut hit_map);
        }
        if let Some(menu) = state.context_menu {
            self.paint_context_menu(&mut scene, size, menu, &mut hit_map);
        }
        if let Some(editor) = state.rename_editor {
            self.paint_rename_editor(&mut scene, size, editor, &mut hit_map);
        }

        let overlay = if state.rename_editor.is_some() {
            hit_map.rename_editor
        } else if state.context_menu.is_some() {
            hit_map.context_menu
        } else if state.shortcuts.is_some() {
            hit_map.shortcuts
        } else {
            PhysicalRect::default()
        };
        let overlay = (overlay.width > 0 && overlay.height > 0).then(|| {
            (
                PhysicalPosition::new(
                    u32::try_from(overlay.x).unwrap_or_default(),
                    u32::try_from(overlay.y).unwrap_or_default(),
                ),
                PhysicalSize::new(overlay.width, overlay.height),
            )
        });
        let background_alpha = if self.transparent_content { 0 } else { u8::MAX };
        let presented = self.renderer.render_composited(
            &scene,
            Color::from_rgba8(BACKGROUND.r, BACKGROUND.g, BACKGROUND.b, background_alpha),
            state.embedded_frames,
            overlay,
        )?;
        Ok((layout, hit_map, presented))
    }

    fn paint_top_controls(
        &mut self,
        scene: &mut Scene,
        layout: ChromeLayout,
        fullscreen: bool,
        hit_map: &mut ChromeHitMap,
    ) -> (PhysicalRect, i32) {
        let scale = self.scale_factor;
        let tab_bar = layout.tab_bar;
        let button_width = (CHROME_CONTROL_LOGICAL * scale).round() as u32;
        let leading_inset = leading_control_inset(scale, fullscreen);
        hit_map.toggle = PhysicalRect {
            x: leading_inset as i32,
            y: tab_bar.y,
            width: button_width,
            height: tab_bar.height,
        };

        let system_width = system_control_width(scale);
        let system_start = tab_bar.width.saturating_sub(system_width);
        let app_width = button_width.saturating_mul(3);
        let app_start = system_start.saturating_sub(app_width);
        hit_map.split_horizontal = control_rect(app_start, tab_bar, button_width);
        hit_map.split_vertical = control_rect(
            app_start.saturating_add(button_width),
            tab_bar,
            button_width,
        );
        hit_map.gear = control_rect(
            app_start.saturating_add(button_width.saturating_mul(2)),
            tab_bar,
            button_width,
        );

        if system_width > 0 {
            let system_button = system_width / 3;
            hit_map.minimize = control_rect(system_start, tab_bar, system_button);
            hit_map.maximize = control_rect(
                system_start.saturating_add(system_button),
                tab_bar,
                system_button,
            );
            hit_map.close = control_rect(
                system_start.saturating_add(system_button.saturating_mul(2)),
                tab_bar,
                system_width.saturating_sub(system_button.saturating_mul(2)),
            );
        }

        let tabs_start = layout
            .sidebar
            .width
            .max(leading_inset.saturating_add(button_width));
        let tabs_area = PhysicalRect {
            x: tabs_start as i32,
            y: tab_bar.y,
            width: app_start.saturating_sub(tabs_start),
            height: tab_bar.height,
        };

        paint_rects(
            scene,
            [
                rect(hit_map.toggle, SIDEBAR),
                rect(hit_map.split_horizontal, SIDEBAR),
                rect(hit_map.split_vertical, SIDEBAR),
                rect(hit_map.gear, SIDEBAR),
                RenderRect::new(
                    hit_map.toggle.x as f32 + (9.0 * scale) as f32,
                    (10.0 * scale) as f32,
                    (16.0 * scale) as f32,
                    (2.0 * scale).max(1.0) as f32,
                    ACCENT,
                    1.0,
                ),
                RenderRect::new(
                    hit_map.toggle.x as f32 + (9.0 * scale) as f32,
                    (16.0 * scale) as f32,
                    (16.0 * scale) as f32,
                    (2.0 * scale).max(1.0) as f32,
                    ACCENT,
                    1.0,
                ),
                RenderRect::new(
                    hit_map.toggle.x as f32 + (9.0 * scale) as f32,
                    (22.0 * scale) as f32,
                    (16.0 * scale) as f32,
                    (2.0 * scale).max(1.0) as f32,
                    ACCENT,
                    1.0,
                ),
            ],
        );
        for (button, label) in [
            (hit_map.split_horizontal, "↔"),
            (hit_map.split_vertical, "↕"),
        ] {
            self.text.paint_text(
                scene,
                label,
                (button.x as f32 + (9.0 * scale) as f32, (8.0 * scale) as f32),
                ACCENT,
                true,
            );
        }
        self.paint_gear(scene, hit_map.gear);

        if system_width > 0 {
            self.paint_window_controls(scene, hit_map);
        }

        (
            tabs_area,
            hit_map.toggle.x + i32::try_from(hit_map.toggle.width).unwrap_or(i32::MAX),
        )
    }

    fn paint_window_controls(&mut self, scene: &mut Scene, hit_map: &ChromeHitMap) {
        let scale = self.scale_factor;
        paint_rects(
            scene,
            [
                rect(hit_map.minimize, BACKGROUND),
                rect(hit_map.maximize, BACKGROUND),
                rect(hit_map.close, BACKGROUND),
                RenderRect::new(
                    hit_map.minimize.x as f32
                        + (hit_map.minimize.width as f32 - 10.0 * scale as f32) / 2.0,
                    17.0 * scale as f32,
                    10.0 * scale as f32,
                    scale.max(1.0) as f32,
                    TEXT,
                    1.0,
                ),
            ],
        );

        let max_x =
            hit_map.maximize.x as f32 + (hit_map.maximize.width as f32 - 10.0 * scale as f32) / 2.0;
        let max_y = 12.0 * scale as f32;
        paint_rects(
            scene,
            [
                RenderRect::new(
                    max_x,
                    max_y,
                    10.0 * scale as f32,
                    scale.max(1.0) as f32,
                    TEXT,
                    1.0,
                ),
                RenderRect::new(
                    max_x,
                    max_y,
                    scale.max(1.0) as f32,
                    10.0 * scale as f32,
                    TEXT,
                    1.0,
                ),
                RenderRect::new(
                    max_x + 9.0 * scale as f32,
                    max_y,
                    scale.max(1.0) as f32,
                    10.0 * scale as f32,
                    TEXT,
                    1.0,
                ),
                RenderRect::new(
                    max_x,
                    max_y + 9.0 * scale as f32,
                    10.0 * scale as f32,
                    scale.max(1.0) as f32,
                    TEXT,
                    1.0,
                ),
            ],
        );
        let close_center_x = hit_map.close.x as f64 + f64::from(hit_map.close.width) / 2.0;
        let close_center_y = f64::from(hit_map.close.height) / 2.0;
        let radius = 5.0 * scale;
        paint_line(
            scene,
            (close_center_x - radius, close_center_y - radius),
            (close_center_x + radius, close_center_y + radius),
            scale,
        );
        paint_line(
            scene,
            (close_center_x + radius, close_center_y - radius),
            (close_center_x - radius, close_center_y + radius),
            scale,
        );
    }

    fn paint_gear(&self, scene: &mut Scene, rect: PhysicalRect) {
        let center = (
            rect.x as f64 + f64::from(rect.width) / 2.0,
            rect.y as f64 + f64::from(rect.height) / 2.0,
        );
        // The hub is a second subpath, so an even-odd fill punches it out of the rim.
        scene.fill(
            Fill::EvenOdd,
            Affine::IDENTITY,
            Color::from_rgb8(TEXT.r, TEXT.g, TEXT.b),
            None,
            &gear_path(center, self.scale_factor),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_sidebar(
        &mut self,
        scene: &mut Scene,
        area: PhysicalRect,
        mode: SidebarMode,
        workspaces: &[Workspace],
        active_workspace: Option<WorkspaceId>,
        hovered_workspace: Option<WorkspaceId>,
        sidebar_label_x: i32,
        hit_map: &mut ChromeHitMap,
    ) {
        let scale = self.scale_factor;
        let header_height = (44.0 * scale).round() as u32;
        let row_height = (38.0 * scale).round() as u32;
        let (list_bottom, footer_height) = sidebar_footer_metrics(area.height, scale);
        let padding = (10.0 * scale).round() as u32;
        let status_slot = (12.0 * scale).round() as u32;

        if mode == SidebarMode::Expanded {
            self.text.paint_text(
                scene,
                "Workspaces",
                (
                    sidebar_label_x as f32 + (8.0 * scale) as f32,
                    (12.0 * scale) as f32,
                ),
                MUTED,
                true,
            );
        }

        for (index, workspace) in workspaces.iter().enumerate() {
            let y = header_height.saturating_add(index as u32 * row_height);
            if y.saturating_add(row_height) > list_bottom {
                break;
            }
            let row = PhysicalRect {
                x: 0,
                y: y as i32,
                width: area.width,
                height: row_height,
            };
            hit_map.workspace_rows.push((workspace.id, row));
            if Some(workspace.id) == active_workspace || Some(workspace.id) == hovered_workspace {
                paint_rects(
                    scene,
                    [rect(
                        row,
                        if Some(workspace.id) == active_workspace {
                            ACTIVE
                        } else {
                            HOVER
                        },
                    )],
                );
            }

            // Leave a stable leading slot for a future attention badge. Phase 2 draws no badge.
            let label_x = padding.saturating_add(status_slot);
            let label = if mode == SidebarMode::Expanded {
                workspace.label.clone()
            } else {
                (index + 1).to_string()
            };
            self.text.paint_text(
                scene,
                &label,
                (label_x as f32, y as f32 + (10.0 * scale) as f32),
                if Some(workspace.id) == active_workspace {
                    TEXT
                } else {
                    MUTED
                },
                Some(workspace.id) == active_workspace,
            );

            if mode == SidebarMode::Expanded {
                let close_width = (30.0 * scale).round() as u32;
                let close = PhysicalRect {
                    x: area.width.saturating_sub(close_width) as i32,
                    y: y as i32,
                    width: close_width,
                    height: row_height,
                };
                hit_map.close_buttons.push((workspace.id, close));
                self.text.paint_text(
                    scene,
                    "×",
                    (
                        close.x as f32 + (8.0 * scale) as f32,
                        y as f32 + (10.0 * scale) as f32,
                    ),
                    DANGER,
                    false,
                );
            }
        }

        if mode == SidebarMode::Expanded {
            hit_map.new_workspace = PhysicalRect {
                x: 0,
                y: list_bottom as i32,
                width: area.width,
                height: footer_height,
            };
            self.text.paint_text(
                scene,
                "+ New Workspace",
                (padding as f32, list_bottom as f32 + (13.0 * scale) as f32),
                ACCENT,
                true,
            );
        }
    }

    fn paint_shortcuts(
        &mut self,
        scene: &mut Scene,
        size: PhysicalSize<u32>,
        state: ShortcutsRenderState,
        hit_map: &mut ChromeHitMap,
    ) {
        hit_map.shortcuts = shortcuts_rect(size, self.scale_factor);
        hit_map.shortcuts_close = shortcuts_close_rect(hit_map.shortcuts, self.scale_factor);
        paint_shortcuts_panel(
            &mut self.text,
            self.scale_factor,
            scene,
            hit_map.shortcuts,
            state,
        );
    }

    fn paint_settings_menu(
        &mut self,
        scene: &mut Scene,
        size: PhysicalSize<u32>,
        hovered: Option<SettingsMenuItem>,
        hit_map: &mut ChromeHitMap,
    ) {
        let scale = self.scale_factor;
        let row_height = (SETTINGS_MENU_ROW_LOGICAL * scale).round() as u32;
        hit_map.settings_menu = settings_menu_rect(hit_map.gear, size, scale);
        paint_rects(
            scene,
            [
                rect(hit_map.settings_menu, SIDEBAR),
                RenderRect::new(
                    hit_map.settings_menu.x as f32,
                    hit_map.settings_menu.y as f32,
                    hit_map.settings_menu.width as f32,
                    scale.max(1.0) as f32,
                    BORDER,
                    1.0,
                ),
            ],
        );
        for (index, item) in SettingsMenuItem::ALL.into_iter().enumerate() {
            let row = PhysicalRect {
                x: hit_map.settings_menu.x,
                y: hit_map.settings_menu.y + (index as u32 * row_height) as i32,
                width: hit_map.settings_menu.width,
                height: row_height.min(
                    hit_map
                        .settings_menu
                        .height
                        .saturating_sub(index as u32 * row_height),
                ),
            };
            if row.height == 0 {
                break;
            }
            if hovered == Some(item) {
                paint_rects(scene, [rect(row, HOVER)]);
            }
            hit_map.settings_items.push(row);
            self.text.paint_text(
                scene,
                item.label(),
                (
                    row.x as f32 + 12.0 * scale as f32,
                    row.y as f32 + 9.0 * scale as f32,
                ),
                TEXT,
                false,
            );
        }
    }

    fn paint_context_menu(
        &mut self,
        scene: &mut Scene,
        size: PhysicalSize<u32>,
        state: ContextMenuRenderState<'_>,
        hit_map: &mut ChromeHitMap,
    ) {
        let scale = self.scale_factor;
        let width = (CONTEXT_MENU_WIDTH_LOGICAL * scale).round() as u32;
        let row_height = (CONTEXT_MENU_ROW_LOGICAL * scale).round() as u32;
        let rows: u32 = if let Some(entries) = state.launch_entries {
            u32::try_from(entries.len()).unwrap_or(u32::MAX)
        } else if state.recovery_actions {
            3
        } else {
            1 + u32::from(state.automatic_title_action) + 2 * u32::from(state.terminal_actions)
        };
        let height = row_height.saturating_mul(rows);
        let max_x = size.width.saturating_sub(width);
        let max_y = size.height.saturating_sub(height);
        hit_map.context_menu = PhysicalRect {
            x: state.anchor.x.max(0.0).min(f64::from(max_x)).round() as i32,
            y: state.anchor.y.max(0.0).min(f64::from(max_y)).round() as i32,
            width: width.min(size.width),
            height: height.min(size.height),
        };
        paint_rects(scene, [rect(hit_map.context_menu, SIDEBAR)]);
        let labels = if let Some(entries) = state.launch_entries {
            entries.iter().map(|entry| entry.label.as_str()).collect()
        } else if state.recovery_actions {
            vec!["Reset Terminal", "Restart Terminal", "Cancel"]
        } else if state.automatic_title_action && state.terminal_actions {
            vec![
                "Rename",
                "Use Automatic Title",
                "Reset Terminal",
                "Restart Terminal",
            ]
        } else if state.automatic_title_action {
            vec!["Rename", "Use Automatic Title"]
        } else if state.terminal_actions {
            vec!["Rename", "Reset Terminal", "Restart Terminal"]
        } else {
            vec!["Rename"]
        };
        for (index, label) in labels.iter().enumerate() {
            let row = PhysicalRect {
                x: hit_map.context_menu.x,
                y: hit_map.context_menu.y
                    + i32::try_from(index as u32 * row_height).unwrap_or_default(),
                width: hit_map.context_menu.width,
                height: row_height.min(
                    hit_map
                        .context_menu
                        .height
                        .saturating_sub(index as u32 * row_height),
                ),
            };
            hit_map.context_items.push(row);
            if state.selected == Some(index) {
                paint_rects(scene, [rect(row, ACTIVE)]);
            }
            self.text.paint_text(
                scene,
                label,
                (
                    row.x as f32 + 12.0 * scale as f32,
                    row.y as f32 + 9.0 * scale as f32,
                ),
                TEXT,
                false,
            );
        }
    }

    fn paint_rename_editor(
        &mut self,
        scene: &mut Scene,
        size: PhysicalSize<u32>,
        state: RenameEditorRenderState<'_>,
        hit_map: &mut ChromeHitMap,
    ) {
        paint_rename_editor(
            &mut self.text,
            self.scale_factor,
            scene,
            size,
            state,
            &mut hit_map.rename_editor,
        );
    }
}

fn paint_rename_editor(
    text: &mut TextSystem,
    scale: f64,
    scene: &mut Scene,
    size: PhysicalSize<u32>,
    state: RenameEditorRenderState<'_>,
    editor_rect: &mut PhysicalRect,
) {
    let width = ((RENAME_EDITOR_WIDTH_LOGICAL * scale).round() as u32).min(size.width);
    let height = ((RENAME_EDITOR_HEIGHT_LOGICAL * scale).round() as u32).min(size.height);
    *editor_rect = PhysicalRect {
        x: i32::try_from(size.width.saturating_sub(width) / 2).unwrap_or_default(),
        y: i32::try_from(size.height.saturating_sub(height) / 3).unwrap_or_default(),
        width,
        height,
    };
    let field = PhysicalRect {
        x: editor_rect.x + (12.0 * scale).round() as i32,
        y: editor_rect.y + (38.0 * scale).round() as i32,
        width: editor_rect
            .width
            .saturating_sub((24.0 * scale).round() as u32),
        height: (34.0 * scale).round() as u32,
    };
    paint_rects(
        scene,
        [
            rect(*editor_rect, SIDEBAR),
            rect(field, BACKGROUND),
            RenderRect::new(
                field.x as f32,
                field.y as f32,
                field.width as f32,
                scale.max(1.0) as f32,
                ACCENT,
                1.0,
            ),
        ],
    );
    text.paint_text(
        scene,
        state.label,
        (
            editor_rect.x as f32 + 12.0 * scale as f32,
            editor_rect.y as f32 + 10.0 * scale as f32,
        ),
        TEXT,
        true,
    );
    let field_clip = Rect::new(
        f64::from(field.x),
        f64::from(field.y),
        f64::from(field.right()),
        f64::from(field.bottom()),
    );
    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &field_clip);
    text.paint_text(
        scene,
        state.display_value,
        (
            field.x as f32 + 8.0 * scale as f32,
            field.y as f32 + 8.0 * scale as f32,
        ),
        TEXT,
        false,
    );
    scene.pop_layer();
    if let Some(error) = state.error {
        text.paint_text(
            scene,
            error,
            (
                editor_rect.x as f32 + 12.0 * scale as f32,
                field.bottom() as f32 + 7.0 * scale as f32,
            ),
            DANGER,
            false,
        );
    }
}

fn paint_line(scene: &mut Scene, start: (f64, f64), end: (f64, f64), scale: f64) {
    let mut path = BezPath::new();
    path.move_to(start);
    path.line_to(end);
    scene.stroke(
        &Stroke::new(scale.max(1.0)),
        Affine::IDENTITY,
        Color::from_rgb8(TEXT.r, TEXT.g, TEXT.b),
        None,
        &path,
    );
}

/// Outline of the settings cog, centred on `center`.
///
/// The rim carries [`GEAR_TEETH`] trapezoidal teeth — wider at the root than at the tip — and the
/// hub follows as a second subpath so an even-odd fill leaves it hollow. Filling rather than
/// stroking is what separates a gear from a sun: thin spokes standing off a thin circle read as
/// rays at this size.
fn gear_path(center: (f64, f64), scale: f64) -> BezPath {
    let tip = GEAR_TIP_LOGICAL * scale;
    let root = GEAR_ROOT_LOGICAL * scale;
    let step = std::f64::consts::TAU / GEAR_TEETH as f64;
    let tip_gap = step * 0.17;
    let root_gap = step * 0.30;
    let point = |radius: f64, angle: f64| {
        (
            center.0 + radius * angle.cos(),
            center.1 + radius * angle.sin(),
        )
    };
    // Control point for a quadratic that hugs the root circle across the gap between two teeth,
    // so the rim between them stays round instead of collapsing into a chord.
    let half_valley = (step - 2.0 * root_gap) / 2.0;
    let valley_radius = root / half_valley.cos();

    let mut path = BezPath::new();
    path.move_to(point(root, -root_gap));
    for index in 0..GEAR_TEETH {
        let angle = index as f64 * step;
        if index > 0 {
            path.quad_to(
                point(valley_radius, angle - step / 2.0),
                point(root, angle - root_gap),
            );
        }
        path.line_to(point(tip, angle - tip_gap));
        path.line_to(point(tip, angle + tip_gap));
        path.line_to(point(root, angle + root_gap));
    }
    let wrap = std::f64::consts::TAU;
    path.quad_to(
        point(valley_radius, wrap - step / 2.0),
        point(root, wrap - root_gap),
    );
    path.close_path();

    path.extend(Circle::new(center, GEAR_HUB_LOGICAL * scale).path_elements(0.05));
    path
}

fn control_rect(offset: u32, tab_bar: PhysicalRect, width: u32) -> PhysicalRect {
    PhysicalRect {
        x: tab_bar.x + offset as i32,
        y: tab_bar.y,
        width: width.min(tab_bar.width.saturating_sub(offset)),
        height: tab_bar.height,
    }
}

fn system_control_width(scale: f64) -> u32 {
    if cfg!(target_os = "macos") {
        0
    } else if cfg!(windows) {
        (138.0 * scale).round() as u32
    } else {
        (114.0 * scale).round() as u32
    }
}

fn leading_control_inset(scale: f64, fullscreen: bool) -> u32 {
    if cfg!(target_os = "macos") && !fullscreen {
        (MACOS_TRAFFIC_LIGHTS_LOGICAL * scale).round() as u32
    } else {
        0
    }
}

/// Placement of an in-chrome shortcuts panel: centred horizontally, a third of the way down, and
/// clamped to the chrome so it never overhangs.
fn shortcuts_rect(size: PhysicalSize<u32>, scale: f64) -> PhysicalRect {
    let (width_logical, height_logical) = shortcuts_logical_size();
    let width = ((width_logical * scale).round() as u32).min(size.width);
    let height = ((height_logical * scale).round() as u32).min(size.height);
    PhysicalRect {
        x: i32::try_from(size.width.saturating_sub(width) / 2).unwrap_or_default(),
        y: i32::try_from(size.height.saturating_sub(height) / 3).unwrap_or_default(),
        width,
        height,
    }
}

fn settings_menu_rect(gear: PhysicalRect, size: PhysicalSize<u32>, scale: f64) -> PhysicalRect {
    let width = (SETTINGS_MENU_WIDTH_LOGICAL * scale).round() as u32;
    let height =
        ((SETTINGS_MENU_ROW_LOGICAL * SettingsMenuItem::ALL.len() as f64) * scale).round() as u32;
    let right = gear.right().max(0) as u32;
    let x = right
        .saturating_sub(width)
        .min(size.width.saturating_sub(width));
    let y = gear.bottom();
    PhysicalRect {
        x: x as i32,
        y,
        width: width.min(size.width),
        height: height.min(size.height.saturating_sub(y.max(0) as u32)),
    }
}

fn text_system(config: &UiConfig, scale_factor: f64) -> TextSystem {
    TextSystem::new(
        config
            .font
            .clone()
            .with_size(FontSize::new((13.0 * scale_factor) as f32)),
    )
}

fn tab_title_clip(tab: PhysicalRect, scale_factor: f64) -> Option<Rect> {
    let padding = (12.0 * scale_factor).round() as u32;
    let width = tab.width.checked_sub(padding.saturating_mul(2))?;
    (width > 0).then(|| {
        Rect::new(
            f64::from(tab.x) + f64::from(padding),
            f64::from(tab.y),
            f64::from(tab.x) + f64::from(padding) + f64::from(width),
            f64::from(tab.y) + f64::from(tab.height),
        )
    })
}

fn sidebar_footer_metrics(area_height: u32, scale_factor: f64) -> (u32, u32) {
    let available_height =
        area_height.saturating_sub(pane_bottom_resize_gutter(scale_factor).min(area_height));
    let footer_height = ((44.0 * scale_factor).round() as u32).min(available_height);
    (
        available_height.saturating_sub(footer_height),
        footer_height,
    )
}

/// Strips of the chrome that panes leave uncovered so its resize border stays reachable,
/// painted with the pane background for a seamless edge. Order is fixed: bottom, trailing,
/// leading (the last only when the sidebar is hidden).
fn resize_gutter_rects(size: PhysicalSize<u32>, layout: ChromeLayout) -> Vec<PhysicalRect> {
    let content = layout.content;
    let mut gutters = Vec::new();
    let bottom = content.bottom().max(0);
    if u32::try_from(bottom).unwrap_or_default() < size.height {
        gutters.push(PhysicalRect {
            x: content.x,
            y: bottom,
            width: size
                .width
                .saturating_sub(u32::try_from(content.x.max(0)).unwrap_or_default()),
            height: size
                .height
                .saturating_sub(u32::try_from(bottom).unwrap_or_default()),
        });
    }
    let right = content.right().max(0);
    if u32::try_from(right).unwrap_or_default() < size.width {
        gutters.push(PhysicalRect {
            x: right,
            y: content.y,
            width: size
                .width
                .saturating_sub(u32::try_from(right).unwrap_or_default()),
            height: content.height,
        });
    }
    if layout.sidebar.width == 0 && content.x > 0 {
        gutters.push(PhysicalRect {
            x: 0,
            y: content.y,
            width: u32::try_from(content.x).unwrap_or_default(),
            height: size
                .height
                .saturating_sub(u32::try_from(content.y.max(0)).unwrap_or_default()),
        });
    }
    gutters
}

fn rect(rect: PhysicalRect, color: Rgb) -> RenderRect {
    RenderRect::new(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
        color,
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use vello::kurbo::{PathEl, Point};

    use super::*;

    #[test]
    fn sidebar_modes_allocate_expected_widths() {
        let size = PhysicalSize::new(1000, 600);
        assert_eq!(
            compute_chrome_layout(size, 1.0, SidebarMode::Expanded)
                .sidebar
                .width,
            220
        );
        assert_eq!(
            compute_chrome_layout(size, 1.0, SidebarMode::Compact)
                .sidebar
                .width,
            44
        );
        assert_eq!(
            compute_chrome_layout(size, 1.0, SidebarMode::Hidden)
                .sidebar
                .width,
            0
        );
    }

    #[test]
    fn expanded_sidebar_never_starves_the_minimum_pane_width() {
        let layout = compute_chrome_layout(PhysicalSize::new(300, 200), 1.0, SidebarMode::Expanded);
        assert_eq!(layout.sidebar.width, 140);
        assert_eq!(
            layout.content.width,
            160u32.saturating_sub(pane_side_resize_gutter(1.0))
        );
        assert!(layout.content.height >= 80);
        assert_eq!(
            layout.tab_bar,
            PhysicalRect {
                x: 0,
                y: 0,
                width: 300,
                height: 35
            }
        );
        assert_eq!(layout.content.y, 35);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_content_leaves_the_bottom_resize_gutter_uncovered() {
        let size = PhysicalSize::new(1000, 600);
        let layout = compute_chrome_layout(size, 1.0, SidebarMode::Expanded);

        assert_eq!(layout.content.bottom(), 590);
        assert_eq!(
            resize_gutter_rects(size, layout),
            vec![PhysicalRect {
                x: 220,
                y: 590,
                width: 780,
                height: 10,
            }]
        );

        let short_layout =
            compute_chrome_layout(PhysicalSize::new(1000, 100), 1.0, SidebarMode::Expanded);
        assert_eq!(short_layout.content.height, 80);
        assert_eq!(short_layout.content.bottom(), 90);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_content_leaves_the_native_resize_border_reachable() {
        let size = PhysicalSize::new(1000, 600);
        let layout = compute_chrome_layout(size, 1.0, SidebarMode::Expanded);

        assert_eq!(layout.content.right(), 994);
        assert_eq!(layout.content.bottom(), 594);
        assert_eq!(
            resize_gutter_rects(size, layout),
            vec![
                PhysicalRect {
                    x: 220,
                    y: 594,
                    width: 780,
                    height: 6,
                },
                PhysicalRect {
                    x: 994,
                    y: 35,
                    width: 6,
                    height: 559,
                },
            ]
        );

        // A hidden sidebar exposes the leading edge, so the content moves off it too.
        let hidden = compute_chrome_layout(size, 1.0, SidebarMode::Hidden);
        assert_eq!(hidden.content.x, 6);
        assert_eq!(
            resize_gutter_rects(size, hidden),
            vec![
                PhysicalRect {
                    x: 6,
                    y: 594,
                    width: 994,
                    height: 6,
                },
                PhysicalRect {
                    x: 994,
                    y: 35,
                    width: 6,
                    height: 559,
                },
                PhysicalRect {
                    x: 0,
                    y: 35,
                    width: 6,
                    height: 565,
                },
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_workspace_footer_does_not_cover_the_bottom_resize_gutter() {
        let (y, height) = sidebar_footer_metrics(600, 1.0);
        assert_eq!(y + height, 594);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_workspace_footer_does_not_cover_the_bottom_resize_gutter() {
        let (y, height) = sidebar_footer_metrics(600, 1.0);
        let new_workspace = PhysicalRect {
            x: 0,
            y: i32::try_from(y).unwrap(),
            width: 220,
            height,
        };

        assert_eq!(new_workspace.bottom(), 590);
        assert!(new_workspace.contains(0.0, 589.0));
        assert!(!new_workspace.contains(0.0, 590.0));
        assert!(!new_workspace.contains(0.0, 599.0));
    }

    #[test]
    fn sidebar_mode_cycles_through_all_presentations() {
        assert_eq!(SidebarMode::Expanded.next(), SidebarMode::Compact);
        assert_eq!(SidebarMode::Compact.next(), SidebarMode::Hidden);
        assert_eq!(SidebarMode::Hidden.next(), SidebarMode::Expanded);
    }

    #[test]
    fn tab_title_clip_stays_inside_tab_padding() {
        let clip = tab_title_clip(
            PhysicalRect {
                x: 73,
                y: 0,
                width: 150,
                height: 35,
            },
            1.0,
        )
        .unwrap();

        assert_eq!(clip, Rect::new(85.0, 0.0, 211.0, 35.0));
    }

    #[test]
    fn tab_title_clip_omits_tabs_without_room_for_padding() {
        assert!(
            tab_title_clip(
                PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 23,
                    height: 35,
                },
                1.0,
            )
            .is_none()
        );
    }

    #[test]
    fn application_controls_are_right_aligned_and_distinct() {
        let bar = PhysicalRect {
            x: 220,
            y: 0,
            width: 780,
            height: 35,
        };
        let width = CHROME_CONTROL_LOGICAL as u32;
        let app_start = bar
            .width
            .saturating_sub(system_control_width(1.0))
            .saturating_sub(width * 3);
        let horizontal = control_rect(app_start, bar, width);
        let vertical = control_rect(app_start + width, bar, width);
        let gear = control_rect(app_start + width * 2, bar, width);
        assert!(horizontal.x < vertical.x && vertical.x < gear.x);
        assert_eq!(
            gear.right(),
            1000 - i32::try_from(system_control_width(1.0)).unwrap()
        );
    }

    #[test]
    fn settings_menu_is_anchored_to_gear_and_clamped_to_window() {
        let rect = settings_menu_rect(
            PhysicalRect {
                x: 560,
                y: 0,
                width: 34,
                height: 35,
            },
            PhysicalSize::new(600, 400),
            1.0,
        );
        assert_eq!(
            rect,
            PhysicalRect {
                x: 404,
                y: 35,
                width: 190,
                height: 102
            }
        );
    }

    #[test]
    fn gear_path_teeth_span_root_to_tip_radii() {
        let center = (20.0, 20.0);
        let path = gear_path(center, 2.0);
        let rim = path
            .elements()
            .iter()
            .take_while(|element| !matches!(element, PathEl::ClosePath))
            .filter_map(|element| match element {
                PathEl::MoveTo(point) | PathEl::LineTo(point) => Some(*point),
                PathEl::QuadTo(_, point) => Some(*point),
                _ => None,
            })
            .map(|point| point.distance(Point::new(center.0, center.1)))
            .collect::<Vec<_>>();

        // Four points per tooth, plus the closing repeat of the starting root point.
        assert_eq!(rim.len(), GEAR_TEETH * 4 + 1);
        let tip = GEAR_TIP_LOGICAL * 2.0;
        let root = GEAR_ROOT_LOGICAL * 2.0;
        assert!(rim.iter().all(|radius| *radius >= root - 1e-6));
        assert!(rim.iter().all(|radius| *radius <= tip + 1e-6));
        assert_eq!(
            rim.iter()
                .filter(|radius| (*radius - tip).abs() < 1e-6)
                .count(),
            GEAR_TEETH * 2
        );
        assert_eq!(
            rim.iter()
                .filter(|radius| (*radius - root).abs() < 1e-6)
                .count(),
            GEAR_TEETH * 2 + 1
        );
    }

    #[test]
    fn gear_path_hub_is_a_hole_inside_the_rim() {
        let center = (20.0, 20.0);
        let path = gear_path(center, 2.0);
        // Everything after the rim's `ClosePath` is the hub subpath.
        let hub = path
            .elements()
            .iter()
            .skip_while(|element| !matches!(element, PathEl::ClosePath))
            .skip(1)
            .filter_map(|element| match element {
                PathEl::MoveTo(point) | PathEl::LineTo(point) => Some(*point),
                PathEl::CurveTo(_, _, point) => Some(*point),
                _ => None,
            })
            .map(|point| point.distance(Point::new(center.0, center.1)))
            .collect::<Vec<_>>();

        assert!(!hub.is_empty());
        assert!(hub.iter().all(|radius| *radius < GEAR_ROOT_LOGICAL * 2.0));
    }

    #[test]
    fn settings_menu_rows_map_to_items_and_reject_the_margins() {
        let size = PhysicalSize::new(190, 102);
        assert_eq!(
            settings_menu_item_at(size, 1.0, PhysicalPosition::new(10.0, 5.0)),
            Some(SettingsMenuItem::Settings)
        );
        assert_eq!(
            settings_menu_item_at(size, 1.0, PhysicalPosition::new(10.0, 40.0)),
            Some(SettingsMenuItem::Shortcuts)
        );
        assert_eq!(
            settings_menu_item_at(size, 1.0, PhysicalPosition::new(10.0, 101.0)),
            Some(SettingsMenuItem::Documentation)
        );
        assert_eq!(
            settings_menu_item_at(size, 1.0, PhysicalPosition::new(10.0, 102.0)),
            None
        );
        assert_eq!(
            settings_menu_item_at(size, 1.0, PhysicalPosition::new(-1.0, 40.0)),
            None
        );
    }

    #[test]
    fn the_shortcuts_panel_is_centred_and_clamped_to_the_chrome() {
        let (width, height) = shortcuts_logical_size();
        let panel = shortcuts_rect(PhysicalSize::new(1000, 800), 1.0);
        assert_eq!(panel.width, width as u32);
        assert_eq!(panel.height, height as u32);
        assert_eq!(panel.x, (1000 - width as i32) / 2);

        // A chrome smaller than the panel gets a panel trimmed to fit, still fully on screen.
        let cramped = shortcuts_rect(PhysicalSize::new(200, 150), 1.0);
        assert_eq!(cramped.x, 0);
        assert_eq!(cramped.y, 0);
        assert_eq!(cramped.width, 200);
        assert_eq!(cramped.height, 150);
    }

    #[test]
    fn the_shortcuts_close_button_sits_in_the_header_trailing_corner() {
        let panel = PhysicalRect {
            x: 100,
            y: 40,
            width: 460,
            height: 560,
        };
        let close = shortcuts_close_rect(panel, 1.0);
        assert_eq!(close.right(), panel.right());
        assert_eq!(close.y, panel.y);
        assert_eq!(close.height as f64, shortcuts_header_height(1.0));
    }

    #[test]
    fn fullscreen_removes_macos_traffic_light_reservation() {
        assert_eq!(leading_control_inset(1.0, true), 0);
        if cfg!(target_os = "macos") {
            assert_eq!(leading_control_inset(1.0, false), 64);
        } else {
            assert_eq!(leading_control_inset(1.0, false), 0);
        }
    }
}
