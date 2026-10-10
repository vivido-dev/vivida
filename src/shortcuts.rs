//! The keyboard shortcuts the gear menu's "Shortcuts" row puts on screen.
//!
//! Vivida consumes a shortcut before the pane ever sees it ([`handle_shell_shortcut`]), so a
//! binding listed under `Terminal` is one that survives that pass. `Mod+Shift+B` reaches the
//! sidebar rather than Vivido's search-backward, for instance, so it appears only once and in the
//! section that actually handles it.
//!
//! [`handle_shell_shortcut`]: crate::Shell::handle_shell_shortcut

use crate::strings::Label;

/// One titled block of rows in the shortcuts window.
pub struct Section {
    pub title: Label,
    pub rows: &'static [Row],
}

/// A shortcut and what it does. `keys` is already formatted for this platform.
pub struct Row {
    pub keys: &'static str,
    pub description: Label,
}

const fn row(keys: &'static str, description: Label) -> Row {
    Row { keys, description }
}

#[cfg(target_os = "macos")]
const VIVIDA: &[Row] = &[
    row("⌘T", Label::NewTab),
    row("⌘D", Label::SplitHorizontal),
    row("⌘⇧D", Label::SplitVertical),
    row("⌘W", Label::CloseFocusedPane),
    row("⌘⇧N", Label::NewSpaceShortcut),
    row("⌘⇧W", Label::CloseSpace),
    row("⌘⇧B", Label::CycleSidebar),
    row("⌘⇧]", Label::NextTab),
    row("⌘⇧[", Label::PreviousTab),
    row("⌘1 – ⌘9", Label::SwitchToSpace),
    row("⌃⇧F12", Label::RecoverTerminal),
];

#[cfg(not(target_os = "macos"))]
const VIVIDA: &[Row] = &[
    row("Ctrl T", Label::NewTab),
    row("Ctrl D", Label::SplitHorizontal),
    row("Ctrl Shift D", Label::SplitVertical),
    row("Ctrl W", Label::CloseFocusedPane),
    row("Ctrl Shift N", Label::NewSpaceShortcut),
    row("Ctrl Shift W", Label::CloseSpace),
    row("Ctrl Shift B", Label::CycleSidebar),
    row("Ctrl Shift ]", Label::NextTab),
    row("Ctrl Shift [", Label::PreviousTab),
    row("Ctrl 1 – Ctrl 9", Label::SwitchToSpace),
    row("Ctrl Shift F12", Label::RecoverTerminal),
];

#[cfg(target_os = "macos")]
const TERMINAL: &[Row] = &[
    row("⌘C", Label::CopySelection),
    row("⌘V", Label::Paste),
    row("⌘⇧P", Label::CommandPalette),
    row("⌘F", Label::SearchForward),
    row("⌘B", Label::SearchBackward),
    row("⌘K", Label::ClearScrollback),
    row("⌘0", Label::ResetFontSize),
    row("⌘+ / ⌘−", Label::ChangeFontSize),
    row("⌃⌘F", Label::ToggleFullscreen),
    row("⌃⇧O", Label::OpenLinkOnScreen),
    row("⇧PageUp / ⇧PageDown", Label::ScrollOnePage),
    row("⇧Home / ⇧End", Label::ScrollToEnd),
];

#[cfg(not(target_os = "macos"))]
const TERMINAL: &[Row] = &[
    row("Ctrl Shift C", Label::CopySelection),
    row("Ctrl Shift V", Label::Paste),
    row("Shift Insert", Label::PastePrimarySelection),
    row("Ctrl Shift P", Label::CommandPalette),
    row("Ctrl Shift F", Label::SearchForward),
    row("Ctrl 0", Label::ResetFontSize),
    row("Ctrl + / Ctrl −", Label::ChangeFontSize),
    row("Ctrl Shift O", Label::OpenLinkOnScreen),
    row("Shift PageUp / Shift PageDown", Label::ScrollOnePage),
    row("Shift Home / Shift End", Label::ScrollToEnd),
];

const SEARCH: &[Row] = &[
    row("Enter", Label::ConfirmMatch),
    row("Escape", Label::CancelSearch),
    row("F3 / Shift F3", Label::NextOrPreviousMatch),
    row("Ctrl U", Label::ClearQuery),
    row("Ctrl P / Ctrl N", Label::SearchHistory),
];

const SECTIONS: &[Section] = &[
    Section {
        title: Label::ProductName,
        rows: VIVIDA,
    },
    Section {
        title: Label::SectionTerminal,
        rows: TERMINAL,
    },
    Section {
        title: Label::SectionSearch,
        rows: SEARCH,
    },
];

pub fn sections() -> &'static [Section] {
    SECTIONS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::Locale;

    #[test]
    fn every_section_lists_described_shortcuts_in_every_locale() {
        assert!(!sections().is_empty());
        for locale in Locale::ALL {
            for section in sections() {
                assert!(!section.title.t(*locale).is_empty());
                assert!(
                    !section.rows.is_empty(),
                    "{} has no rows",
                    section.title.t(*locale)
                );
                for row in section.rows {
                    assert!(
                        !row.keys.is_empty(),
                        "{} has an unlabelled row",
                        section.title.t(*locale)
                    );
                    assert!(
                        !row.description.t(*locale).is_empty(),
                        "{} has an undescribed row {}",
                        section.title.t(*locale),
                        row.keys
                    );
                }
            }
        }
    }
}
