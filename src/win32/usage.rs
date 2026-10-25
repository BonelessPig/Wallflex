//! Wallflex's own CPU and memory use, so it can keep itself within its
//! budget.

use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, SetProcessWorkingSetSize,
};
use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;

use super::struct_size;

/// Percent.
const PERCENT: f64 = 100.0;

/// Measures this process's CPU use between calls, from the exact cycle
/// counts Windows keeps for its threads.
///
/// Not `GetProcessTimes`: that charges a whole 15.6 ms clock tick to
/// whichever thread is running when the tick lands, and Wallflex's
/// frame timers wake it right on the ticks. Over a second of animation
/// that read 270-390 ms where the cycle count said ~100 ms, so the
/// governor kept stepping down for load that wasn't there. (Task
/// Manager uses the same sampled times, so it shows those jumps too.)
#[derive(Default)]
pub struct CpuMeter {
    /// The counts at the last call.
    last: Option<Cycles>,
}

/// This process's cycles and the processor's timestamp counter, read
/// together. Windows counts thread cycles in timestamp-counter units, so
/// the two divide without knowing the clock speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cycles {
    /// Cycles used by every thread of this process so far.
    used: u64,
    /// The timestamp counter: cycles of one processor's time so far.
    clock: u64,
}

impl CpuMeter {
    /// Wallflex's share of the whole CPU (%) since the last call; 0 the
    /// first time, or if the counts can't be read.
    pub fn sample(&mut self) -> f64 {
        let Some(now) = read_cycles() else {
            return 0.0;
        };
        let cores = std::thread::available_parallelism()
            .ok()
            .and_then(|n| u32::try_from(n.get()).ok())
            .map_or(1.0, f64::from);
        let share = self.last.map_or(0.0, |then| share(then, now, cores));
        self.last = Some(now);
        share
    }
}

/// The share of `cores` processors (%) used between two readings.
fn share(then: Cycles, now: Cycles, cores: f64) -> f64 {
    let used = u64_to_f64(now.used.saturating_sub(then.used));
    let clock = u64_to_f64(now.clock.saturating_sub(then.clock));
    if clock > 0.0 && cores > 0.0 {
        used / clock / cores * PERCENT
    } else {
        0.0
    }
}

/// This process's cycle count and the timestamp counter, now.
fn read_cycles() -> Option<Cycles> {
    let mut used = 0_u64;
    // SAFETY: the current-process pseudo handle and a valid place to
    // write.
    unsafe { QueryProcessCycleTime(GetCurrentProcess(), &raw mut used) }
        .ok()?;
    Some(Cycles {
        used,
        clock: timestamp_counter()?,
    })
}

/// The processor's timestamp counter (the unit Windows counts thread
/// cycles in).
#[cfg(target_arch = "x86_64")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the other processors' version, which has none"
)]
fn timestamp_counter() -> Option<u64> {
    // SAFETY: RDTSC only reads a counter; every x86-64 processor has it.
    Some(unsafe { core::arch::x86_64::_rdtsc() })
}

/// No timestamp counter to compare with on other processors, so
/// Wallflex's own use isn't measured there (the PC's load still is).
#[cfg(not(target_arch = "x86_64"))]
fn timestamp_counter() -> Option<u64> {
    None
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
    reason = "differences of cycle counts over a second or so are far \
              below 2^53"
)]
fn u64_to_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::{CpuMeter, Cycles, share};
    use std::time::{Duration, Instant};

    #[test]
    fn shares_cycles_among_processors() {
        let then = Cycles { used: 0, clock: 0 };
        let now = Cycles {
            used: 50,
            clock: 1_000,
        };
        assert!((share(then, now, 4.0) - 1.25).abs() < 1e-9);
        // A counter that didn't move gives no share, not a division by 0.
        assert!(share(now, now, 4.0).abs() < 1e-9);
    }

    #[test]
    fn measures_a_busy_thread() {
        // One thread spinning for a while is at least about one
        // processor's worth of the whole CPU (other tests may be running
        // in this process too, so only the floor is tight).
        let cores = std::thread::available_parallelism()
            .map_or(1.0, |n| f64::from(u32::try_from(n.get()).unwrap()));
        let mut meter = CpuMeter::default();
        let _ = meter.sample();
        let start = Instant::now();
        let mut spins = 0_u64;
        while start.elapsed() < Duration::from_millis(200) {
            spins = std::hint::black_box(spins.wrapping_add(1));
        }
        let measured = meter.sample();
        let one_processor = 100.0 / cores;
        assert!(
            measured > one_processor * 0.7 && measured <= 100.0,
            "{measured:.2}% vs {one_processor:.2}% for one processor"
        );
    }
}
