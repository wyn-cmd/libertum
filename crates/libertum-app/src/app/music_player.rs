//! Music players in the document: inserting one, playing it, and what it shows.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use iced::Task;

use libertum_audio::Snapshot;
use libertum_model::{AudioCopy, AudioFormat, MediaKind, MediaLink};

use super::{App, Message, Modal, clip_id};
use crate::canvas::{PlayerOverlay, PlayerStatus};
use crate::docview::{PLAYER_ASPECT, PLAYER_DESIGN_HEIGHT};
use crate::strings as s;
use crate::wiki_fetch::{self, AudioFetchError};

/// `1:07`, or `1:02:03` past an hour.
pub fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// How a click inside a player lands. Points from the player's left and top.
#[derive(Debug, PartialEq)]
pub enum Zone {
    /// The play/pause button.
    Button,
    /// The progress bar, as a fraction of its length.
    Bar(f32),
    /// Anywhere else: an ordinary click in the text.
    Elsewhere,
}

pub fn zone(x: f32, y: f32, width: f32, height: f32) -> Zone {
    // The player is laid out on a 440 by 40 design, scaled to the line it fills.
    let scale = (height / PLAYER_DESIGN_HEIGHT).max(0.01);
    let (x, y, width, height) = (x / scale, y / scale, width / scale, height / scale);
    let inner = (width - 52.0 - 14.0).max(10.0);
    if x < 46.0 {
        Zone::Button
    } else if x >= 48.0 && y >= height * 0.5 {
        Zone::Bar(((x - 52.0) / inner).clamp(0.0, 1.0))
    } else {
        Zone::Elsewhere
    }
}

impl App {
    /// The links of every player in the document.
    pub(super) fn player_urls(&self) -> HashSet<String> {
        self.doc
            .iter()
            .flat_map(|doc| doc.document().paragraphs())
            .flat_map(|paragraph| paragraph.images().iter())
            .filter_map(|image| image.link())
            .filter(|link| link.kind == MediaKind::Audio)
            .map(|link| link.url)
            .collect()
    }

    /// The players in view, as the canvas draws them.
    pub(super) fn player_overlays(&self) -> Vec<PlayerOverlay> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let snap = &self.music.snap;
        doc.players_on(self.pages_near_view())
            .into_iter()
            .map(|spot| {
                let mine = snap.clip == Some(clip_id(&spot.url));
                let loading = self.music.loading.as_deref() == Some(spot.url.as_str());
                let status = if loading {
                    PlayerStatus::Loading
                } else if mine && let Some(why) = &snap.error {
                    PlayerStatus::Failed(why.clone())
                } else if mine && snap.playing {
                    PlayerStatus::Playing
                } else if mine {
                    PlayerStatus::Paused
                } else {
                    PlayerStatus::Idle
                };
                let (fraction, elapsed, total) = if mine {
                    (
                        snap.duration
                            .filter(|total| !total.is_zero())
                            .map(|total| snap.position.as_secs_f32() / total.as_secs_f32()),
                        clock(snap.position),
                        snap.duration.map(clock),
                    )
                } else {
                    (None, clock(Duration::ZERO), None)
                };
                PlayerOverlay {
                    page: spot.page,
                    x: spot.x,
                    y: spot.y,
                    width: spot.width,
                    height: spot.height,
                    title: if spot.title.is_empty() {
                        spot.url.clone()
                    } else {
                        spot.title.clone()
                    },
                    status,
                    fraction,
                    elapsed,
                    total,
                }
            })
            .collect()
    }

    /// Insert: the track is remembered by its link, and the player goes on its own line at the
    /// caret.
    pub(super) fn insert_player(&mut self) -> Task<Message> {
        let Some(Modal::MusicEdit(draft)) = &self.modal else {
            return Task::none();
        };
        if !draft.can_save() {
            return Task::none();
        }
        let (title, url, credit) = (
            draft.title.trim().to_owned(),
            draft.url.trim().to_owned(),
            draft.credit.clone(),
        );
        let copy = draft.current_copy().cloned();
        let Ok(id) = self.music.list.upsert(&title, &url, &credit) else {
            return Task::none();
        };
        // A copy the writer made or removed in the pop-up wins over what was there.
        self.music.list.set_copy(id, copy);
        let link = MediaLink {
            url,
            kind: MediaKind::Audio,
        };
        let width = self.text_column_width();
        self.modal = None;
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let result = doc.edit(|ed| {
            ed.insert_player(
                crate::linked::placeholder(MediaKind::Audio),
                width,
                width / PLAYER_ASPECT,
                &link,
                &title,
            )
        });
        self.canvas_focused = true;
        let task = self.finish_edit(result);
        self.refresh_format();
        task
    }

    /// The audio for `url` if it is already to hand: the copy kept in the document, or what was
    /// fetched earlier this session.
    fn audio_for(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        self.music
            .list
            .by_url(url)
            .and_then(|track| track.copy.as_ref())
            .map(|copy| copy.bytes.clone())
            .or_else(|| self.music.fetched.get(url.trim()).cloned())
    }

    fn start_clip(&mut self, url: &str, bytes: Arc<Vec<u8>>) {
        let extension = AudioFormat::sniff(&bytes).map_or("", AudioFormat::extension);
        self.music
            .player
            .play(clip_id(url), bytes, extension, Duration::ZERO);
    }

    /// A click on a player's button or bar.
    pub(super) fn on_player_click(
        &mut self,
        url: String,
        _title: String,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Task<Message> {
        let id = clip_id(&url);
        let snap: Snapshot = self.music.player.snapshot();
        let mine = snap.clip == Some(id);
        self.music.watch_until = Some(Instant::now() + Duration::from_millis(1500));
        self.music.snap = snap.clone();
        if let Zone::Bar(fraction) = zone(x, y, width, height)
            && mine
            && snap.error.is_none()
            && let Some(total) = snap.duration
        {
            self.music.player.seek(total.mul_f32(fraction));
            return Task::none();
        }
        if mine && snap.playing {
            self.music.player.pause();
            return Task::none();
        }
        if mine && snap.error.is_none() {
            self.music.player.resume();
            return Task::none();
        }
        if self.music.loading.as_deref() == Some(url.as_str()) {
            return Task::none();
        }
        if let Some(bytes) = self.audio_for(&url) {
            self.start_clip(&url, bytes);
            return Task::none();
        }
        // Playing a link that was never kept fetches it, now, because the writer pressed play.
        self.music.loading = Some(url.clone());
        let asked = url.clone();
        Task::perform(
            async move { wiki_fetch::fetch_audio(&url, Arc::new(AtomicBool::new(false))) },
            move |result| Message::MusicLoaded(asked.clone(), result),
        )
    }

    pub(super) fn on_music_loaded(
        &mut self,
        url: String,
        result: Result<AudioCopy, AudioFetchError>,
    ) -> Task<Message> {
        // The document changed or closed while it was fetching: nothing should start.
        if self.music.loading.as_deref() != Some(url.as_str()) {
            return Task::none();
        }
        self.music.loading = None;
        match result {
            Ok(copy) => {
                self.music.fetched.insert(url.clone(), copy.bytes.clone());
                self.start_clip(&url, copy.bytes);
                self.music.watch_until = Some(Instant::now() + Duration::from_millis(1500));
            }
            Err(error) => {
                self.modal = Some(Modal::Error(format!(
                    "{} {}",
                    s::MUSIC_PLAY_FAILED,
                    super::music_popups::fetch_failure(&error)
                )));
            }
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_shows_minutes_and_seconds() {
        assert_eq!(clock(Duration::from_secs(7)), "0:07");
        assert_eq!(clock(Duration::from_secs(67)), "1:07");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn a_click_lands_on_the_button_the_bar_or_the_text() {
        assert_eq!(zone(10.0, 20.0, 400.0, 40.0), Zone::Button);
        // A player scaled to half its design: the same places, at half the distance.
        assert_eq!(zone(5.0, 10.0, 220.0, 20.0), Zone::Button);
        assert_eq!(zone(50.0, 5.0, 220.0, 20.0), Zone::Elsewhere);
        assert!(matches!(zone(100.0, 15.0, 220.0, 20.0), Zone::Bar(_)));
        assert_eq!(zone(100.0, 5.0, 400.0, 40.0), Zone::Elsewhere);
        assert_eq!(zone(52.0, 30.0, 400.0, 40.0), Zone::Bar(0.0));
        assert_eq!(zone(399.0, 30.0, 400.0, 40.0), Zone::Bar(1.0));
        let Zone::Bar(half) = zone(52.0 + 167.0, 30.0, 400.0, 40.0) else {
            panic!("on the bar")
        };
        assert!((half - 0.5).abs() < 0.01);
    }
}
