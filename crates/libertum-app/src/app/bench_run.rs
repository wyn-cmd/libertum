//! Running the benchmark (`crate::bench` holds the numbers) inside the live window.
//!
//! The user's own document is set aside, untouched, and a generated one takes its place on the
//! real canvas, so the frame rate is the one the compositor really delivers. Every message that
//! could edit, save or close that stand-in document is dropped while a run is going
//! (`allows`), so nothing a stray key press does can reach the user's file.
//!
//! One step is a state machine driven by the window's frames: build the document, wait for
//! pagination, scroll at a steady speed, type into the middle, then time a save.

use iced::widget::{column, container, mouse_area, row, space, stack, text};

use super::*;
use crate::bench::{self, FrameStats, Report, StepResult};
use crate::widgets::controls;

/// Settling time once pagination has finished, before scrolling.
const SETTLE: Duration = Duration::from_millis(600);
/// Give up on a document that has not paginated by then; the step counts as a miss.
const LAYOUT_TIMEOUT: Duration = Duration::from_secs(90);
/// How long each step scrolls for.
const SCROLL_TIME: Duration = Duration::from_secs(4);
/// Logical pixels per second while scrolling: a very fast skim, about eight pages a second at
/// 100% zoom, to stress the page cache as well as the frame rate.
const SCROLL_SPEED: f32 = 6_000.0;
/// Frames ignored at the start of a scroll, while the first pages are rasterised.
const WARMUP_FRAMES: usize = 15;
/// Characters typed per step, one per frame.
const TYPING_FRAMES: usize = 60;
/// Typing stops after this long even if fewer characters were typed: on a document the app
/// can barely move through, every keystroke can take seconds.
const TYPING_TIME: Duration = Duration::from_secs(3);
/// A step scrolling slower than this is plainly past the limit and is not measured again.
const HOPELESS_FPS: f64 = 15.0;
/// No frame for this long means the window was hidden or the screen asleep (the compositor
/// stops sending frames to a window nobody can see), not that the app was slow. The stage's
/// clock is restarted instead of scoring the pause.
const PAUSE_GAP: Duration = Duration::from_millis(1_500);
/// Restarts allowed per step, so a size that really takes seconds per frame is still measured.
const MAX_PAUSES: usize = 3;
/// Fewest keystrokes timed before the time cap may end typing.
const MIN_TYPING_SAMPLES: usize = 5;

/// Where a step is.
enum Phase {
    /// Waiting for pagination to finish and the window to report a size.
    Layout {
        started: Instant,
        ready_since: Option<Instant>,
    },
    Scroll {
        started: Instant,
        last: Instant,
        direction: f32,
        frames: usize,
        intervals: Vec<f64>,
    },
    Typing {
        started: Instant,
        samples: Vec<f64>,
    },
}

/// A benchmark in progress.
pub(super) struct Run {
    /// The user's document, kept whole while the generated one is shown.
    stash_doc: Option<DocView>,
    stash_scroll: (f32, f32),
    stash_zoom: Zoom,
    step: usize,
    chars_per_page: f64,
    chars: usize,
    steps: Vec<StepResult>,
    phase: Phase,
    scroll_stats: Option<FrameStats>,
    /// A step that missed 60 fps is measured once more, in case a one-off stall (another
    /// program, a background task) caused it. This holds the first attempt meanwhile.
    first_attempt: Option<StepResult>,
    /// When the last frame arrived, and how many pauses this step has restarted for.
    last_frame: Option<Instant>,
    pauses: usize,
    /// The most recent frame rate, for the progress panel.
    live_fps: Option<f32>,
}

impl Run {
    fn label(&self) -> &'static str {
        match self.phase {
            Phase::Layout { .. } => s::BENCH_PHASE_LAYOUT,
            Phase::Scroll { .. } => s::BENCH_PHASE_SCROLL,
            Phase::Typing { .. } => s::BENCH_PHASE_TYPING,
        }
    }
}

/// Whether a message may be handled while a benchmark runs. Only what the run itself needs
/// and what keeps the window correct; everything else is dropped.
pub(super) fn allows(message: &Message) -> bool {
    matches!(
        message,
        Message::Frame(_)
            | Message::FpsFrame(_)
            | Message::Doc(_)
            | Message::ScaleFactor(_)
            | Message::SystemTheme(_)
            | Message::WindowResized(_)
            // The canvas reports its size this way; a run cannot start without it. Every other
            // canvas event (clicks, scrolling, zoom) is the user's and is dropped.
            | Message::Canvas(CanvasEvent::Viewport(_))
            | Message::CancelBenchmark
            | Message::CloseRequested
    )
}

/// One right-aligned table cell of a fixed width.
fn cell<'a>(label: String, width: f32, colour: iced::Color) -> Element<'a, Message> {
    text(label)
        .size(12)
        .width(Length::Fixed(width))
        .align_x(iced::alignment::Horizontal::Right)
        .color(colour)
        .into()
}

/// `LIBERTUM_BENCHMARK=1` or `=show`: run at start-up.
pub(super) fn start_requested() -> bool {
    std::env::var("LIBERTUM_BENCHMARK").is_ok_and(|v| v != "0" && !v.is_empty())
}

/// `LIBERTUM_BENCHMARK=1` also logs the report and exits when it finishes; `=show` leaves the
/// results window open instead (for screenshots).
pub(super) fn auto_exit() -> bool {
    std::env::var("LIBERTUM_BENCHMARK").is_ok_and(|v| v == "1")
}

/// Time to apply and lay out one keystroke, in milliseconds.
fn typed_character(doc: &mut DocView) -> f64 {
    let started = Instant::now();
    if let Err(error) = doc.edit(|editor| editor.insert_text("e")) {
        tracing::warn!(%error, "benchmark keystroke failed");
    }
    started.elapsed().as_secs_f64() * 1_000.0
}

/// Serialises `document` in `format` and writes it to a temporary file, returning the time in
/// milliseconds and the size. The file is removed afterwards.
fn timed_save(document: Document, format: DefaultFormat) -> Option<(f64, usize)> {
    let extension = match format {
        DefaultFormat::Odt => s::EXT_ODT,
        DefaultFormat::Docx => s::EXT_DOCX,
        DefaultFormat::Rtf => s::EXT_RTF,
        DefaultFormat::Markdown => s::EXT_MARKDOWN,
        DefaultFormat::Native => s::EXT_NATIVE,
    };
    let path = std::env::temp_dir().join(format!(
        "libertum-benchmark-{}.{extension}",
        std::process::id()
    ));
    let started = Instant::now();
    let bytes = match format {
        DefaultFormat::Odt => Odt::new(document).to_bytes().map_err(|e| e.to_string()),
        DefaultFormat::Docx => Docx::new(document).to_bytes().map_err(|e| e.to_string()),
        DefaultFormat::Rtf => Rtf::new(document).to_bytes().map_err(|e| e.to_string()),
        DefaultFormat::Markdown => Markdown::new(document)
            .to_bytes()
            .map_err(|e| e.to_string()),
        DefaultFormat::Native => Native::new(document).to_bytes().map_err(|e| e.to_string()),
    };
    let written = bytes.and_then(|bytes| {
        std::fs::write(&path, &bytes)
            .map(|()| bytes.len())
            .map_err(|e| e.to_string())
    });
    let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
    if let Err(error) = std::fs::remove_file(&path) {
        tracing::warn!(%error, "could not remove the benchmark's temporary file");
    }
    match written {
        Ok(size) => Some((elapsed, size)),
        Err(error) => {
            tracing::warn!(%error, "benchmark save failed");
            None
        }
    }
}

impl App {
    /// Starts a run: the user's document is set aside and the first generated one is shown.
    pub(super) fn bench_start(&mut self) -> Task<Message> {
        if self.bench.is_some() {
            return Task::none();
        }
        self.settings_open = false;
        self.bench_report = None;
        self.bench_report_open = false;
        self.bench_copied = false;
        self.state.open_menu = None;
        let mut run = Run {
            stash_doc: self.doc.take(),
            stash_scroll: self.scroll,
            stash_zoom: self.state.zoom,
            step: 0,
            chars_per_page: bench::START_CHARS_PER_PAGE,
            chars: 0,
            steps: Vec::new(),
            phase: Phase::Layout {
                started: Instant::now(),
                ready_since: None,
            },
            scroll_stats: None,
            first_attempt: None,
            last_frame: None,
            pauses: 0,
            live_fps: None,
        };
        // The same zoom every time, so runs on one machine can be compared.
        self.state.zoom = Zoom::Percent(100);
        let task = self.bench_install_step(&mut run);
        self.bench = Some(run);
        task
    }

    /// Builds the document for the current step and shows it.
    fn bench_install_step(&mut self, run: &mut Run) -> Task<Message> {
        let target = bench::SCHEDULE[run.step];
        let (document, chars) = bench::build_document(target, run.chars_per_page);
        let (view, events) = DocView::open(document);
        self.doc = Some(view);
        self.scroll = (0.0, 0.0);
        self.scroll_pending = (0.0, 0.0);
        self.fling = (0.0, 0.0);
        run.chars = chars;
        run.scroll_stats = None;
        run.pauses = 0;
        run.phase = Phase::Layout {
            started: Instant::now(),
            ready_since: None,
        };
        self.refresh_view();
        Task::run(events, Message::Doc)
    }

    /// One window frame of a run.
    pub(super) fn bench_frame(&mut self, now: Instant) -> Task<Message> {
        let Some(mut run) = self.bench.take() else {
            return Task::none();
        };
        let mut finished = false;
        let mut task = Task::none();
        let paused = run
            .last_frame
            .is_some_and(|last| now.duration_since(last) > PAUSE_GAP)
            && run.pauses < MAX_PAUSES;
        run.last_frame = Some(now);
        if paused {
            run.pauses += 1;
            tracing::info!(
                step = run.step,
                "no frames for a while; restarting the stage"
            );
        }
        let phase = std::mem::replace(
            &mut run.phase,
            Phase::Typing {
                started: now,
                samples: Vec::new(),
            },
        );
        match phase {
            Phase::Layout {
                mut started,
                mut ready_since,
            } => {
                if paused {
                    started = now;
                    ready_since = None;
                }
                let ready = self.viewport.is_some()
                    && self.scale_factor_known
                    && self
                        .doc
                        .as_ref()
                        .is_some_and(|d| d.source.pages.is_complete());
                let ready_since = match (ready, ready_since) {
                    (false, _) => None,
                    (true, None) => Some(now),
                    (true, Some(t)) => Some(t),
                };
                if now.duration_since(started) > LAYOUT_TIMEOUT {
                    // A document this size would not even paginate: that is a miss.
                    tracing::warn!(step = run.step, "benchmark pagination timed out");
                    let pages = self.doc.as_ref().map_or(0, DocView::page_total);
                    let retry = self.bench_record(&mut run, None, pages, Vec::new(), false);
                    (finished, task) = self.bench_next(&mut run, retry);
                } else if ready_since.is_some_and(|t| now.duration_since(t) >= SETTLE) {
                    let max_y = self.metrics().max_scroll(self.viewport_physical()).1;
                    let distance = SCROLL_SPEED * self.scale_factor * SCROLL_TIME.as_secs_f32();
                    // Start mid-document so the pages scrolled through are not the cheap first
                    // ones (a short document starts at the top and bounces).
                    self.scroll.1 = ((max_y - distance) / 2.0).max(0.0);
                    run.phase = Phase::Scroll {
                        started: now,
                        last: now,
                        direction: 1.0,
                        frames: 0,
                        intervals: Vec::new(),
                    };
                } else {
                    run.phase = Phase::Layout {
                        started,
                        ready_since,
                    };
                }
            }
            Phase::Scroll {
                mut started,
                mut last,
                mut direction,
                mut frames,
                mut intervals,
            } => {
                if paused {
                    // Measure the stretch again from the top, with the pause left out.
                    started = now;
                    last = now;
                    frames = 0;
                    intervals.clear();
                }
                let dt = now.duration_since(last);
                frames += 1;
                intervals.push(dt.as_secs_f64() * 1_000.0);
                if frames > WARMUP_FRAMES {
                    let recent = &intervals[intervals.len().saturating_sub(30)..];
                    let mean = recent.iter().sum::<f64>() / recent.len() as f64;
                    run.live_fps = Some((1_000.0 / mean.max(0.001)) as f32);
                }
                let max_y = self.metrics().max_scroll(self.viewport_physical()).1;
                self.scroll.1 += direction * SCROLL_SPEED * self.scale_factor * dt.as_secs_f32();
                if self.scroll.1 >= max_y {
                    self.scroll.1 = max_y;
                    direction = -1.0;
                } else if self.scroll.1 <= 0.0 {
                    self.scroll.1 = 0.0;
                    direction = 1.0;
                }
                self.refresh_view();
                if now.duration_since(started) >= SCROLL_TIME {
                    // The first frames, while pages are rasterised, are left out, unless the stretch
                    // was so slow that little else is left: then every frame counts.
                    let skip = if intervals.len() > WARMUP_FRAMES + 3 {
                        WARMUP_FRAMES
                    } else {
                        0
                    };
                    run.scroll_stats = FrameStats::from_intervals(&intervals[skip..]);
                    self.bench_place_caret();
                    run.phase = Phase::Typing {
                        started: now,
                        samples: Vec::new(),
                    };
                } else {
                    run.phase = Phase::Scroll {
                        started,
                        last: now,
                        direction,
                        frames,
                        intervals,
                    };
                }
            }
            Phase::Typing {
                mut started,
                mut samples,
            } => {
                if paused {
                    started = now;
                }
                if let Some(doc) = self.doc.as_mut() {
                    samples.push(typed_character(doc));
                }
                self.refresh_view();
                let out_of_time = samples.len() >= MIN_TYPING_SAMPLES
                    && now.duration_since(started) >= TYPING_TIME;
                if samples.len() >= TYPING_FRAMES || out_of_time || self.doc.is_none() {
                    let stats = run.scroll_stats.take();
                    let pages = self.doc.as_ref().map_or(0, DocView::page_total);
                    let retry = self.bench_record(&mut run, stats, pages, samples, true);
                    (finished, task) = self.bench_next(&mut run, retry);
                } else {
                    run.phase = Phase::Typing { started, samples };
                }
            }
        }
        if finished {
            self.bench_finish(run, true)
        } else {
            self.bench = Some(run);
            task
        }
    }

    /// Puts the caret at the start of the middle paragraph and brings its page into view, so
    /// each keystroke re-lays out a page that is on screen.
    fn bench_place_caret(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let count = doc.document().paragraph_count();
        let Some(block) = doc.document().paragraph_ids().nth(count / 2) else {
            return;
        };
        doc.move_caret(|editor| {
            editor.set_caret(libertum_editor::DocPosition { block, offset: 0 }, false);
        });
        if let Some(page) = doc.caret_page_index() {
            self.scroll.1 = self.metrics().page_top(page);
        }
        self.refresh_view();
    }

    /// Times a save of the current document and stores the step's figures. Returns true when
    /// the step missed 60 fps for the first time and should be measured again (`may_retry`);
    /// the better of the two attempts is the one kept.
    fn bench_record(
        &mut self,
        run: &mut Run,
        scroll: Option<FrameStats>,
        pages: usize,
        typing: Vec<f64>,
        may_retry: bool,
    ) -> bool {
        let (words, save) = match self.doc.as_ref() {
            Some(doc) => (
                doc.word_count(),
                timed_save(doc.document().clone(), self.settings.default_format),
            ),
            None => (0, None),
        };
        let (typing_mean_ms, typing_p95_ms) = bench::mean_and_p95(&typing);
        if pages > 0 && run.chars > 0 {
            // Real characters per page, so the next document lands near its page target.
            run.chars_per_page = run.chars as f64 / pages as f64;
        }
        let result = StepResult {
            pages,
            words,
            scroll,
            typing_mean_ms,
            typing_p95_ms,
            save_ms: save.map(|(ms, _)| ms),
            save_bytes: save.map_or(0, |(_, bytes)| bytes),
        };
        let earlier = run.first_attempt.take();
        let held = result
            .scroll
            .is_some_and(|stats| stats.holds(bench::TARGETS[0]));
        // Only a size after one that held is judged hopeless: a bad first step is more likely a
        // stall at start-up (the window has only just appeared), so it is always measured again.
        let hopeless = !run.steps.is_empty()
            && result
                .scroll
                .is_none_or(|stats| stats.mean_fps < HOPELESS_FPS);
        if may_retry && earlier.is_none() && !held && !hopeless {
            run.first_attempt = Some(result);
            return true;
        }
        let fps = |step: &StepResult| step.scroll.map_or(0.0, |stats| stats.mean_fps);
        let best = match earlier {
            Some(first) if fps(&first) > fps(&result) => first,
            _ => result,
        };
        run.steps.push(best);
        false
    }

    /// Moves to the next step (or repeats this one when `retry`). Returns whether the run is
    /// over.
    fn bench_next(&mut self, run: &mut Run, retry: bool) -> (bool, Task<Message>) {
        if retry {
            return (false, self.bench_install_step(run));
        }
        if bench::should_stop(&run.steps) {
            return (true, Task::none());
        }
        run.step += 1;
        (false, self.bench_install_step(run))
    }

    /// Ends a run: the user's document comes back exactly as it was. `completed` is false for
    /// a cancelled run, which produces no report.
    fn bench_finish(&mut self, run: Run, completed: bool) -> Task<Message> {
        self.doc = run.stash_doc;
        self.scroll = run.stash_scroll;
        self.state.zoom = run.stash_zoom;
        self.scroll_pending = (0.0, 0.0);
        self.fling = (0.0, 0.0);
        if completed && !run.steps.is_empty() {
            let format = match self.settings.default_format {
                DefaultFormat::Odt => "ODT",
                DefaultFormat::Docx => "DOCX",
                DefaultFormat::Rtf => "RTF",
                DefaultFormat::Markdown => "Markdown",
                DefaultFormat::Native => "LBTD",
            };
            let report = Report::from_steps(run.steps, format);
            self.bench_save_note = Some(self.bench_save(&report));
            self.bench_report = Some(report);
            self.bench_report_open = true;
        }
        self.refresh_view();
        self.refresh_format();
        if auto_exit() {
            if let Some(report) = &self.bench_report {
                tracing::info!("\n{}", report.to_text());
            }
            return iced::exit();
        }
        self.schedule_spell_check()
    }

    /// Saves a finished report so it can be looked at again, and says where (or why not).
    fn bench_save(&self, report: &Report) -> String {
        let Some(dir) = &self.bench_dir else {
            return s::BENCH_NOT_SAVED.to_owned();
        };
        match bench::save(report, dir) {
            Ok(path) => format!(
                "{} {}",
                s::BENCH_SAVED_TO,
                path.with_extension("txt").display()
            ),
            Err(error) => {
                tracing::warn!(%error, "could not save the benchmark result");
                format!("{} {error}", s::BENCH_NOT_SAVED_BECAUSE)
            }
        }
    }

    /// Loads the newest saved result, if none is showing yet (Settings calls this, so "Show
    /// last result" works in a later session).
    pub(super) fn bench_load_saved(&mut self) {
        if self.bench_report.is_some() {
            return;
        }
        let Some(dir) = &self.bench_dir else {
            return;
        };
        if let Some((report, path)) = bench::load_latest(dir) {
            self.bench_save_note = Some(format!(
                "{} {}",
                s::BENCH_SAVED_AT,
                path.with_extension("txt").display()
            ));
            self.bench_report = Some(report);
        }
    }

    /// Cancels a run in progress.
    pub(super) fn bench_cancel(&mut self) -> Task<Message> {
        match self.bench.take() {
            Some(run) => self.bench_finish(run, false),
            None => Task::none(),
        }
    }

    // ------------------------------------------------------------------ views

    /// The progress panel shown over the generated document while a run goes.
    pub(super) fn bench_progress_layer(&self, t: Tokens) -> Element<'_, Message> {
        let Some(run) = &self.bench else {
            return space().into();
        };
        let target = bench::SCHEDULE[run.step.min(bench::SCHEDULE.len() - 1)];
        let fps = run
            .live_fps
            .map_or_else(String::new, |fps| format!("  {fps:.0} fps"));
        let cancel = button(text(s::BENCH_CANCEL).size(13).wrapping(Wrapping::None))
            .padding([6, 14])
            .style(move |_, status| controls::flat(t, status))
            .on_press(Message::CancelBenchmark);
        let card = container(
            column![
                text(s::BENCH_RUNNING).size(14).color(t.text),
                text(format!(
                    "{} {} \u{2013} {target} {}{fps}",
                    s::BENCH_STEP,
                    run.step + 1,
                    s::BENCH_PAGES
                ))
                .size(12)
                .color(t.text_secondary),
                text(run.label()).size(12).color(t.text_secondary),
                row![space::horizontal(), cancel],
            ]
            .spacing(6),
        )
        .padding(14)
        .width(Length::Fixed(260.0))
        .style(move |_| container::Style {
            background: Some(t.chrome.into()),
            border: border::rounded(10),
            ..container::Style::default()
        });
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Bottom)
            // Sits just above the word and page counter pill (which is 14 px up and about
            // 27 px tall), centred over the document like it.
            .padding(Padding {
                bottom: 56.0,
                ..Padding::ZERO
            })
            .into()
    }

    /// The results window: what each frame-rate target reached, then one row per size tried.
    pub(super) fn bench_report_layer(&self, t: Tokens) -> Element<'_, Message> {
        let Some(report) = &self.bench_report else {
            return space().into();
        };
        let header = |labels: &[(&'static str, f32)]| -> Element<'_, Message> {
            row(labels
                .iter()
                .map(|&(label, width)| cell(label.to_owned(), width, t.text_secondary)))
            .spacing(8)
            .into()
        };

        let mut body = column![].spacing(6);
        let display = match report.display_hz {
            Some(hz) => format!("{} {hz} Hz", s::BENCH_DISPLAY),
            None => s::BENCH_DISPLAY_SLOW.to_owned(),
        };
        let display = if report.taken_unix > 0 {
            format!("{}\n{display}", report.taken_text())
        } else {
            display
        };
        body = body.push(text(display).size(12).color(t.text_secondary));

        // Targets.
        let limit_cols = [
            (s::BENCH_COL_TARGET, 70.0),
            (s::BENCH_COL_LARGEST, 130.0),
            (s::BENCH_COL_WORDS, 110.0),
            (s::BENCH_COL_FIRST_MISS, 130.0),
        ];
        body = body
            .push(header(&limit_cols))
            .push(controls::hairline(t.chrome_border));
        for limit in &report.limits {
            let (largest, words, miss, colour) = if !limit.supported {
                (
                    s::BENCH_TABLE_NA.to_owned(),
                    String::new(),
                    s::BENCH_TABLE_DISPLAY.to_owned(),
                    t.text_secondary,
                )
            } else if let Some((pages, words)) = limit.best {
                (
                    format!("{pages} {}", s::BENCH_PAGES),
                    format!("~{words}"),
                    limit.failed_at.map_or_else(
                        || s::BENCH_TABLE_NONE.to_owned(),
                        |pages| format!("{pages} {}", s::BENCH_PAGES),
                    ),
                    t.text,
                )
            } else {
                (
                    s::BENCH_TABLE_NOT_REACHED.to_owned(),
                    String::new(),
                    limit
                        .failed_at
                        .map_or_else(String::new, |p| format!("{p} {}", s::BENCH_PAGES)),
                    t.text_secondary,
                )
            };
            body = body.push(
                row![
                    cell(format!("{} fps", limit.target), limit_cols[0].1, colour),
                    cell(largest, limit_cols[1].1, colour),
                    cell(words, limit_cols[2].1, colour),
                    cell(miss, limit_cols[3].1, colour),
                ]
                .spacing(8),
            );
        }

        // Sizes.
        let step_cols = [
            (s::BENCH_COL_PAGES, 60.0),
            (s::BENCH_COL_WORDS, 90.0),
            (s::BENCH_COL_SCROLL, 80.0),
            (s::BENCH_COL_SLOW, 90.0),
            (s::BENCH_COL_TYPING, 90.0),
            (s::BENCH_COL_SAVE, 80.0),
        ];
        body = body
            .push(space().height(Length::Fixed(10.0)))
            .push(header(&step_cols))
            .push(controls::hairline(t.chrome_border));
        for step in &report.steps {
            let holds = step
                .scroll
                .is_some_and(|stats| stats.holds(bench::TARGETS[0]));
            let colour = if holds { t.text } else { t.text_secondary };
            let (mean, slow) = step.scroll.map_or_else(
                || (s::BENCH_NO_RESULT.to_owned(), String::new()),
                |stats| {
                    (
                        format!("{:.0}", stats.mean_fps),
                        format!("{:.0}", stats.p95_fps),
                    )
                },
            );
            let save = step
                .save_ms
                .map_or_else(|| s::BENCH_TABLE_NA.to_owned(), |ms| format!("{ms:.0}"));
            body = body.push(
                row![
                    cell(step.pages.to_string(), step_cols[0].1, colour),
                    cell(step.words.to_string(), step_cols[1].1, colour),
                    cell(mean, step_cols[2].1, colour),
                    cell(slow, step_cols[3].1, colour),
                    cell(format!("{:.1}", step.typing_p95_ms), step_cols[4].1, colour),
                    cell(save, step_cols[5].1, colour),
                ]
                .spacing(8),
            );
        }

        // Estimated save times.
        body = body.push(space().height(Length::Fixed(10.0))).push(
            text(format!("{} ({})", s::BENCH_SAVE_ESTIMATE, report.format))
                .size(12)
                .color(t.text_secondary),
        );
        let estimates: Vec<String> = bench::ESTIMATE_PAGES
            .iter()
            .filter_map(|&pages| {
                report
                    .estimated_save_ms(pages)
                    .map(|ms| format!("{pages} {}: {}", s::BENCH_PAGES, bench::seconds(ms)))
            })
            .collect();
        body = body.push(text(estimates.join("   ")).size(12).color(t.text));
        body = body.push(text(s::BENCH_NOTE).size(11).color(t.text_secondary));
        if let Some(note) = &self.bench_save_note {
            body = body.push(text(note.clone()).size(11).color(t.text_secondary));
        }

        let copy_label = if self.bench_copied {
            s::BENCH_COPIED
        } else {
            s::BENCH_COPY
        };
        let copy = button(text(copy_label).size(13).wrapping(Wrapping::None))
            .padding([6, 14])
            .style(move |_, status| controls::flat(t, status))
            .on_press(Message::CopyBenchmarkReport);
        let close = button(text(s::BENCH_CLOSE).size(13).wrapping(Wrapping::None))
            .padding([6, 18])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::CloseBenchmarkReport);
        let card = container(
            column![
                text(s::BENCH_REPORT_TITLE).size(16).color(t.text),
                iced::widget::scrollable(body).height(Length::Fixed(460.0)),
                row![copy, space::horizontal(), close],
            ]
            .spacing(14),
        )
        .padding(20)
        .width(Length::Fixed(660.0))
        .style(move |_| container::Style {
            background: Some(t.chrome.into()),
            border: border::rounded(10),
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseBenchmarkReport),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::three_paragraph_doc;
    use super::*;

    #[test]
    fn a_run_lets_through_only_what_it_needs() {
        assert!(allows(&Message::CancelBenchmark));
        assert!(allows(&Message::CloseRequested));
        assert!(!allows(&Message::AutosaveTick));
        assert!(!allows(&Message::OpenSettings));
        assert!(allows(&Message::Canvas(CanvasEvent::Viewport(Size::new(
            900.0, 600.0
        )))));
        assert!(!allows(&Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: 10.0,
            stepped: true
        })));
    }

    #[test]
    fn a_run_shows_a_generated_document_and_cancelling_gives_the_users_back() {
        let mut app = three_paragraph_doc();
        let before = app.doc().document().clone();
        let zoom = app.state.zoom;
        let _ = app.update(Message::RunBenchmark);
        assert!(app.bench.is_some(), "the run started");
        assert!(
            app.doc().document().paragraph_count() > before.paragraph_count(),
            "the canvas shows the generated document"
        );
        assert_eq!(app.state.zoom, Zoom::Percent(100));

        let _ = app.update(Message::CancelBenchmark);
        assert!(app.bench.is_none());
        assert!(
            app.bench_report.is_none(),
            "a cancelled run leaves no report"
        );
        assert!(
            app.doc().document().content_eq(&before),
            "the user's document is back, untouched"
        );
        assert_eq!(app.state.zoom, zoom);
    }

    #[test]
    fn edits_and_saves_cannot_reach_the_generated_document_during_a_run() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::RunBenchmark);
        let generated = app.doc().document().clone();
        let _ = app.update(Message::PasteText(Some("stray text".into())));
        let _ = app.update(Message::AutosaveTick);
        assert!(
            app.doc().document().content_eq(&generated),
            "a dropped message changed nothing"
        );
        let _ = app.update(Message::CancelBenchmark);
    }

    #[test]
    fn a_finished_run_keeps_a_report_and_opens_it() {
        let mut app = three_paragraph_doc();
        let dir = std::env::temp_dir().join(format!("libertum-bench-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        app.bench_dir = Some(dir.clone());
        let _ = app.update(Message::RunBenchmark);
        let mut run = app.bench.take().expect("running");
        run.steps.push(StepResult {
            pages: 20,
            words: 10_000,
            scroll: FrameStats::from_intervals(&[16.6; 60]),
            typing_mean_ms: 1.0,
            typing_p95_ms: 2.0,
            save_ms: Some(40.0),
            save_bytes: 10_000,
        });
        let _ = app.bench_finish(run, true);
        assert!(app.bench_report_open);
        assert!(app.bench_report.is_some());
        assert_eq!(app.doc().document().paragraph_count(), 3);
        let _ = app.update(Message::CloseBenchmarkReport);
        assert!(!app.bench_report_open);
        assert!(app.bench_report.is_some(), "Settings can show it again");
        assert!(
            crate::bench::load_latest(&dir).is_some(),
            "the result was saved to disk"
        );
        assert!(
            app.bench_save_note
                .as_deref()
                .is_some_and(|note| note.starts_with(s::BENCH_SAVED_TO))
        );

        // A later session finds it again from Settings.
        let mut later = three_paragraph_doc();
        later.bench_dir = Some(dir.clone());
        assert!(later.bench_report.is_none());
        let _ = later.update(Message::OpenSettings);
        assert!(later.bench_report.is_some(), "the saved result was loaded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_default_format_can_be_timed() {
        let (document, _) = bench::build_document(2, 3_000.0);
        for format in [
            DefaultFormat::Odt,
            DefaultFormat::Docx,
            DefaultFormat::Rtf,
            DefaultFormat::Markdown,
            DefaultFormat::Native,
        ] {
            let (ms, bytes) = timed_save(document.clone(), format).expect("saves");
            assert!(ms >= 0.0 && bytes > 0, "{format:?}");
        }
    }
}
