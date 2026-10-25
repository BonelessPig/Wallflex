//! Network throughput from the interface byte counters.

use std::time::Instant;

use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

use crate::sensors::{Network, rate_kbps};

/// `InterfaceAndOperStatusFlags` bit: a physical adapter.
const HARDWARE_INTERFACE: u8 = 1 << 0;
/// `InterfaceAndOperStatusFlags` bit: a filter driver layered on an
/// adapter (counting it too would double the traffic).
const FILTER_INTERFACE: u8 = 1 << 1;

/// Remembers the last byte counts so rates can be measured.
#[derive(Default)]
pub struct NetReader {
    /// Bytes received, bytes sent, and when.
    last: Option<(u64, u64, Instant)>,
}

impl NetReader {
    /// Throughput since the previous call; `None` on the first call.
    pub fn read(&mut self) -> Option<Network> {
        let (received, sent, link) = totals()?;
        let now = Instant::now();
        let rates = self.last.and_then(|(prev_in, prev_out, then)| {
            let elapsed = now.saturating_duration_since(then);
            Some(Network {
                down_kbps: rate_kbps(prev_in, received, elapsed)?,
                up_kbps: rate_kbps(prev_out, sent, elapsed)?,
                link_kbps: (link > 0)
                    .then(|| u64_to_f64(link) / BITS_PER_KILOBIT),
            })
        });
        self.last = Some((received, sent, now));
        rates
    }
}

/// Frees a table from `GetIfTable2` on drop.
struct IfTable(*mut MIB_IF_TABLE2);

impl Drop for IfTable {
    fn drop(&mut self) {
        // SAFETY: the table came from GetIfTable2 and is freed once.
        unsafe { FreeMibTable(self.0.cast_const().cast()) };
    }
}

/// Bits in a kilobit.
const BITS_PER_KILOBIT: f64 = 1_000.0;

/// A link speed as `f64` (precision beyond 2^53 bit/s isn't needed).
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "link speeds are far below 2^53 bit/s"
)]
fn u64_to_f64(value: u64) -> f64 {
    value as f64
}

/// Total bytes received and sent by physical adapters that are up, and
/// the fastest one's receive link speed (bits per second).
fn totals() -> Option<(u64, u64, u64)> {
    let mut raw: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: on success Windows allocates the table and stores its
    // address in `raw`; `IfTable` frees it.
    if unsafe { GetIfTable2(&raw mut raw) }.is_err() || raw.is_null() {
        return None;
    }
    let table = IfTable(raw);
    // SAFETY: `raw` was returned by `GetIfTable2` and checked non-null
    // above; it remains valid until `table` drops. `NumEntries` rows are
    // stored contiguously starting at `Table`.
    let rows: &[MIB_IF_ROW2] = unsafe {
        let count = usize::try_from((*raw).NumEntries).ok()?;
        std::slice::from_raw_parts((&raw const (*raw).Table).cast(), count)
    };

    let mut received = 0_u64;
    let mut sent = 0_u64;
    let mut link = 0_u64;
    for row in rows {
        let flags = row.InterfaceAndOperStatusFlags._bitfield;
        let counted = flags & HARDWARE_INTERFACE != 0
            && flags & FILTER_INTERFACE == 0
            && row.OperStatus == IfOperStatusUp;
        if counted {
            received = received.saturating_add(row.InOctets);
            sent = sent.saturating_add(row.OutOctets);
            link = link.max(row.ReceiveLinkSpeed);
        }
    }
    Some((received, sent, link))
}
