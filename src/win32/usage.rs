//! Wallflex's own CPU and memory use, so it can keep itself within its
//! budget.

use std::time::Instant;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, SetProcessWorkingSetSize,
};

use super::struct_size;

/// 100 ns units per second (FILETIME's unit).
const TICKS_PER_SECOND: f64 = 10_000_000.0;
/// Percent.
const PERCENT: f64 = 100.0;

/// Measures this process's CPU use between calls.
#[derive(Default)]
pub struct CpuMeter {
    /// When, and how much CPU time had been used, at the last call.
    last: Option<(Instant, u64)>,
}

impl CpuMeter {
    /// Wallflex's share of the whole CPU (%) since the last call; 0 the
    /// first time.
    pub fn sample(&mut self) -> f64 {
        let now = Instant::now();
        let Some(used) = cpu_time() else {
            return 0.0;
        };
        let share = self.last.map_or(0.0, |(then, before)| {
            let seconds = now.duration_since(then).as_secs_f64();
            let busy =
                u64_to_f64(used.saturating_sub(before)) / TICKS_PER_SECOND;
            let cores = std::thread::available_parallelism()
                .ok()
                .and_then(|n| u32::try_from(n.get()).ok())
                .map_or(1.0, f64::from);
            if seconds > 0.0 {
                busy / seconds / cores * PERCENT
            } else {
                0.0
            }
        });
        self.last = Some((now, used));
        share
    }
}

/// Kernel plus user CPU time used so far, in 100 ns units.
fn cpu_time() -> Option<u64> {
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: the current-process pseudo handle and four valid places
    // to write.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut *created,
            &raw mut *exited,
            &raw mut *kernel,
            &raw mut *user,
        )
    }
    .ok()?;
    Some(ticks(*kernel).saturating_add(ticks(*user)))
}

/// A FILETIME as one number.
fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Memory only this process uses (private bytes).
pub fn private_bytes() -> Option<u64> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: struct_size::<PROCESS_MEMORY_COUNTERS_EX>(),
        ..Default::default()
    };
    // SAFETY: `counters` is the size it says; the extended struct is
    // accepted where the basic one is asked for.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast(),
            counters.cb,
        )
    };
    ok.as_bool()
        .then(|| u64::try_from(counters.PrivateUsage).unwrap_or(u64::MAX))
}

/// Asks Windows to page out what this process isn't using right now.
pub fn trim_working_set() {
    // SAFETY: (-1, -1) is the documented "trim" request for the current
    // process; failure just leaves memory as it was.
    let _ = unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX)
    };
}

/// A count as `f64` (precision beyond 2^53 isn't needed here).
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "CPU time in 100 ns units fits f64 precision for centuries"
)]
fn u64_to_f64(value: u64) -> f64 {
    value as f64
}
