//! The Libertum installer: one `.exe` that carries the program, its install script and the
//! README, and installs them for the current user.
//!
//! Three modes:
//!
//! - **Install** (double-click, no arguments): asks first, extracts the carried files into
//!   a temporary folder, runs `install.ps1` from there with no console window, removes the
//!   folder, and says how it went. The script does the real work (copy to
//!   `%LOCALAPPDATA%\Libertum`, shortcuts, "Open with" registration, uninstaller), so the
//!   `.exe` and the documented manual route install exactly the same way.
//! - **Update** (`--update <pid>`, started by Libertum itself): waits for that process to
//!   exit, installs with no questions and no window of its own, and opens the new version.
//!   Only a failure is shown, in a message box.
//! - **Pack** (`libertum-installer pack --app <exe> --script <ps1> --readme <md> --out <exe>`):
//!   writes a copy of this program with the three files appended (see [`payload`]). This is
//!   how a release installer is made; see `packaging/windows/build-installer.ps1`.
//!
//! The installer is unsigned, like the program: Windows SmartScreen may warn the first
//! time ("More info", then "Run anyway").

// Double-clicking the installer must not open a console window behind its dialogs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod payload;

use std::path::PathBuf;
use std::process::ExitCode;

use payload::File;

/// The files an installer must carry, by the names `install.ps1` expects beside it.
const APP_NAME: &str = "libertum.exe";
const SCRIPT_NAME: &str = "install.ps1";
const README_NAME: &str = "README.md";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("pack") {
        return match pack(&args[1..]) {
            Ok(out) => {
                println!("wrote {}", out.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("libertum-installer pack: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let update_after = match args.as_slice() {
        [flag, pid] if flag == "--update" => match pid.parse::<u32>() {
            Ok(pid) => Some(pid),
            Err(_) => {
                eprintln!("libertum-installer: --update needs a process number");
                return ExitCode::FAILURE;
            }
        },
        _ => None,
    };
    install(update_after)
}

/// `pack --app <exe> --script <ps1> --readme <md> --out <exe>`.
fn pack(args: &[String]) -> Result<PathBuf, String> {
    let mut app = None;
    let mut script = None;
    let mut readme = None;
    let mut out = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--app" => app = Some(value),
            "--script" => script = Some(value),
            "--readme" => readme = Some(value),
            "--out" => out = Some(value),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let (Some(app), Some(script), Some(readme), Some(out)) = (app, script, readme, out) else {
        return Err("usage: pack --app <exe> --script <ps1> --readme <md> --out <exe>".into());
    };
    let read = |path: &PathBuf| {
        std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let files = vec![
        File {
            name: APP_NAME.into(),
            data: read(&app)?,
        },
        File {
            name: SCRIPT_NAME.into(),
            data: read(&script)?,
        },
        File {
            name: README_NAME.into(),
            data: read(&readme)?,
        },
    ];
    if !files[0].data.starts_with(b"MZ") {
        return Err(format!("{} is not a Windows program", app.display()));
    }
    let me = std::env::current_exe().map_err(|error| error.to_string())?;
    let mine = read(&me)?;
    // A packed installer packs from its own stub, never from stub + old files.
    let stub_len = payload::stub_len(&mine).unwrap_or(mine.len());
    let packed = payload::pack(&mine[..stub_len], &files).map_err(|error| error.to_string())?;
    if out.exists() {
        return Err(format!(
            "{} exists already; choose another name",
            out.display()
        ));
    }
    std::fs::write(&out, packed).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(out)
}

#[cfg(not(windows))]
fn install(_update_after: Option<u32>) -> ExitCode {
    // Say what this installer carries, so a copy on the wrong machine is still explained.
    let carried = std::env::current_exe()
        .and_then(std::fs::read)
        .map_err(|error| error.to_string())
        .and_then(|exe| payload::read(&exe).map_err(|error| error.to_string()));
    match carried {
        Ok(files) => {
            let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
            eprintln!(
                "This is the Windows installer for Libertum ({}); run it on Windows.",
                names.join(", ")
            );
        }
        Err(error) => eprintln!("The Libertum installer runs on Windows ({error})."),
    }
    ExitCode::FAILURE
}

#[cfg(windows)]
fn install(update_after: Option<u32>) -> ExitCode {
    let version = env!("CARGO_PKG_VERSION");
    let files = match std::env::current_exe()
        .and_then(std::fs::read)
        .map_err(|error| error.to_string())
        .and_then(|exe| payload::read(&exe).map_err(|error| error.to_string()))
    {
        Ok(files) => files,
        Err(error) => {
            ui::error(&format!("Libertum could not be installed: {error}."));
            return ExitCode::FAILURE;
        }
    };
    if !files.iter().any(|f| f.name == APP_NAME) || !files.iter().any(|f| f.name == SCRIPT_NAME) {
        ui::error("Libertum could not be installed: this installer is incomplete.");
        return ExitCode::FAILURE;
    }
    if let Some(pid) = update_after {
        return update_quietly(pid, &files);
    }
    let question = format!(
        "Install Libertum {version} for this Windows user?\n\n\
         It is installed in your user folder (no administrator rights needed), with a Start \
         menu and a desktop shortcut, and offered in \"Open with\" for .lbtd, .odt, .docx, \
         .rtf and .md files.\n\n\
         An earlier copy is replaced. Your documents and settings are kept. Close Libertum \
         first if it is open."
    );
    if !ui::ask(&question) {
        return ExitCode::SUCCESS;
    }
    match run_script(&files) {
        Ok(()) => {
            if ui::ask("Libertum is installed.\n\nOpen it now?")
                && let Some(exe) = installed_exe()
            {
                let _ = std::process::Command::new(exe).spawn();
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            ui::error(&format!("Libertum could not be installed.\n\n{error}"));
            ExitCode::FAILURE
        }
    }
}

/// The silent route Libertum's own updater takes: once the old program has exited, install
/// and open the new one. The question and the "installed" message are skipped; a failure is
/// still shown, because nobody else will tell the user.
#[cfg(windows)]
fn update_quietly(pid: u32, files: &[File]) -> ExitCode {
    ui::wait_for_exit(pid, 60_000);
    match run_script(files) {
        Ok(()) => {
            if let Some(exe) = installed_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            ui::error(&format!("Libertum could not be updated.\n\n{error}"));
            ExitCode::FAILURE
        }
    }
}

/// Where `install.ps1` puts the program by default.
#[cfg(windows)]
fn installed_exe() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("Libertum").join(APP_NAME))
}

/// Extracts the files into a fresh temporary folder, runs the install script there with
/// no window, and removes the folder whatever happens.
#[cfg(windows)]
fn run_script(files: &[File]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`: PowerShell runs without a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let folder = std::env::temp_dir().join(format!(
        "libertum-setup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    ));
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let result = (|| {
        for file in files {
            // Names were checked as plain file names when the payload was read.
            let path = folder.join(&file.name);
            std::fs::write(&path, &file.data)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(folder.join(SCRIPT_NAME))
            .current_dir(&folder)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("PowerShell could not be started: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&output.stderr);
        let text = if text.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).into_owned()
        } else {
            text.into_owned()
        };
        // The script's own message is the first line PowerShell reports.
        let message: String = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("The install script stopped with an error.")
            .chars()
            .take(600)
            .collect();
        Err(message)
    })();
    let _ = std::fs::remove_dir_all(&folder);
    result
}

/// Windows message boxes (user32), the installer's only window.
#[cfg(windows)]
mod ui {
    use std::ffi::c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            window: *mut c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x10_0000;

    /// Waits up to `milliseconds` for process `pid` to exit. A process that is already gone,
    /// or cannot be opened, counts as exited.
    pub fn wait_for_exit(pid: u32, milliseconds: u32) {
        // SAFETY: plain Win32 calls; the handle is checked for null and closed once.
        unsafe {
            let handle = OpenProcess(SYNCHRONIZE, 0, pid);
            if !handle.is_null() {
                WaitForSingleObject(handle, milliseconds);
                CloseHandle(handle);
            }
        }
    }

    const MB_YESNO: u32 = 0x4;
    const MB_ICONERROR: u32 = 0x10;
    const MB_ICONQUESTION: u32 = 0x20;
    const MB_SETFOREGROUND: u32 = 0x1_0000;
    const IDYES: i32 = 6;
    const CAPTION: &str = "Libertum Setup";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn show(text: &str, kind: u32) -> i32 {
        let (text, caption) = (wide(text), wide(CAPTION));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call, and a null
        // owner window is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                kind | MB_SETFOREGROUND,
            )
        }
    }

    /// A Yes/No question; true for Yes.
    pub fn ask(text: &str) -> bool {
        show(text, MB_YESNO | MB_ICONQUESTION) == IDYES
    }

    pub fn error(text: &str) {
        show(text, MB_ICONERROR);
    }
}
