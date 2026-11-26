//! Music and audio a writer attaches to a story: a link, a title, the chapters it belongs to,
//! and optionally a copy of the file kept inside the document.
//!
//! The link is the source of truth. It is what an AO3 chapter embeds (`<audio src="...">`),
//! and it is always kept. The copy is only there so the audio still plays with no connection;
//! it is made on the writer's say-so and can be dropped again without losing the link.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The largest audio file kept in a document, bytes.
pub const MAX_AUDIO_BYTES: usize = 30_000_000;

/// The audio encodings recognised by their first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Mp3,
    Ogg,
    Wav,
    Flac,
    M4a,
}

impl AudioFormat {
    /// What the bytes are, read from their header; `None` for anything else (a web page where
    /// a file was expected, say).
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"ID3")
            || (bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
        {
            Some(Self::Mp3)
        } else if bytes.starts_with(b"OggS") {
            Some(Self::Ogg)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            Some(Self::Wav)
        } else if bytes.starts_with(b"fLaC") {
            Some(Self::Flac)
        } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
            Some(Self::M4a)
        } else {
            None
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::M4a => "m4a",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Ogg => "audio/ogg",
            Self::Wav => "audio/wav",
            Self::Flac => "audio/flac",
            Self::M4a => "audio/mp4",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        [Self::Mp3, Self::Ogg, Self::Wav, Self::Flac, Self::M4a]
            .into_iter()
            .find(|format| format.extension() == extension)
    }
}

/// A fixed hash (FNV-1a), so the name of a stored copy is the same on every machine and build.
pub fn audio_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The copy of a track's file kept in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioCopy {
    pub format: AudioFormat,
    pub bytes: Arc<Vec<u8>>,
}

impl AudioCopy {
    pub fn hash(&self) -> u64 {
        audio_hash(&self.bytes)
    }
}

/// Why a copy was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error("that is not an audio file (MP3, OGG, WAV, FLAC or M4A)")]
    NotAudio,
    #[error("the file is larger than the 30 MB a document keeps")]
    TooLarge,
}

impl AudioCopy {
    pub fn new(bytes: Vec<u8>) -> Result<Self, AudioError> {
        if bytes.len() > MAX_AUDIO_BYTES {
            return Err(AudioError::TooLarge);
        }
        let format = AudioFormat::sniff(&bytes).ok_or(AudioError::NotAudio)?;
        Ok(Self {
            format,
            bytes: Arc::new(bytes),
        })
    }
}

/// What is saved in `audio.json` for a track. The bytes are a separate entry, `audio/<hash>.<ext>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredCopy {
    pub hash: u64,
    pub format: Option<AudioFormat>,
}

/// One track: what the writer typed about a link, and the copy kept for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub id: u64,
    pub title: String,
    /// Where the audio is: an https address. A player in the document points at it.
    pub url: String,
    /// Who made it, for the credit that goes with it.
    pub credit: String,
    /// The copy kept in the document. Not part of `audio.json`; the file holds it in its own
    /// entry and gives it back through [`Soundtrack::from_json`].
    #[serde(skip)]
    pub copy: Option<AudioCopy>,
}

impl Track {
    /// The `<audio>` element AO3's HTML editor accepts. `None` without an https link.
    pub fn embed(&self) -> Option<String> {
        let url = self.url.trim();
        if !url.starts_with("https://") {
            return None;
        }
        let escaped = url
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;");
        Some(format!("<audio controls src=\"{escaped}\"></audio>"))
    }
}

/// Why a track could not be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackError {
    NoTitle,
    NoSource,
    /// The link is not an https address.
    NotHttps,
}

/// The tracks of a document, in the order they were added. A player in the text finds its
/// track by link.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Soundtrack {
    items: Vec<Track>,
    next_id: u64,
}

/// The shape of `audio.json`.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Saved {
    items: Vec<SavedTrack>,
    next_id: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SavedTrack {
    #[serde(flatten)]
    track: Track,
    stored: StoredCopy,
}

impl Soundtrack {
    pub fn items(&self) -> &[Track] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn get(&self, id: u64) -> Option<&Track> {
        self.items.iter().find(|track| track.id == id)
    }

    /// The track for a link (ignoring surrounding spaces).
    pub fn by_url(&self, url: &str) -> Option<&Track> {
        let url = url.trim();
        self.items.iter().find(|track| track.url == url)
    }

    /// Whether a track with these could be saved; `has_copy` is whether it keeps a file.
    pub fn validate(title: &str, url: &str, has_copy: bool) -> Result<(), TrackError> {
        if title.trim().is_empty() {
            Err(TrackError::NoTitle)
        } else if url.trim().is_empty() {
            if has_copy {
                Ok(())
            } else {
                Err(TrackError::NoSource)
            }
        } else if !url.trim().starts_with("https://") {
            Err(TrackError::NotHttps)
        } else {
            Ok(())
        }
    }

    /// Adds the track for `url`, or updates it when there already is one. The copy is kept
    /// only while the link is the one it was made from (here, the same link: so it is kept).
    pub fn upsert(&mut self, title: &str, url: &str, credit: &str) -> Result<u64, TrackError> {
        Self::validate(title, url, false)?;
        let url = url.trim();
        if let Some(track) = self.items.iter_mut().find(|t| t.url == url) {
            track.title = title.trim().to_owned();
            track.credit = credit.trim().to_owned();
            return Ok(track.id);
        }
        self.next_id = self
            .next_id
            .max(self.items.iter().map(|t| t.id).max().unwrap_or(0))
            + 1;
        let id = self.next_id;
        self.items.push(Track {
            id,
            title: title.trim().to_owned(),
            url: url.to_owned(),
            credit: credit.trim().to_owned(),
            copy: None,
        });
        Ok(id)
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|track| track.id != id);
    }

    /// Keeps `copy` for the track, or drops its copy when `None`.
    pub fn set_copy(&mut self, id: u64, copy: Option<AudioCopy>) {
        if let Some(track) = self.items.iter_mut().find(|t| t.id == id) {
            track.copy = copy;
        }
    }

    /// Drops the tracks no player points at any more, so a deleted player does not leave its
    /// audio inside the file.
    pub fn retain_urls(&mut self, used: &std::collections::HashSet<String>) {
        self.items.retain(|track| used.contains(&track.url));
    }

    /// The distinct copies, by hash, for a save to write once each.
    pub fn copies(&self) -> std::collections::BTreeMap<u64, AudioCopy> {
        self.items
            .iter()
            .filter_map(|track| track.copy.clone())
            .map(|copy| (copy.hash(), copy))
            .collect()
    }

    /// The bytes of `audio.json`.
    pub fn to_json(&self) -> Vec<u8> {
        let saved = Saved {
            items: self
                .items
                .iter()
                .map(|track| SavedTrack {
                    stored: track
                        .copy
                        .as_ref()
                        .map(|copy| StoredCopy {
                            hash: copy.hash(),
                            format: Some(copy.format),
                        })
                        .unwrap_or_default(),
                    track: track.clone(),
                })
                .collect(),
            next_id: self.next_id,
        };
        serde_json::to_vec(&saved).unwrap_or_default()
    }

    /// Reads `audio.json`, then gives each track its copy from `find` (by hash). A copy that is
    /// missing leaves the track as a link, which is never worse than losing the track.
    pub fn from_json(bytes: &[u8], mut find: impl FnMut(u64) -> Option<AudioCopy>) -> Self {
        let saved: Saved = serde_json::from_slice(bytes).unwrap_or_default();
        let mut soundtrack = Soundtrack {
            next_id: saved.next_id,
            items: Vec::new(),
        };
        for SavedTrack { mut track, stored } in saved.items {
            if stored.format.is_some() {
                track.copy = find(stored.hash);
            }
            soundtrack.next_id = soundtrack.next_id.max(track.id);
            soundtrack.items.push(track);
        }
        soundtrack
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp3() -> Vec<u8> {
        let mut bytes = b"ID3\x04\0\0\0\0\0\0".to_vec();
        bytes.extend_from_slice(&[0xAB; 64]);
        bytes
    }

    #[test]
    fn formats_are_read_from_the_first_bytes() {
        assert_eq!(AudioFormat::sniff(&mp3()), Some(AudioFormat::Mp3));
        assert_eq!(AudioFormat::sniff(b"OggS\0\x02"), Some(AudioFormat::Ogg));
        assert_eq!(
            AudioFormat::sniff(b"RIFF\0\0\0\0WAVEfmt "),
            Some(AudioFormat::Wav)
        );
        assert_eq!(AudioFormat::sniff(b"fLaC\0\0"), Some(AudioFormat::Flac));
        assert_eq!(
            AudioFormat::sniff(b"\0\0\0\x20ftypM4A "),
            Some(AudioFormat::M4a)
        );
        assert_eq!(AudioFormat::sniff(b"<!DOCTYPE html><html>"), None);
        assert_eq!(AudioFormat::sniff(b""), None);
    }

    #[test]
    fn a_web_page_or_a_huge_file_is_not_kept() {
        assert_eq!(
            AudioCopy::new(b"<html>sign in</html>".to_vec()),
            Err(AudioError::NotAudio)
        );
        let mut big = mp3();
        big.resize(MAX_AUDIO_BYTES + 1, 0);
        assert_eq!(AudioCopy::new(big), Err(AudioError::TooLarge));
    }

    #[test]
    fn a_track_needs_a_title_and_an_https_link() {
        let mut list = Soundtrack::default();
        assert_eq!(
            list.upsert(" ", "https://a.example/x.mp3", ""),
            Err(TrackError::NoTitle)
        );
        assert_eq!(list.upsert("Song", "", ""), Err(TrackError::NoSource));
        assert_eq!(
            list.upsert("Song", "http://a.example/x.mp3", ""),
            Err(TrackError::NotHttps)
        );
        assert!(
            list.upsert("Song", " https://a.example/x.mp3 ", "Ana")
                .is_ok()
        );
        assert_eq!(list.items()[0].url, "https://a.example/x.mp3");
    }

    #[test]
    fn the_embed_is_the_audio_element_ao3_accepts() {
        let mut list = Soundtrack::default();
        let id = list
            .upsert("Song", "https://a.example/x.mp3?a=1&b=\"2\"", "")
            .unwrap();
        assert_eq!(
            list.get(id).unwrap().embed().as_deref(),
            Some(
                "<audio controls src=\"https://a.example/x.mp3?a=1&amp;b=&quot;2&quot;\"></audio>"
            )
        );
    }

    #[test]
    fn the_same_link_is_one_track_and_keeps_its_copy() {
        let mut list = Soundtrack::default();
        let id = list
            .upsert("Song", "https://a.example/x.mp3", "Ana")
            .unwrap();
        list.set_copy(id, Some(AudioCopy::new(mp3()).unwrap()));
        let again = list
            .upsert("Renamed", "https://a.example/x.mp3", "Ana")
            .unwrap();
        assert_eq!(again, id);
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(id).unwrap().title, "Renamed");
        assert!(list.get(id).unwrap().copy.is_some());
        assert_eq!(
            list.by_url(" https://a.example/x.mp3 ").map(|t| t.id),
            Some(id)
        );
        assert!(list.by_url("https://a.example/other.mp3").is_none());
    }

    #[test]
    fn a_copy_survives_a_round_trip() {
        let mut list = Soundtrack::default();
        let id = list
            .upsert("Song", "https://a.example/x.mp3", "Ana")
            .unwrap();
        let copy = AudioCopy::new(mp3()).unwrap();
        list.set_copy(id, Some(copy.clone()));
        let copies = list.copies();
        let json = list.to_json();
        let back = Soundtrack::from_json(&json, |hash| copies.get(&hash).cloned());
        assert_eq!(back, list);
        assert_eq!(back.get(id).unwrap().copy, Some(copy));

        // A missing entry leaves the link.
        let bare = Soundtrack::from_json(&json, |_| None);
        assert_eq!(bare.get(id).unwrap().copy, None);
        assert_eq!(bare.get(id).unwrap().url, "https://a.example/x.mp3");
    }

    #[test]
    fn tracks_no_player_points_at_are_dropped_and_ids_are_not_reused() {
        let mut list = Soundtrack::default();
        let a = list.upsert("A", "https://a.example/a.mp3", "").unwrap();
        list.upsert("B", "https://a.example/b.mp3", "").unwrap();
        let used: std::collections::HashSet<String> =
            ["https://a.example/b.mp3".to_owned()].into_iter().collect();
        list.retain_urls(&used);
        assert_eq!(list.len(), 1);
        assert!(list.get(a).is_none());
        let c = list.upsert("C", "https://a.example/c.mp3", "").unwrap();
        assert!(c > a);
    }
}
