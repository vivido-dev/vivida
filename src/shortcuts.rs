//! The keyboard shortcuts the gear menu's "Shortcuts" row puts on screen.
//!
//! Vivida consumes a shortcut before the pane ever sees it ([`handle_shell_shortcut`]), so a
//! binding listed under `Terminal` is one that survives that pass. `Mod+Shift+B` reaches the
//! sidebar rather than Vivido's search-backward, for instance, so it appears only once and in the
//! section that actually handles it.
//!
//! [`handle_shell_shortcut`]: crate::Shell::handle_shell_shortcut

/// One titled block of rows in the shortcuts window.
pub struct Section {
    pub title: &'static str,
    pub rows: &'static [Row],
}

/// A shortcut and what it does. `keys` is already formatted for this platform.
pub struct Row {
    pub keys: &'static str,
    pub description: &'static str,
}

const fn row(keys: &'static str, description: &'static str) -> Row {
    Row { keys, description }
}

#[cfg(target_os = "macos")]
const VIVIDA: &[Row] = &[
    row("⌘T", "New tab"),
    row("⌘D", "Split the pane left and right"),
    row("⌘⇧D", "Split the pane top and bottom"),
    row("⌘W", "Close the focused pane"),
    row("⌘⇧N", "New workspace"),
    row("⌘⇧W", "Close the workspace"),
    row("⌘⇧B", "Expand, shrink, or hide the sidebar"),
    row("⌘⇧]", "Next tab"),
    row("⌘⇧[", "Previous tab"),
    row("⌘1 – ⌘9", "Switch to workspace 1 through 9"),
    row("⌃⇧F12", "Recover a stuck terminal"),
];

#[cfg(not(target_os = "macos"))]
const VIVIDA: &[Row] = &[
    row("Ctrl T", "New tab"),
    row("Ctrl D", "Split the pane left and right"),
    row("Ctrl Shift D", "Split the pane top and bottom"),
    row("Ctrl W", "Close the focused pane"),
    row("Ctrl Shift N", "New workspace"),
    row("Ctrl Shift W", "Close the workspace"),
    row("Ctrl Shift B", "Expand, shrink, or hide the sidebar"),
    row("Ctrl Shift ]", "Next tab"),
    row("Ctrl Shift [", "Previous tab"),
    row("Ctrl 1 – Ctrl 9", "Switch to workspace 1 through 9"),
    row("Ctrl Shift F12", "Recover a stuck terminal"),
];

#[cfg(target_os = "macos")]
const TERMINAL: &[Row] = &[
    row("⌘C", "Copy the selection"),
    row("⌘V", "Paste"),
    row("⌘F", "Search forward"),
    row("⌘B", "Search backward"),
    row("⌘K", "Clear the scrollback"),
    row("⌘0", "Reset the font size"),
    row("⌘+ / ⌘−", "Increase or decrease the font size"),
    row("⌃⌘F", "Toggle fullscreen"),
    row("⌃⇧O", "Open a link shown on screen"),
    row("⇧PageUp / ⇧PageDown", "Scroll one page"),
    row("⇧Home / ⇧End", "Scroll to the top or bottom"),
];

#[cfg(not(target_os = "macos"))]
const TERMINAL: &[Row] = &[
    row("Ctrl Shift C", "Copy the selection"),
    row("Ctrl Shift V", "Paste"),
    row("Shift Insert", "Paste the primary selection"),
    row("Ctrl Shift F", "Search forward"),
    row("Ctrl 0", "Reset the font size"),
    row("Ctrl + / Ctrl −", "Increase or decrease the font size"),
    row("Ctrl Shift O", "Open a link shown on screen"),
    row("Shift PageUp / Shift PageDown", "Scroll one page"),
    row("Shift Home / Shift End", "Scroll to the top or bottom"),
];

const SEARCH: &[Row] = &[
    row("Enter", "Confirm the match"),
    row("Escape", "Cancel the search"),
    row("F3 / Shift F3", "Next or previous match"),
    row("Ctrl U", "Clear the query"),
    row("Ctrl P / Ctrl N", "Previous or next search in history"),
];

const SECTIONS: &[Section] = &[
    Section {
        title: "Vivida",
        rows: VIVIDA,
    },
    Section {
        title: "Terminal",
        rows: TERMINAL,
    },
    Section {
        title: "While searching",
        rows: SEARCH,
    },
];

pub fn sections() -> &'static [Section] {
    SECTIONS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_lists_described_shortcuts() {
        assert!(!sections().is_empty());
        for section in sections() {
            assert!(!section.title.is_empty());
            assert!(!section.rows.is_empty(), "{} has no rows", section.title);
            for row in section.rows {
                assert!(
                    !row.keys.is_empty(),
                    "{} has an unlabelled row",
                    section.title
                );
                assert!(
                    !row.description.is_empty(),
                    "{} has an undescribed row {}",
                    section.title,
                    row.keys
                );
            }
        }
    }
}
