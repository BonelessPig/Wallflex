//! Gathers raw sensor data from built-in Windows sources. Nothing here
//! needs admin rights, a kernel driver or extra software. The maths lives
//! in `crate::sensors`.

mod disk;
mod gpu;
mod net;
mod pdh;

use windows::Win32::System::Performance::{PDH_FMT_DOUBLE, PDH_FMT_LARGE};
use windows::Win32::System::SystemInformation::{
    GetSystemFirmwareTable, GlobalMemoryStatusEx, MEMORYSTATUSEX, RSMB,
};

use super::struct_size;
use crate::sensors::{
    Cpu, Gpu, Memory, Snapshot, cpu_clock_ghz, gpu_usage_by_luid,
    memory_modules, split_processors, vram_by_luid,
};
use pdh::Counter;

/// Everything needed to take readings. Create once and call
/// [`Sensors::sample`] about once a second from one thread, with
/// [`Sensors::sample_quick`] in between for faster CPU and network
/// updates; rates are measured between calls.
pub struct Sensors {
    /// Processor counters, if PDH is available.
    cpu_counters: Option<pdh::Query>,
    /// GPU counters, if PDH is available.
    gpu_counters: Option<pdh::Query>,
    /// The last full reading's GPUs, RAM and SSDs, reused by quick
    /// readings.
    last: Snapshot,
    /// GPU temperature, fan, power and VRAM size.
    gpus: gpu::GpuReader,
    /// Network byte counters.
    net: net::NetReader,
    /// SSD temperatures.
    disks: disk::DiskReader,
    /// Memory sticks installed (read once; 0 when unknown).
    modules: usize,
}

impl Sensors {
    /// Opens the sources. A source that's unavailable just gives
    /// missing values later.
    pub fn new() -> Self {
        Self {
            cpu_counters: pdh::Query::open(&Counter::CPU),
            gpu_counters: pdh::Query::open(&Counter::GPU),
            last: Snapshot::default(),
            gpus: gpu::GpuReader::new(),
            net: net::NetReader::default(),
            disks: disk::DiskReader::default(),
            modules: installed_modules(),
        }
    }

    /// Takes one reading of everything.
    pub fn sample(&mut self) -> Snapshot {
        let gpu_counters = self
            .gpu_counters
            .as_mut()
            .and_then(|query| query.collect().then_some(query));
        let (engines, vram) = match gpu_counters {
            Some(query) => (
                query.array(Counter::GpuEngine, PDH_FMT_DOUBLE),
                query.array(Counter::GpuMemory, PDH_FMT_LARGE),
            ),
            None => (Vec::new(), Vec::new()),
        };
        self.last.memory = memory(self.modules);
        self.last.gpus = self.gpus(&engines, &vram);
        self.last.disks = self.disks.read();
        self.sample_quick()
    }

    /// Takes a quick reading: the CPU and network now, the GPUs, RAM and
    /// SSDs as of the last [`Sensors::sample`].
    pub fn sample_quick(&mut self) -> Snapshot {
        Snapshot {
            cpu: self.cpu(),
            network: self.net.read(),
            ..self.last.clone()
        }
    }

    /// Processor use and clock from the CPU counters.
    fn cpu(&mut self) -> Cpu {
        let Some(query) = self
            .cpu_counters
            .as_mut()
            .and_then(|query| query.collect().then_some(query))
        else {
            return Cpu::default();
        };
        let (total, per_core) =
            split_processors(query.array(Counter::CpuCores, PDH_FMT_DOUBLE));
        let clock_ghz = query
            .value(Counter::CpuFrequency)
            .zip(query.value(Counter::CpuPerformance))
            .and_then(|(mhz, percent)| cpu_clock_ghz(mhz, percent));
        Cpu {
            total,
            per_core,
            clock_ghz,
        }
    }

    /// Combines D3DKMT adapter data with the GPU counters.
    fn gpus(
        &mut self,
        engines: &[(String, f64)],
        vram: &[(String, f64)],
    ) -> Vec<Gpu> {
        // Empty until the rate counter has its second sample.
        let has_usage = !engines.is_empty();
        let usage =
            gpu_usage_by_luid(engines.iter().map(|(n, v)| (n.as_str(), *v)));
        let used = vram_by_luid(vram.iter().filter_map(|(name, bytes)| {
            Some((name.as_str(), pdh::whole_bytes(*bytes)?))
        }));

        self.gpus
            .read()
            .into_iter()
            .map(|adapter| Gpu {
                usage: has_usage
                    .then(|| usage.get(&adapter.luid).copied().unwrap_or(0.0)),
                vram_used_bytes: used.get(&adapter.luid).copied(),
                vram_total_bytes: (adapter.vram_total_bytes > 0)
                    .then_some(adapter.vram_total_bytes),
                name: adapter.name.unwrap_or_else(|| "GPU".to_owned()),
                temp_c: adapter.temp_c,
                temp_max_c: adapter.temp_max_c,
                fan_rpm: adapter.fan_rpm,
                fan_max_rpm: adapter.fan_max_rpm,
                power_percent: adapter.power_percent,
            })
            .collect()
    }
}

/// RAM in use: installed minus available, as Task Manager shows it.
fn memory(modules: usize) -> Option<Memory> {
    let mut status = MEMORYSTATUSEX {
        dwLength: struct_size::<MEMORYSTATUSEX>(),
        ..Default::default()
    };
    // SAFETY: `status` is a valid MEMORYSTATUSEX with its size set.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }.ok()?;
    Some(Memory {
        used_bytes: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        total_bytes: status.ullTotalPhys,
        modules,
    })
}

/// Memory sticks installed, from the SMBIOS table (no admin needed).
/// 0 when Windows can't provide it.
fn installed_modules() -> usize {
    // SAFETY: no buffer, so it only returns the size needed.
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, None) };
    let Ok(size) = usize::try_from(size) else {
        return 0;
    };
    if size == 0 {
        return 0;
    }
    let mut table = vec![0_u8; size];
    // SAFETY: `table` is a writable buffer of the size asked for.
    let written =
        unsafe { GetSystemFirmwareTable(RSMB, 0, Some(table.as_mut_slice())) };
    match usize::try_from(written) {
        Ok(written) if written > 0 && written <= size => {
            memory_modules(table.get(..written).unwrap_or_default())
        }
        _ => 0,
    }
}
