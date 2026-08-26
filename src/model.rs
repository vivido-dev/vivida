use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use winit::window::WindowId;

use crate::layout::{Axis, Node};

pub const MAX_NAME_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    ControlCharacter,
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("name must not be empty"),
            Self::ControlCharacter => {
                formatter.write_str("name must not contain control characters")
            }
        }
    }
}

impl std::error::Error for NameError {}

pub fn normalize_name(name: &str) -> Result<String, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().any(char::is_control) {
        return Err(NameError::ControlCharacter);
    }
    Ok(name.chars().take(MAX_NAME_CHARS).collect())
}

pub fn names_equal(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_lowercase)
        .eq(right.chars().flat_map(char::to_lowercase))
}

pub fn automatic_name(name: &str, fallback: &str) -> String {
    let filtered = name
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    normalize_name(&filtered)
        .or_else(|_| normalize_name(fallback))
        .expect("the internal fallback name is valid")
}

pub fn unique_name<'a>(base: &str, used: impl IntoIterator<Item = &'a str>) -> String {
    let used = used.into_iter().map(folded_name).collect::<HashSet<_>>();
    if !used.contains(&folded_name(base)) {
        return base.to_owned();
    }
    for ordinal in 2u64.. {
        let suffix = format!(" ({ordinal})");
        let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
        let candidate = format!("{}{}", base.chars().take(keep).collect::<String>(), suffix);
        if !used.contains(&folded_name(&candidate)) {
            return candidate;
        }
    }
    unreachable!("the unbounded ordinal space must contain an unused name")
}

fn folded_name(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneKey {
    pub workspace_id: WorkspaceId,
    pub tab_id: TabId,
    pub pane_id: PaneId,
}

#[derive(Debug)]
pub struct Tab {
    pub id: TabId,
    pub root: Node,
    pub title: String,
    context_title: String,
    custom_title: Option<String>,
    pub focused_pane: PaneId,
    pub panes: BTreeMap<PaneId, WindowId>,
    next_pane_id: u64,
}

impl Tab {
    pub fn new(id: TabId, pane_window_id: WindowId) -> Self {
        let pane_id = PaneId(1);
        Self {
            id,
            root: Node::Leaf(pane_id),
            title: "shell".to_owned(),
            context_title: "shell".to_owned(),
            custom_title: None,
            focused_pane: pane_id,
            panes: BTreeMap::from([(pane_id, pane_window_id)]),
            next_pane_id: 2,
        }
    }

    pub fn restore(
        id: TabId,
        root: Node,
        title: String,
        custom_title: bool,
        focused_pane: PaneId,
        panes: BTreeMap<PaneId, WindowId>,
    ) -> Self {
        let next_pane_id = panes.keys().map(|id| id.0).max().unwrap_or(0) + 1;
        let focused_pane = if panes.contains_key(&focused_pane) {
            focused_pane
        } else {
            root.first_pane()
        };
        let title = automatic_name(&title, "shell");
        Self {
            id,
            root,
            title: title.clone(),
            context_title: if custom_title {
                "shell".to_owned()
            } else {
                title.clone()
            },
            custom_title: custom_title.then_some(title),
            focused_pane,
            panes,
            next_pane_id,
        }
    }

    pub fn is_title_custom(&self) -> bool {
        self.custom_title.is_some()
    }

    pub fn persisted_title(&self) -> &str {
        self.custom_title.as_deref().unwrap_or(&self.context_title)
    }

    pub fn set_context_title(&mut self, title: &str) -> bool {
        if self.custom_title.is_some() {
            return false;
        }
        let title = automatic_name(title, "shell");
        if self.context_title == title {
            return false;
        }
        self.context_title = title;
        true
    }

    pub fn set_custom_title(&mut self, title: String) {
        self.title.clone_from(&title);
        self.custom_title = Some(title);
    }

    pub fn reset_title(&mut self) {
        self.custom_title = None;
    }

    pub fn window_id(&self, pane_id: PaneId) -> Option<WindowId> {
        self.panes.get(&pane_id).copied()
    }

    pub fn focused_window_id(&self) -> Option<WindowId> {
        self.window_id(self.focused_pane)
            .or_else(|| self.window_id(self.root.first_pane()))
    }

    pub fn split(&mut self, axis: Axis, pane_window_id: WindowId) -> Option<PaneId> {
        let pane_id = PaneId(self.next_pane_id);
        if !self.root.split(self.focused_pane, pane_id, axis) {
            return None;
        }
        self.next_pane_id += 1;
        self.panes.insert(pane_id, pane_window_id);
        self.focused_pane = pane_id;
        Some(pane_id)
    }

    pub fn remove_pane(&mut self, pane_id: PaneId) -> bool {
        if self.panes.remove(&pane_id).is_none() {
            return false;
        }
        if self.panes.is_empty() {
            return true;
        }
        self.root.remove(pane_id);
        if self.focused_pane == pane_id {
            self.focused_pane = self.root.first_pane();
        }
        true
    }
}

#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub label: String,
    pub identity_cwd: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: TabId,
    next_tab_id: u64,
}

impl Workspace {
    pub fn new(id: WorkspaceId, label: String, identity_cwd: PathBuf, pane_id: WindowId) -> Self {
        let tab_id = TabId(1);
        Self {
            id,
            label,
            identity_cwd,
            tabs: vec![Tab::new(tab_id, pane_id)],
            active_tab: tab_id,
            next_tab_id: 2,
        }
    }

    pub fn restore(
        id: WorkspaceId,
        label: String,
        identity_cwd: PathBuf,
        tabs: Vec<Tab>,
        active_tab: TabId,
    ) -> Self {
        let next_tab_id = tabs.iter().map(|tab| tab.id.0).max().unwrap_or(0) + 1;
        let active_tab = if tabs.iter().any(|tab| tab.id == active_tab) {
            active_tab
        } else {
            tabs.first().map_or(TabId(1), |tab| tab.id)
        };
        Self {
            id,
            label,
            identity_cwd,
            tabs,
            active_tab,
            next_tab_id,
        }
    }

    pub fn add_tab(&mut self, pane_window_id: WindowId) -> TabId {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(id, pane_window_id));
        self.active_tab = id;
        id
    }

    pub fn switch_tab(&mut self, tab_id: TabId) -> bool {
        if self.active_tab == tab_id || !self.tabs.iter().any(|tab| tab.id == tab_id) {
            return false;
        }
        self.active_tab = tab_id;
        true
    }

    pub fn remove_empty_tabs(&mut self) {
        self.tabs.retain(|tab| !tab.panes.is_empty());
        if !self.tabs.iter().any(|tab| tab.id == self.active_tab) {
            self.active_tab = self.tabs.first().map_or(TabId(1), |tab| tab.id);
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == self.active_tab)
    }

    pub fn pane_count(&self) -> usize {
        self.tabs.iter().map(|tab| tab.panes.len()).sum()
    }

    pub fn refresh_tab_display_titles(&mut self) -> bool {
        let mut used = HashSet::new();
        let mut changed = false;
        for tab in self
            .tabs
            .iter_mut()
            .filter(|tab| tab.custom_title.is_some())
        {
            let custom = tab.custom_title.as_deref().unwrap_or_default();
            let display = unique_name(custom, used.iter().map(String::as_str));
            used.insert(folded_name(&display));
            if tab.custom_title.as_deref() != Some(display.as_str()) {
                tab.custom_title = Some(display.clone());
                changed = true;
            }
            if tab.title != display {
                tab.title = display;
                changed = true;
            }
        }
        for tab in &mut self.tabs {
            if tab.custom_title.is_some() {
                continue;
            }
            let display = unique_name(&tab.context_title, used.iter().map(String::as_str));
            used.insert(folded_name(&display));
            if tab.title != display {
                tab.title = display;
                changed = true;
            }
        }
        changed
    }
}

pub fn remove_owned_pane(workspaces: &mut [Workspace], key: PaneKey) -> bool {
    let Some(workspace) = workspaces
        .iter_mut()
        .find(|workspace| workspace.id == key.workspace_id)
    else {
        return false;
    };
    let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == key.tab_id) else {
        return false;
    };
    tab.remove_pane(key.pane_id)
}

pub fn focus_owned_pane(
    workspaces: &mut [Workspace],
    active_workspace: Option<WorkspaceId>,
    key: PaneKey,
    window_id: WindowId,
) -> bool {
    if active_workspace != Some(key.workspace_id) {
        return false;
    }
    let Some(workspace) = workspaces
        .iter_mut()
        .find(|workspace| workspace.id == key.workspace_id && workspace.active_tab == key.tab_id)
    else {
        return false;
    };
    let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == key.tab_id) else {
        return false;
    };
    if tab.window_id(key.pane_id) != Some(window_id) {
        return false;
    }
    tab.focused_pane = key.pane_id;
    true
}

pub fn workspace_label(cwd: &std::path::Path) -> String {
    automatic_name(
        cwd.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("workspace"),
        "workspace",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_pane_ids_restart_per_workspace() {
        let first = Workspace::new(
            WorkspaceId(1),
            "one".into(),
            "/one".into(),
            WindowId::from(10),
        );
        let second = Workspace::new(
            WorkspaceId(2),
            "two".into(),
            "/two".into(),
            WindowId::from(20),
        );
        assert_eq!(first.active_tab().unwrap().focused_pane, PaneId(1));
        assert_eq!(second.active_tab().unwrap().focused_pane, PaneId(1));
    }

    #[test]
    fn owner_scoped_removal_preserves_reused_pane_id() {
        let first = Workspace::new(
            WorkspaceId(1),
            "one".into(),
            "/one".into(),
            WindowId::from(10),
        );
        let second = Workspace::new(
            WorkspaceId(2),
            "two".into(),
            "/two".into(),
            WindowId::from(20),
        );
        let mut workspaces = vec![first, second];

        assert!(remove_owned_pane(
            &mut workspaces,
            PaneKey {
                workspace_id: WorkspaceId(1),
                tab_id: TabId(1),
                pane_id: PaneId(1)
            },
        ));
        assert_eq!(workspaces[0].pane_count(), 0);
        assert_eq!(workspaces[1].pane_count(), 1);
        assert_eq!(
            workspaces[1].active_tab().unwrap().window_id(PaneId(1)),
            Some(WindowId::from(20))
        );
    }

    #[test]
    fn owner_scoped_focus_changes_only_the_clicked_active_pane() {
        let mut first = Workspace::new(
            WorkspaceId(1),
            "one".into(),
            "/one".into(),
            WindowId::from(10),
        );
        first
            .active_tab_mut()
            .unwrap()
            .split(Axis::Horizontal, WindowId::from(11))
            .unwrap();
        let mut second = Workspace::new(
            WorkspaceId(2),
            "two".into(),
            "/two".into(),
            WindowId::from(20),
        );
        second
            .active_tab_mut()
            .unwrap()
            .split(Axis::Horizontal, WindowId::from(21))
            .unwrap();
        let mut workspaces = vec![first, second];

        assert!(focus_owned_pane(
            &mut workspaces,
            Some(WorkspaceId(1)),
            PaneKey {
                workspace_id: WorkspaceId(1),
                tab_id: TabId(1),
                pane_id: PaneId(1),
            },
            WindowId::from(10),
        ));
        assert_eq!(workspaces[0].active_tab().unwrap().focused_pane, PaneId(1));
        assert_eq!(workspaces[1].active_tab().unwrap().focused_pane, PaneId(2));

        assert!(!focus_owned_pane(
            &mut workspaces,
            Some(WorkspaceId(1)),
            PaneKey {
                workspace_id: WorkspaceId(2),
                tab_id: TabId(1),
                pane_id: PaneId(1),
            },
            WindowId::from(20),
        ));
        assert!(!focus_owned_pane(
            &mut workspaces,
            Some(WorkspaceId(1)),
            PaneKey {
                workspace_id: WorkspaceId(1),
                tab_id: TabId(1),
                pane_id: PaneId(1),
            },
            WindowId::from(99),
        ));
        assert_eq!(workspaces[0].active_tab().unwrap().focused_pane, PaneId(1));
        assert_eq!(workspaces[1].active_tab().unwrap().focused_pane, PaneId(2));
    }

    #[test]
    fn removing_the_focused_split_selects_the_surviving_pane() {
        let mut workspace = Workspace::new(
            WorkspaceId(1),
            "one".into(),
            "/one".into(),
            WindowId::from(10),
        );
        workspace
            .active_tab_mut()
            .unwrap()
            .split(Axis::Horizontal, WindowId::from(11))
            .unwrap();
        let mut workspaces = vec![workspace];

        assert!(remove_owned_pane(
            &mut workspaces,
            PaneKey {
                workspace_id: WorkspaceId(1),
                tab_id: TabId(1),
                pane_id: PaneId(2),
            },
        ));
        let tab = workspaces[0].active_tab().unwrap();
        assert_eq!(tab.focused_pane, PaneId(1));
        assert_eq!(tab.focused_window_id(), Some(WindowId::from(10)));
    }

    #[test]
    fn label_uses_directory_basename_and_has_a_fallback() {
        assert_eq!(
            workspace_label(std::path::Path::new("/tmp/project")),
            "project"
        );
        assert_eq!(workspace_label(std::path::Path::new("/")), "workspace");
    }

    #[test]
    fn names_are_trimmed_bounded_and_case_insensitive() {
        assert_eq!(normalize_name("  Project A  ").unwrap(), "Project A");
        assert_eq!(
            normalize_name(&"x".repeat(200)).unwrap().chars().count(),
            128
        );
        assert!(normalize_name("bad\nname").is_err());
        assert!(names_equal("RÉSUMÉ", "résumé"));
    }

    #[test]
    fn automatic_tab_names_are_unique_and_custom_names_take_priority() {
        let mut workspace = Workspace::new(
            WorkspaceId(1),
            "one".into(),
            "/one".into(),
            WindowId::from(10),
        );
        let second = workspace.add_tab(WindowId::from(20));
        workspace.tabs[1].set_custom_title("shell".into());
        assert!(workspace.refresh_tab_display_titles());
        assert_eq!(workspace.tabs[0].title, "shell (2)");
        assert_eq!(workspace.tabs[1].title, "shell");
        assert_eq!(workspace.tabs[1].id, second);
    }

    #[test]
    fn pinned_title_ignores_context_until_reset() {
        let mut tab = Tab::new(TabId(1), WindowId::from(10));
        tab.set_custom_title("Editor".into());
        assert!(!tab.set_context_title("server"));
        assert_eq!(tab.title, "Editor");
        tab.reset_title();
        assert!(tab.set_context_title("server"));
    }
}
