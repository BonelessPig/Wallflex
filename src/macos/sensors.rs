//! Readings on macOS, from public calls that need no permission: the
//! Mach host's tick counts and memory statistics, `sysctl`, and the
//! interface counters from `getifaddrs`. GPUs and SSD temperatures
//! aren't read yet (`docs/PORTING.md` says where they'd come from).

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::time::Instant;

use super::mach::{
    CPU_STATES, HOST_VM_INFO64, HOST_VM_INFO64_COUNT, KERN_SUCCESS,
    PROCESSOR_CPU_LOAD_INFO, Port, VmStatistics64, host_processor_info,
    host_statistics64, mach_host_self, mach_port_deallocate, task_self,
    vm_deallocate,
};
use crate::sensors::counters::{
    CpuTicks, cores_by_threads, tick_loads, wrapped_delta,
};
use crate::sensors::{Cpu, Memory, Network, Snapshot, rate_kbps, u64_to_f64};

/// Bits per second in a kilobit per second.
const BITS_PER_KILOBIT: f64 = 1_000.0;
/// Only interfaces named like this are counted: Ethernet and Wi-Fi.
/// The rest (bridges, VPN tunnels, `AirDrop`'s `awdl`) carry traffic
/// that's also counted on these, or that never leaves the Mac.
const COUNTED_PREFIX: &str = "en";

/// Everything needed to take readings. Create once and call
/// [`Sensors::sample`] about once a second; rates are measured between
/// calls.
pub struct Sensors {
    /// This host's port.
    host: Port,
    /// The last tick counts, per logical processor.
    ticks: Vec<CpuTicks>,
    /// Each physical core's logical processors (read once).
    cores: Vec<Vec<usize>>,
    /// The page size the memory statistics count in.
    page_bytes: u64,
    /// Installed memory.
    memory_bytes: u64,
    /// Network byte counters.
    net: NetReader,
}

impl Sensors {
    /// Opens the sources. One that's unavailable just gives missing
    /// values later.
    pub fn new() -> Self {
        let count = |name: &CStr| {
            sysctl_number(name).and_then(|n| usize::try_from(n).ok())
        };
        let cores = match (count(c"hw.logicalcpu"), count(c"hw.physicalcpu")) {
            (Some(logical), Some(physical)) => {
                cores_by_threads(logical, physical)
            }
            _ => Vec::new(),
        };
        Self {
            host: mach_host_self(),
            ticks: Vec::new(),
            cores,
            page_bytes: sysctl_number(c"hw.pagesize").unwrap_or(0),
            memory_bytes: sysctl_number(c"hw.memsize").unwrap_or(0),
            net: NetReader::default(),
        }
    }

    /// Takes one reading of everything.
    pub fn sample(&mut self) -> Snapshot {
        Snapshot {
            cpu: self.cpu(),
            memory: self.memory(),
            network: self.net.read(),
            ..Snapshot::default()
        }
    }

    /// Processor use since the last reading. Empty on the first.
    fn cpu(&mut self) -> Cpu {
        let now = processor_ticks(self.host).unwrap_or_default();
        let (per_core, total) = tick_loads(&self.ticks, &now);
        self.ticks = now;
        Cpu {
            total,
            per_core,
            cores: self.cores.clone(),
            clock_ghz: None,
        }
    }

    /// RAM in use, counted as Activity Monitor's "Memory Used": app
    /// memory (anonymous pages that can't be purged), wired memory and
    /// what the compressor holds.
    fn memory(&self) -> Option<Memory> {
        if self.memory_bytes == 0 || self.page_bytes == 0 {
            return None;
        }
        let vm = vm_statistics(self.host)?;
        let pages = u64::from(vm.internal_page_count)
            .saturating_sub(u64::from(vm.purgeable_count))
            .saturating_add(u64::from(vm.wire_count))
            .saturating_add(u64::from(vm.compressor_page_count));
        Some(Memory {
            used_bytes: pages
                .saturating_mul(self.page_bytes)
                .min(self.memory_bytes),
            total_bytes: self.memory_bytes,
            modules: 0,
        })
    }
}

impl Drop for Sensors {
    fn drop(&mut self) {
        // SAFETY: gives back the send right `mach_host_self` made in
        // `new`; nothing uses `self.host` after this. Failing would
        // only leak the right, so the result is ignored.
        let _ = unsafe { mach_port_deallocate(task_self(), self.host) };
    }
}

/// Every logical processor's tick counts, in processor order.
fn processor_ticks(host: Port) -> Option<Vec<CpuTicks>> {
    let mut processors = 0_u32;
    let mut info: *mut i32 = std::ptr::null_mut();
    let mut words = 0_u32;
    // SAFETY: all three out-parameters are valid locals. On success the
    // kernel puts `words` 32-bit words at `info`, freed below.
    let result = unsafe {
        host_processor_info(
            host,
            PROCESSOR_CPU_LOAD_INFO,
            &raw mut processors,
            &raw mut info,
            &raw mut words,
        )
    };
    if result != KERN_SUCCESS || info.is_null() {
        return None;
    }
    let length = usize::try_from(words).ok();
    let ticks = length.map(|length| {
        // SAFETY: the kernel just returned `length` words at `info`,
        // valid until the `vm_deallocate` below; the slice doesn't
        // outlive this closure. Tick counts are unsigned, so reading
        // them as u32 is what the SDK's `cpu_tick` arrays do.
        let words =
            unsafe { std::slice::from_raw_parts(info.cast::<u32>(), length) };
        let (processors, _rest) = words.as_chunks::<CPU_STATES>();
        processors.to_vec()
    });
    let bytes = length.and_then(|n| n.checked_mul(size_of::<i32>()));
    // SAFETY: frees exactly the memory the call above returned; nothing
    // refers to it any more. Failing would only leak it, so the result
    // is ignored.
    let _ = unsafe {
        vm_deallocate(task_self(), info.expose_provenance(), bytes.unwrap_or(0))
    };
    ticks
}

/// The host's virtual memory counts.
fn vm_statistics(host: Port) -> Option<VmStatistics64> {
    let mut vm = VmStatistics64::default();
    let mut count = HOST_VM_INFO64_COUNT;
    // SAFETY: `vm` is a `vm_statistics64` laid out as the SDK's (its
    // size is checked by a test), and `count` says how many words it
    // holds, so the kernel writes no further.
    let result = unsafe {
        host_statistics64(
            host,
            HOST_VM_INFO64,
            (&raw mut vm).cast(),
            &raw mut count,
        )
    };
    (result == KERN_SUCCESS).then_some(vm)
}

/// A numeric `sysctl` value (32 or 64 bits).
fn sysctl_number(name: &CStr) -> Option<u64> {
    let mut buffer = [0_u8; size_of::<u64>()];
    let mut size = buffer.len();
    // SAFETY: `name` is null-terminated, and `size` tells the call how
    // many bytes `buffer` holds; it writes no more and says how many it
    // did. No new value is set.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    let bytes = buffer.get(..size)?;
    if let Ok(four) = <[u8; 4]>::try_from(bytes) {
        Some(u64::from(u32::from_ne_bytes(four)))
    } else {
        <[u8; 8]>::try_from(bytes).ok().map(u64::from_ne_bytes)
    }
}

/// One interface's counters.
#[derive(Clone, Copy, Debug)]
struct Counters {
    /// Bytes received (wraps at 4 GB).
    received: u32,
    /// Bytes sent (wraps at 4 GB).
    sent: u32,
    /// Link speed in bits per second.
    baud: u32,
}

/// Remembers the last byte counts so rates can be measured.
#[derive(Default)]
struct NetReader {
    /// Each counted interface's counters, and when they were read.
    last: Option<(BTreeMap<String, Counters>, Instant)>,
}

impl NetReader {
    /// Throughput since the previous call; `None` on the first call.
    fn read(&mut self) -> Option<Network> {
        let now_counters = interfaces()?;
        let now = Instant::now();
        let rates = self.last.as_ref().and_then(|(before, then)| {
            let (mut down, mut up) = (0_u64, 0_u64);
            for (name, new) in &now_counters {
                if let Some(old) = before.get(name) {
                    down = down.saturating_add(wrapped_delta(
                        old.received,
                        new.received,
                    ));
                    up = up.saturating_add(wrapped_delta(old.sent, new.sent));
                }
            }
            let elapsed = now.saturating_duration_since(*then);
            let link = now_counters.values().map(|c| c.baud).max().unwrap_or(0);
            Some(Network {
                down_kbps: rate_kbps(0, down, elapsed)?,
                up_kbps: rate_kbps(0, up, elapsed)?,
                link_kbps: (link > 0)
                    .then(|| u64_to_f64(u64::from(link)) / BITS_PER_KILOBIT),
            })
        });
        self.last = Some((now_counters, now));
        rates
    }
}

/// The counters of every counted interface that's up.
fn interfaces() -> Option<BTreeMap<String, Counters>> {
    let up = u32::try_from(libc::IFF_UP).ok()?;
    let link_family = u8::try_from(libc::AF_LINK).ok()?;
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is a valid out-parameter; on success it holds a
    // list freed by `freeifaddrs` below.
    if unsafe { libc::getifaddrs(&raw mut list) } != 0 {
        return None;
    }
    let mut found = BTreeMap::new();
    let mut entry = list;
    while !entry.is_null() {
        // SAFETY: `entry` is a node of the list `getifaddrs` returned,
        // which stays valid until `freeifaddrs`.
        let node = unsafe { &*entry };
        if let Some((name, counters)) = link_counters(node, up, link_family) {
            found.insert(name, counters);
        }
        entry = node.ifa_next;
    }
    // SAFETY: frees the list `getifaddrs` returned; no node is used
    // after this.
    unsafe { libc::freeifaddrs(list) };
    Some(found)
}

/// A list node's name and counters, if it's the link-level entry of a
/// counted interface that's up.
fn link_counters(
    node: &libc::ifaddrs,
    up: u32,
    link_family: u8,
) -> Option<(String, Counters)> {
    if node.ifa_flags & up == 0
        || node.ifa_addr.is_null()
        || node.ifa_data.is_null()
        || node.ifa_name.is_null()
    {
        return None;
    }
    // SAFETY: checked non-null; it points into the list, valid while the
    // caller holds it.
    let family = unsafe { (*node.ifa_addr).sa_family };
    if family != link_family {
        return None;
    }
    // SAFETY: checked non-null; a null-terminated name in the list.
    let name = unsafe { CStr::from_ptr(node.ifa_name) }
        .to_string_lossy()
        .into_owned();
    if !name.starts_with(COUNTED_PREFIX) {
        return None;
    }
    // SAFETY: for `AF_LINK` entries `ifa_data` points to the
    // interface's `if_data` (checked non-null above), valid while the
    // caller holds the list.
    let data = unsafe { &*node.ifa_data.cast::<libc::if_data>() };
    Some((
        name,
        Counters {
            received: data.ifi_ibytes,
            sent: data.ifi_obytes,
            baud: data.ifi_baudrate,
        },
    ))
}
