//! Windows performance counters (PDH): CPU use and clock, GPU use and
//! VRAM in use.

use windows::Win32::System::Performance::{
    PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

use crate::log;

/// PDH's success status.
const PDH_OK: u32 = 0;
/// Item status: valid data.
const PDH_CSTATUS_VALID_DATA: u32 = 0;
/// Item status: valid, and new since the last sample.
const PDH_CSTATUS_NEW_DATA: u32 = 1;

/// The counters Wallflex reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counter {
    /// Use per logical processor, plus `_Total`.
    CpuCores,
    /// Current performance vs nominal clock, in % (passes 100 when
    /// boosting).
    CpuPerformance,
    /// Nominal clock, in MHz.
    CpuFrequency,
    /// Use per process, per GPU engine.
    GpuEngine,
    /// Dedicated VRAM in use per adapter part, in bytes.
    GpuMemory,
}

impl Counter {
    /// Every counter, in the order they're added to the query.
    const ALL: [Self; 5] = [
        Self::CpuCores,
        Self::CpuPerformance,
        Self::CpuFrequency,
        Self::GpuEngine,
        Self::GpuMemory,
    ];

    /// The counter's English path.
    fn path(self) -> PCWSTR {
        match self {
            Self::CpuCores => w!(r"\Processor(*)\% Processor Time"),
            Self::CpuPerformance => {
                w!(r"\Processor Information(_Total)\% Processor Performance")
            }
            Self::CpuFrequency => {
                w!(r"\Processor Information(_Total)\Processor Frequency")
            }
            Self::GpuEngine => w!(r"\GPU Engine(*)\Utilization Percentage"),
            Self::GpuMemory => w!(r"\GPU Adapter Memory(*)\Dedicated Usage"),
        }
    }
}

/// One PDH query holding every counter, sampled once per reading. Rate
/// counters need two samples, so they read as missing the first time.
pub struct Query {
    /// The query, closed on drop.
    handle: PDH_HQUERY,
    /// Counters that were added successfully.
    counters: Vec<(Counter, PDH_HCOUNTER)>,
    /// Reused buffer for wildcard counters.
    buffer: Vec<PDH_FMT_COUNTERVALUE_ITEM_W>,
}

impl Query {
    /// Opens the query and adds every counter that exists on this PC.
    /// `None` if PDH itself is unavailable.
    pub fn open() -> Option<Self> {
        let mut handle = PDH_HQUERY::default();
        // SAFETY: `handle` is a valid place for the call to write to; no
        // data source means live counters.
        let status =
            unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &raw mut handle) };
        if status != PDH_OK {
            log!("performance counters unavailable ({status:#x})");
            return None;
        }
        let mut query = Self {
            handle,
            counters: Vec::new(),
            buffer: Vec::new(),
        };
        for counter in Counter::ALL {
            let mut added = PDH_HCOUNTER::default();
            // SAFETY: the query is open, the path is a constant and
            // `added` is a valid place to write to.
            let status = unsafe {
                PdhAddEnglishCounterW(
                    query.handle,
                    counter.path(),
                    0,
                    &raw mut added,
                )
            };
            if status == PDH_OK {
                query.counters.push((counter, added));
            } else {
                log!("counter {counter:?} unavailable ({status:#x})");
            }
        }
        Some(query)
    }

    /// Takes a sample. Returns whether it worked.
    pub fn collect(&mut self) -> bool {
        // SAFETY: the query is open until drop.
        let status = unsafe { PdhCollectQueryData(self.handle) };
        status == PDH_OK
    }

    /// A single-instance counter's value, or `None`.
    pub fn value(&self, counter: Counter) -> Option<f64> {
        let handle = self.handle_of(counter)?;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: the counter belongs to this open query and `value` is
        // a valid place to write to.
        let status = unsafe {
            PdhGetFormattedCounterValue(
                handle,
                PDH_FMT_DOUBLE,
                None,
                &raw mut value,
            )
        };
        if status != PDH_OK || !is_valid(value.CStatus) {
            return None;
        }
        // SAFETY: PDH_FMT_DOUBLE fills the double member.
        Some(unsafe { value.Anonymous.doubleValue })
    }

    /// Every instance of a wildcard counter as `(name, value)`. Empty if
    /// it isn't available or has no data yet. `format` is
    /// [`PDH_FMT_DOUBLE`] or [`PDH_FMT_LARGE`]; values come back as
    /// `f64` either way.
    pub fn array(
        &mut self,
        counter: Counter,
        format: PDH_FMT,
    ) -> Vec<(String, f64)> {
        let Some(handle) = self.handle_of(counter) else {
            return Vec::new();
        };
        let Some(count) = self.fill_buffer(handle, format) else {
            return Vec::new();
        };
        self.buffer
            .iter()
            .take(count)
            .filter(|item| is_valid(item.FmtValue.CStatus))
            .filter_map(|item| {
                // SAFETY: PDH wrote a null-terminated name inside our
                // buffer, which is still alive.
                let name = unsafe { item.szName.to_string() }.ok()?;
                // SAFETY: the member read matches the requested format.
                let value = unsafe {
                    if format == PDH_FMT_LARGE {
                        large_to_f64(item.FmtValue.Anonymous.largeValue)
                    } else {
                        item.FmtValue.Anonymous.doubleValue
                    }
                };
                Some((name, value))
            })
            .collect()
    }

    /// The handle for `counter`, if it was added.
    fn handle_of(&self, counter: Counter) -> Option<PDH_HCOUNTER> {
        self.counters
            .iter()
            .find(|(c, _)| *c == counter)
            .map(|&(_, handle)| handle)
    }

    /// Reads a wildcard counter into `self.buffer`. Returns the number of
    /// items, or `None` if there's no data (or the instances changed
    /// between the two calls; the next reading retries).
    fn fill_buffer(
        &mut self,
        handle: PDH_HCOUNTER,
        format: PDH_FMT,
    ) -> Option<usize> {
        let mut bytes = 0_u32;
        let mut count = 0_u32;
        // SAFETY: with no buffer the call only writes the sizes.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                handle,
                format,
                &raw mut bytes,
                &raw mut count,
                None,
            )
        };
        if status != PDH_MORE_DATA {
            return None;
        }
        // The buffer holds the items followed by their names, so size it
        // in whole items (which also keeps it aligned).
        let item_size = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let items = usize::try_from(bytes).ok()?.div_ceil(item_size);
        self.buffer.clear();
        self.buffer
            .resize(items, PDH_FMT_COUNTERVALUE_ITEM_W::default());
        // SAFETY: the buffer is at least `bytes` long, as PDH asked.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                handle,
                format,
                &raw mut bytes,
                &raw mut count,
                Some(self.buffer.as_mut_ptr()),
            )
        };
        if status != PDH_OK {
            return None;
        }
        usize::try_from(count)
            .ok()
            .map(|c| c.min(self.buffer.len()))
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the query was opened by this object and is closed once.
        // Nothing useful can be done on failure.
        let _ = unsafe { PdhCloseQuery(self.handle) };
    }
}

/// Whether a PDH item status means the value can be used.
fn is_valid(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
}

/// A PDH 64-bit integer value as `f64`.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "byte counts far below 2^53"
)]
fn large_to_f64(value: i64) -> f64 {
    value as f64
}

/// A byte count that came back as `f64` from [`Query::array`], as an
/// integer. `None` for negative or non-finite values.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked non-negative and finite; the value started as an \
              integer byte count"
)]
pub fn whole_bytes(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0).then_some(value as u64)
}
