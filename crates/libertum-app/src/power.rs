//! Lower scheduling priority for work nobody is waiting on.
//!
//! Pagination, paragraph shaping and page rastering are throughput jobs: finishing them a little
//! later costs nothing the writer can see, but running them on a performance core at full speed
//! costs battery. Each platform has a way to say so, and each is a request the system may ignore:
//!
//! - Linux: a higher nice value for the calling thread (per thread, not per process).
//! - macOS: a quality-of-service class, which steers the thread towards efficiency cores.
//! - Windows: a lower thread priority, and for [`Level::Background`] also execution-speed
//!   throttling ("EcoQoS"), which does the same on hybrid CPUs.
//!
//! Failure is ignored: the thread simply keeps its normal priority.

/// How far to lower the calling thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Work the writer will see soon (page bitmaps for the viewport): only slightly lowered, so
    /// it still keeps up when the machine is busy.
    Utility,
    /// Work nobody waits for (finishing pagination, shaping ahead of it).
    Background,
}

/// Lowers the calling thread. Call it first thing in a worker thread.
pub fn lower_current_thread(level: Level) {
    imp::lower(level);
}

/// [`lower_current_thread`] for [`Level::Background`], as a plain function a worker-start hook can
/// hold.
pub fn background() {
    lower_current_thread(Level::Background);
}

/// The nice value for a level (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn nice_for(level: Level) -> i32 {
    // Not 19: a thread that holds the layout lock and is starved of CPU makes the window wait
    // for it, so the gap to the UI thread is kept modest.
    match level {
        Level::Utility => 3,
        Level::Background => 10,
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Level, nice_for};

    pub fn lower(level: Level) {
        // SAFETY: `gettid` takes no arguments and returns the calling thread's id;
        // `setpriority` with `PRIO_PROCESS` and a thread id changes only that thread on Linux.
        unsafe {
            let tid = libc::syscall(libc::SYS_gettid);
            if tid > 0 {
                let _ = libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, nice_for(level));
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Level;

    pub fn lower(level: Level) {
        let class = match level {
            Level::Utility => libc::qos_class_t::QOS_CLASS_UTILITY,
            Level::Background => libc::qos_class_t::QOS_CLASS_BACKGROUND,
        };
        // SAFETY: sets the QoS class of the calling thread; no pointers are involved.
        let _ = unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
    }
}

#[cfg(windows)]
mod imp {
    use super::Level;
    use std::ffi::c_void;

    const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
    /// `THREAD_INFORMATION_CLASS::ThreadPowerThrottling`.
    const THREAD_POWER_THROTTLING: i32 = 3;
    const THREAD_POWER_THROTTLING_CURRENT_VERSION: u32 = 1;
    const THREAD_POWER_THROTTLING_EXECUTION_SPEED: u32 = 1;

    #[repr(C)]
    struct ThreadPowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }

    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn SetThreadPriority(thread: *mut c_void, priority: i32) -> i32;
        fn SetThreadInformation(
            thread: *mut c_void,
            class: i32,
            information: *const c_void,
            size: u32,
        ) -> i32;
    }

    pub fn lower(level: Level) {
        // SAFETY: `GetCurrentThread` returns a pseudo handle that needs no closing; the state
        // struct outlives the call and its size is passed with it.
        unsafe {
            let thread = GetCurrentThread();
            let _ = SetThreadPriority(thread, THREAD_PRIORITY_BELOW_NORMAL);
            if level == Level::Background {
                let state = ThreadPowerThrottlingState {
                    version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
                    control_mask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
                    state_mask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
                };
                let _ = SetThreadInformation(
                    thread,
                    THREAD_POWER_THROTTLING,
                    (&state as *const ThreadPowerThrottlingState).cast(),
                    std::mem::size_of::<ThreadPowerThrottlingState>() as u32,
                );
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::Level;

    pub fn lower(_level: Level) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowering_a_thread_never_panics_and_background_is_the_lower_of_the_two() {
        std::thread::spawn(|| {
            lower_current_thread(Level::Utility);
            lower_current_thread(Level::Background);
        })
        .join()
        .expect("a worker can lower itself");
        assert!(nice_for(Level::Background) > nice_for(Level::Utility));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn on_linux_only_the_calling_thread_is_lowered() {
        let nice_of_current = || unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
            libc::getpriority(libc::PRIO_PROCESS, tid)
        };
        let before = nice_of_current();
        let (in_worker, in_worker_before) = std::thread::spawn(move || {
            let was = nice_of_current();
            lower_current_thread(Level::Background);
            (nice_of_current(), was)
        })
        .join()
        .expect("worker");
        assert_eq!(in_worker, in_worker_before.max(nice_for(Level::Background)));
        assert_eq!(
            nice_of_current(),
            before,
            "the test's own thread keeps its priority"
        );
    }
}
