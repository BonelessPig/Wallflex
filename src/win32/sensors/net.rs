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
        let (received, sent) = totals()?;
        let now = Instant::now();
        let rates = self.last.and_then(|(prev_in, prev_out, then)| {
            let elapsed = now.saturating_duration_since(then);
            Some(Network {
                down_kbps: rate_kbps(prev_in, received, elapsed)?,
                up_kbps: rate_kbps(prev_out, sent, elapsed)?,
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

/// Total bytes received and sent by physical adapters that are up.
fn totals() -> Option<(u64, u64)> {
    let mut raw: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: on success Windows allocates the table and stores its
    // address in `raw`; `IfTable` frees it.
    if unsafe { GetIfTable2(&raw mut raw) }.is_err() || raw.is_null() {
        return None;
    }
    let table = IfTable(raw);
    // SAFETY: the table is valid until `table` drops, and `NumEntries`
    // rows are stored one after another starting at `Table`.
    let rows: &[MIB_IF_ROW2] = unsafe {
        let count = usize::try_from((*table.0).NumEntries).ok()?;
        std::slice::from_raw_parts((&raw const (*table.0).Table).cast(), count)
    };

    let mut received = 0_u64;
    let mut sent = 0_u64;
    for row in rows {
        let flags = row.InterfaceAndOperStatusFlags._bitfield;
        let counted = flags & HARDWARE_INTERFACE != 0
            && flags & FILTER_INTERFACE == 0
            && row.OperStatus == IfOperStatusUp;
        if counted {
            received = received.saturating_add(row.InOctets);
            sent = sent.saturating_add(row.OutOctets);
        }
    }
    Some((received, sent))
}
