//! GPU temperature, fan speed, power and VRAM size from the D3DKMT
//! display API in gdi32, the source Task Manager uses. Works for any GPU
//! whose driver reports it.

use std::collections::HashMap;

use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTER_PERFDATACAPS,
    D3DKMT_ADAPTERADDRESS, D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO,
    D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2, D3DKMT_QUERYADAPTERINFO,
    D3DKMT_SEGMENTSIZEINFO, D3DKMTCloseAdapter, D3DKMTEnumAdapters2,
    D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERADDRESS,
    KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERPERFDATA_CAPS,
    KMTQAITYPE_ADAPTERREGISTRYINFO, KMTQAITYPE_GETSEGMENTSIZE,
    KMTQUERYADAPTERINFOTYPE,
};

use super::super::struct_size;
use crate::sensors::luid_key;

/// D3DKMT reports temperature and power in tenths.
const TENTHS: f64 = 10.0;
/// Adapter LUIDs change after a driver reload, so cap the cache.
const MAX_CACHED_ADAPTERS: usize = 16;
/// Sorts adapters without a PCI address after real ones.
const PCI_UNKNOWN: (u32, u32, u32) = (u32::MAX, u32::MAX, u32::MAX);

/// One adapter's readings.
pub struct Adapter {
    /// LUID, formatted like GPU counter instance names.
    pub luid: String,
    /// PCI bus, device, function: a stable order matching nvidia-smi's.
    pub pci: (u32, u32, u32),
    /// Adapter name.
    pub name: Option<String>,
    /// Temperature, °C.
    pub temp_c: Option<f64>,
    /// Warning (throttle) temperature, °C.
    pub temp_max_c: Option<f64>,
    /// Fan speed, RPM.
    pub fan_rpm: Option<u32>,
    /// Highest fan speed, RPM.
    pub fan_max_rpm: Option<u32>,
    /// Power, % of the card's limit.
    pub power_percent: Option<f64>,
    /// Dedicated VRAM, bytes; 0 if unknown.
    pub vram_total_bytes: u64,
}

/// What never changes for an adapter, read once and cached.
#[derive(Clone)]
struct StaticInfo {
    /// Adapter name.
    name: Option<String>,
    /// PCI slot.
    pci: (u32, u32, u32),
    /// Dedicated VRAM, bytes.
    vram_total_bytes: u64,
    /// Whether the driver reports fan speed.
    has_fan: bool,
    /// Highest fan speed, RPM.
    fan_max_rpm: Option<u32>,
    /// Warning temperature, °C.
    temp_max_c: Option<f64>,
}

/// Reads every adapter's D3DKMT data.
pub struct GpuReader {
    /// Static info per LUID.
    cache: HashMap<String, StaticInfo>,
}

impl GpuReader {
    /// Creates a reader with an empty cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Every adapter with data, in PCI slot order. Software adapters
    /// (Microsoft Basic Render) are left out.
    pub fn read(&mut self) -> Vec<Adapter> {
        let adapters = enumerate();
        let mut found: Vec<Adapter> = adapters
            .iter()
            .filter_map(|info| self.read_adapter(info))
            .collect();
        for info in &adapters {
            close(info.hAdapter);
        }
        found.sort_by_key(|a| a.pci);
        found
    }

    /// Reads one adapter, or `None` if it reports nothing.
    fn read_adapter(&mut self, info: &D3DKMT_ADAPTERINFO) -> Option<Adapter> {
        let luid =
            luid_key(info.AdapterLuid.HighPart, info.AdapterLuid.LowPart);
        if !self.cache.contains_key(&luid)
            && self.cache.len() >= MAX_CACHED_ADAPTERS
        {
            self.cache.clear();
        }
        let info_static = self
            .cache
            .entry(luid.clone())
            .or_insert_with(|| read_static(info.hAdapter))
            .clone();

        let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
        // SAFETY: PERFDATA queries take a D3DKMT_ADAPTER_PERFDATA.
        let has_perf = unsafe {
            query(info.hAdapter, KMTQAITYPE_ADAPTERPERFDATA, &mut perf)
        };
        if !has_perf && info_static.vram_total_bytes == 0 {
            return None;
        }

        Some(Adapter {
            luid,
            pci: info_static.pci,
            name: info_static.name,
            temp_c: (has_perf && perf.Temperature != 0)
                .then(|| f64::from(perf.Temperature) / TENTHS),
            temp_max_c: info_static.temp_max_c,
            fan_rpm: (has_perf && info_static.has_fan).then_some(perf.FanRPM),
            fan_max_rpm: info_static.fan_max_rpm,
            power_percent: (has_perf && perf.Power != 0)
                .then(|| f64::from(perf.Power) / TENTHS),
            vram_total_bytes: info_static.vram_total_bytes,
        })
    }
}

/// Opens every adapter. Each returned handle must be [`close`]d.
fn enumerate() -> Vec<D3DKMT_ADAPTERINFO> {
    let mut request = D3DKMT_ENUMADAPTERS2::default();
    // SAFETY: with no array the call only writes the adapter count.
    if unsafe { D3DKMTEnumAdapters2(&raw mut request) }.is_err() {
        return Vec::new();
    }
    let count = usize::try_from(request.NumAdapters).unwrap_or(0);
    let mut adapters = vec![D3DKMT_ADAPTERINFO::default(); count];
    request.pAdapters = adapters.as_mut_ptr();
    // SAFETY: the array has room for the `NumAdapters` entries the call
    // may fill. It fails if an adapter appeared in between; the next
    // reading retries.
    if unsafe { D3DKMTEnumAdapters2(&raw mut request) }.is_err() {
        return Vec::new();
    }
    adapters.truncate(usize::try_from(request.NumAdapters).unwrap_or(0));
    adapters
}

/// Closes an adapter handle from [`enumerate`].
fn close(handle: u32) {
    let request = D3DKMT_CLOSEADAPTER { hAdapter: handle };
    // SAFETY: plain value; the handle came from `enumerate`. Nothing
    // useful can be done on failure.
    let _ = unsafe { D3DKMTCloseAdapter(&raw const request) };
}

/// Runs one adapter query into `out`. Returns whether it worked.
///
/// # Safety
///
/// `T` must be the struct that `kind` writes.
unsafe fn query<T>(
    handle: u32,
    kind: KMTQUERYADAPTERINFOTYPE,
    out: &mut T,
) -> bool {
    let mut request = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: kind,
        pPrivateDriverData: std::ptr::from_mut(out).cast(),
        PrivateDriverDataSize: struct_size::<T>(),
    };
    // SAFETY: the data pointer and size describe `out`, which outlives
    // the call, and the caller guarantees it's the right struct.
    unsafe { D3DKMTQueryAdapterInfo(&raw mut request) }.is_ok()
}

/// Reads the name, slot, VRAM size and fan/temperature limits.
fn read_static(handle: u32) -> StaticInfo {
    let mut registry = D3DKMT_ADAPTERREGISTRYINFO::default();
    let mut address = D3DKMT_ADAPTERADDRESS::default();
    let mut segments = D3DKMT_SEGMENTSIZEINFO::default();
    let mut caps = D3DKMT_ADAPTER_PERFDATACAPS::default();
    // SAFETY: each query kind is paired with the struct it writes.
    let (has_name, has_address, has_segments, has_caps) = unsafe {
        (
            query(handle, KMTQAITYPE_ADAPTERREGISTRYINFO, &mut registry),
            query(handle, KMTQAITYPE_ADAPTERADDRESS, &mut address),
            query(handle, KMTQAITYPE_GETSEGMENTSIZE, &mut segments),
            query(handle, KMTQAITYPE_ADAPTERPERFDATA_CAPS, &mut caps),
        )
    };
    StaticInfo {
        name: has_name.then(|| wide_to_string(&registry.AdapterString)),
        pci: if has_address {
            (
                address.BusNumber,
                address.DeviceNumber,
                address.FunctionNumber,
            )
        } else {
            PCI_UNKNOWN
        },
        vram_total_bytes: if has_segments {
            segments.DedicatedVideoMemorySize
        } else {
            0
        },
        has_fan: has_caps && caps.MaxFanRPM > 0,
        fan_max_rpm: (has_caps && caps.MaxFanRPM > 0).then_some(caps.MaxFanRPM),
        // Where the card starts to throttle (83 °C on recent NVIDIA cards).
        temp_max_c: (has_caps && caps.TemperatureWarning > 0)
            .then(|| f64::from(caps.TemperatureWarning) / TENTHS),
    }
}

/// A null-terminated UTF-16 buffer as a trimmed string.
fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(wide.get(..end).unwrap_or_default())
        .trim()
        .to_owned()
}
