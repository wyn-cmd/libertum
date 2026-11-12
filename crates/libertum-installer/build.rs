//! Embeds the Libertum logo as the icon of the Windows executable, so Explorer, the Start
//! Menu, the desktop shortcut and "Open with" show it. Other platforms have nothing to do.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../libertum-app/assets/logo.ico");
    #[cfg(windows)]
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    // Only when the target is Windows (a Windows host may still cross-compile elsewhere).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../libertum-app/assets/logo.ico");
    resource.set("FileDescription", "Libertum Setup");
    resource.set("ProductName", "Libertum");
    if let Err(error) = resource.compile() {
        // A missing resource compiler must not stop the build: the program just has no icon.
        println!("cargo:warning=could not embed the application icon: {error}");
    }
}
