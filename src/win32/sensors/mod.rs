//! Gathers raw sensor data from built-in Windows sources. Nothing here
//! needs admin rights, a kernel driver or extra software. The maths lives
//! in `crate::sensors`.

mod disk;
mod gpu;
mod net;
mod pdh;

use windows::Win32::System::Performance::{PDH_FMT_DOUBLE, PDH_FMT_LARGE};
use windows::Win32::System::SystemInformation::{
    GlobalMemoryStatusEx, MEMORYSTATUSEX,
};

use super::struct_size;
use crate::sensors::{
    Cpu, Gpu, Memory, Snapshot, cpu_clock_ghz, gpu_usage_by_luid,
    split_processors, vram_by_luid,
};
use pdh::Counter;

/// Everything needed to take readings. Create once and call
/// [`Sensors::sample`] about once a second from one thread; rates are
/// measured between calls.
pub struct Sensors {
    /// Performance counters, if PDH is available.
    counters: Option<pdh::Query>,
    /// GPU temperature, fan, power and VRAM size.
    gpus: gpu::GpuReader,
    /// Network byte counters.
    net: net::NetReader,
    /// SSD temperatures.
    disks: disk::DiskReader,
}

impl Sensors {
    /// Opens the sources. A source that's unavailable just gives
    /// missing values later.
    pub fn new() -> Self {
        Self {
            counters: pdh::Query::open(),
            gpus: gpu::GpuReader::new(),
            net: net::NetReader::default(),
            disks: disk::DiskReader::default(),
        }
    }

    /// Takes one reading of everything.
    pub fn sample(&mut self) -> Snapshot {
        let counters = self
            .counters
            .as_mut()
            .and_then(|query| query.collect().then_some(query));
        let (cpu, engines, vram) = match counters {
            Some(query) => {
                let (total, per_core) = split_processors(
                    query.array(Counter::CpuCores, PDH_FMT_DOUBLE),
                );
                let clock_ghz = query
                    .value(Counter::CpuFrequency)
                    .zip(query.value(Counter::CpuPerformance))
                    .and_then(|(mhz, percent)| cpu_clock_ghz(mhz, percent));
                let cpu = Cpu {
                    total,
                    per_core,
                    clock_ghz,
                };
                let engines = query.array(Counter::GpuEngine, PDH_FMT_DOUBLE);
                let vram = query.array(Counter::GpuMemory, PDH_FMT_LARGE);
                (cpu, engines, vram)
            }
            None => (Cpu::default(), Vec::new(), Vec::new()),
        };

        Snapshot {
            cpu,
            memory: memory(),
            network: self.net.read(),
            gpus: self.gpus(&engines, &vram),
            disks: self.disks.read(),
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
fn memory() -> Option<Memory> {
    let mut status = MEMORYSTATUSEX {
        dwLength: struct_size::<MEMORYSTATUSEX>(),
        ..Default::default()
    };
    // SAFETY: `status` is a valid MEMORYSTATUSEX with its size set.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }.ok()?;
    Some(Memory {
        used_bytes: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        total_bytes: status.ullTotalPhys,
    })
}
