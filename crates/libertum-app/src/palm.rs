//! Palm rejection for the document: right after the writer types, the canvas ignores
//! trackpad input (moves, clicks, scrolls and pinches), so a palm resting on a laptop that
//! has no palm rejection of its own cannot move the caret or scroll the page.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, key::Named};

/// How long after the last typed key pointer input is ignored.
pub const REJECT_WINDOW: Duration = Duration::from_millis(500);

const NEVER: u64 = u64::MAX;
static LAST_TYPED_MS: AtomicU64 = AtomicU64::new(NEVER);

fn clock() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// True for a key press that types, deletes or breaks a line: a character key with no
/// Ctrl, Alt or Cmd held (those are shortcuts, and Ctrl+wheel must still zoom), Backspace,
/// Delete or Enter. Arrows, modifiers and function keys are not typing.
pub fn is_typing(event: &keyboard::Event) -> bool {
    let keyboard::Event::KeyPressed {
        key,
        text,
        modifiers,
        ..
    } = event
    else {
        return false;
    };
    if modifiers.command() || modifiers.control() || modifiers.alt() {
        return false;
    }
    if matches!(
        key,
        Key::Named(Named::Backspace | Named::Delete | Named::Enter)
    ) {
        return true;
    }
    text.as_ref()
        .is_some_and(|t| t.chars().any(|c| !c.is_control()))
}

/// Records a keyboard event; typing starts (or extends) the rejection window.
pub fn note_key(event: &keyboard::Event) {
    if is_typing(event) {
        note_typed(Instant::now());
    }
}

fn note_typed(now: Instant) {
    let ms = now.saturating_duration_since(clock()).as_millis() as u64;
    LAST_TYPED_MS.store(ms, Ordering::Relaxed);
}

/// True while pointer input should be ignored because the writer has just typed.
pub fn typing_recently() -> bool {
    recently_at(Instant::now())
}

fn recently_at(now: Instant) -> bool {
    let last = LAST_TYPED_MS.load(Ordering::Relaxed);
    if last == NEVER {
        return false;
    }
    let now_ms = now.saturating_duration_since(clock()).as_millis() as u64;
    now_ms.saturating_sub(last) < REJECT_WINDOW.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Location, Modifiers};

    fn press(key: Key, text: Option<&str>, modifiers: Modifiers) -> keyboard::Event {
        keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: Location::Standard,
            modifiers,
            text: text.map(Into::into),
            repeat: false,
        }
    }

    #[test]
    fn letters_backspace_and_enter_are_typing_but_shortcuts_and_arrows_are_not() {
        let none = Modifiers::empty();
        assert!(is_typing(&press(
            Key::Character("a".into()),
            Some("a"),
            none
        )));
        assert!(is_typing(&press(Key::Named(Named::Backspace), None, none)));
        assert!(is_typing(&press(
            Key::Named(Named::Enter),
            Some("\r"),
            none
        )));
        assert!(!is_typing(&press(
            Key::Character("c".into()),
            Some("c"),
            Modifiers::CTRL
        )));
        assert!(!is_typing(&press(Key::Named(Named::ArrowDown), None, none)));
        assert!(!is_typing(&press(Key::Named(Named::Control), None, none)));
    }

    #[test]
    fn input_is_ignored_for_the_window_after_typing_and_then_allowed() {
        let t = Instant::now();
        note_typed(t);
        assert!(recently_at(t + Duration::from_millis(100)));
        assert!(!recently_at(t + REJECT_WINDOW + Duration::from_millis(50)));
    }
}
