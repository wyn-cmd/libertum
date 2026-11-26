//! Fetching for the Characters tab's Import: the curl-backed [`Fetcher`] and the lookup that
//! runs on a worker thread. Only started by the writer pressing Fetch.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use libertum_wiki::{Candidate, FetchError, Fetcher, Outcome, Request, Response, USER_AGENT};

use crate::update::hide_console_window;

/// Most bytes of one answer.
const MAX_BYTES: u64 = 2_000_000;

/// Makes requests with `curl`, which every supported system has (the update check uses it too):
/// https only, no redirect to another host, a time and a size limit, and a User-Agent that says
/// what this is.
pub struct CurlFetcher {
    pub cancel: Arc<AtomicBool>,
}

impl Fetcher for CurlFetcher {
    fn get(&mut self, url: &str) -> Result<Response, FetchError> {
        let Some(host) = url
            .strip_prefix("https://")
            .map(|rest| rest.split('/').next().unwrap_or(""))
        else {
            return Err(FetchError::Network("only https addresses are used".into()));
        };
        let mut curl = Command::new("curl");
        curl.args([
            "-sS",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--location",
            "--max-redirs",
            "3",
            "--max-time",
            "25",
            "--max-filesize",
            &MAX_BYTES.to_string(),
            "-A",
            USER_AGENT,
            "-H",
            "Accept: application/json, text/plain, */*",
            "-w",
            "\n%{http_code}\n%{url_effective}",
            url,
        ]);
        hide_console_window(&mut curl);
        let output = curl.output().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                FetchError::Network("curl is not installed".into())
            } else {
                FetchError::Network(error.to_string())
            }
        })?;
        if output.status.code() == Some(63) {
            return Err(FetchError::TooLarge);
        }
        if !output.status.success() {
            return Err(FetchError::Network(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        parse_output(&String::from_utf8_lossy(&output.stdout), host)
    }

    fn pause(&mut self, duration: Duration) {
        // In short steps, so Stop is noticed within a moment.
        let step = Duration::from_millis(100);
        let mut left = duration;
        while !left.is_zero() && !self.cancelled() {
            let now = left.min(step);
            std::thread::sleep(now);
            left -= now;
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Splits curl's output (the body, then the status and the final address on their own lines)
/// and checks that the answer came from the host that was asked.
fn parse_output(output: &str, host: &str) -> Result<Response, FetchError> {
    let mut parts = output.rsplitn(3, '\n');
    let effective = parts.next().unwrap_or("");
    let status = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("");
    let status: u16 = status
        .trim()
        .parse()
        .map_err(|_| FetchError::Network("no answer".into()))?;
    let final_host = effective
        .trim()
        .strip_prefix("https://")
        .map(|rest| rest.split('/').next().unwrap_or(""));
    if final_host != Some(host) {
        return Err(FetchError::Redirected);
    }
    Ok(Response {
        status,
        body: body.to_owned(),
    })
}

/// What a lookup brought back, ready to show.
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub candidates: Vec<Candidate>,
    /// What AO3's recent works said about a fandom (the fandom as typed, then the profile).
    pub profiles: Vec<(String, libertum_wiki::Profile)>,
    /// One line per site that was tried and gave nothing, or why it stopped.
    pub report: Vec<String>,
}

/// The most requests one fandom's lookup may make.
pub const MAX_REQUESTS: usize = 60;

/// Looks up each fandom in turn. Runs on a worker thread; blocks until done or cancelled.
pub fn find(requests: Vec<Request>, cancel: Arc<AtomicBool>) -> Found {
    let mut fetcher = CurlFetcher { cancel };
    let mut found = Found::default();
    for request in requests {
        let outcome = libertum_wiki::run(&mut fetcher, &request);
        found.report.extend(describe(&request.fandom, &outcome));
        if let Some(profile) = outcome.profile {
            found.profiles.push((request.fandom.clone(), profile));
        }
        found.candidates.extend(outcome.candidates);
        if outcome.stopped.is_some() {
            break;
        }
    }
    found
}

/// The lines that explain an outcome to the writer.
pub fn describe(fandom: &str, outcome: &Outcome) -> Vec<String> {
    use crate::strings as s;
    use libertum_wiki::{SkipReason, Stop};
    let mut lines = Vec::new();
    for skip in &outcome.skipped {
        let why = match &skip.reason {
            SkipReason::RobotsDisallow => s::IMPORT_SKIP_ROBOTS.to_owned(),
            SkipReason::RobotsUnreachable => s::IMPORT_SKIP_ROBOTS_UNREADABLE.to_owned(),
            SkipReason::NotFound => s::IMPORT_SKIP_NOT_FOUND.to_owned(),
            SkipReason::NotMediaWiki => s::IMPORT_SKIP_NOT_WIKI.to_owned(),
            SkipReason::Refused(code) => format!("{} ({code})", s::IMPORT_SKIP_REFUSED),
            SkipReason::NoCharacters => s::IMPORT_SKIP_EMPTY.to_owned(),
            SkipReason::NoWikiName => s::IMPORT_SKIP_NO_NAME.to_owned(),
            SkipReason::Blocked => s::IMPORT_SKIP_BLOCKED.to_owned(),
            SkipReason::UnexpectedPage => s::IMPORT_SKIP_UNEXPECTED.to_owned(),
            SkipReason::Network(detail) => format!("{}: {detail}", s::IMPORT_SKIP_NETWORK),
        };
        lines.push(format!("{fandom}, {}: {why}", skip.site));
    }
    match outcome.stopped {
        Some(Stop::SlowDown) => lines.push(s::IMPORT_STOPPED_SLOW.to_owned()),
        Some(Stop::OutOfRequests) => lines.push(s::IMPORT_STOPPED_LIMIT.to_owned()),
        Some(Stop::Cancelled) => lines.push(s::IMPORT_STOPPED_CANCEL.to_owned()),
        None => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_output_is_split_and_a_redirect_to_another_host_is_refused() {
        let ok = parse_output(
            "{\"a\":1}\n200\nhttps://x.fandom.com/api.php?x=1",
            "x.fandom.com",
        )
        .unwrap();
        assert_eq!((ok.status, ok.body.as_str()), (200, "{\"a\":1}"));
        let multiline = parse_output(
            "line1\nline2\n404\nhttps://x.fandom.com/robots.txt",
            "x.fandom.com",
        )
        .unwrap();
        assert_eq!(multiline.body, "line1\nline2");
        assert_eq!(
            parse_output("<html>\n200\nhttps://www.fandom.com/", "x.fandom.com").unwrap_err(),
            FetchError::Redirected
        );
        assert_eq!(
            parse_output("x\n200\nhttp://x.fandom.com/", "x.fandom.com").unwrap_err(),
            FetchError::Redirected,
            "a plain http address is never accepted"
        );
        assert!(parse_output("garbage", "x.fandom.com").is_err());
    }

    /// Talks to the real sites: `cargo test -p libertum-app live_ -- --ignored --nocapture`. A handful
    /// of requests, one at a time, so it is safe to run by hand; never part of the normal run.
    #[test]
    #[ignore = "uses the network"]
    fn live_fandom_and_wikipedia_answer_through_curl() {
        let sites = libertum_wiki::built_in();
        let pick = |id: &str| {
            sites
                .iter()
                .filter(|s| s.id == id)
                .cloned()
                .collect::<Vec<_>>()
        };
        let requests = vec![
            Request {
                fandom: "Harry Potter".into(),
                slug: "harrypotter".into(),
                sites: pick("fandom"),
                categories: vec!["Characters".into(), "Individuals".into()],
                max_requests: 12,
            },
            Request {
                fandom: "Harry Potter".into(),
                slug: String::new(),
                sites: pick("wikipedia"),
                categories: vec!["Harry Potter characters".into()],
                max_requests: 4,
            },
        ];
        let found = find(requests, Arc::new(AtomicBool::new(false)));
        for line in &found.report {
            println!("report: {line}");
        }
        println!("{} names; first ten:", found.candidates.len());
        for c in found.candidates.iter().take(10) {
            println!("  {} <- {} ({})", c.name, c.title, c.site);
        }
        assert!(!found.candidates.is_empty());
    }

    #[test]
    fn a_stopped_fetcher_stops_pausing() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut fetcher = CurlFetcher { cancel };
        let started = std::time::Instant::now();
        fetcher.pause(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

/// Why an audio file was not fetched, in words for the writer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioFetchError {
    NotHttps,
    /// The site's `robots.txt` does not allow the file (or could not be read).
    Robots(String),
    Refused(u16),
    TooLarge,
    NotAudio,
    Stopped,
    Network(String),
}

/// Most bytes one audio fetch keeps.
const MAX_AUDIO_FETCH: u64 = libertum_model::MAX_AUDIO_BYTES as u64;
/// Most redirects followed, each one checked against its own site's `robots.txt`.
const MAX_HOPS: usize = 3;

/// `https://host/path?query` as (host, path and query). Anything else is `None`.
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    if host.is_empty() || host.contains('@') || host.chars().any(char::is_whitespace) {
        return None;
    }
    let path = rest[end..].split('#').next().unwrap_or("");
    Some((
        host,
        if path.is_empty() || path.starts_with('?') {
            "/"
        } else {
            path
        },
    ))
}

/// Downloads the audio file at `url` for a copy kept in the document. Only started by the
/// writer pressing the button, once. It reads the site's `robots.txt` first (for every site a
/// redirect leads to as well), uses https only, caps the size and the time, and keeps the file
/// only if its first bytes say it is audio. Blocks until done or cancelled.
pub fn fetch_audio(
    url: &str,
    cancel: Arc<AtomicBool>,
) -> Result<libertum_model::AudioCopy, AudioFetchError> {
    let mut current = url.trim().to_owned();
    for _ in 0..=MAX_HOPS {
        let (host, path) = split_url(&current).ok_or(AudioFetchError::NotHttps)?;
        check_robots(host, path, &cancel)?;
        match curl_once(&current, &cancel)? {
            Hop::Done(bytes) => {
                return libertum_model::AudioCopy::new(bytes).map_err(|error| match error {
                    libertum_model::AudioError::NotAudio => AudioFetchError::NotAudio,
                    libertum_model::AudioError::TooLarge => AudioFetchError::TooLarge,
                });
            }
            Hop::Redirect(next) => current = next,
        }
    }
    Err(AudioFetchError::Network("too many redirects".into()))
}

fn check_robots(host: &str, path: &str, cancel: &Arc<AtomicBool>) -> Result<(), AudioFetchError> {
    let mut fetcher = CurlFetcher {
        cancel: cancel.clone(),
    };
    let robots = match fetcher.get(&format!("https://{host}/robots.txt")) {
        Ok(response) => libertum_wiki::Robots::from_response(response.status, &response.body),
        Err(_) => return Err(AudioFetchError::Robots(host.to_owned())),
    };
    if robots.allows(libertum_wiki::ROBOTS_TOKEN, path) {
        Ok(())
    } else {
        Err(AudioFetchError::Robots(host.to_owned()))
    }
}

enum Hop {
    Done(Vec<u8>),
    Redirect(String),
}

/// One request, to a file in the temporary folder that is removed again. The process is polled
/// so Stop is noticed within a moment.
fn curl_once(url: &str, cancel: &Arc<AtomicBool>) -> Result<Hop, AudioFetchError> {
    use std::process::Stdio;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let file = std::env::temp_dir().join(format!(
        "libertum-audio-{}-{nanos}.part",
        std::process::id()
    ));
    let mut curl = Command::new("curl");
    curl.args([
        "-sS",
        "--proto",
        "=https",
        "--max-time",
        "180",
        "--max-filesize",
        &MAX_AUDIO_FETCH.to_string(),
        "-A",
        USER_AGENT,
        "-H",
        "Accept: audio/*, */*",
        "-o",
    ])
    .arg(&file)
    .args(["-w", "%{http_code}\n%{redirect_url}", url])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    hide_console_window(&mut curl);
    let child = curl.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AudioFetchError::Network("curl is not installed".into())
        } else {
            AudioFetchError::Network(error.to_string())
        }
    })?;
    let mut child = child;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&file);
                    return Err(AudioFetchError::Stopped);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                let _ = std::fs::remove_file(&file);
                return Err(AudioFetchError::Network(error.to_string()));
            }
        }
    };
    let output = child
        .wait_with_output()
        .map_err(|error| AudioFetchError::Network(error.to_string()))?;
    let bytes = std::fs::read(&file).unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    if status.code() == Some(63) {
        return Err(AudioFetchError::TooLarge);
    }
    if !status.success() {
        return Err(AudioFetchError::Network(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let written = String::from_utf8_lossy(&output.stdout);
    let mut lines = written.lines();
    let code: u16 = lines
        .next()
        .and_then(|line| line.trim().parse().ok())
        .ok_or_else(|| AudioFetchError::Network("no answer".into()))?;
    let redirect = lines.next().unwrap_or("").trim();
    if (300..400).contains(&code) && !redirect.is_empty() {
        return Ok(Hop::Redirect(redirect.to_owned()));
    }
    if code != 200 {
        return Err(AudioFetchError::Refused(code));
    }
    Ok(Hop::Done(bytes))
}

#[cfg(test)]
mod audio_tests {
    use super::*;

    #[test]
    fn an_address_splits_into_site_and_path() {
        assert_eq!(
            split_url("https://a.example/x/y.mp3?z=1"),
            Some(("a.example", "/x/y.mp3?z=1"))
        );
        assert_eq!(split_url("https://a.example"), Some(("a.example", "/")));
        assert_eq!(split_url("https://a.example?x=1"), Some(("a.example", "/")));
        assert_eq!(
            split_url("https://a.example/f.mp3#t=5"),
            Some(("a.example", "/f.mp3"))
        );
        assert_eq!(split_url("http://a.example/x.mp3"), None);
        assert_eq!(split_url("https://user@a.example/x.mp3"), None);
        assert_eq!(split_url("https:///x.mp3"), None);
        assert_eq!(split_url("file:///etc/passwd"), None);
    }

    #[test]
    fn a_plain_http_address_is_refused_before_anything_is_fetched() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            fetch_audio("http://a.example/x.mp3", cancel).map(|_| ()),
            Err(AudioFetchError::NotHttps)
        );
    }
}
