//! The few Mach kernel calls the sensors need, declared here rather
//! than taken from `libc` (which marks its Mach items deprecated in
//! favour of a separate crate). They're in `libSystem`, which every
//! macOS program links.

/// A Mach port name.
pub type Port = u32;
/// A Mach call's result; [`KERN_SUCCESS`] when it worked.
pub type KernReturn = i32;

/// The call worked.
pub const KERN_SUCCESS: KernReturn = 0;
/// `host_processor_info` flavour: per-processor tick counts.
pub const PROCESSOR_CPU_LOAD_INFO: i32 = 2;
/// `host_statistics64` flavour: virtual memory counts.
pub const HOST_VM_INFO64: i32 = 4;
/// Ticks per processor in a `PROCESSOR_CPU_LOAD_INFO` reading
/// (`CPU_STATE_MAX`).
pub const CPU_STATES: usize = 4;

/// `vm_statistics64` from `<mach/vm_statistics.h>`: page counts, in
/// pages of `hw.pagesize` bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VmStatistics64 {
    /// Free pages.
    pub free_count: u32,
    /// Active pages.
    pub active_count: u32,
    /// Inactive pages.
    pub inactive_count: u32,
    /// Pages wired down (can't be paged out).
    pub wire_count: u32,
    /// Zero-filled pages.
    pub zero_fill_count: u64,
    /// Pages reactivated.
    pub reactivations: u64,
    /// Page-ins.
    pub pageins: u64,
    /// Page-outs.
    pub pageouts: u64,
    /// Faults.
    pub faults: u64,
    /// Copy-on-write faults.
    pub cow_faults: u64,
    /// Object cache lookups.
    pub lookups: u64,
    /// Object cache hits.
    pub hits: u64,
    /// Purges.
    pub purges: u64,
    /// Purgeable pages.
    pub purgeable_count: u32,
    /// Speculative pages.
    pub speculative_count: u32,
    /// Decompressions.
    pub decompressions: u64,
    /// Compressions.
    pub compressions: u64,
    /// Swap-ins.
    pub swapins: u64,
    /// Swap-outs.
    pub swapouts: u64,
    /// Pages the compressor uses.
    pub compressor_page_count: u32,
    /// Throttled pages.
    pub throttled_count: u32,
    /// File-backed pages.
    pub external_page_count: u32,
    /// Anonymous (app) pages.
    pub internal_page_count: u32,
    /// Pages held compressed, before compression.
    pub total_uncompressed_pages_in_compressor: u64,
}

/// [`VmStatistics64`]'s size in 32-bit words (`HOST_VM_INFO64_COUNT`).
pub const HOST_VM_INFO64_COUNT: u32 = 38;

// SAFETY: these match the declarations in the macOS SDK's
// <mach/mach_host.h>, <mach/mach_init.h> and <mach/vm_map.h>.
unsafe extern "C" {
    /// The current task's port; read through [`task_self`].
    static mach_task_self_: Port;
    /// A send right to this host's port.
    pub safe fn mach_host_self() -> Port;
    /// Per-processor information of the given flavour, in memory the
    /// caller frees with `vm_deallocate`.
    pub fn host_processor_info(
        host: Port,
        flavor: i32,
        processor_count: *mut u32,
        info: *mut *mut i32,
        info_count: *mut u32,
    ) -> KernReturn;
    /// Host statistics of the given flavour, into the caller's buffer.
    pub fn host_statistics64(
        host: Port,
        flavor: i32,
        info: *mut i32,
        info_count: *mut u32,
    ) -> KernReturn;
    /// Frees memory a Mach call returned.
    pub fn vm_deallocate(task: Port, address: usize, size: usize)
    -> KernReturn;
    /// Gives up a port right.
    pub fn mach_port_deallocate(task: Port, name: Port) -> KernReturn;
}

/// The current task's port.
pub fn task_self() -> Port {
    // SAFETY: a plain value the C runtime sets before `main` and never
    // changes.
    unsafe { mach_task_self_ }
}

#[cfg(test)]
mod tests {
    use super::{HOST_VM_INFO64_COUNT, VmStatistics64};

    #[test]
    fn vm_statistics_matches_the_sdk() {
        let words = size_of::<VmStatistics64>()
            .checked_div(size_of::<u32>())
            .unwrap();
        assert_eq!(u32::try_from(words).unwrap(), HOST_VM_INFO64_COUNT);
    }
}
