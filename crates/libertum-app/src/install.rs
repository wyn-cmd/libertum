//! Downloading and installing an update.
//!
//! [`plan`] decides, from the machine Libertum is running on and the files attached to the
//! release, what can be done without the user:
//!
//! - **Windows**: run the release's setup program silently once Libertum has closed.
//! - **Linux AppImage**: replace the AppImage file, then start the new one.
//! - **Linux `.deb` / `.rpm`**: download the package and install it through `pkexec`, the
//!   desktop's own administrator prompt (the password dialog is polkit's, not Libertum's).
//!   Without `pkexec` the package is downloaded and its install command shown instead.
//! - **macOS**: replace `Libertum.app` from the release's zip, then open the new one.
//! - Anything else (a build run from a source tree, a folder Libertum cannot write to): the
//!   release page, as before.
//!
//! [`prepare`] does the slow part on a background thread: it downloads the file and checks it
//! against the release's `SHA256SUMS`. A package is installed during this step, while the
//! window is still open, so a refused password or a failed install is reported and the
//! installed version is left as it was. The step that replaces the running program is
//! [`Staged::apply`], which the app calls as it quits, so the usual unsaved-changes prompt
//! still comes first.
//!
//! The checksum file comes from the same release as the download, so it catches a damaged or
//! truncated download, not a release that was itself tampered with (nothing is code-signed).

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::update::{Asset, Release, hide_console_window};

/// The checksum file attached to every release that can be installed automatically.
pub const CHECKSUMS: &str = "SHA256SUMS";

/// The largest checksum file read, in bytes.
const MAX_CHECKSUMS_BYTES: u64 = 64 * 1024;

/// What the running machine looks like; separate from [`plan`] so it can be tested.
#[derive(Debug, Clone, Default)]
pub struct Host {
    pub os: &'static str,
    pub arch: &'static str,
    /// The running program.
    pub exe: Option<PathBuf>,
    /// The AppImage file Libertum was started from (`$APPIMAGE`), if any.
    pub appimage: Option<PathBuf>,
    /// Package tools found on `PATH` (`dpkg`, `dnf`, `zypper`, `rpm`).
    pub tools: Vec<&'static str>,
    /// `pkexec` was found, so a package can be installed through polkit's password prompt
    /// without the user copying a command anywhere.
    pub pkexec: bool,
}

impl Host {
    pub fn detect() -> Self {
        let tools = ["dpkg", "dnf", "zypper", "rpm"]
            .into_iter()
            .filter(|tool| on_path(tool))
            .collect();
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            exe: std::env::current_exe().ok(),
            appimage: std::env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .filter(|path| path.is_file()),
            tools,
            pkexec: on_path("pkexec"),
        }
    }

    fn has(&self, tool: &str) -> bool {
        self.tools.contains(&tool)
    }
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()))
}

/// How a Linux package is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageTool {
    Apt,
    Dnf,
    Zypper,
    Rpm,
}

/// What an update can do on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Run the setup program silently when Libertum closes.
    Windows { installer: Asset },
    /// Replace the AppImage at `target`.
    AppImage { image: Asset, target: PathBuf },
    /// Download a package and install it with `pkexec` (polkit's password prompt), then
    /// start `restart` once Libertum has closed.
    PackageAuto {
        package: Asset,
        tool: PackageTool,
        restart: PathBuf,
    },
    /// Download a package and show the command that installs it.
    Package { package: Asset, tool: PackageTool },
    /// Replace the app bundle at `bundle` from the release's zip.
    Mac { zip: Asset, bundle: PathBuf },
    /// Nothing automatic applies; open the release page.
    Page,
}

/// Decides what to do for `release` on `host`. Every automatic route needs the release to
/// carry [`CHECKSUMS`] and the file for this machine.
pub fn plan(host: &Host, release: &Release) -> Plan {
    let version = &release.version;
    let find = |name: String| release.asset(&name).cloned();
    if release.asset(CHECKSUMS).is_none() {
        return Plan::Page;
    }
    match host.os {
        "windows" => find(format!("Libertum-Setup-{version}.exe"))
            .map_or(Plan::Page, |installer| Plan::Windows { installer }),
        "macos" => {
            let arch = match host.arch {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                _ => return Plan::Page,
            };
            let Some(bundle) = host.exe.as_deref().and_then(app_bundle) else {
                return Plan::Page;
            };
            find(format!("Libertum-{version}-macos-{arch}.zip"))
                .map_or(Plan::Page, |zip| Plan::Mac { zip, bundle })
        }
        "linux" if host.arch == "x86_64" => {
            if let Some(target) = &host.appimage {
                return find(format!("Libertum-{version}-x86_64.AppImage")).map_or(
                    Plan::Page,
                    |image| Plan::AppImage {
                        image,
                        target: target.clone(),
                    },
                );
            }
            // A packaged install lives under /usr; anything else is left to the user.
            if !host
                .exe
                .as_deref()
                .is_some_and(|exe| exe.starts_with("/usr"))
            {
                return Plan::Page;
            }
            let deb = || find(format!("libertum_{version}_amd64.deb"));
            let rpm = || find(format!("libertum-{version}-1.x86_64.rpm"));
            if host.has("dpkg") {
                deb().map(|package| (package, PackageTool::Apt))
            } else if host.has("dnf") {
                rpm().map(|package| (package, PackageTool::Dnf))
            } else if host.has("zypper") {
                rpm().map(|package| (package, PackageTool::Zypper))
            } else if host.has("rpm") {
                rpm().map(|package| (package, PackageTool::Rpm))
            } else {
                None
            }
            .map_or(Plan::Page, |(package, tool)| {
                match &host.exe {
                    // polkit asks for the administrator password itself, so the update needs
                    // no command line from the user.
                    Some(restart) if host.pkexec => Plan::PackageAuto {
                        package,
                        tool,
                        restart: restart.clone(),
                    },
                    _ => Plan::Package { package, tool },
                }
            })
        }
        _ => Plan::Page,
    }
}

/// `…/Libertum.app` for a program at `…/Libertum.app/Contents/MacOS/libertum`.
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// A downloaded, verified update that is waiting for Libertum to close.
#[derive(Debug, Clone)]
pub struct Staged(Step);

#[derive(Debug, Clone)]
enum Step {
    Windows {
        installer: PathBuf,
    },
    AppImage {
        new: PathBuf,
        target: PathBuf,
    },
    Mac {
        work: PathBuf,
        new_app: PathBuf,
        bundle: PathBuf,
    },
    /// The package is already installed; start this program after Libertum closes.
    Restart {
        exe: PathBuf,
    },
}

/// The result of [`prepare`].
#[derive(Debug, Clone)]
pub enum Prepared {
    /// Ready; install when Libertum closes.
    Restart(Staged),
    /// A package was saved to `file`; the user runs `command` in a terminal.
    Command { command: String, file: PathBuf },
}

/// Downloads and checks the file the plan needs. Blocking: run it off the UI thread.
pub fn prepare(release: &Release, plan: Plan) -> Result<Prepared, String> {
    let sums = release
        .asset(CHECKSUMS)
        .ok_or("the release has no checksum file")?;
    let sums = fetch_text(&sums.url)?;
    match plan {
        Plan::Page => Err("this installation cannot be updated automatically".to_owned()),
        Plan::Windows { installer } => {
            let dir = std::env::temp_dir().join(format!("libertum-update-{}", std::process::id()));
            let path = download_verified(&installer, &sums, &dir, &installer.name)?;
            Ok(Prepared::Restart(Staged(Step::Windows { installer: path })))
        }
        Plan::AppImage { image, target } => {
            // Beside the old file, so the final rename never crosses a file system.
            let dir = target.parent().ok_or("the AppImage has no folder")?;
            let name = format!(".libertum-update-{}.AppImage", release.version);
            let new = download_verified(&image, &sums, dir, &name)?;
            if let Err(error) = make_executable(&new) {
                let _ = std::fs::remove_file(&new);
                return Err(error);
            }
            Ok(Prepared::Restart(Staged(Step::AppImage { new, target })))
        }
        Plan::PackageAuto {
            package,
            tool,
            restart,
        } => {
            // Root can read the temporary folder and the package manager copies the file
            // into its cache, so nothing is left in the user's folders.
            let dir = std::env::temp_dir().join(format!("libertum-update-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let file = download_verified(&package, &sums, &dir, &package.name)?;
            if let Err(error) = install_package(tool, &file) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
            let _ = std::fs::remove_dir_all(&dir);
            Ok(Prepared::Restart(Staged(Step::Restart { exe: restart })))
        }
        Plan::Package { package, tool } => {
            let dir = downloads_dir();
            let file = download_verified(&package, &sums, &dir, &package.name)?;
            Ok(Prepared::Command {
                command: install_command(tool, &file),
                file,
            })
        }
        Plan::Mac { zip, bundle } => {
            // Beside the app, so the swap is a rename and fails early if the folder is read-only.
            let parent = bundle.parent().ok_or("the app has no folder")?;
            let work = parent.join(format!(".Libertum-update-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&work);
            std::fs::create_dir_all(&work)
                .map_err(|error| format!("{} cannot be written to: {error}", parent.display()))?;
            let result = (|| {
                let file = download_verified(&zip, &sums, &work, &zip.name)?;
                let out = work.join("app");
                let status = Command::new("ditto")
                    .args(["-x", "-k"])
                    .arg(&file)
                    .arg(&out)
                    .status()
                    .map_err(|error| format!("could not unpack the update: {error}"))?;
                let _ = std::fs::remove_file(&file);
                let new_app = out.join("Libertum.app");
                if !status.success() || !new_app.is_dir() {
                    return Err("the update did not unpack to Libertum.app".to_owned());
                }
                Ok(new_app)
            })();
            match result {
                Ok(new_app) => Ok(Prepared::Restart(Staged(Step::Mac {
                    work,
                    new_app,
                    bundle,
                }))),
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&work);
                    Err(error)
                }
            }
        }
    }
}

impl Staged {
    /// Starts the last step. Call it as Libertum quits, then exit at once. Nothing here
    /// waits for the update to finish, and a failure leaves the installed version as it was.
    pub fn apply(self) {
        let started = match self.0 {
            Step::Windows { installer } => Command::new(&installer)
                .args(["--update", &std::process::id().to_string()])
                .spawn()
                .map(drop)
                .map_err(|error| error.to_string()),
            Step::AppImage { new, target } => replace_file(&new, &target)
                .map_err(|error| {
                    let _ = std::fs::remove_file(&new);
                    error.to_string()
                })
                .and_then(|()| start_new_version(&target)),
            Step::Mac {
                work,
                new_app,
                bundle,
            } => swap_mac_app(&work, &new_app, &bundle),
            Step::Restart { exe } => start_new_version(&exe),
        };
        if let Err(error) = started {
            tracing::warn!(%error, "the update could not be started");
        }
    }
}

/// Puts `new` in place of `target` in one step. On Unix a rename over a file that is running
/// is safe: the running copy keeps the old file until it exits, and anything starting
/// afterwards gets the new one, so there is no moment with a half-written program.
fn replace_file(new: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(new, target)
}

/// Starts the new version on its own, cut loose from this process and from the old image's
/// mount (an AppImage's), which is about to go away.
fn start_new_version(image: &Path) -> Result<(), String> {
    let mut command = Command::new(image);
    for variable in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        command.env_remove(variable);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(drop).map_err(|error| error.to_string())
}

/// After Libertum exits: swap the bundles, clean up, open the new app. If the swap fails the
/// old app is put back and opened instead.
fn swap_mac_app(work: &Path, new_app: &Path, bundle: &Path) -> Result<(), String> {
    const SCRIPT: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
xattr -dr com.apple.quarantine "$2" 2>/dev/null
if mv "$3" "$3.old" && mv "$2" "$3"; then rm -rf "$3.old"; else mv "$3.old" "$3" 2>/dev/null; fi
rm -rf "$4"
open "$3""#;
    let mut command = Command::new("sh");
    command
        .args(["-c", SCRIPT, "libertum-update"])
        .arg(std::process::id().to_string())
        .arg(new_app)
        .arg(bundle)
        .arg(work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(drop).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("could not make the update runnable: {error}"))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Where a package is saved for the user: the Downloads folder, else the home folder, else
/// the temporary folder. Always an absolute path, so the command works from any directory.
fn downloads_dir() -> PathBuf {
    let from_xdg = Command::new("xdg-user-dir")
        .arg("DOWNLOAD")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    from_xdg
        .filter(|path| path.is_absolute() && path.is_dir())
        .or_else(|| {
            home.as_ref()
                .map(|home| home.join("Downloads"))
                .filter(|p| p.is_dir())
        })
        .or(home.filter(|path| path.is_dir()))
        .unwrap_or_else(std::env::temp_dir)
}

/// Installs a downloaded package through `pkexec`, so polkit puts up its own password
/// dialog and the user types nothing into a terminal. The output is captured rather than
/// printed: a dismissed prompt or a failed install is shown in the update window.
fn install_package(tool: PackageTool, file: &Path) -> Result<(), String> {
    let mut command = Command::new("pkexec");
    match tool {
        PackageTool::Apt => {
            command.args(["apt-get", "install", "-y"]).arg(file);
            // No debconf or configuration questions: there is nothing to ask about.
            command.env("DEBIAN_FRONTEND", "noninteractive");
        }
        PackageTool::Dnf => {
            command.args(["dnf", "install", "-y"]).arg(file);
        }
        PackageTool::Zypper => {
            // The packages are unsigned; without this zypper would stop at that question.
            command
                .args([
                    "zypper",
                    "--non-interactive",
                    "install",
                    "--allow-unsigned-rpm",
                ])
                .arg(file);
        }
        PackageTool::Rpm => {
            command.args(["rpm", "-U", "--replacepkgs"]).arg(file);
        }
    }
    hide_console_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("could not run pkexec: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the package could not be installed: {}",
        last_line(&String::from_utf8_lossy(&output.stderr))
    ))
}

/// The last non-empty line of a program's error output, for a one-line message; a command
/// that failed silently says so.
fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no reason was given")
        .chars()
        .take(500)
        .collect()
}

/// The terminal command that installs `file`, with the full path quoted for the shell.
pub fn install_command(tool: PackageTool, file: &Path) -> String {
    let path = shell_quote(&file.to_string_lossy());
    match tool {
        PackageTool::Apt => format!("sudo apt install {path}"),
        PackageTool::Dnf => format!("sudo dnf install {path}"),
        PackageTool::Zypper => format!("sudo zypper install {path}"),
        PackageTool::Rpm => format!("sudo rpm -U {path}"),
    }
}

/// Single-quotes `text` for a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Downloads `asset` into `dir` as `name`, and checks it against its line in `sums`. The
/// file only gets its final name once it has passed, so a half-written download is never
/// mistaken for a good one.
fn download_verified(asset: &Asset, sums: &str, dir: &Path, name: &str) -> Result<PathBuf, String> {
    let expected = expected_hash(sums, &asset.name)
        .ok_or_else(|| format!("{} is not in the checksum file", asset.name))?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let part = dir.join(format!("{name}.part"));
    let done = dir.join(name);
    let result = (|| {
        download(&asset.url, &part)?;
        let actual =
            file_hash(&part).map_err(|error| format!("could not read the download: {error}"))?;
        if actual != expected {
            return Err(format!("{} did not match its checksum", asset.name));
        }
        std::fs::rename(&part, &done).map_err(|error| format!("{}: {error}", done.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| done)
}

/// The expected SHA-256 for `name` from a `sha256sum`-style file (`<hash>  <name>`).
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn file_hash(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

fn curl(url: &str) -> Command {
    let mut command = Command::new("curl");
    command
        .args([
            "-fsSL",
            "--connect-timeout",
            "15",
            // A download that stalls for half a minute is given up on.
            "--speed-limit",
            "1000",
            "--speed-time",
            "30",
            "--max-time",
            "900",
            "-A",
            concat!("Libertum/", env!("CARGO_PKG_VERSION")),
        ])
        .arg(url);
    hide_console_window(&mut command);
    command
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    let output = curl(url)
        .arg("-o")
        .arg(to)
        .output()
        .map_err(|error| format!("could not run curl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "the download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn fetch_text(url: &str) -> Result<String, String> {
    let output = curl(url)
        .output()
        .map_err(|error| format!("could not run curl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "the checksum download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.len() as u64 > MAX_CHECKSUMS_BYTES {
        return Err("the checksum file is too large".to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            url: format!("https://github.com/wyn-cmd/libertum/releases/download/v0.4.0/{name}"),
        }
    }

    fn release(names: &[&str]) -> Release {
        Release {
            version: "0.4.0".to_owned(),
            url: "https://github.com/wyn-cmd/libertum/releases/tag/v0.4.0".to_owned(),
            assets: names.iter().map(|name| asset(name)).collect(),
        }
    }

    const ALL: &[&str] = &[
        "SHA256SUMS",
        "Libertum-Setup-0.4.0.exe",
        "Libertum-0.4.0-x86_64.AppImage",
        "libertum_0.4.0_amd64.deb",
        "libertum-0.4.0-1.x86_64.rpm",
        "Libertum-0.4.0-macos-arm64.zip",
        "Libertum-0.4.0-macos-x64.zip",
    ];

    fn host(os: &'static str, exe: &str) -> Host {
        Host {
            os,
            arch: "x86_64",
            exe: Some(PathBuf::from(exe)),
            ..Host::default()
        }
    }

    #[test]
    fn windows_runs_the_versioned_setup_program() {
        let plan = plan(&host("windows", r"C:\x\libertum.exe"), &release(ALL));
        assert_eq!(
            plan,
            Plan::Windows {
                installer: asset("Libertum-Setup-0.4.0.exe")
            }
        );
    }

    #[test]
    fn nothing_is_automatic_without_a_checksum_file_or_the_right_file() {
        let no_sums: Vec<&str> = ALL.iter().copied().filter(|n| *n != "SHA256SUMS").collect();
        assert_eq!(plan(&host("windows", "x"), &release(&no_sums)), Plan::Page);
        assert_eq!(
            plan(&host("windows", "x"), &release(&["SHA256SUMS"])),
            Plan::Page
        );
    }

    #[test]
    fn an_appimage_replaces_itself_and_a_package_install_gets_a_command() {
        let mut linux = host("linux", "/tmp/.mount_x/usr/bin/libertum");
        linux.appimage = Some(PathBuf::from("/home/a/Libertum.AppImage"));
        assert_eq!(
            plan(&linux, &release(ALL)),
            Plan::AppImage {
                image: asset("Libertum-0.4.0-x86_64.AppImage"),
                target: PathBuf::from("/home/a/Libertum.AppImage")
            }
        );

        let mut deb = host("linux", "/usr/bin/libertum");
        deb.tools = vec!["dpkg"];
        assert!(matches!(
            plan(&deb, &release(ALL)),
            Plan::Package {
                tool: PackageTool::Apt,
                ..
            }
        ));
        let mut rpm = host("linux", "/usr/bin/libertum");
        rpm.tools = vec!["rpm", "dnf"];
        assert!(matches!(
            plan(&rpm, &release(ALL)),
            Plan::Package {
                tool: PackageTool::Dnf,
                ..
            }
        ));
        // Run from somewhere else, with no AppImage: leave it to the user.
        assert_eq!(
            plan(&host("linux", "/home/a/libertum"), &release(ALL)),
            Plan::Page
        );
    }

    #[test]
    fn a_package_installs_itself_when_pkexec_is_available() {
        let mut deb = host("linux", "/usr/bin/libertum");
        deb.tools = vec!["dpkg"];
        deb.pkexec = true;
        assert_eq!(
            plan(&deb, &release(ALL)),
            Plan::PackageAuto {
                package: asset("libertum_0.4.0_amd64.deb"),
                tool: PackageTool::Apt,
                restart: PathBuf::from("/usr/bin/libertum"),
            }
        );

        let mut rpm = host("linux", "/usr/bin/libertum");
        rpm.tools = vec!["rpm", "zypper"];
        rpm.pkexec = true;
        assert_eq!(
            plan(&rpm, &release(ALL)),
            Plan::PackageAuto {
                package: asset("libertum-0.4.0-1.x86_64.rpm"),
                tool: PackageTool::Zypper,
                restart: PathBuf::from("/usr/bin/libertum"),
            }
        );

        // Without `pkexec` the package is still offered as a command to run.
        let mut manual = host("linux", "/usr/bin/libertum");
        manual.tools = vec!["dpkg"];
        assert!(matches!(
            plan(&manual, &release(ALL)),
            Plan::Package {
                tool: PackageTool::Apt,
                ..
            }
        ));
    }

    #[test]
    fn the_last_error_line_is_short_and_never_empty() {
        assert_eq!(last_line("first\n  second  \n\n"), "second");
        assert_eq!(last_line("   "), "no reason was given");
        assert_eq!(last_line(&"x".repeat(600)).chars().count(), 500);
    }

    #[test]
    fn a_mac_app_is_replaced_only_when_it_is_a_bundle() {
        let mut mac = host(
            "macos",
            "/Applications/Libertum.app/Contents/MacOS/libertum",
        );
        mac.arch = "aarch64";
        assert_eq!(
            plan(&mac, &release(ALL)),
            Plan::Mac {
                zip: asset("Libertum-0.4.0-macos-arm64.zip"),
                bundle: PathBuf::from("/Applications/Libertum.app")
            }
        );
        let loose = host("macos", "/Users/a/target/release/libertum");
        assert_eq!(plan(&loose, &release(ALL)), Plan::Page);
    }

    #[test]
    fn the_install_command_uses_the_full_path_and_survives_odd_names() {
        let path = Path::new("/home/a b/Downloads/libertum_0.4.0_amd64.deb");
        assert_eq!(
            install_command(PackageTool::Apt, path),
            "sudo apt install '/home/a b/Downloads/libertum_0.4.0_amd64.deb'"
        );
        assert_eq!(
            install_command(PackageTool::Rpm, Path::new("/tmp/it's.rpm")),
            r"sudo rpm -U '/tmp/it'\''s.rpm'"
        );
    }

    #[test]
    fn checksums_are_read_from_sha256sum_output() {
        let hash = "a".repeat(64);
        let sums = format!(
            "{hash}  one.exe\n{}  *two.exe\nnot a line\n",
            "B".repeat(64)
        );
        assert_eq!(expected_hash(&sums, "one.exe"), Some(hash));
        assert_eq!(expected_hash(&sums, "two.exe"), Some("b".repeat(64)));
        assert_eq!(expected_hash(&sums, "three.exe"), None);
        assert_eq!(expected_hash("abc  one.exe", "one.exe"), None);
    }

    #[test]
    fn a_file_is_hashed_and_a_wrong_hash_is_refused() {
        let dir =
            std::env::temp_dir().join(format!("libertum-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("data");
        std::fs::write(&file, b"abc").expect("write");
        // SHA-256 of "abc".
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(file_hash(&file).expect("hash"), abc);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_a_file_swaps_the_content_in_one_step_and_leaves_no_leftover() {
        let dir = std::env::temp_dir().join(format!("libertum-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let target = dir.join("Libertum.AppImage");
        let new = dir.join(".new");
        std::fs::write(&target, b"old").expect("old");
        std::fs::write(&new, b"new").expect("new");
        // The old file stays open, as a running AppImage's does.
        let mut open = std::fs::File::open(&target).expect("open");
        replace_file(&new, &target).expect("replace");
        assert_eq!(std::fs::read(&target).expect("read"), b"new");
        assert!(!new.exists());
        let mut old = String::new();
        let _ = open.read_to_string(&mut old);
        assert!(
            old == "old" || cfg!(windows),
            "the open copy is undisturbed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A release whose files are plain files in `dir`, fetched through `file://` addresses.
    fn local_release(dir: &Path, files: &[(&str, &[u8])], sums: &str) -> Release {
        std::fs::create_dir_all(dir).expect("dir");
        std::fs::write(dir.join("SHA256SUMS"), sums).expect("sums");
        let url = |name: &str| {
            format!(
                "file:///{}",
                dir.join(name).to_string_lossy().replace('\\', "/")
            )
        };
        let mut assets = vec![Asset {
            name: "SHA256SUMS".to_owned(),
            url: url("SHA256SUMS"),
        }];
        for (name, data) in files {
            std::fs::write(dir.join(name), data).expect("file");
            assets.push(Asset {
                name: (*name).to_owned(),
                url: url(name),
            });
        }
        Release {
            version: "0.4.0".to_owned(),
            url: String::new(),
            assets,
        }
    }

    #[test]
    fn a_download_that_matches_its_checksum_is_staged_and_a_bad_one_is_thrown_away() {
        let base =
            std::env::temp_dir().join(format!("libertum-prepare-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let name = "Libertum-0.4.0-x86_64.AppImage";

        // Good: the new file appears beside the target, under a hidden name, with no .part left.
        let server = base.join("good");
        let release = local_release(&server, &[(name, b"abc")], &format!("{abc}  {name}\n"));
        let target_dir = base.join("apps");
        std::fs::create_dir_all(&target_dir).expect("dir");
        let target = target_dir.join("Libertum.AppImage");
        std::fs::write(&target, b"old").expect("old");
        let plan = Plan::AppImage {
            image: release.asset(name).expect("asset").clone(),
            target: target.clone(),
        };
        let Ok(Prepared::Restart(Staged(Step::AppImage { new, target: t }))) =
            prepare(&release, plan.clone())
        else {
            panic!("a matching download is staged");
        };
        assert_eq!(t, target);
        assert_eq!(new.parent(), target.parent(), "same folder as the old file");
        assert_eq!(std::fs::read(&new).expect("new"), b"abc");
        assert_eq!(
            std::fs::read(&target).expect("old"),
            b"old",
            "nothing replaced yet"
        );
        replace_file(&new, &target).expect("swap");
        assert_eq!(std::fs::read(&target).expect("now"), b"abc");
        let leftovers: Vec<_> = std::fs::read_dir(&target_dir)
            .expect("list")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "no temporary file stays: {leftovers:?}");

        // Bad: a wrong checksum fails, and the existing file and folder are untouched.
        let server = base.join("bad");
        let wrong = "0".repeat(64);
        let release = local_release(&server, &[(name, b"abc")], &format!("{wrong}  {name}\n"));
        let plan = Plan::AppImage {
            image: release.asset(name).expect("asset").clone(),
            target: target.clone(),
        };
        let error = prepare(&release, plan).expect_err("refused");
        assert!(error.contains("checksum"), "{error}");
        assert_eq!(std::fs::read(&target).expect("still new"), b"abc");
        assert_eq!(std::fs::read_dir(&target_dir).expect("list").count(), 1);

        // Not listed at all is refused the same way.
        let server = base.join("unlisted");
        let release = local_release(&server, &[(name, b"abc")], "");
        let plan = Plan::AppImage {
            image: release.asset(name).expect("asset").clone(),
            target,
        };
        assert!(prepare(&release, plan).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
