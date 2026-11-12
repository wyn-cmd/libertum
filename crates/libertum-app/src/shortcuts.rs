//! The single keyboard shortcut map (PLAN §5: `Cmd` on macOS, `Ctrl` elsewhere).
//!
//! Every shortcut is declared here as a [`Shortcut`] plus a binding in [`lookup`]. Iced's
//! `Modifiers::command()` already resolves to Cmd on macOS and Ctrl elsewhere.

use iced::keyboard::{self, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    ToggleSidebar,
    /// Cmd/Ctrl+S, Cmd/Ctrl+Shift+S and Cmd/Ctrl+O.
    Save,
    SaveAs,
    Open,
    /// Escape: close an open menu or colour popover.
    CloseMenu,
    /// Cmd+Return (macOS) / Ctrl+Enter: start the rest of the paragraph on a new page.
    PageBreak,
    /// Cmd/Ctrl+B, I, U: toggle bold, italic, underline on the selection.
    Bold,
    Italic,
    Underline,
    /// Cmd/Ctrl+Shift+= and Cmd/Ctrl+= (Word's bindings): superscript and subscript.
    Superscript,
    Subscript,
    /// Ctrl+L, E, R, J (Windows and Linux, as Word) or Cmd+{, |, } (macOS, as Pages and
    /// TextEdit; macOS has no standard justify binding): paragraph alignment.
    AlignLeft,
    AlignCentre,
    AlignRight,
    AlignJustify,
    /// Cmd/Ctrl+F: the Find & Replace bar.
    Find,
    /// F3 or Cmd/Ctrl+G: next match.
    FindNext,
    /// Shift+F3 or Cmd/Ctrl+Shift+G: previous match.
    FindPrevious,
    /// Cmd/Ctrl+,: the Settings window.
    Settings,
    /// Cmd/Ctrl+W: back to the home screen (closing the document).
    CloseDocument,
    /// F11 (Windows and Linux) or Ctrl+Cmd+F (macOS): full screen on or off.
    FullScreen,
}

/// Resolve a key press to a shortcut, if one is bound.
pub fn lookup(key: &Key, modifiers: Modifiers) -> Option<Shortcut> {
    if matches!(key.as_ref(), Key::Named(keyboard::key::Named::Escape)) {
        return Some(Shortcut::CloseMenu);
    }
    if !cfg!(target_os = "macos")
        && matches!(key.as_ref(), Key::Named(keyboard::key::Named::F11))
        && !modifiers.alt()
        && !modifiers.command()
        && !modifiers.shift()
    {
        return Some(Shortcut::FullScreen);
    }
    if matches!(key.as_ref(), Key::Named(keyboard::key::Named::F3))
        && !modifiers.alt()
        && !modifiers.command()
    {
        return Some(if modifiers.shift() {
            Shortcut::FindPrevious
        } else {
            Shortcut::FindNext
        });
    }
    if matches!(key.as_ref(), Key::Named(keyboard::key::Named::Enter))
        && modifiers.command()
        && !modifiers.alt()
        && !modifiers.shift()
    {
        return Some(Shortcut::PageBreak);
    }
    let Key::Character(c) = key.as_ref() else {
        return None;
    };
    if cfg!(target_os = "macos")
        && modifiers.logo()
        && modifiers.control()
        && !modifiers.alt()
        && c.eq_ignore_ascii_case("f")
    {
        return Some(Shortcut::FullScreen);
    }
    if modifiers.command() && modifiers.alt() && !modifiers.shift() && c.eq_ignore_ascii_case("i") {
        return Some(Shortcut::ToggleSidebar);
    }
    if modifiers.command() && !modifiers.alt() {
        if c.eq_ignore_ascii_case("s") {
            return Some(if modifiers.shift() {
                Shortcut::SaveAs
            } else {
                Shortcut::Save
            });
        }
        if c.eq_ignore_ascii_case("o") && !modifiers.shift() {
            return Some(Shortcut::Open);
        }
        if c.eq_ignore_ascii_case("f") && !modifiers.shift() {
            return Some(Shortcut::Find);
        }
        if c == "," && !modifiers.shift() {
            return Some(Shortcut::Settings);
        }
        if c.eq_ignore_ascii_case("w") && !modifiers.shift() {
            return Some(Shortcut::CloseDocument);
        }
        if c.eq_ignore_ascii_case("g") {
            return Some(if modifiers.shift() {
                Shortcut::FindPrevious
            } else {
                Shortcut::FindNext
            });
        }
        // With Shift, "=" usually arrives as "+" (the shifted character on most layouts).
        match (c, modifiers.shift()) {
            ("+", _) | ("=", true) => return Some(Shortcut::Superscript),
            ("=", false) => return Some(Shortcut::Subscript),
            _ => {}
        }
        if cfg!(target_os = "macos") {
            match c {
                "{" | "[" if modifiers.shift() => return Some(Shortcut::AlignLeft),
                "|" | "\\" if modifiers.shift() => return Some(Shortcut::AlignCentre),
                "}" | "]" if modifiers.shift() => return Some(Shortcut::AlignRight),
                _ => {}
            }
        } else if !modifiers.shift() {
            let lower = c.to_ascii_lowercase();
            match lower.as_str() {
                "l" => return Some(Shortcut::AlignLeft),
                "e" => return Some(Shortcut::AlignCentre),
                "r" => return Some(Shortcut::AlignRight),
                "j" => return Some(Shortcut::AlignJustify),
                _ => {}
            }
        }
    }
    if modifiers.command() && !modifiers.alt() && !modifiers.shift() {
        if c.eq_ignore_ascii_case("b") {
            return Some(Shortcut::Bold);
        }
        if c.eq_ignore_ascii_case("i") {
            return Some(Shortcut::Italic);
        }
        if c.eq_ignore_ascii_case("u") {
            return Some(Shortcut::Underline);
        }
    }
    None
}

/// "Cmd+X" on macOS, "Ctrl+X" elsewhere: a command-modified character, for menus.
pub fn command_key_label(key: &str) -> &'static str {
    match (cfg!(target_os = "macos"), key) {
        (true, "X") => "Cmd+X",
        (true, "C") => "Cmd+C",
        (true, "V") => "Cmd+V",
        (true, "A") => "Cmd+A",
        (false, "X") => "Ctrl+X",
        (false, "C") => "Ctrl+C",
        (false, "V") => "Ctrl+V",
        (false, "A") => "Ctrl+A",
        _ => "",
    }
}

pub fn from_event(event: &keyboard::Event) -> Option<Shortcut> {
    match event {
        keyboard::Event::KeyPressed {
            key,
            modifiers,
            repeat: false,
            ..
        } => lookup(key, *modifiers),
        _ => None,
    }
}

/// Human-readable binding for tooltips and menus.
pub fn label(shortcut: Shortcut) -> &'static str {
    match shortcut {
        Shortcut::Find => {
            if cfg!(target_os = "macos") {
                "Cmd+F"
            } else {
                "Ctrl+F"
            }
        }
        Shortcut::FindNext => "F3",
        Shortcut::FullScreen => {
            if cfg!(target_os = "macos") {
                "Ctrl+Cmd+F"
            } else {
                "F11"
            }
        }
        Shortcut::FindPrevious => "Shift+F3",
        Shortcut::CloseDocument => {
            if cfg!(target_os = "macos") {
                "Cmd+W"
            } else {
                "Ctrl+W"
            }
        }
        Shortcut::Settings => {
            if cfg!(target_os = "macos") {
                "Cmd+,"
            } else {
                "Ctrl+,"
            }
        }
        Shortcut::Save => {
            if cfg!(target_os = "macos") {
                "Cmd+S"
            } else {
                "Ctrl+S"
            }
        }
        Shortcut::SaveAs => {
            if cfg!(target_os = "macos") {
                "Cmd+Shift+S"
            } else {
                "Ctrl+Shift+S"
            }
        }
        Shortcut::Open => {
            if cfg!(target_os = "macos") {
                "Cmd+O"
            } else {
                "Ctrl+O"
            }
        }
        Shortcut::ToggleSidebar => {
            if cfg!(target_os = "macos") {
                "Cmd+Alt+I"
            } else {
                "Ctrl+Alt+I"
            }
        }
        Shortcut::CloseMenu => "Esc",
        Shortcut::PageBreak => {
            if cfg!(target_os = "macos") {
                "Cmd+Return"
            } else {
                "Ctrl+Enter"
            }
        }
        Shortcut::Bold => {
            if cfg!(target_os = "macos") {
                "Cmd+B"
            } else {
                "Ctrl+B"
            }
        }
        Shortcut::Italic => {
            if cfg!(target_os = "macos") {
                "Cmd+I"
            } else {
                "Ctrl+I"
            }
        }
        Shortcut::Underline => {
            if cfg!(target_os = "macos") {
                "Cmd+U"
            } else {
                "Ctrl+U"
            }
        }
        Shortcut::Superscript => {
            if cfg!(target_os = "macos") {
                "Cmd+Shift+="
            } else {
                "Ctrl+Shift+="
            }
        }
        Shortcut::Subscript => {
            if cfg!(target_os = "macos") {
                "Cmd+="
            } else {
                "Ctrl+="
            }
        }
        Shortcut::AlignLeft => {
            if cfg!(target_os = "macos") {
                "Cmd+{"
            } else {
                "Ctrl+L"
            }
        }
        Shortcut::AlignCentre => {
            if cfg!(target_os = "macos") {
                "Cmd+|"
            } else {
                "Ctrl+E"
            }
        }
        Shortcut::AlignRight => {
            if cfg!(target_os = "macos") {
                "Cmd+}"
            } else {
                "Ctrl+R"
            }
        }
        Shortcut::AlignJustify => {
            if cfg!(target_os = "macos") {
                ""
            } else {
                "Ctrl+J"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_alt() -> Modifiers {
        let command = if cfg!(target_os = "macos") {
            Modifiers::LOGO
        } else {
            Modifiers::CTRL
        };
        command | Modifiers::ALT
    }

    #[test]
    fn command_alt_i_toggles_sidebar() {
        let key = Key::Character("i".into());
        assert_eq!(lookup(&key, command_alt()), Some(Shortcut::ToggleSidebar));
        let upper = Key::Character("I".into());
        assert_eq!(lookup(&upper, command_alt()), Some(Shortcut::ToggleSidebar));
    }

    fn command() -> Modifiers {
        if cfg!(target_os = "macos") {
            Modifiers::LOGO
        } else {
            Modifiers::CTRL
        }
    }

    #[test]
    fn command_enter_is_a_page_break_on_every_platform() {
        let enter = Key::Named(keyboard::key::Named::Enter);
        assert_eq!(lookup(&enter, command()), Some(Shortcut::PageBreak));
        // Plain Enter, Shift+Enter and Alt+Enter stay text input.
        assert_eq!(lookup(&enter, Modifiers::NONE), None);
        assert_eq!(lookup(&enter, command() | Modifiers::SHIFT), None);
        assert_eq!(lookup(&enter, command() | Modifiers::ALT), None);
        // On macOS, Ctrl+Return is not Cmd+Return.
        if cfg!(target_os = "macos") {
            assert_eq!(lookup(&enter, Modifiers::CTRL), None);
        }
    }

    #[test]
    fn command_b_i_u_toggle_character_formatting() {
        let key = |c: &str| Key::Character(c.into());
        assert_eq!(lookup(&key("b"), command()), Some(Shortcut::Bold));
        assert_eq!(lookup(&key("I"), command()), Some(Shortcut::Italic));
        assert_eq!(lookup(&key("u"), command()), Some(Shortcut::Underline));
        // Cmd/Ctrl+Alt+I is still the sidebar, and plain letters are text.
        assert_eq!(
            lookup(&key("i"), command() | Modifiers::ALT),
            Some(Shortcut::ToggleSidebar)
        );
        assert_eq!(lookup(&key("b"), Modifiers::NONE), None);
    }

    #[test]
    fn superscript_subscript_and_alignment_shortcuts() {
        let key = |c: &str| Key::Character(c.into());
        let shift = command() | Modifiers::SHIFT;
        assert_eq!(lookup(&key("+"), shift), Some(Shortcut::Superscript));
        assert_eq!(lookup(&key("="), shift), Some(Shortcut::Superscript));
        assert_eq!(lookup(&key("="), command()), Some(Shortcut::Subscript));
        assert_eq!(lookup(&key("="), Modifiers::NONE), None);
        if cfg!(target_os = "macos") {
            assert_eq!(lookup(&key("{"), shift), Some(Shortcut::AlignLeft));
            assert_eq!(lookup(&key("|"), shift), Some(Shortcut::AlignCentre));
            assert_eq!(lookup(&key("}"), shift), Some(Shortcut::AlignRight));
        } else {
            assert_eq!(lookup(&key("l"), command()), Some(Shortcut::AlignLeft));
            assert_eq!(lookup(&key("E"), command()), Some(Shortcut::AlignCentre));
            assert_eq!(lookup(&key("r"), command()), Some(Shortcut::AlignRight));
            assert_eq!(lookup(&key("j"), command()), Some(Shortcut::AlignJustify));
            assert_eq!(lookup(&key("e"), Modifiers::NONE), None);
        }
    }

    #[test]
    fn escape_closes_menus() {
        let key = Key::Named(keyboard::key::Named::Escape);
        assert_eq!(lookup(&key, Modifiers::NONE), Some(Shortcut::CloseMenu));
    }

    #[test]
    fn file_shortcuts_are_bound() {
        let command = if cfg!(target_os = "macos") {
            Modifiers::LOGO
        } else {
            Modifiers::CTRL
        };
        assert_eq!(
            lookup(&Key::Character("s".into()), command),
            Some(Shortcut::Save)
        );
        assert_eq!(
            lookup(&Key::Character("S".into()), command | Modifiers::SHIFT),
            Some(Shortcut::SaveAs)
        );
        assert_eq!(
            lookup(&Key::Character("o".into()), command),
            Some(Shortcut::Open)
        );
    }

    #[test]
    fn other_combinations_are_unbound() {
        let key = Key::Character("i".into());
        assert_eq!(lookup(&key, Modifiers::ALT), None);
        assert_eq!(lookup(&key, Modifiers::NONE), None);
        assert_eq!(lookup(&Key::Character("x".into()), command_alt()), None);
    }
}
