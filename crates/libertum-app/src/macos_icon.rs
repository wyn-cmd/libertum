//! The Dock icon on macOS.
//!
//! Iced's window icon is ignored by macOS, and this build has no `.app` bundle, so without
//! this the Dock shows the generic executable icon. `NSApplication` owns the Dock tile, so
//! the logo in `assets/logo.png` (the same file the Windows executables embed and the window
//! icon uses) is set on it directly.
//!
//! AppKit must be used from the main thread, and winit asks to be called only after its event
//! loop exists. `App::boot` runs after iced has created that loop (iced_winit's
//! `Instance::new` follows `EventLoop::new`), so it is called from there.

use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// Shows the logo in the Dock. A logo that will not decode leaves the generic icon rather
/// than stopping the launch; everything here is best-effort.
pub fn set_dock_icon() {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("the application logo can only be set from the main thread");
        return;
    };
    let data = NSData::with_bytes(LOGO_PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        tracing::warn!("the application logo is not a usable PNG");
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    // SAFETY: `mtm` proves this is the main thread, which is where AppKit requires this call.
    unsafe { application.setApplicationIconImage(Some(&image)) };
}
