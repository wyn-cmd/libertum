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
    /// Cmd/Ctrl+Alt+1 to 4: Heading 1 to 4 on the selected paragraphs; Cmd/Ctrl+Alt+0: body text.
    Heading(u8),
    /// Cmd/Ctrl+N: the home screen, to pick a template for a new document.
    NewDocument,
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
    if modifiers.command()
        && modifiers.alt()
        && !modifiers.shift()
        && let Some(level @ 0..=4) = c.parse::<u8>().ok().filter(|_| c.len() == 1)
    {
        return Some(Shortcut::Heading(level));
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
        if c.eq_ignore_ascii_case("n") && !modifiers.shift() {
            return Some(Shortcut::NewDocument);
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
            physical_key,
            modifiers,
            repeat: false,
            ..
        } => lookup(&latin_key(key, physical_key, *modifiers), *modifiers),
        _ => None,
    }
}

/// The key a shortcut should see. With a Cyrillic, Greek, Arabic, Hebrew or other non-Latin
/// layout active, Ctrl+S reports the layout's own letter for that key (`і` on Ukrainian), so
/// every shortcut would stop working. While Ctrl (Cmd on macOS) is held, a character key whose
/// letter is not Latin is read by where it sits on the keyboard, as it would on a US layout.
/// Without Ctrl the key is left alone, because then it is text. Latin layouts (AZERTY,
/// Dvorak) are untouched: their letters are what the writer sees on the keys.
pub fn latin_key(key: &Key, physical: &keyboard::key::Physical, modifiers: Modifiers) -> Key {
    use keyboard::key::{Code, Physical};
    let Key::Character(text) = key else {
        return key.clone();
    };
    if !modifiers.command() || text.is_ascii() {
        return key.clone();
    }
    let Physical::Code(code) = physical else {
        return key.clone();
    };
    let latin = match code {
        Code::KeyA => "a",
        Code::KeyB => "b",
        Code::KeyC => "c",
        Code::KeyD => "d",
        Code::KeyE => "e",
        Code::KeyF => "f",
        Code::KeyG => "g",
        Code::KeyH => "h",
        Code::KeyI => "i",
        Code::KeyJ => "j",
        Code::KeyK => "k",
        Code::KeyL => "l",
        Code::KeyM => "m",
        Code::KeyN => "n",
        Code::KeyO => "o",
        Code::KeyP => "p",
        Code::KeyQ => "q",
        Code::KeyR => "r",
        Code::KeyS => "s",
        Code::KeyT => "t",
        Code::KeyU => "u",
        Code::KeyV => "v",
        Code::KeyW => "w",
        Code::KeyX => "x",
        Code::KeyY => "y",
        Code::KeyZ => "z",
        Code::Digit0 => "0",
        Code::Digit1 => "1",
        Code::Digit2 => "2",
        Code::Digit3 => "3",
        Code::Digit4 => "4",
        Code::Digit5 => "5",
        Code::Digit6 => "6",
        Code::Digit7 => "7",
        Code::Digit8 => "8",
        Code::Digit9 => "9",
        Code::Comma => ",",
        Code::Period => ".",
        Code::Equal => "=",
        Code::Minus => "-",
        Code::BracketLeft => "[",
        Code::BracketRight => "]",
        Code::Backslash => "\\",
        _ => return key.clone(),
    };
    Key::Character(latin.into())
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
        Shortcut::Heading(0) => {
            if cfg!(target_os = "macos") {
                "Cmd+Alt+0"
            } else {
                "Ctrl+Alt+0"
            }
        }
        Shortcut::Heading(1) => {
            if cfg!(target_os = "macos") {
                "Cmd+Alt+1"
            } else {
                "Ctrl+Alt+1"
            }
        }
        Shortcut::Heading(2) => {
            if cfg!(target_os = "macos") {
                "Cmd+Alt+2"
            } else {
                "Ctrl+Alt+2"
            }
        }
        Shortcut::Heading(3) => {
            if cfg!(target_os = "macos") {
                "Cmd+Alt+3"
            } else {
                "Ctrl+Alt+3"
            }
        }
        Shortcut::Heading(4) => {
            if cfg!(target_os = "macos") {
                "Cmd+Alt+4"
            } else {
                "Ctrl+Alt+4"
            }
        }
        Shortcut::Heading(_) => "",
        Shortcut::NewDocument => {
            if cfg!(target_os = "macos") {
                "Cmd+N"
            } else {
                "Ctrl+N"
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
    fn command_n_opens_the_template_picker() {
        let n = Key::Character("n".into());
        assert_eq!(lookup(&n, command()), Some(Shortcut::NewDocument));
        assert_eq!(
            lookup(&Key::Character("N".into()), command()),
            Some(Shortcut::NewDocument)
        );
        assert_eq!(lookup(&n, Modifiers::NONE), None);
        assert_eq!(lookup(&n, command() | Modifiers::SHIFT), None);
        assert_eq!(lookup(&n, command() | Modifiers::ALT), None);
    }

    #[test]
    fn command_alt_digits_pick_heading_styles() {
        let ca = command_alt();
        for n in 0..=4u8 {
            assert_eq!(
                lookup(&Key::Character(n.to_string().as_str().into()), ca),
                Some(Shortcut::Heading(n))
            );
        }
        let five = Key::Character("5".into());
        assert_eq!(lookup(&five, ca), None);
        let one = Key::Character("1".into());
        assert_eq!(lookup(&one, command()), None);
        assert_eq!(lookup(&one, Modifiers::ALT), None);
        assert_eq!(lookup(&one, ca | Modifiers::SHIFT), None);
    }

    fn on_key(code: keyboard::key::Code) -> keyboard::key::Physical {
        keyboard::key::Physical::Code(code)
    }

    #[test]
    fn shortcuts_work_by_position_on_a_cyrillic_layout() {
        use keyboard::key::Code;
        // Ukrainian and Russian layouts: the letter on the S, F, N, B, I and O keys.
        for (letter, code, expected) in [
            ("і", Code::KeyS, Shortcut::Save),
            ("ы", Code::KeyS, Shortcut::Save),
            ("а", Code::KeyF, Shortcut::Find),
            ("т", Code::KeyN, Shortcut::NewDocument),
            ("и", Code::KeyB, Shortcut::Bold),
            ("ш", Code::KeyI, Shortcut::Italic),
            ("щ", Code::KeyO, Shortcut::Open),
            ("ц", Code::KeyW, Shortcut::CloseDocument),
        ] {
            let event = keyboard::Event::KeyPressed {
                key: Key::Character(letter.into()),
                modified_key: Key::Character(letter.into()),
                physical_key: on_key(code),
                location: keyboard::Location::Standard,
                modifiers: command(),
                text: None,
                repeat: false,
            };
            assert_eq!(from_event(&event), Some(expected), "{letter}");
        }
        // Without Ctrl it is text, whatever key it is on.
        let typed = keyboard::Event::KeyPressed {
            key: Key::Character("і".into()),
            modified_key: Key::Character("і".into()),
            physical_key: on_key(Code::KeyS),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::NONE,
            text: Some("і".into()),
            repeat: false,
        };
        assert_eq!(from_event(&typed), None);
    }

    #[test]
    fn latin_layouts_keep_their_own_letters() {
        use keyboard::key::Code;
        // AZERTY: the A key types "q", and Ctrl+Q there is "q", not Ctrl+A.
        let azerty = latin_key(&Key::Character("q".into()), &on_key(Code::KeyA), command());
        assert_eq!(azerty, Key::Character("q".into()));
        // A key with no known position, or a named key, is left alone.
        let unknown =
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        assert_eq!(
            latin_key(&Key::Character("і".into()), &unknown, command()),
            Key::Character("і".into())
        );
        let enter = Key::Named(keyboard::key::Named::Enter);
        assert_eq!(latin_key(&enter, &on_key(Code::KeyA), command()), enter);
        // Digits and punctuation on the Cyrillic layout's own keys.
        assert_eq!(
            latin_key(&Key::Character("б".into()), &on_key(Code::Comma), command()),
            Key::Character(",".into())
        );
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
