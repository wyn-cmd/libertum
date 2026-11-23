//! Libertum application shell: Pages-style toolbar, document canvas, inspector.

// A release build is a windowed program on Windows: without this, starting it from the
// Start Menu, a shortcut or a document opens a console window behind the app. Debug builds
// keep the console for logs. Output still reaches a pipe or file it is redirected to.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bench;
mod canvas;
mod docview;
mod fps;
mod home;
mod icons;
mod inspector;
mod install;
mod language;
mod linked;
#[cfg(all(target_os = "macos", not(test)))]
mod macos_icon;
mod macros;
mod mcp;
mod mcp_runner;
mod palm;
mod perf;
mod recovery;
mod script;

#[global_allocator]
static ALLOCATOR: script::Counting = script::Counting;
mod settings;
mod shortcuts;
mod shot;
mod spell;
mod state;
mod stitch;
mod strings;
mod templates;
mod theme;
mod toolbar;
mod update;
mod widgets;

use iced::{Font, Size, window};

use crate::app::App;
use crate::settings::Settings;

/// Choose Iced's window compositor (PLAN §4.5, two layers).
///
/// The window is always composited on the GPU when one exists: Iced's tiny-skia compositor
/// measured ~15 fps while scrolling (docs/spikes.md), so it is only a last-resort fallback.
/// The Graphics acceleration setting does not affect this; it only picks the page renderer
/// (CPU raster vs vello).
///
/// Iced 0.14's `iced_renderer::fallback::Compositor` reads the `ICED_BACKEND` environment
/// variable: a comma-separated, ordered list of backend names (`wgpu`, `tiny-skia`). There is
/// no Rust-side setting, so the variable must be set before `iced::application(..).run()`.
/// A value the user has already exported is respected.
///
/// TODO(§4.5): crash-flag detection ("GPU start in progress") and one-time fallback notice.
fn select_renderer(_settings: &Settings) {
    if std::env::var_os("ICED_BACKEND").is_some() {
        tracing::info!("ICED_BACKEND already set; leaving renderer selection to the environment");
        return;
    }
    // GPU first; if wgpu initialisation fails Iced falls through to tiny-skia.
    let backend = "wgpu,tiny-skia";
    // SAFETY: called at the start of `main`, before any other thread has been spawned.
    unsafe { std::env::set_var("ICED_BACKEND", backend) };
    tracing::info!(backend, "compositor selected");
    limit_wgpu_backends();
}

/// Stops wgpu creating an OpenGL instance on Windows.
///
/// Iced 0.14 asks wgpu for every backend unless `WGPU_BACKEND` is set, and each backend loads
/// its driver. Measured on the development laptop (500-page document at rest): all backends
/// 279 MB, Vulkan and DX12 only 227 MB. OpenGL on Windows is only a last resort for GPUs
/// without Vulkan or DX12; those fall through to the tiny-skia compositor. A value the user
/// has already exported is respected, and other platforms are untouched.
fn limit_wgpu_backends() {
    #[cfg(windows)]
    if std::env::var_os("WGPU_BACKEND").is_none() {
        let backends = "vulkan,dx12";
        // SAFETY: called at the start of `main`, before any other thread has been spawned.
        unsafe { std::env::set_var("WGPU_BACKEND", backends) };
        tracing::info!(backends, "graphics backends limited");
    }
}

/// The platform UI font.
///
/// Iced hands family names to cosmic-text, which resolves them against the installed system
/// fonts, so a named family works when it is installed and otherwise falls back to the
/// default sans-serif. Windows: Segoe UI. macOS: `Font::DEFAULT`, which cosmic-text maps to
/// the system sans-serif (Helvetica), since the hidden `.AppleSystemUIFont` face is not in
/// the font database. Linux: the default sans-serif.
fn platform_font() -> Font {
    if cfg!(target_os = "windows") {
        Font::with_name("Segoe UI")
    } else {
        Font::DEFAULT
    }
}

/// The application logo, shown as the window and taskbar icon.
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// The logo decoded to the RGBA pixels Iced wants. A logo that will not decode is not fatal:
/// the window simply gets the platform's default icon.
fn app_icon() -> Option<window::Icon> {
    let media = match libertum_model::Media::new(LOGO_PNG.to_vec()) {
        Ok(media) => media,
        Err(error) => {
            tracing::warn!(%error, "the application logo is not a usable PNG");
            return None;
        }
    };
    let decoded = libertum_render::images::decode(&media)?;
    match window::icon::from_rgba(decoded.rgba, decoded.width, decoded.height) {
        Ok(icon) => Some(icon),
        Err(error) => {
            tracing::warn!(%error, "the application logo has the wrong pixel count");
            None
        }
    }
}

fn window_settings() -> window::Settings {
    window::Settings {
        icon: app_icon(),
        // Fits 1366x768 at 100% (outer height 680 + title bar < 728 usable) and
        // 1920x1080 at 125% (864 logical high, minus taskbar) without being maximised.
        size: Size::new(1200.0, 680.0),
        min_size: Some(Size::new(720.0, 480.0)),
        // macOS: merge the toolbar into the title bar like Pages (hidden title, transparent
        // title bar, content under it). The toolbar leaves room for the traffic lights.
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        // Linux: the Wayland app id and X11 WM_CLASS, which the desktop matches to
        // `libertum.desktop` (its `StartupWMClass`) to find the installed icon. Left empty,
        // the dock and taskbar show a generic icon.
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            application_id: LINUX_APPLICATION_ID.to_owned(),
            ..window::settings::PlatformSpecific::default()
        },
        ..window::Settings::default()
    }
}

/// The window's application id on Linux; it must equal the desktop entry's file name (without
/// `.desktop`) and its `StartupWMClass`.
#[cfg(target_os = "linux")]
const LINUX_APPLICATION_ID: &str = "libertum";

/// The first command-line argument that is not an option (`-x` or `--x`), as a path.
fn document_argument(args: impl Iterator<Item = std::ffi::OsString>) -> Option<std::path::PathBuf> {
    args.map(std::path::PathBuf::from)
        .find(|path| !path.to_string_lossy().starts_with('-'))
}

/// How many malloc arenas glibc may create. By default it makes up to eight per core, one for
/// each thread that allocates, and keeps freed memory in them, so a document laid out on several
/// threads held about 900 MB where two arenas hold about 350 MB (measured on a 500-page demo).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const MALLOC_ARENAS: libc::c_int = 2;

/// Caps glibc's malloc arenas. Must run before the program starts other threads. A
/// `MALLOC_ARENA_MAX` set in the environment is left alone. Returns
/// whether the limit was applied.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn limit_malloc_arenas() -> bool {
    if std::env::var_os("MALLOC_ARENA_MAX").is_some() {
        return false;
    }
    // SAFETY: `mallopt` only sets an allocator tunable; it reads no pointers. It is not
    // thread-safe against concurrent allocator calls, hence the call at the top of `main`.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, MALLOC_ARENAS) == 1 }
}

fn main() -> iced::Result {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    limit_malloc_arenas();
    // `LIBERTUM_LOG=debug` for diagnostics (for example, input method events).
    let level = match std::env::var("LIBERTUM_LOG").ok().as_deref() {
        Some("debug") => tracing::Level::DEBUG,
        Some("trace") => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    // iced_winit logs the whole window configuration at INFO, including every byte of the
    // window icon's pixels, one per line: about a million lines on every start (into the
    // system journal when started from the desktop). Only its warnings and errors are kept.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(level)
        .with_target("iced_winit", tracing::Level::WARN);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    perf::init();
    let (settings, settings_path) = Settings::load_or_default();
    select_renderer(&settings);
    // A document to open: Windows passes the file that was double-clicked (or chosen with
    // "Open with") as the first argument.
    let open = document_argument(std::env::args_os().skip(1));

    let result = iced::application(
        move || App::boot(settings.clone(), settings_path.clone(), open.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .default_font(platform_font())
    .theme(App::theme)
    .subscription(App::subscription)
    .window(window_settings())
    // Iced's window-wide 4x MSAA only smooths `canvas` geometry, and Libertum draws none:
    // the UI is quads (smoothed in their shader), text, SVG icons and page textures, which
    // carry their own antialiasing. Leaving it on costs a 4x colour buffer and a resolve pass
    // on every frame. Screenshots with it on and off were pixel-identical (docs/decisions.md,
    // 2026-11-23). The "Smooth text and shapes on the page" setting is separate and unchanged.
    .antialiasing(false)
    .exit_on_close_request(false)
    .centered()
    .run();

    if let Some(summary) = perf::summary() {
        tracing::info!("{summary}");
    }
    result
}

#[cfg(test)]
mod argument_tests {
    use super::*;

    #[test]
    fn the_application_logo_decodes_to_a_window_icon() {
        let icon = app_icon().expect("the bundled logo decodes");
        let (rgba, size) = icon.into_raw();
        assert_eq!(size, Size::new(512, 512));
        assert_eq!(rgba.len(), 512 * 512 * 4);
    }

    /// The icon on Linux depends on the window's id matching the desktop entry.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_application_id_matches_the_desktop_entry() {
        let settings = window_settings();
        assert_eq!(
            settings.platform_specific.application_id,
            LINUX_APPLICATION_ID
        );
        let entry = include_str!("../../../packaging/linux/libertum.desktop");
        let value = |key: &str| {
            entry
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(str::trim)
        };
        assert_eq!(value("StartupWMClass="), Some(LINUX_APPLICATION_ID));
        assert_eq!(value("Icon="), Some(LINUX_APPLICATION_ID));
    }

    /// `mallopt` reports 1 when it accepts the setting. The environment variable is the user's
    /// override, so the answer depends on whether it is set.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn the_malloc_arena_limit_is_applied_unless_the_user_set_one() {
        assert_eq!(
            limit_malloc_arenas(),
            std::env::var_os("MALLOC_ARENA_MAX").is_none()
        );
    }

    #[test]
    fn the_first_non_option_argument_is_the_document() {
        let args = |list: &[&str]| {
            list.iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(document_argument(args(&[]).into_iter()), None);
        assert_eq!(
            document_argument(args(&[r"C:\Books\My Novel.odt"]).into_iter()),
            Some(std::path::PathBuf::from(r"C:\Books\My Novel.odt"))
        );
        assert_eq!(
            document_argument(args(&["--flag", "notes.md"]).into_iter()),
            Some(std::path::PathBuf::from("notes.md"))
        );
    }
}
