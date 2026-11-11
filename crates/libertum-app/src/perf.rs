//! Frame-time measurement (Phase 0, Spike A in-app).
//!
//! Enabled with `LIBERTUM_PERF=1`. The canvas calls [`frame`] at the start of every `draw`, so
//! the interval between two calls is the real frame interval (it includes vsync and present
//! time, which a timer around `draw` alone would not). Only intervals between two consecutive
//! *scrolling* frames are recorded, so idle time never pollutes the numbers.
//!
//! Output (through `tracing`, at INFO): one line per second while scrolling, a WARN for any
//! frame longer than 33 ms (the PLAN §2 hard limit), and a summary on exit ([`summary`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static RECORDER: OnceLock<Mutex<Recorder>> = OnceLock::new();

/// A frame longer than this breaks the PLAN §2 "no frames > 33 ms" budget.
const SLOW_FRAME_MS: f64 = 33.0;

#[derive(Default)]
struct Recorder {
    last_frame: Option<Instant>,
    prev_scrolling: bool,
    intervals: Vec<f64>,
    draw_ms: Vec<f64>,
    update_ms: Vec<f64>,
    render_ms: Vec<f64>,
    frames_scrolling: u64,
    frames_with_blank: u64,
    frames_with_stale: u64,
    /// Keystroke latency breakdown.
    edit_sync_ms: Vec<f64>,
    edit_to_screen_ms: Vec<f64>,
    /// When the oldest edit not yet on screen was made.
    pending_edit: Option<Instant>,
    window_start: Option<Instant>,
    window_intervals: Vec<f64>,
    window_draw: Vec<f64>,
    window_update: Vec<f64>,
}

/// Reads `LIBERTUM_PERF`. Call once at start-up.
pub fn init() {
    let on = std::env::var("LIBERTUM_PERF").is_ok_and(|v| v != "0" && !v.is_empty());
    ENABLED.store(on, Ordering::Relaxed);
    if on {
        let _ = RECORDER.set(Mutex::new(Recorder::default()));
        tracing::info!("LIBERTUM_PERF=1: frame-time recording on");
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn with<R>(f: impl FnOnce(&mut Recorder) -> R) -> Option<R> {
    if !enabled() {
        return None;
    }
    let m = RECORDER.get()?;
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(&mut g))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Marks the start of a frame. `scrolling` is true when the scroll offset changed since the
/// previous frame.
pub fn frame(scrolling: bool) {
    let now = Instant::now();
    with(|r| {
        if let Some(last) = r.last_frame
            && scrolling
            && r.prev_scrolling
        {
            let dt = ms(now - last);
            r.intervals.push(dt);
            r.window_intervals.push(dt);
            if dt > SLOW_FRAME_MS {
                tracing::warn!(interval_ms = format!("{dt:.1}"), "slow frame");
            }
        }
        if scrolling {
            r.frames_scrolling += 1;
        }
        r.last_frame = Some(now);
        r.prev_scrolling = scrolling;

        let start = *r.window_start.get_or_insert(now);
        if now - start >= Duration::from_secs(1) {
            if !r.window_intervals.is_empty() {
                let n = r.window_intervals.len();
                let mean = r.window_intervals.iter().sum::<f64>() / n as f64;
                let max = r.window_intervals.iter().copied().fold(0.0, f64::max);
                let avg = |v: &Vec<f64>| {
                    if v.is_empty() {
                        0.0
                    } else {
                        v.iter().sum::<f64>() / v.len() as f64
                    }
                };
                tracing::info!(
                    frames = n,
                    mean_ms = format!("{mean:.1}"),
                    max_ms = format!("{max:.1}"),
                    draw_ms = format!("{:.2}", avg(&r.window_draw)),
                    update_ms = format!("{:.2}", avg(&r.window_update)),
                    "scroll frames, last second"
                );
            }
            r.window_intervals.clear();
            r.window_draw.clear();
            r.window_update.clear();
            r.window_start = Some(now);
        }
    });
}

/// Records how long building the frame's primitives took (`draw`), and whether any visible
/// page had no bitmap at all (`blank`) or showed older strips while current ones rendered
/// (`stale`).
pub fn draw_done(elapsed: Duration, blank: bool, stale: bool) {
    with(|r| {
        r.draw_ms.push(ms(elapsed));
        r.window_draw.push(ms(elapsed));
        r.frames_with_blank += u64::from(blank);
        r.frames_with_stale += u64::from(stale);
    });
}

/// An edit was applied to the model and layout (`elapsed` = model + layout + page map).
pub fn edit_synced(started: Instant) {
    with(|r| {
        r.edit_sync_ms.push(ms(started.elapsed()));
        r.pending_edit.get_or_insert(started);
    });
}

/// A frame showed every visible page at its current content: an edit is now on screen.
pub fn screen_current() {
    with(|r| {
        if let Some(t) = r.pending_edit.take() {
            r.edit_to_screen_ms.push(ms(t.elapsed()));
        }
    });
}

/// Records one `App::update` call.
pub fn update_done(elapsed: Duration) {
    with(|r| {
        r.update_ms.push(ms(elapsed));
        r.window_update.push(ms(elapsed));
    });
}

/// Records one page rasterisation on the render worker.
pub fn render_done(elapsed: Duration) {
    with(|r| r.render_ms.push(ms(elapsed)));
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn stats(label: &str, v: &[f64]) -> String {
    if v.is_empty() {
        return format!("{label}: no samples");
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    format!(
        "{label}: n={} mean={mean:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2} (ms)",
        s.len(),
        percentile(&s, 0.5),
        percentile(&s, 0.95),
        percentile(&s, 0.99),
        s.last().copied().unwrap_or(f64::NAN),
    )
}

/// Final report, or `None` when recording is off.
pub fn summary() -> Option<String> {
    with(|r| {
        let over_17 = r.intervals.iter().filter(|&&d| d > 16.7 * 1.5).count();
        let over_33 = r.intervals.iter().filter(|&&d| d > SLOW_FRAME_MS).count();
        let mut out = String::from("LIBERTUM_PERF summary\n");
        out += &format!(
            "  {}\n",
            stats("frame interval while scrolling", &r.intervals)
        );
        out += &format!(
            "  scrolling frames: {}; intervals > 25 ms (missed a 60 fps slot): {over_17}; > 33 ms: {over_33}\n",
            r.frames_scrolling
        );
        out += &format!(
            "  frames with a blank visible page: {}; with stale strips: {}\n",
            r.frames_with_blank, r.frames_with_stale
        );
        out += &format!("  {}\n", stats("draw()", &r.draw_ms));
        out += &format!("  {}\n", stats("update()", &r.update_ms));
        out += &format!("  {}\n", stats("page raster (worker)", &r.render_ms));
        out += &format!(
            "  {}\n",
            stats("edit sync (model + layout)", &r.edit_sync_ms)
        );
        out += &format!("  {}", stats("keystroke to screen", &r.edit_to_screen_ms));
        out
    })
}
