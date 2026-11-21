//! The built-in benchmark: how large a document this machine can scroll smoothly.
//!
//! The pure parts live here (the sample document, the step schedule, frame statistics, the
//! pass rules and the report) so they can be tested without a window. `app/bench_run.rs` drives
//! the steps inside the running app, because a frame rate only means something when it is
//! measured through the real canvas and the real compositor.
//!
//! A run builds a document by repeating a stored essay until it reaches a page count, measures
//! that size, and moves to the next size until the lowest frame-rate target fails.
//!
//! What counts as "holding" a target `T` fps (the default assumption):
//! the mean rate is at least 95% of `T` and the 95th-percentile frame is no slower than 80% of
//! `T` (so a 60 fps target tolerates the odd 20 ms frame but not a stutter every second).

use std::path::{Path, PathBuf};

use libertum_model::{
    BlockId, CharFormat, DateTimeFormat, Document, ParaFormat, ParaProps, Paragraph, Run,
};
use serde::{Deserialize, Serialize};

use crate::strings as s;

/// The essay that is copied over and over to make the test document.
const ESSAY: &str = include_str!("../assets/benchmark-essay.txt");

/// Page counts to try, in order. A run stops at the first size that misses the lowest target.
pub const SCHEDULE: &[usize] = &[
    20, 50, 100, 150, 200, 300, 400, 600, 800, 1_200, 1_600, 2_400, 3_200, 5_000, 8_000, 12_000,
];

/// Frame-rate targets reported, when the display can show them.
pub const TARGETS: [u32; 3] = [60, 90, 120];

/// Display refresh rates the measured ceiling is snapped to.
const REFRESH_RATES: [u32; 11] = [30, 48, 50, 60, 75, 90, 100, 120, 144, 165, 240];

/// Characters per page used for the first document, before the real figure is known. It is
/// replaced by the measured one after the first step.
pub const START_CHARS_PER_PAGE: f64 = 3_350.0;

/// Page counts the save-time estimate is quoted for.
pub const ESTIMATE_PAGES: [usize; 4] = [100, 300, 500, 1_000];

/// Builds a document of about `pages` pages from the stored essay, given how many characters
/// fit on a page. Returns the document and its character count.
pub fn build_document(pages: usize, chars_per_page: f64) -> (Document, usize) {
    let wanted = (pages as f64 * chars_per_page.max(500.0)) as usize;
    let blocks: Vec<&str> = ESSAY
        .split("\n\n")
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();
    let mut paragraphs = Vec::new();
    let mut chars = 0usize;
    while chars < wanted {
        for block in &blocks {
            let (text, heading) = match block.strip_prefix("# ") {
                Some(rest) => (rest, true),
                None => (*block, false),
            };
            chars += text.chars().count();
            paragraphs.push(paragraph(text, heading));
            if chars >= wanted {
                break;
            }
        }
    }
    (Document::from_paragraphs(paragraphs), chars)
}

fn paragraph(text: &str, heading: bool) -> Paragraph {
    let runs = if heading {
        vec![Run {
            range: 0..text.len(),
            format: CharFormat {
                size_pt: Some(16.0),
                weight: Some(700),
                ..CharFormat::default()
            },
        }]
    } else {
        Vec::new()
    };
    let mut p = Paragraph::with_runs(BlockId(0), text, runs);
    p.props = ParaProps {
        format: ParaFormat {
            space_before_pt: Some(if heading { 18.0 } else { 0.0 }),
            space_after_pt: Some(if heading { 8.0 } else { 6.0 }),
            ..ParaFormat::default()
        },
        ..ParaProps::default()
    };
    p
}

/// Frame-rate figures for one measured stretch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    pub frames: usize,
    /// Mean frames per second over the stretch.
    pub mean_fps: f64,
    /// The rate implied by the 95th-percentile (slow) frame interval.
    pub p95_fps: f64,
    /// The slowest single frame, in milliseconds.
    pub worst_ms: f64,
}

impl FrameStats {
    /// From the gaps between consecutive frames, in milliseconds. `None` with fewer than three
    /// frames. A stretch that managed only a few frames is a real, very low rate and is
    /// reported as one rather than hidden.
    pub fn from_intervals(intervals: &[f64]) -> Option<Self> {
        if intervals.len() < 3 {
            return None;
        }
        let total: f64 = intervals.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut sorted = intervals.to_vec();
        sorted.sort_by(f64::total_cmp);
        let p95 = sorted[((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1)];
        Some(Self {
            frames: intervals.len(),
            mean_fps: 1_000.0 * intervals.len() as f64 / total,
            p95_fps: 1_000.0 / p95.max(0.001),
            worst_ms: sorted[sorted.len() - 1],
        })
    }

    /// Whether this stretch held `target` frames per second.
    pub fn holds(&self, target: u32) -> bool {
        let target = f64::from(target);
        self.mean_fps >= target * 0.95 && self.p95_fps >= target * 0.8
    }
}

/// What one step (one document size) measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    /// Pages the document really paginated to (the schedule's figure is only a target).
    pub pages: usize,
    pub words: usize,
    /// `None` when pagination did not finish in time.
    pub scroll: Option<FrameStats>,
    /// Time for one typed character to be applied and the layout brought up to date.
    pub typing_mean_ms: f64,
    pub typing_p95_ms: f64,
    /// Time to serialise the document in the default format and write it to disk.
    /// `None` when the save failed (the report then shows no figure rather than a false one).
    pub save_ms: Option<f64>,
    pub save_bytes: usize,
}

/// Mean and 95th percentile of `samples` (milliseconds); zeros when empty.
pub fn mean_and_p95(samples: &[f64]) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p95 = sorted[((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1)];
    (mean, p95)
}

/// The display's refresh rate, estimated from the best rate the lightest step reached: the
/// highest common refresh rate whose 94% the mean rate meets. `None` below 30 fps.
pub fn display_hz(first: &StepResult) -> Option<u32> {
    let mean = first.scroll?.mean_fps;
    REFRESH_RATES
        .iter()
        .copied()
        .filter(|rate| mean >= f64::from(*rate) * 0.94)
        .max()
}

/// Whether the run should stop after `steps`: the lowest target has failed, the display cannot
/// reach 60 anyway, or the schedule is used up.
pub fn should_stop(steps: &[StepResult]) -> bool {
    let Some(first) = steps.first() else {
        return false;
    };
    if steps.len() >= SCHEDULE.len() {
        return true;
    }
    match display_hz(first) {
        Some(hz) if hz >= TARGETS[0] => !steps
            .last()
            .and_then(|step| step.scroll)
            .is_some_and(|stats| stats.holds(TARGETS[0])),
        _ => true,
    }
}

/// How far one frame-rate target got.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub target: u32,
    /// The display can show this rate (judged from the lightest step).
    pub supported: bool,
    /// The largest document that held the target, if any did.
    pub best: Option<(usize, usize)>,
    /// The first size that missed it.
    pub failed_at: Option<usize>,
}

/// The finished benchmark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub steps: Vec<StepResult>,
    pub display_hz: Option<u32>,
    pub limits: Vec<Limit>,
    /// The default save format's name, for the report.
    pub format: String,
    /// When the run finished (Unix seconds); 0 when the clock was unreadable.
    pub taken_unix: i64,
}

impl Report {
    pub fn from_steps(steps: Vec<StepResult>, format: &str) -> Self {
        let hz = steps.first().and_then(display_hz);
        let limits = TARGETS
            .iter()
            .map(|&target| {
                let supported = hz.is_some_and(|hz| hz >= target);
                let mut best = None;
                let mut failed_at = None;
                if supported {
                    for step in &steps {
                        if step.scroll.is_some_and(|stats| stats.holds(target)) {
                            best = Some((step.pages, step.words));
                        } else {
                            failed_at = Some(step.pages);
                            break;
                        }
                    }
                }
                Limit {
                    target,
                    supported,
                    best,
                    failed_at,
                }
            })
            .collect();
        Self {
            steps,
            display_hz: hz,
            limits,
            format: format.to_owned(),
            taken_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64),
        }
    }

    /// Estimated save time for a document of `pages` pages, scaled from the largest step
    /// measured. Saving is close to linear in size, so this is an estimate and is labelled so.
    pub fn estimated_save_ms(&self, pages: usize) -> Option<f64> {
        let (step, ms) = self
            .steps
            .iter()
            .rev()
            .find_map(|step| step.save_ms.filter(|_| step.pages > 0).map(|ms| (step, ms)))?;
        Some(ms / step.pages as f64 * pages as f64)
    }

    /// The best frame rate seen at any step (for a machine that cannot reach 60).
    pub fn best_fps(&self) -> f64 {
        self.steps
            .iter()
            .filter_map(|step| step.scroll.map(|stats| stats.mean_fps))
            .fold(0.0, f64::max)
    }

    /// When the run finished, as a local date and time.
    pub fn taken_text(&self) -> String {
        libertum_editor::format_date_time(self.taken_unix, DateTimeFormat::DateTimeLong)
    }

    /// The report as plain text, for the window and for "Copy report".
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(s::BENCH_REPORT_TITLE);
        out.push('\n');
        if self.taken_unix > 0 {
            out.push_str(&self.taken_text());
            out.push('\n');
        }
        match self.display_hz {
            Some(hz) => out.push_str(&format!("{} {hz} Hz\n", s::BENCH_DISPLAY)),
            None => out.push_str(&format!("{}\n", s::BENCH_DISPLAY_SLOW)),
        }
        out.push('\n');
        for limit in &self.limits {
            out.push_str(&limit_line(limit, self.best_fps()));
            out.push('\n');
        }
        out.push('\n');
        out.push_str(s::BENCH_TABLE_HEAD);
        out.push('\n');
        for step in &self.steps {
            let fps = step.scroll.map_or_else(
                || s::BENCH_NO_RESULT.to_owned(),
                |stats| format!("{:.0} / {:.0}", stats.mean_fps, stats.p95_fps),
            );
            let save = step
                .save_ms
                .map_or_else(|| "n/a".to_owned(), |ms| format!("{ms:.0}"));
            out.push_str(&format!(
                "{:>6} {:>9} {:>11} {:>10.1} {:>10}\n",
                step.pages, step.words, fps, step.typing_p95_ms, save
            ));
        }
        out.push('\n');
        out.push_str(&format!("{} ({})\n", s::BENCH_SAVE_ESTIMATE, self.format));
        for pages in ESTIMATE_PAGES {
            if let Some(ms) = self.estimated_save_ms(pages) {
                out.push_str(&format!("{pages:>6} {}: {}\n", s::BENCH_PAGES, seconds(ms)));
            }
        }
        out.push('\n');
        out.push_str(s::BENCH_NOTE);
        out.push('\n');
        out
    }
}

/// How many saved results are kept; older ones are deleted.
const KEEP_RESULTS: usize = 10;

/// Where results are kept: a folder in the platform data folder.
pub fn results_dir() -> Option<PathBuf> {
    crate::recovery::data_dir().map(|dir| dir.join("benchmarks"))
}

/// The file names a saved result has, from its time: `benchmark-<unix>.json` (read back by
/// Libertum) with a `.txt` copy that can be opened in any editor.
fn result_paths(dir: &Path, unix: i64) -> (PathBuf, PathBuf) {
    let stem = format!("benchmark-{unix:012}");
    (
        dir.join(format!("{stem}.json")),
        dir.join(format!("{stem}.txt")),
    )
}

/// Saves `report` in `dir` (created if needed) as JSON and plain text, then deletes all but the
/// newest few results. Returns the JSON file's path.
pub fn save(report: &Report, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let (json_path, text_path) = result_paths(dir, report.taken_unix);
    let json = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
    // Written beside the target and renamed, so a crash never leaves half a file that
    // `load_latest` would trip over.
    let partial = json_path.with_extension("json.part");
    std::fs::write(&partial, json)?;
    std::fs::rename(&partial, &json_path)?;
    std::fs::write(&text_path, report.to_text())?;
    prune(dir);
    Ok(json_path)
}

/// The saved result files in `dir`, oldest first.
fn saved_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("benchmark-"))
        })
        .collect();
    files.sort();
    files
}

fn prune(dir: &Path) {
    let files = saved_files(dir);
    for old in files.iter().take(files.len().saturating_sub(KEEP_RESULTS)) {
        if let Err(error) = std::fs::remove_file(old) {
            tracing::warn!(%error, path = %old.display(), "could not delete an old benchmark result");
        }
        if let Err(error) = std::fs::remove_file(old.with_extension("txt")) {
            tracing::warn!(%error, "could not delete an old benchmark report");
        }
    }
}

/// The newest saved result in `dir` that can be read, with its file. A file that is damaged
/// or from an incompatible version is skipped in favour of the one before it.
pub fn load_latest(dir: &Path) -> Option<(Report, PathBuf)> {
    saved_files(dir).into_iter().rev().find_map(|path| {
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Report>(&bytes) {
            Ok(report) => Some((report, path)),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "unreadable benchmark result");
                None
            }
        }
    })
}

/// One headline line for a target.
pub fn limit_line(limit: &Limit, best_fps: f64) -> String {
    let target = limit.target;
    if !limit.supported {
        return format!("{target} fps: {}", s::BENCH_UNSUPPORTED);
    }
    match (limit.best, limit.failed_at) {
        (None, _) => format!("{target} fps: {} ({best_fps:.0} fps)", s::BENCH_NOT_REACHED),
        (Some((pages, words)), Some(failed)) => format!(
            "{target} fps: {} {pages} {} (~{words} {}); {} {failed}",
            s::BENCH_UP_TO,
            s::BENCH_PAGES,
            s::BENCH_WORDS,
            s::BENCH_MISSED_AT,
        ),
        (Some((pages, words)), None) => format!(
            "{target} fps: {} {pages} {} (~{words} {}); {}",
            s::BENCH_AT_LEAST,
            s::BENCH_PAGES,
            s::BENCH_WORDS,
            s::BENCH_NO_LIMIT,
        ),
    }
}

/// Milliseconds as a short human duration.
pub fn seconds(ms: f64) -> String {
    if ms < 1_000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(mean: f64, p95: f64) -> FrameStats {
        FrameStats {
            frames: 100,
            mean_fps: mean,
            p95_fps: p95,
            worst_ms: 1_000.0 / p95,
        }
    }

    fn step(pages: usize, mean: f64, p95: f64) -> StepResult {
        StepResult {
            pages,
            words: pages * 500,
            scroll: Some(stats(mean, p95)),
            typing_mean_ms: 1.0,
            typing_p95_ms: 2.0,
            save_ms: Some(pages as f64 * 2.0),
            save_bytes: pages * 1_000,
        }
    }

    #[test]
    fn steady_sixty_frames_a_second_holds_sixty() {
        let intervals = vec![16.67; 120];
        let stats = FrameStats::from_intervals(&intervals).expect("enough frames");
        assert!((stats.mean_fps - 60.0).abs() < 0.1);
        assert!(stats.holds(60));
        assert!(!stats.holds(90));
    }

    #[test]
    fn a_stutter_fails_even_when_the_mean_is_high() {
        // One frame in ten takes 50 ms: the mean is ~48 fps and the p95 frame is slow.
        let mut intervals = Vec::new();
        for i in 0..100 {
            intervals.push(if i % 10 == 0 { 50.0 } else { 14.0 });
        }
        let stats = FrameStats::from_intervals(&intervals).expect("enough frames");
        assert!(!stats.holds(60));
    }

    #[test]
    fn too_few_frames_give_no_stats() {
        assert!(FrameStats::from_intervals(&[16.0; 2]).is_none());
        assert!(FrameStats::from_intervals(&[]).is_none());
    }

    #[test]
    fn the_display_rate_is_snapped_from_the_lightest_step() {
        assert_eq!(display_hz(&step(20, 59.8, 58.0)), Some(60));
        assert_eq!(display_hz(&step(20, 143.0, 130.0)), Some(144));
        assert_eq!(display_hz(&step(20, 119.0, 110.0)), Some(120));
        assert_eq!(display_hz(&step(20, 20.0, 15.0)), None);
    }

    #[test]
    fn a_run_stops_at_the_first_size_that_misses_sixty() {
        let mut steps = vec![step(20, 60.0, 58.0)];
        assert!(!should_stop(&steps));
        steps.push(step(50, 59.0, 55.0));
        assert!(!should_stop(&steps));
        steps.push(step(100, 41.0, 30.0));
        assert!(should_stop(&steps));
    }

    #[test]
    fn a_machine_that_cannot_reach_sixty_stops_after_one_step() {
        assert!(should_stop(&[step(20, 40.0, 35.0)]));
    }

    #[test]
    fn limits_report_the_largest_size_that_held_each_target() {
        let steps = vec![
            step(20, 120.0, 110.0),
            step(50, 119.0, 100.0),
            step(100, 80.0, 70.0),
            step(150, 60.0, 55.0),
            step(200, 40.0, 30.0),
        ];
        let report = Report::from_steps(steps, "ODT");
        assert_eq!(report.display_hz, Some(120));
        let by_target = |t: u32| {
            report
                .limits
                .iter()
                .find(|l| l.target == t)
                .expect("target")
        };
        assert_eq!(by_target(120).best, Some((50, 25_000)));
        assert_eq!(by_target(120).failed_at, Some(100));
        assert_eq!(by_target(90).best, Some((50, 25_000)));
        assert_eq!(by_target(60).best, Some((150, 75_000)));
        assert_eq!(by_target(60).failed_at, Some(200));
    }

    #[test]
    fn rates_the_display_cannot_show_are_marked_unsupported() {
        let report = Report::from_steps(vec![step(20, 60.0, 58.0), step(50, 30.0, 20.0)], "DOCX");
        let by_target = |t: u32| {
            report
                .limits
                .iter()
                .find(|l| l.target == t)
                .expect("target")
        };
        assert!(by_target(60).supported);
        assert!(!by_target(90).supported);
        assert!(!by_target(120).supported);
        assert!(report.to_text().contains(s::BENCH_UNSUPPORTED));
    }

    #[test]
    fn save_time_is_scaled_from_the_largest_step() {
        let report = Report::from_steps(vec![step(20, 60.0, 58.0), step(100, 60.0, 58.0)], "ODT");
        // 2 ms per page in the helper, so 1,000 pages is 2,000 ms.
        let ms = report.estimated_save_ms(1_000).expect("estimate");
        assert!((ms - 2_000.0).abs() < 1e-6);
        assert_eq!(seconds(ms), "2.0 s");
        assert_eq!(seconds(450.0), "450 ms");
    }

    #[test]
    fn the_test_document_grows_with_the_page_target() {
        let (small, small_chars) = build_document(5, 3_000.0);
        let (large, large_chars) = build_document(50, 3_000.0);
        assert!(small_chars >= 15_000);
        assert!(large_chars >= 150_000);
        assert!(large.paragraph_count() > small.paragraph_count());
        assert!(large.word_count() > small.word_count());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("libertum-bench-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_saved_result_reads_back_identical() {
        let dir = temp_dir("roundtrip");
        let mut report =
            Report::from_steps(vec![step(20, 60.0, 58.0), step(50, 30.0, 20.0)], "ODT");
        report.taken_unix = 1_700_000_000;
        let path = save(&report, &dir).expect("saves");
        assert!(path.exists());
        assert!(
            path.with_extension("txt").exists(),
            "a text copy is kept too"
        );
        let (loaded, from) = load_latest(&dir).expect("loads");
        assert_eq!(loaded, report);
        assert_eq!(from, path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_newest_result_is_the_one_loaded_and_old_ones_are_pruned() {
        let dir = temp_dir("prune");
        for n in 0..(KEEP_RESULTS as i64 + 3) {
            let mut report = Report::from_steps(vec![step(20, 60.0, 58.0)], "ODT");
            report.taken_unix = 1_700_000_000 + n;
            save(&report, &dir).expect("saves");
        }
        assert_eq!(saved_files(&dir).len(), KEEP_RESULTS);
        let (latest, _) = load_latest(&dir).expect("loads");
        assert_eq!(latest.taken_unix, 1_700_000_000 + KEEP_RESULTS as i64 + 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_result_is_skipped_for_the_one_before() {
        let dir = temp_dir("damaged");
        let mut good = Report::from_steps(vec![step(20, 60.0, 58.0)], "ODT");
        good.taken_unix = 1_700_000_000;
        save(&good, &dir).expect("saves");
        std::fs::write(dir.join("benchmark-000001800000000.json"), b"{ not json").expect("writes");
        let (loaded, _) = load_latest(&dir).expect("falls back");
        assert_eq!(loaded, good);
        assert!(load_latest(&dir.join("missing")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_report_text_names_every_target() {
        let report = Report::from_steps(vec![step(20, 60.0, 58.0), step(50, 30.0, 20.0)], "ODT");
        let text = report.to_text();
        for target in TARGETS {
            assert!(text.contains(&format!("{target} fps")), "{target} missing");
        }
    }
}
