//! One clip at a time, played on its own thread.

use std::sync::mpsc::{self, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::decode::Clip;
use crate::output::{Opener, Sink, system_opener};

/// What the interface shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The clip loaded, by the number the caller gave it.
    pub clip: Option<u64>,
    pub playing: bool,
    pub position: Duration,
    /// Unknown for an MP3 without a length header.
    pub duration: Option<Duration>,
    /// Why the last start failed, in words.
    pub error: Option<String>,
    /// The clip played to its end.
    pub ended: bool,
}

enum Command {
    Play {
        clip: u64,
        bytes: Arc<Vec<u8>>,
        extension: String,
        from: Duration,
    },
    Pause,
    Resume,
    Seek(Duration),
    Stop,
    Quit,
}

/// The handle the interface holds. The thread starts with the first clip, and dropping the
/// player stops the sound and closes the device before it returns.
pub struct Player {
    shared: Arc<Mutex<Snapshot>>,
    tx: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
    opener: Opener,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        Self::with_opener(system_opener())
    }

    pub fn with_opener(opener: Opener) -> Self {
        Self {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            tx: None,
            thread: None,
            opener,
        }
    }

    /// Starts `clip` (`bytes` is the file, `extension` helps recognise it) at `from`.
    pub fn play(&mut self, clip: u64, bytes: Arc<Vec<u8>>, extension: &str, from: Duration) {
        self.send(Command::Play {
            clip,
            bytes,
            extension: extension.to_owned(),
            from,
        });
    }

    pub fn pause(&mut self) {
        self.send_if_running(Command::Pause);
    }

    pub fn resume(&mut self) {
        self.send_if_running(Command::Resume);
    }

    pub fn seek(&mut self, to: Duration) {
        self.send_if_running(Command::Seek(to));
    }

    /// Stops the sound and forgets the clip.
    pub fn stop(&mut self) {
        self.send_if_running(Command::Stop);
    }

    /// Stops the sound and closes the device now, waiting for the thread to end. The player
    /// can be used again afterwards (it starts a new thread with the next clip).
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Command::Quit);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = Snapshot::default();
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn send_if_running(&mut self, command: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(command);
        }
    }

    fn send(&mut self, command: Command) {
        if self.tx.is_none() {
            let opener = self.opener.clone();
            let (tx, rx) = mpsc::channel();
            let shared = self.shared.clone();
            self.thread = std::thread::Builder::new()
                .name("libertum-audio".into())
                .spawn(move || run(rx, shared, opener))
                .ok();
            self.tx = Some(tx);
        }
        self.send_if_running(command);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Command::Quit);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread's own state.
struct Worker {
    clip: Option<(u64, Clip)>,
    sink: Option<Box<dyn Sink>>,
    playing: bool,
    /// Where the clip was when counting frames began, and how many frames were written since.
    base: Duration,
    written: u64,
    shared: Arc<Mutex<Snapshot>>,
    opener: Opener,
}

impl Worker {
    fn update(&self, change: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut snapshot) = self.shared.lock() {
            change(&mut snapshot);
        }
    }

    /// Where the sound the writer hears is now: what was written, less what is still queued.
    fn position(&mut self) -> Duration {
        let Some((_, clip)) = &self.clip else {
            return Duration::ZERO;
        };
        let rate = u64::from(clip.sample_rate.max(1));
        let queued = self.sink.as_mut().map_or(0, |sink| sink.latency_frames());
        let heard = self.written.saturating_sub(queued);
        let at = self.base + Duration::from_secs_f64(heard as f64 / rate as f64);
        match clip.duration {
            Some(total) => at.min(total),
            None => at,
        }
    }

    fn close_sink(&mut self) {
        // Dropping the device flushes what is queued and closes it.
        self.sink = None;
    }

    fn open_sink(&mut self) -> bool {
        let Some((_, clip)) = &self.clip else {
            return false;
        };
        match (self.opener)(clip.sample_rate, clip.channels) {
            Ok(sink) => {
                self.sink = Some(sink);
                true
            }
            Err(error) => {
                self.update(|s| {
                    s.error = Some(error);
                    s.playing = false;
                });
                false
            }
        }
    }

    fn start_at(&mut self, at: Duration) {
        if let Some((_, clip)) = &mut self.clip {
            self.base = if at.is_zero() {
                Duration::ZERO
            } else {
                clip.seek(at)
            };
        }
        self.written = 0;
        self.playing = self.open_sink();
        let (playing, base) = (self.playing, self.base);
        self.update(|s| {
            s.playing = playing;
            s.position = base;
        });
    }

    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Play {
                clip,
                bytes,
                extension,
                from,
            } => {
                self.close_sink();
                self.playing = false;
                match Clip::open(bytes, &extension) {
                    Ok(opened) => {
                        let duration = opened.duration;
                        self.clip = Some((clip, opened));
                        self.update(|s| {
                            *s = Snapshot {
                                clip: Some(clip),
                                duration,
                                ..Snapshot::default()
                            };
                        });
                        self.start_at(from);
                    }
                    Err(error) => {
                        self.clip = None;
                        self.update(|s| {
                            *s = Snapshot {
                                clip: Some(clip),
                                error: Some(error.to_string()),
                                ..Snapshot::default()
                            };
                        });
                    }
                }
            }
            Command::Pause => {
                if self.playing {
                    let at = self.position();
                    self.close_sink();
                    self.playing = false;
                    // Resuming starts again from exactly here.
                    if let Some((_, clip)) = &mut self.clip {
                        self.base = clip.seek(at);
                    }
                    self.written = 0;
                    let base = self.base;
                    self.update(|s| {
                        s.playing = false;
                        s.position = base;
                    });
                }
            }
            Command::Resume => {
                if !self.playing && self.clip.is_some() {
                    let ended = self.shared.lock().map(|s| s.ended).unwrap_or(false);
                    let at = if ended { Duration::ZERO } else { self.base };
                    self.update(|s| s.ended = false);
                    self.start_at(at);
                }
            }
            Command::Seek(to) => {
                if let Some((_, clip)) = &mut self.clip {
                    self.base = clip.seek(to);
                    self.written = 0;
                    if let Some(sink) = &mut self.sink {
                        sink.flush();
                    }
                    let base = self.base;
                    self.update(|s| {
                        s.position = base;
                        s.ended = false;
                    });
                }
            }
            Command::Stop => {
                self.close_sink();
                self.playing = false;
                self.clip = None;
                self.update(|s| *s = Snapshot::default());
            }
            Command::Quit => {
                self.close_sink();
                return false;
            }
        }
        true
    }

    /// Decodes and plays one stretch. Returns a command that arrived while the tail of the clip
    /// was playing out.
    fn step(&mut self, rx: &mpsc::Receiver<Command>) -> Option<Command> {
        let chunk = self.clip.as_mut().and_then(|(_, clip)| clip.next_chunk());
        let channels = self
            .clip
            .as_ref()
            .map_or(1, |(_, c)| usize::from(c.channels.max(1)));
        match chunk {
            Some(samples) => {
                let failed = match self.sink.as_mut() {
                    Some(sink) => sink.write(&samples).err(),
                    None => Some("the sound device closed".to_owned()),
                };
                if let Some(error) = failed {
                    self.close_sink();
                    self.playing = false;
                    self.update(|s| {
                        s.error = Some(error);
                        s.playing = false;
                    });
                    return None;
                }
                self.written += (samples.len() / channels) as u64;
                let at = self.position();
                self.update(|s| s.position = at);
                None
            }
            None => {
                // The end: let what is queued play out, unless the writer acts first.
                let mut waited = Duration::ZERO;
                let mut pending = None;
                let step = Duration::from_millis(40);
                while waited < Duration::from_millis(1500) {
                    let queued = self.sink.as_mut().map_or(0, |sink| sink.latency_frames());
                    if queued == 0 {
                        break;
                    }
                    match rx.recv_timeout(step) {
                        Ok(command) => {
                            pending = Some(command);
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => waited += step,
                        // The handle is gone: the end of the clip does not matter now.
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                if pending.is_none() {
                    self.close_sink();
                    self.playing = false;
                    let total = self.clip.as_ref().and_then(|(_, c)| c.duration);
                    let at = total.unwrap_or(self.base);
                    self.update(|s| {
                        s.playing = false;
                        s.ended = true;
                        s.position = total.unwrap_or(at.max(s.position));
                    });
                }
                pending
            }
        }
    }
}

fn run(rx: mpsc::Receiver<Command>, shared: Arc<Mutex<Snapshot>>, opener: Opener) {
    let mut worker = Worker {
        clip: None,
        sink: None,
        playing: false,
        base: Duration::ZERO,
        written: 0,
        shared,
        opener,
    };
    loop {
        let command = if worker.playing {
            match rx.try_recv() {
                Ok(command) => Some(command),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        if let Some(command) = command {
            if !worker.handle(command) {
                break;
            }
            continue;
        }
        if let Some(pending) = worker.step(&rx)
            && !worker.handle(pending)
        {
            break;
        }
    }
    worker.close_sink();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Instant;

    /// A mono 16-bit WAV, `seconds` long, a rising ramp.
    fn wav(rate: u32, seconds: u32) -> Vec<u8> {
        let frames = rate * seconds;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(frames * 2).to_le_bytes());
        for i in 0..frames {
            out.extend_from_slice(&((i % 2000) as i16 - 1000).to_le_bytes());
        }
        out
    }

    #[derive(Default)]
    struct Counters {
        open: AtomicUsize,
        opened_ever: AtomicUsize,
        frames: AtomicU64,
        flushes: AtomicUsize,
    }

    struct Fake {
        counters: Arc<Counters>,
        rate: u32,
        pace: bool,
    }

    impl Sink for Fake {
        fn write(&mut self, samples: &[f32]) -> Result<(), String> {
            self.counters
                .frames
                .fetch_add(samples.len() as u64, Ordering::SeqCst);
            if self.pace {
                // Twenty times faster than real time.
                std::thread::sleep(Duration::from_secs_f64(
                    samples.len() as f64 / f64::from(self.rate) / 20.0,
                ));
            }
            Ok(())
        }

        fn latency_frames(&mut self) -> u64 {
            0
        }

        fn flush(&mut self) {
            self.counters.flushes.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.counters.open.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn player(pace: bool) -> (Player, Arc<Counters>) {
        let counters = Arc::new(Counters::default());
        let seen = counters.clone();
        let opener: Opener = Arc::new(move |rate, _| {
            seen.open.fetch_add(1, Ordering::SeqCst);
            seen.opened_ever.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Fake {
                counters: seen.clone(),
                rate,
                pace,
            }))
        });
        (Player::with_opener(opener), counters)
    }

    fn wait(player: &Player, what: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let start = Instant::now();
        loop {
            let snapshot = player.snapshot();
            if what(&snapshot) {
                return snapshot;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {snapshot:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_clip_reports_its_length_and_plays_to_the_end_then_closes_the_device() {
        let (mut player, counters) = player(false);
        player.play(7, Arc::new(wav(8000, 2)), "wav", Duration::ZERO);
        let end = wait(&player, |s| s.ended);
        assert_eq!(end.clip, Some(7));
        assert_eq!(end.duration, Some(Duration::from_secs(2)));
        assert!(!end.playing);
        assert_eq!(counters.frames.load(Ordering::SeqCst), 16_000);
        assert_eq!(
            counters.open.load(Ordering::SeqCst),
            0,
            "the device is released at the end"
        );
    }

    #[test]
    fn pause_releases_the_device_and_resume_carries_on_from_there() {
        let (mut player, counters) = player(true);
        player.play(1, Arc::new(wav(8000, 4)), "wav", Duration::ZERO);
        wait(&player, |s| {
            s.playing && s.position > Duration::from_millis(500)
        });
        player.pause();
        let paused = wait(&player, |s| !s.playing);
        assert_eq!(
            counters.open.load(Ordering::SeqCst),
            0,
            "no device while paused"
        );
        assert!(
            paused.position > Duration::from_millis(400)
                && paused.position < Duration::from_secs(4)
        );
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            player.snapshot().position,
            paused.position,
            "paused means still"
        );

        let before = counters.frames.load(Ordering::SeqCst);
        player.resume();
        let done = wait(&player, |s| s.ended);
        assert!(!done.playing);
        // Everything after the pause was played once; nothing before it again.
        let after = counters.frames.load(Ordering::SeqCst) - before;
        assert!(
            after < 32_000 - 3_000,
            "played {after} samples after the pause"
        );
        assert_eq!(counters.opened_ever.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn seeking_moves_the_position_and_stop_forgets_the_clip() {
        let (mut player, counters) = player(true);
        player.play(2, Arc::new(wav(8000, 10)), "wav", Duration::ZERO);
        wait(&player, |s| s.playing);
        player.seek(Duration::from_secs(8));
        let moved = wait(&player, |s| s.position >= Duration::from_secs(8));
        assert!(moved.position <= Duration::from_secs(10));
        player.stop();
        let stopped = wait(&player, |s| s.clip.is_none());
        assert!(!stopped.playing);
        wait(&player, |_| counters.open.load(Ordering::SeqCst) == 0);
    }

    #[test]
    fn something_that_is_not_audio_reports_why_and_opens_no_device() {
        let (mut player, counters) = player(false);
        player.play(
            3,
            Arc::new(b"<html>not audio</html>".to_vec()),
            "mp3",
            Duration::ZERO,
        );
        let failed = wait(&player, |s| s.error.is_some());
        assert!(!failed.playing);
        assert_eq!(counters.opened_ever.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dropping_the_player_stops_the_sound_and_closes_the_device() {
        let (mut player, counters) = player(true);
        player.play(4, Arc::new(wav(8000, 30)), "wav", Duration::ZERO);
        wait(&player, |s| s.playing);
        drop(player);
        assert_eq!(counters.open.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_device_that_cannot_open_is_an_error_not_a_crash() {
        let opener: Opener = Arc::new(|_, _| Err("no sound server".to_owned()));
        let mut player = Player::with_opener(opener);
        player.play(5, Arc::new(wav(8000, 1)), "wav", Duration::ZERO);
        let failed = wait(&player, |s| s.error.is_some());
        assert_eq!(failed.error.as_deref(), Some("no sound server"));
        assert!(!failed.playing);
    }
}
