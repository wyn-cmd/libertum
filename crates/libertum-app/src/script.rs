//! The `script` step of a recipe (`docs/mcp-plan.md`, phase 6): a short Koto program that
//! turns the values a recipe has kept into one new value, such as trimming a tool's answer or
//! building a prompt.
//!
//! A script is not trusted. It starts with a fresh runtime that has only the language's pure
//! parts (numbers, strings, lists, maps, tuples, ranges, iterators). The `io`, `os`, `koto` and
//! `test` modules and `print` are removed, so it cannot read or write a file, start a program,
//! reach the network or print, and `import` is refused when the recipe is read, so it cannot
//! load another file either. It is stopped after [`TIME_LIMIT`] and after it holds more than
//! the memory cap (Settings, developer mode; 512 MB each by default, 1 GB for all stopped scripts).
//!
//! The script runs on a short-lived worker thread while the app thread waits for it. A counting
//! global allocator ([`Counting`]) charges each allocation to the thread that made it. When a
//! script thread goes over its cap, the allocator marks the run as tripped and parks that thread
//! for good, so the allocation never fails (a failed allocation would abort the whole app). The
//! app thread then reports the limit. The parked thread's memory (at most the cap) is not freed
//! until the app exits; that is the price of a cap that cannot crash the app.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use koto::prelude::*;

/// How long a script may run before it is stopped.
pub const TIME_LIMIT: Duration = Duration::from_secs(2);
/// Longest script accepted, in bytes.
pub const MAX_CODE_BYTES: usize = 20_000;
/// Default memory cap for a script, in megabytes.
pub const DEFAULT_MEMORY_MB: u32 = 512;
/// Default for all scripts together: the memory that stopped scripts may leave behind.
pub const DEFAULT_TOTAL_MEMORY_MB: u32 = 1024;
/// Smallest and largest cap that is honoured, in megabytes.
pub const MIN_MEMORY_MB: u32 = 16;
pub const MAX_MEMORY_MB: u32 = 16_384;

/// Names taken out of the runtime before a script sees it.
const REMOVED: [&str; 5] = ["io", "os", "koto", "test", "print"];

/// What the allocator charges one script run against.
struct Budget {
    live: AtomicUsize,
    limit: usize,
    tripped: AtomicBool,
}

thread_local! {
    /// The budget of the script running on this thread, or null on every other thread.
    /// `const` and without a destructor, so it is safe to read inside the allocator.
    static BUDGET: Cell<*const Budget> = const { Cell::new(std::ptr::null()) };
}

/// The app's global allocator: the system allocator, plus a byte count for script threads.
pub struct Counting;

// SAFETY: every method forwards to `System` with the arguments it was given; the counting
// only reads and updates atomics and never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        charge(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        charge(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        release(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() {
            charge(new_size - layout.size());
        } else {
            release(layout.size() - new_size);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn charge(bytes: usize) {
    let budget = BUDGET.with(Cell::get);
    if budget.is_null() {
        return;
    }
    // SAFETY: the pointer is set by `run_worker` from an `Arc` it keeps alive until the
    // pointer is cleared, and only on this thread.
    let budget = unsafe { &*budget };
    let now = budget.live.fetch_add(bytes, Ordering::Relaxed) + bytes;
    if now > budget.limit {
        budget.tripped.store(true, Ordering::Release);
        // Never hand back an allocation over the cap, and never fail it either.
        loop {
            std::thread::park();
        }
    }
}

fn release(bytes: usize) {
    let budget = BUDGET.with(Cell::get);
    if !budget.is_null() {
        // SAFETY: as in `charge`.
        let budget = unsafe { &*budget };
        // Only the script's own thread updates the count, so a plain load and store suffice;
        // memory that was made elsewhere and freed here must not take it below zero.
        let live = budget.live.load(Ordering::Relaxed);
        budget
            .live
            .store(live.saturating_sub(bytes), Ordering::Relaxed);
    }
}

fn engine() -> Koto {
    let koto = Koto::with_settings(KotoSettings::default().with_execution_limit(TIME_LIMIT));
    {
        let mut prelude = koto.prelude().data_mut();
        for name in REMOVED {
            prelude.shift_remove(name);
        }
    }
    koto
}

/// Keep a memory limit in megabytes between [`MIN_MEMORY_MB`] and [`MAX_MEMORY_MB`].
pub fn clamp_mb(megabytes: u32) -> u32 {
    megabytes.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB)
}

/// Whether `import` appears as a whole word. Words that merely contain it, such as `important`
/// or `reimport_count`, are fine; it is a keyword in Koto, so it can never be a name. A string
/// that says "import" on its own is refused too, which is the safe side.
fn uses_import(code: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices("import").any(|(start, found)| {
        let before = code[..start].chars().next_back();
        let after = code[start + found.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// Reject a script before it is ever run: too long, an `import`, or not valid Koto.
pub fn check(code: &str) -> Result<(), String> {
    if code.trim().is_empty() {
        return Err("the script is empty".to_string());
    }
    if code.len() > MAX_CODE_BYTES {
        return Err(format!("the script is longer than {MAX_CODE_BYTES} bytes"));
    }
    // Any use of `import` (which includes `from x import y`) could load a file.
    if uses_import(code) {
        return Err("a script may not use import".to_string());
    }
    engine()
        .compile(code)
        .map(|_| ())
        .map_err(|error| first_line(&error.to_string()))
}

/// Run `code` with each of `vars` available to it as a string under its own name. The script's
/// last expression is its answer, which must be text, a number or true/false. It is stopped
/// after [`TIME_LIMIT`] or when it holds more than `memory_mb` megabytes.
///
/// `total_mb` is the shared cap: a script that goes over its own cap is parked and its memory
/// stays allocated until exit, so the memory such scripts have left behind is added up for the
/// whole session. A script may use at most what is left of the shared cap, and once less than
/// [`MIN_MEMORY_MB`] is left no script runs until Libertum is restarted.
pub fn run(
    code: &str,
    vars: &HashMap<String, String>,
    memory_mb: u32,
    total_mb: u32,
) -> Result<String, String> {
    run_counting(code, vars, memory_mb, total_mb, &LEFT_BEHIND)
}

/// Bytes held by scripts that were stopped for using too much memory (never given back).
static LEFT_BEHIND: AtomicUsize = AtomicUsize::new(0);

fn run_counting(
    code: &str,
    vars: &HashMap<String, String>,
    memory_mb: u32,
    total_mb: u32,
    left_behind: &AtomicUsize,
) -> Result<String, String> {
    const MB: usize = 1024 * 1024;
    // Checked again here so nothing reaches the runtime that `check` would have refused.
    if uses_import(code) || code.len() > MAX_CODE_BYTES {
        return Err("the script is not allowed".to_string());
    }
    let memory_mb = clamp_mb(memory_mb);
    let total = clamp_mb(total_mb) as usize * MB;
    let remaining = total.saturating_sub(left_behind.load(Ordering::Relaxed));
    if remaining < MIN_MEMORY_MB as usize * MB {
        return Err(format!(
            "scripts are off until Libertum restarts: stopped scripts still hold {} MB of the {} MB shared limit",
            left_behind.load(Ordering::Relaxed) / MB,
            total / MB
        ));
    }
    let limit = (memory_mb as usize * MB).min(remaining);
    let budget = Arc::new(Budget {
        live: AtomicUsize::new(0),
        limit,
        tripped: AtomicBool::new(false),
    });
    let (sender, receiver) = mpsc::channel();
    let worker_budget = Arc::clone(&budget);
    let code = code.to_owned();
    let vars = vars.clone();
    std::thread::Builder::new()
        .name("libertum-script".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let result = run_worker(&code, &vars, &worker_budget);
            let _ = sender.send(result);
        })
        .map_err(|error| format!("the script could not start: {error}"))?;
    // The runtime stops itself after the time limit; the margin covers start-up and copying
    // the answer back. Over the memory cap the worker is parked and never answers.
    let deadline = std::time::Instant::now() + TIME_LIMIT + Duration::from_secs(3);
    loop {
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if budget.tripped.load(Ordering::Acquire) {
                    left_behind.fetch_add(budget.live.load(Ordering::Relaxed), Ordering::Relaxed);
                    return Err(format!("the script used more than {} MB", limit / MB));
                }
                if std::time::Instant::now() > deadline {
                    return Err("the script timed out".to_string());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the script stopped unexpectedly".to_string());
            }
        }
    }
}

fn run_worker(
    code: &str,
    vars: &HashMap<String, String>,
    budget: &Arc<Budget>,
) -> Result<String, String> {
    BUDGET.with(|b| b.set(Arc::as_ptr(budget)));
    let result = evaluate(code, vars);
    // `evaluate` owns the runtime, so it is freed by now; counting stops here.
    BUDGET.with(|b| b.set(std::ptr::null()));
    result
}

fn evaluate(code: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    let mut koto = engine();
    for (name, value) in vars {
        koto.prelude().insert(name.as_str(), value.as_str());
    }
    let answer = koto
        .compile_and_run(code)
        .map_err(|error| first_line(&error.to_string()))?;
    match answer {
        KValue::Str(text) => Ok(text.as_str().to_string()),
        KValue::Number(_) | KValue::Bool(_) => koto
            .value_to_string(answer)
            .map_err(|error| first_line(&error.to_string())),
        KValue::Null => Err("the script gave no answer; end it with the value to keep".to_string()),
        _ => Err("the script's answer must be text, a number or true/false".to_string()),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    line.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn go(code: &str, v: &HashMap<String, String>) -> Result<String, String> {
        run(code, v, DEFAULT_MEMORY_MB, DEFAULT_TOTAL_MEMORY_MB)
    }

    #[test]
    fn a_script_turns_values_into_a_value() {
        let v = vars(&[("text", "  Hello World  ")]);
        assert_eq!(
            go("text.trim().to_uppercase()", &v).as_deref(),
            Ok("HELLO WORLD")
        );
        assert_eq!(go("1 + 2", &v).as_deref(), Ok("3"));
        assert_eq!(go("text.is_empty()", &v).as_deref(), Ok("false"));
        let multi = "words = text.trim().split(' ').to_list()\nwords.last()";
        assert_eq!(go(multi, &v).as_deref(), Ok("World"));
    }

    #[test]
    fn a_script_cannot_reach_files_programs_or_output() {
        let v = vars(&[]);
        for code in [
            "io.read_to_string 'Cargo.toml'",
            "os.command('cmd').spawn()",
            "koto.script_dir()",
            "print 'hello'",
            "import io\nio.read_to_string 'Cargo.toml'",
            "from io import read_to_string",
        ] {
            assert!(go(code, &v).is_err(), "{code} must fail");
        }
        assert!(check("import something").is_err());
        assert!(
            check(
                "x = 1
from a import b"
            )
            .is_err()
        );
        assert!(check("(import)").is_err());
    }

    #[test]
    fn words_that_only_contain_import_are_allowed() {
        assert!(
            check(
                "important = 1
important + 1"
            )
            .is_ok()
        );
        assert!(
            check(
                "reimport_count = 2
reimport_count"
            )
            .is_ok()
        );
        assert!(check("'unimportant'.to_uppercase()").is_ok());
        let v = vars(&[]);
        assert_eq!(
            go(
                "important = 2
important * 3",
                &v
            )
            .as_deref(),
            Ok("6")
        );
    }

    #[test]
    fn a_runaway_script_is_stopped_by_the_time_limit() {
        let started = std::time::Instant::now();
        let error = go("loop\n  x = 1", &vars(&[])).expect_err("must stop");
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < TIME_LIMIT + Duration::from_secs(3));
    }

    #[test]
    fn bad_scripts_are_reported_not_run() {
        assert!(check("").is_err());
        assert!(check("x = (").is_err());
        assert!(check(&"a".repeat(MAX_CODE_BYTES + 1)).is_err());
        assert!(check("1 + 1").is_ok());
        // No answer, or an answer that is not plain text.
        assert!(go("x = 1\nnull", &vars(&[])).is_err());
        assert!(go("[1, 2]", &vars(&[])).is_err());
        assert!(go("missing_name", &vars(&[])).is_err());
    }

    #[test]
    fn each_run_starts_fresh() {
        assert_eq!(go("x = 5\nx", &vars(&[])).as_deref(), Ok("5"));
        assert!(go("x", &vars(&[])).is_err());
    }

    #[test]
    fn a_script_over_the_memory_cap_is_stopped_and_the_next_one_still_runs() {
        // Doubles a string until it passes the 16 MB floor, well inside the time limit.
        let code = "s = 'abcdefgh'\nloop\n  s = s + s";
        let started = std::time::Instant::now();
        let error =
            run(code, &vars(&[]), MIN_MEMORY_MB, DEFAULT_TOTAL_MEMORY_MB).expect_err("must stop");
        assert!(error.contains("more than 16 MB"), "{error}");
        assert!(
            started.elapsed() < TIME_LIMIT,
            "stopped by memory, not time"
        );
        assert_eq!(go("1 + 1", &vars(&[])).as_deref(), Ok("2"));
    }

    #[test]
    fn a_small_script_stays_under_a_small_cap() {
        let v = vars(&[("text", "hello")]);
        assert_eq!(
            run(
                "text.to_uppercase()",
                &v,
                MIN_MEMORY_MB,
                DEFAULT_TOTAL_MEMORY_MB
            )
            .as_deref(),
            Ok("HELLO")
        );
    }

    #[test]
    fn stopped_scripts_use_up_the_shared_cap_and_then_no_script_runs() {
        let left = AtomicUsize::new(0);
        let hog = "s = 'abcdefgh'\nloop\n  s = s + s";
        // Own cap 16 MB, shared cap 40 MB: two scripts can be stopped, then none may start.
        for _ in 0..2 {
            let error = run_counting(hog, &vars(&[]), 16, 40, &left).expect_err("stops");
            assert!(error.contains("more than"), "{error}");
        }
        assert!(left.load(Ordering::Relaxed) >= 2 * 15 * 1024 * 1024);
        let error = run_counting("1 + 1", &vars(&[]), 16, 40, &left).expect_err("refused");
        assert!(error.contains("off until Libertum restarts"), "{error}");
    }

    #[test]
    fn a_script_may_not_use_more_than_is_left_of_the_shared_cap() {
        let left = AtomicUsize::new(20 * 1024 * 1024);
        let hog = "s = 'abcdefgh'\nloop\n  s = s + s";
        // 512 MB each, 40 MB shared, 20 MB already gone: this one is cut at about 20 MB.
        let error = run_counting(hog, &vars(&[]), 512, 40, &left).expect_err("stops");
        assert!(error.contains("more than 20 MB"), "{error}");
    }
}
