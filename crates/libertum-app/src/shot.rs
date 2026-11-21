//! `LIBERTUM_SCREENSHOT=<file.png>`: saves a picture of the window and exits, so UI changes can
//! be checked on platforms where the desktop will not let another program capture the screen
//! (GNOME on Wayland). `LIBERTUM_SCREENSHOT_DELAY=<seconds>` sets how long to wait first
//! (default 6), for the window and the document to settle. Combine it with `LIBERTUM_THEME`,
//! `LIBERTUM_MENU`, `LIBERTUM_AO3` and the other screenshot hooks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::Task;
use iced::window;

use crate::app::Message;

/// Where the picture goes, when the hook is on.
pub fn target() -> Option<PathBuf> {
    std::env::var_os("LIBERTUM_SCREENSHOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// A task that fires [`Message::ScreenshotDue`] after the delay; none when the hook is off.
pub fn start() -> Option<Task<Message>> {
    target()?;
    let seconds = std::env::var("LIBERTUM_SCREENSHOT_DELAY")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(6);
    Some(Task::perform(
        async move { std::thread::sleep(Duration::from_secs(seconds)) },
        |()| Message::ScreenshotDue,
    ))
}

/// Writes the captured window as a PNG.
pub fn save(shot: &window::Screenshot, path: &Path) -> Result<(), String> {
    let image = image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())
        .ok_or_else(|| "the screenshot has the wrong number of pixels".to_owned())?;
    image.save(path).map_err(|error| error.to_string())
}
