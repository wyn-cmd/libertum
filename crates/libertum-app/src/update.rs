//! Checking GitHub for a newer release.
//!
//! The check is deliberately small: it asks the public GitHub releases API with the system
//! `curl` (present on Windows 10+, macOS and normal Linux desktops) and reads `tag_name` and
//! `html_url` from the JSON. It never downloads or runs an installer; "Update" opens the
//! release page in the browser. Nothing about the user or the document is sent.

use std::process::Command;

use serde_json::Value;

/// How often an automatic check may run, in seconds.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

const LATEST_RELEASE: &str = "https://api.github.com/repos/wyn-cmd/libertum/releases/latest";
const RELEASE_URL_PREFIX: &str = "https://github.com/wyn-cmd/libertum/releases/";

/// A release newer than the running build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version without the tag's `v`, for example `0.1.3`.
    pub version: String,
    /// The GitHub release page.
    pub url: String,
}

/// Why a check (or opening the release page) failed.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("curl is not installed, so updates cannot be checked")]
    CurlMissing,
    #[error("could not run curl: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the update check failed: {0}")]
    CurlFailed(String),
    #[error("the release information was not understood")]
    BadRelease,
    #[error("could not open the release page: {0}")]
    Open(std::io::Error),
}

/// The version this build was made from.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Asks GitHub for the latest release. `Ok(None)` means this build is up to date.
pub fn check() -> Result<Option<Release>, UpdateError> {
    let output = match Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            concat!("Libertum/", env!("CARGO_PKG_VERSION")),
            LATEST_RELEASE,
        ])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(UpdateError::CurlMissing);
        }
        Err(error) => return Err(UpdateError::Spawn(error)),
    };

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(UpdateError::CurlFailed(error));
    }

    parse_release(&String::from_utf8_lossy(&output.stdout), current_version())
}

/// Reads one release from the API's JSON. Drafts, pre-releases, releases that are not newer
/// and URLs outside this repository's release pages are ignored or refused.
pub fn parse_release(json: &str, current: &str) -> Result<Option<Release>, UpdateError> {
    let value: Value = serde_json::from_str(json).map_err(|_| UpdateError::BadRelease)?;

    if value.get("draft").and_then(Value::as_bool).unwrap_or(false)
        || value
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Ok(None);
    }

    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or(UpdateError::BadRelease)?;
    let version = tag.trim_start_matches(['v', 'V']).to_owned();

    let url = value
        .get("html_url")
        .and_then(Value::as_str)
        .ok_or(UpdateError::BadRelease)?;
    if !release_url(url) {
        return Err(UpdateError::BadRelease);
    }

    if !is_newer(current, &version) {
        return Ok(None);
    }

    Ok(Some(Release {
        version,
        url: url.to_owned(),
    }))
}

/// Whether `candidate` is newer than `current`. Versions are dotted numbers; a missing part
/// counts as zero, and a suffix after `-` or `+` is ignored (pre-releases are filtered out
/// before this is called).
pub fn is_newer(current: &str, candidate: &str) -> bool {
    let (Some(current), Some(candidate)) = (numeric_parts(current), numeric_parts(candidate))
    else {
        return false;
    };

    for index in 0..current.len().max(candidate.len()) {
        let old = current.get(index).copied().unwrap_or(0);
        let new = candidate.get(index).copied().unwrap_or(0);
        if new > old {
            return true;
        }
        if new < old {
            return false;
        }
    }
    false
}

fn numeric_parts(version: &str) -> Option<Vec<u64>> {
    let core = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()?;
    core.split('.').map(|part| part.parse().ok()).collect()
}

fn release_url(url: &str) -> bool {
    url.starts_with(RELEASE_URL_PREFIX)
}

/// Opens a release page in the user's browser. Only this repository's release URLs are
/// accepted, so a malformed API response cannot make the app launch an arbitrary URL.
pub fn open_release_page(url: &str) -> Result<(), UpdateError> {
    if !release_url(url) {
        return Err(UpdateError::BadRelease);
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        // The shell-free URL handler; `cmd /C start` would re-parse the URL.
        let mut command = Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command.spawn().map(|_| ()).map_err(UpdateError::Open)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
        "draft": false,
        "prerelease": false,
        "tag_name": "v0.1.3",
        "html_url": "https://github.com/wyn-cmd/libertum/releases/tag/v0.1.3"
    }"#;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.1.2", "0.1.3"));
        assert!(is_newer("0.1.2", "v0.1.3"));
        assert!(is_newer("0.1.2", "0.2"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(is_newer("0.1.2", "0.1.2.1"));
        assert!(!is_newer("0.1.3", "0.1.2"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.1.3", "v0.1.3"));
        assert!(!is_newer("0.1.3", "not-a-version"));
        assert!(!is_newer("0.1.3", "0.1.3-beta"));
    }

    #[test]
    fn a_newer_release_is_read() {
        let release = parse_release(RELEASE, "0.1.2")
            .expect("parse")
            .expect("newer");
        assert_eq!(release.version, "0.1.3");
        assert_eq!(
            release.url,
            "https://github.com/wyn-cmd/libertum/releases/tag/v0.1.3"
        );
        assert_eq!(release.version, "0.1.3");
    }

    #[test]
    fn drafts_prereleases_and_old_releases_are_ignored() {
        let draft = RELEASE.replace("\"draft\": false", "\"draft\": true");
        assert!(parse_release(&draft, "0.1.2").expect("parse").is_none());

        let prerelease = RELEASE.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(
            parse_release(&prerelease, "0.1.2")
                .expect("parse")
                .is_none()
        );

        assert!(parse_release(RELEASE, "0.1.3").expect("parse").is_none());
        assert!(parse_release(RELEASE, "0.2.0").expect("parse").is_none());
    }

    #[test]
    fn only_libertum_release_urls_are_accepted() {
        let evil = RELEASE.replace(
            "https://github.com/wyn-cmd/libertum/releases/tag/v0.1.3",
            "https://example.com/installer.exe",
        );
        assert!(matches!(
            parse_release(&evil, "0.1.2"),
            Err(UpdateError::BadRelease)
        ));
        assert!(matches!(
            open_release_page("https://example.com/installer.exe"),
            Err(UpdateError::BadRelease)
        ));
    }

    #[test]
    fn malformed_json_is_refused() {
        assert!(matches!(
            parse_release("not json", "0.1.2"),
            Err(UpdateError::BadRelease)
        ));
        assert!(matches!(
            parse_release("{}", "0.1.2"),
            Err(UpdateError::BadRelease)
        ));
    }
}
