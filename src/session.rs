use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::chrome::SidebarMode;
use crate::layout::Node;
use crate::model::{PaneId, TabId, Workspace, WorkspaceId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub sidebar_mode: SidebarMode,
    pub active_workspace: Option<WorkspaceId>,
    pub workspaces: Vec<WorkspaceSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub id: WorkspaceId,
    pub label: String,
    pub identity_cwd: PathBuf,
    pub active_tab: TabId,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub id: TabId,
    pub root: Node,
    pub title: String,
    pub focused_pane: PaneId,
    pub pane_cwds: Vec<(PaneId, PathBuf)>,
}

pub fn path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/vivida/session.json"));
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|data| data.join("vivida/session.json"));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
            return Some(PathBuf::from(state).join("vivida/session.json"));
        }
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".local/state/vivida/session.json"));
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "macos")]
fn legacy_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/vivida-macos/session.json"))
}

pub fn load_current_or_legacy(path: &Path) -> io::Result<Session> {
    match load(path) {
        Ok(session) => Ok(session),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            #[cfg(target_os = "macos")]
            if let Some(legacy) = legacy_path() {
                return load(&legacy);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

pub fn load(path: &Path) -> io::Result<Session> {
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}

pub fn save(path: &Path, session: &Session) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(session).map_err(io::Error::other)?,
    )?;
    fs::rename(temporary, path)
}

impl Session {
    pub fn capture<F>(
        sidebar_mode: SidebarMode,
        active_workspace: Option<WorkspaceId>,
        workspaces: &[Workspace],
        mut cwd: F,
    ) -> Self
    where
        F: FnMut(winit::window::WindowId) -> Option<PathBuf>,
    {
        let workspaces = workspaces
            .iter()
            .map(|workspace| WorkspaceSnapshot {
                id: workspace.id,
                label: workspace.label.clone(),
                identity_cwd: workspace.identity_cwd.clone(),
                active_tab: workspace.active_tab,
                tabs: workspace
                    .tabs
                    .iter()
                    .map(|tab| TabSnapshot {
                        id: tab.id,
                        root: tab.root.clone(),
                        title: tab.title.clone(),
                        focused_pane: tab.focused_pane,
                        pane_cwds: tab
                            .panes
                            .iter()
                            .map(|(pane, window)| {
                                (
                                    *pane,
                                    cwd(*window).unwrap_or_else(|| workspace.identity_cwd.clone()),
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Self {
            sidebar_mode,
            active_workspace,
            workspaces,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_owner_scoped_layouts() {
        let session = Session {
            sidebar_mode: SidebarMode::Compact,
            active_workspace: Some(WorkspaceId(2)),
            workspaces: vec![WorkspaceSnapshot {
                id: WorkspaceId(2),
                label: "repo".into(),
                identity_cwd: "/repo".into(),
                active_tab: TabId(3),
                tabs: vec![TabSnapshot {
                    id: TabId(3),
                    root: Node::Leaf(PaneId(1)),
                    title: "shell".into(),
                    focused_pane: PaneId(1),
                    pane_cwds: vec![(PaneId(1), "/repo".into())],
                }],
            }],
        };
        let encoded = serde_json::to_vec(&session).unwrap();
        let decoded: Session = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.sidebar_mode, SidebarMode::Compact);
        assert_eq!(decoded.active_workspace, Some(WorkspaceId(2)));
        assert_eq!(decoded.workspaces[0].tabs[0].root, Node::Leaf(PaneId(1)));
    }
}
