//! Pictures and videos that live online, for AO3 chapters (which cannot host files).
//!
//! The document holds the address (see [`libertum_model::MediaLink`]) and a picture to show:
//! a placeholder drawn here, or a preview the writer asked Libertum to download. Nothing is
//! fetched unless the writer chooses to; then the system `curl` makes one request to the
//! address given, and nothing about the document or the writer is sent.

use std::process::Command;
use std::sync::Arc;

use libertum_formats_common::media::{media_from_bytes, png_from_rgba};
use libertum_model::{MediaKind, MediaLink};

use crate::update::hide_console_window;

/// The largest preview that is downloaded, in bytes.
const MAX_PREVIEW_BYTES: usize = 15_000_000;

/// Placeholder size in pixels (16:9).
const PLACEHOLDER_W: u32 = 320;
const PLACEHOLDER_H: u32 = 180;

/// File extensions that mean the address is a video even on a site that is not known.
const VIDEO_EXTENSIONS: [&str; 6] = [".mp4", ".webm", ".ogv", ".ogg", ".m4v", ".mov"];

/// Sites whose pages are videos.
const VIDEO_SITES: [&str; 6] = [
    "youtube.com/",
    "youtu.be/",
    "vimeo.com/",
    "dailymotion.com/",
    "twitch.tv/",
    "bilibili.com/",
];

/// Whether an address looks like a video: a video file, or a page on a video site.
pub fn looks_like_video(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or("");
    VIDEO_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
        || VIDEO_SITES.iter().any(|site| lower.contains(site))
}

/// The address with its scheme made explicit, or `None` when it is not a web address.
pub fn normalise(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let url = if lower.starts_with("https://") || lower.starts_with("http://") {
        trimmed.to_owned()
    } else if trimmed.contains("://") {
        return None;
    } else if trimmed.contains('.') {
        format!("https://{trimmed}")
    } else {
        return None;
    };
    // Something after the scheme.
    url.split("://")
        .nth(1)
        .is_some_and(|rest| !rest.is_empty())
        .then_some(url)
}

/// The address of a video's thumbnail when it can be worked out without asking the site
/// anything: a YouTube video's `hqdefault.jpg`.
pub fn video_thumbnail(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest
        .strip_prefix("www.")
        .or_else(|| rest.strip_prefix("m."))
        .unwrap_or(rest);
    let id = if let Some(path) = rest.strip_prefix("youtu.be/") {
        path.split(['?', '#', '/']).next()
    } else if let Some(path) = rest.strip_prefix("youtube.com/") {
        if let Some(query) = path.strip_prefix("watch?") {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("v="))
                .map(|id| id.split('#').next().unwrap_or(id))
        } else {
            ["embed/", "shorts/", "live/"]
                .iter()
                .find_map(|prefix| path.strip_prefix(prefix))
                .and_then(|id| id.split(['?', '#', '/']).next())
        }
    } else {
        None
    }?;
    let valid = !id.is_empty()
        && id.len() <= 20
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| format!("https://img.youtube.com/vi/{id}/hqdefault.jpg"))
}

/// Where a preview of `link` can be downloaded from: the picture itself, or a video's
/// thumbnail when the site's address scheme gives one. `None` for other videos.
pub fn preview_address(link: &MediaLink) -> Option<String> {
    match link.kind {
        MediaKind::Image => Some(link.url.clone()),
        MediaKind::Video => video_thumbnail(&link.url),
    }
}

/// Downloads a picture (PNG, JPEG, or GIF and BMP converted to PNG) with `curl`: one request,
/// http or https only, at most [`MAX_PREVIEW_BYTES`], a minute at most.
pub fn fetch_image(url: &str) -> Result<Arc<libertum_model::Media>, String> {
    let mut command = Command::new("curl");
    command.args([
        "-fsSL",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
        "--max-filesize",
        &MAX_PREVIEW_BYTES.to_string(),
        "--connect-timeout",
        "15",
        "--max-time",
        "60",
        "-A",
        concat!("Libertum/", env!("CARGO_PKG_VERSION")),
        "--",
        url,
    ]);
    hide_console_window(&mut command);
    let output = command.output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "curl is not installed".to_owned()
        } else {
            format!("could not run curl: {error}")
        }
    })?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    if output.stdout.len() > MAX_PREVIEW_BYTES {
        return Err("the picture is too large".to_owned());
    }
    media_from_bytes(output.stdout).map_err(|_| {
        "the address is not a picture Libertum can show (PNG, JPEG, GIF or BMP)".to_owned()
    })
}

/// A grey 16:9 picture with a play triangle (video) or a sun and hills (picture), shown until
/// a preview is downloaded.
pub fn placeholder(kind: MediaKind) -> Arc<libertum_model::Media> {
    let (w, h) = (PLACEHOLDER_W as i32, PLACEHOLDER_H as i32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let inside_triangle = |p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        let side = |p: (f32, f32), q: (f32, f32), r: (f32, f32)| {
            (p.0 - r.0) * (q.1 - r.1) - (q.0 - r.0) * (p.1 - r.1)
        };
        let (d1, d2, d3) = (side(p, a, b), side(p, b, c), side(p, c, a));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    for y in 0..h {
        for x in 0..w {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let edge = x < 2 || y < 2 || x >= w - 2 || y >= h - 2;
            let mut colour = if edge {
                [168, 172, 180]
            } else {
                [226, 229, 234]
            };
            let ink = [96, 102, 114];
            match kind {
                MediaKind::Video => {
                    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
                    let ring = ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt();
                    if (ring > 44.0 && ring < 48.0)
                        || inside_triangle(
                            p,
                            (cx - 14.0, cy - 24.0),
                            (cx - 14.0, cy + 24.0),
                            (cx + 26.0, cy),
                        )
                    {
                        colour = ink;
                    }
                }
                MediaKind::Image => {
                    let sun = ((p.0 - 232.0).powi(2) + (p.1 - 52.0).powi(2)).sqrt() < 16.0;
                    let far = inside_triangle(p, (60.0, 140.0), (130.0, 70.0), (200.0, 140.0));
                    let near = inside_triangle(p, (130.0, 140.0), (200.0, 90.0), (270.0, 140.0));
                    if sun || far || near {
                        colour = ink;
                    }
                }
            }
            let at = ((y * w + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
    }
    png_from_rgba(PLACEHOLDER_W, PLACEHOLDER_H, rgba).expect("a fixed-size placeholder encodes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_cleaned_and_checked() {
        assert_eq!(
            normalise(" https://a.com/x.png "),
            Some("https://a.com/x.png".into())
        );
        assert_eq!(
            normalise("example.com/a.png"),
            Some("https://example.com/a.png".into())
        );
        assert_eq!(normalise("ftp://example.com/a.png"), None);
        assert_eq!(normalise("file:///etc/passwd"), None);
        assert_eq!(normalise("not a url"), None);
        assert_eq!(normalise("localword"), None);
        assert_eq!(normalise("https://"), None);
    }

    #[test]
    fn videos_are_told_from_pictures() {
        assert!(looks_like_video("https://a.com/clip.MP4?x=1"));
        assert!(looks_like_video("https://www.youtube.com/watch?v=abc"));
        assert!(looks_like_video("https://youtu.be/abc"));
        assert!(!looks_like_video("https://a.com/pic.png"));
    }

    #[test]
    fn a_youtube_thumbnail_is_found_from_the_address() {
        let thumb = |u: &str| video_thumbnail(u);
        let want = Some("https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg".to_owned());
        assert_eq!(thumb("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), want);
        assert_eq!(
            thumb("https://youtube.com/watch?feature=x&v=dQw4w9WgXcQ&t=3"),
            want
        );
        assert_eq!(thumb("https://youtu.be/dQw4w9WgXcQ?t=5"), want);
        assert_eq!(thumb("https://www.youtube.com/embed/dQw4w9WgXcQ"), want);
        assert_eq!(thumb("https://www.youtube.com/shorts/dQw4w9WgXcQ"), want);
        assert_eq!(thumb("https://vimeo.com/12345"), None);
        assert_eq!(thumb("https://youtu.be/bad id"), None);
    }

    #[test]
    fn placeholders_are_pictures() {
        for kind in [MediaKind::Image, MediaKind::Video] {
            let media = placeholder(kind);
            assert_eq!((media.width_px(), media.height_px()), (320, 180));
        }
        assert_ne!(
            placeholder(MediaKind::Image).hash(),
            placeholder(MediaKind::Video).hash()
        );
    }

    /// Serves `body` once on a local port and returns the address.
    fn serve_once(content_type: &'static str, body: Vec<u8>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/p")
    }

    #[test]
    fn a_preview_is_downloaded_and_a_non_picture_is_refused() {
        if Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let png = placeholder(MediaKind::Image);
        let url = serve_once("image/png", png.bytes().to_vec());
        let got = fetch_image(&url).expect("the picture downloads");
        assert_eq!((got.width_px(), got.height_px()), (320, 180));
        let url = serve_once("text/html", b"<html>not a picture</html>".to_vec());
        assert!(fetch_image(&url).is_err());
        // Other schemes are not followed.
        assert!(fetch_image("file:///etc/hostname").is_err());
    }
}
