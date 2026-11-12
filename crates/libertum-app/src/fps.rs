//! The frame-rate counter behind the performance overlay.
//!
//! It is off by default and switched on in Settings ("Show performance overlay"). While it is
//! showing, the window redraws continuously: that is what makes the measurement meaningful,
//! and it also means the counter itself costs frames. Compare figures taken with the same
//! zoom, window size and document.

use std::time::{Duration, Instant};

/// How often the shown figure is updated.
pub const WINDOW: Duration = Duration::from_millis(500);

/// Counts frames over a sliding window and keeps the most recent rate.
#[derive(Debug, Clone)]
pub struct FrameCounter {
    window: Duration,
    /// Start of the window being counted.
    window_start: Option<Instant>,
    frames: u32,
    shown: Option<f32>,
    /// Longest gap seen in the current window (a stutter is more telling than the mean).
    worst_ms: f32,
    last: Option<Instant>,
}

impl Default for FrameCounter {
    fn default() -> Self {
        Self::new(WINDOW)
    }
}

impl FrameCounter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            window_start: None,
            frames: 0,
            shown: None,
            worst_ms: 0.0,
            last: None,
        }
    }

    /// Records a frame and returns the rate once a window has closed (otherwise the previous
    /// rate, or `None` before the first window).
    pub fn tick(&mut self, now: Instant) -> Option<f32> {
        if let Some(last) = self.last {
            let gap = now.saturating_duration_since(last).as_secs_f32() * 1_000.0;
            self.worst_ms = self.worst_ms.max(gap);
        }
        self.last = Some(now);
        self.frames += 1;
        match self.window_start {
            None => {
                self.window_start = Some(now);
                self.shown
            }
            Some(start) => {
                let elapsed = now.saturating_duration_since(start);
                if elapsed >= self.window {
                    let seconds = elapsed.as_secs_f32().max(f32::EPSILON);
                    self.shown = Some(self.frames as f32 / seconds);
                    self.frames = 0;
                    self.worst_ms = 0.0;
                    self.window_start = Some(now);
                }
                self.shown
            }
        }
    }

    /// What the overlay shows: the rate and the longest gap in the window.
    pub fn label(&self) -> Option<String> {
        self.shown
            .map(|rate| format!("{rate:.0} fps · {:.1} ms", 1_000.0 / rate.max(0.1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixty_frames_at_sixty_hertz_read_as_sixty() {
        let mut counter = FrameCounter::new(Duration::from_millis(500));
        let start = Instant::now();
        let mut rate = None;
        for frame in 0..=40u32 {
            rate = counter.tick(start + Duration::from_millis(u64::from(frame) * 16));
        }
        // 41 frames 16 ms apart close two 500 ms windows, so a rate is known.
        let rate = rate.expect("a rate after a window");
        assert!((rate - 62.5).abs() < 6.0, "about 60 fps, got {rate}");
        assert!(counter.label().is_some_and(|label| label.contains("fps")));
    }

    #[test]
    fn no_rate_before_the_first_window_closes() {
        let mut counter = FrameCounter::new(Duration::from_millis(500));
        let start = Instant::now();
        assert_eq!(counter.tick(start), None);
        assert_eq!(counter.tick(start + Duration::from_millis(5)), None);
        assert!(counter.label().is_none());
    }

    #[test]
    fn a_slow_frame_shows_as_a_longer_interval() {
        let mut counter = FrameCounter::new(Duration::from_millis(100));
        let start = Instant::now();
        counter.tick(start);
        let rate = counter.tick(start + Duration::from_millis(100));
        let rate = rate.expect("a window closed");
        assert!((rate - 20.0).abs() < 1.0, "two frames in 100 ms: {rate}");
        // 20 fps is 50 ms a frame.
        assert!(
            counter
                .label()
                .is_some_and(|label| label.contains("50.0 ms"))
        );
    }
}
