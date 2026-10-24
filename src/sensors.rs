//! Sensor readings, and the pure logic that turns raw Windows data into
//! them. The Win32 calls that gather the raw data live in
//! `win32::sensors`.

use std::collections::BTreeMap;
use std::time::Duration;

/// 100%, the cap for usage values.
pub const PERCENT_MAX: f64 = 100.0;
/// Bytes in a gigabyte (GiB, as Task Manager counts).
pub const BYTES_PER_GB: f64 = 1_073_741_824.0;
/// Megahertz in a gigahertz.
const MHZ_PER_GHZ: f64 = 1_000.0;
/// Bits in a byte.
const BITS_PER_BYTE: f64 = 8.0;
/// Bits in a kilobit.
const BITS_PER_KILOBIT: f64 = 1_000.0;
/// Name of the "all processors" instance in processor counters.
const TOTAL_INSTANCE: &str = "_Total";

/// One reading of every sensor. A missing value is `None`: either the
/// PC doesn't have that sensor, or it's a rate still waiting for its
/// second sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Processor use and clock.
    pub cpu: Cpu,
    /// RAM in use.
    pub memory: Option<Memory>,
    /// Network throughput.
    pub network: Option<Network>,
    /// One entry per GPU, in PCI slot order.
    pub gpus: Vec<Gpu>,
    /// SSD temperatures, in drive number order.
    pub disks: Vec<DiskTemp>,
}

/// Processor readings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cpu {
    /// Whole-CPU use, 0 to 100%.
    pub total: Option<f64>,
    /// Use per logical processor, 0 to 100%, in processor order.
    pub per_core: Vec<f64>,
    /// Which logical processors make up each physical core, in core
    /// order (from Windows; empty if unknown).
    pub cores: Vec<Vec<usize>>,
    /// Average current clock, in GHz.
    pub clock_ghz: Option<f64>,
}

/// RAM readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// Bytes in use.
    pub used_bytes: u64,
    /// Bytes installed and usable by Windows.
    pub total_bytes: u64,
    /// Memory sticks installed; 0 when unknown.
    pub modules: usize,
}

/// SMBIOS structure type of a memory slot ("Memory Device").
const SMBIOS_MEMORY_DEVICE: u8 = 17;
/// SMBIOS end-of-table structure type.
const SMBIOS_END: u8 = 127;
/// Bytes before the table in Windows' `RawSMBIOSData`: calling method,
/// major and minor version, DMI revision, then the table length (u32).
const RAW_SMBIOS_HEADER: usize = 8;
/// Where the table length sits in that header.
const RAW_SMBIOS_LENGTH_AT: usize = 4;
/// Where a memory device's size (u16) sits in its structure.
const MEMORY_DEVICE_SIZE_AT: usize = 0x0C;
/// Smallest valid structure: type, length and handle.
const SMBIOS_HEADER_LEN: usize = 4;

/// Counts installed memory sticks in Windows' raw SMBIOS data (from
/// `GetSystemFirmwareTable('RSMB')`): memory slots whose size isn't 0
/// (empty). Malformed data stops the count where it goes wrong.
pub fn memory_modules(raw: &[u8]) -> usize {
    let length = raw
        .get(RAW_SMBIOS_LENGTH_AT..RAW_SMBIOS_HEADER)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0);
    let table = raw
        .get(RAW_SMBIOS_HEADER..)
        .map(|t| t.get(..length).unwrap_or(t))
        .unwrap_or_default();
    let mut count = 0_usize;
    let mut at = 0_usize;
    while let Some(&[kind, len]) = table.get(at..at.saturating_add(2)) {
        let len = usize::from(len);
        if len < SMBIOS_HEADER_LEN || kind == SMBIOS_END {
            break;
        }
        if kind == SMBIOS_MEMORY_DEVICE && memory_device_installed(table, at) {
            count = count.saturating_add(1);
        }
        // The formatted part, then strings ending in a double zero.
        let strings = at.saturating_add(len);
        let Some(end) = table
            .get(strings..)
            .and_then(|rest| rest.windows(2).position(|w| w == [0, 0]))
        else {
            break;
        };
        at = strings.saturating_add(end).saturating_add(2);
    }
    count
}

/// Whether the memory device structure at `at` has a stick in it (its
/// size isn't 0).
fn memory_device_installed(table: &[u8], at: usize) -> bool {
    let from = at.saturating_add(MEMORY_DEVICE_SIZE_AT);
    table
        .get(from..from.saturating_add(2))
        .and_then(|b| <[u8; 2]>::try_from(b).ok())
        .is_some_and(|b| u16::from_le_bytes(b) != 0)
}

/// Network throughput, summed over the physical network adapters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Network {
    /// Download, in kilobits per second.
    pub down_kbps: f64,
    /// Upload, in kilobits per second.
    pub up_kbps: f64,
}

/// One GPU's readings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    /// Adapter name, as the driver reports it.
    pub name: String,
    /// Use of the busiest engine, 0 to 100% (Task Manager's method).
    pub usage: Option<f64>,
    /// Temperature, in °C.
    pub temp_c: Option<f64>,
    /// The driver's warning temperature, where throttling starts, in °C.
    pub temp_max_c: Option<f64>,
    /// Fan speed, in RPM.
    pub fan_rpm: Option<u32>,
    /// Highest fan speed, in RPM.
    pub fan_max_rpm: Option<u32>,
    /// Power draw as a percentage of the card's power limit.
    pub power_percent: Option<f64>,
    /// Dedicated video memory in use, in bytes.
    pub vram_used_bytes: Option<u64>,
    /// Dedicated video memory installed, in bytes.
    pub vram_total_bytes: Option<u64>,
}

/// One SSD's temperature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskTemp {
    /// Windows' physical drive number (`\\.\PhysicalDriveN`).
    pub disk: u32,
    /// The drive's product name, if it reports one.
    pub name: Option<String>,
    /// Overall (composite) temperature, in °C.
    pub temp_c: i16,
}

/// Converts a byte or tick count to `f64` for display maths.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "values shown to 1 decimal place; precision past 2^53 \
              doesn't matter"
)]
pub fn u64_to_f64(value: u64) -> f64 {
    value as f64
}

/// The average current CPU clock in GHz, from the nominal clock in MHz
/// and the "% Processor Performance" counter (which passes 100% when
/// boosting). `None` when the nominal clock is unknown.
pub fn cpu_clock_ghz(
    nominal_mhz: f64,
    performance_percent: f64,
) -> Option<f64> {
    (nominal_mhz > 0.0)
        .then(|| nominal_mhz * performance_percent / PERCENT_MAX / MHZ_PER_GHZ)
}

/// Puts `\Processor(*)` counter values into `loads` by processor
/// number, capped to 0..=100%, and returns the whole-CPU value.
/// Processors missing from this reading keep their last value, so a
/// skipped instance never shifts the others; `loads` only grows.
pub fn processor_loads<I>(items: I, loads: &mut Vec<f64>) -> Option<f64>
where
    I: IntoIterator<Item = (String, f64)>,
{
    let mut total = None;
    for (name, value) in items {
        let value = value.clamp(0.0, PERCENT_MAX);
        if name == TOTAL_INSTANCE {
            total = Some(value);
        } else if let Ok(index) = name.parse::<usize>() {
            if index >= loads.len() && index < MAX_PROCESSORS {
                loads.resize(index.saturating_add(1), 0.0);
            }
            if let Some(slot) = loads.get_mut(index) {
                *slot = value;
            }
        }
    }
    total
}

/// More logical processors than any PC this runs on has; guards
/// against a nonsense instance name.
const MAX_PROCESSORS: usize = 1_024;
/// `RelationProcessorCore` in `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`.
const RELATION_PROCESSOR_CORE: u32 = 0;
/// Offsets in a `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` core record:
/// its size, its group count, and its first `GROUP_AFFINITY`.
const RECORD_SIZE_AT: usize = 4;
/// See [`RECORD_SIZE_AT`].
const GROUP_COUNT_AT: usize = 30;
/// See [`RECORD_SIZE_AT`].
const GROUP_MASKS_AT: usize = 32;
/// Size of a `GROUP_AFFINITY`: mask (8 bytes), group (2), padding (6).
const GROUP_AFFINITY_SIZE: usize = 16;
/// Where the group number sits in a `GROUP_AFFINITY`.
const GROUP_NUMBER_AT: usize = 8;
/// Logical processors per processor group.
const PROCESSORS_PER_GROUP: usize = 64;

/// The logical processors of each physical core, from the raw output of
/// `GetLogicalProcessorInformationEx(RelationProcessorCore)`. Cores come
/// in order of their first processor; a core with two threads lists
/// two, a single-threaded (efficiency) core one.
pub fn cores_from_topology(raw: &[u8]) -> Vec<Vec<usize>> {
    let mut cores = Vec::new();
    let mut at = 0_usize;
    while let Some(size) = read_u32(raw, at.saturating_add(RECORD_SIZE_AT)) {
        let Ok(size) = usize::try_from(size) else {
            break;
        };
        if size == 0 {
            break;
        }
        if read_u32(raw, at) == Some(RELATION_PROCESSOR_CORE) {
            let threads = core_threads(raw, at);
            if !threads.is_empty() {
                cores.push(threads);
            }
        }
        at = at.saturating_add(size);
    }
    cores.sort_by_key(|threads: &Vec<usize>| threads.first().copied());
    cores
}

/// The logical processors in the core record at `at`.
fn core_threads(raw: &[u8], at: usize) -> Vec<usize> {
    let groups = read_u16(raw, at.saturating_add(GROUP_COUNT_AT)).unwrap_or(0);
    let mut threads = Vec::new();
    for group in 0..usize::from(groups) {
        let base = at
            .saturating_add(GROUP_MASKS_AT)
            .saturating_add(group.saturating_mul(GROUP_AFFINITY_SIZE));
        let mask = read_u64(raw, base).unwrap_or(0);
        let number = read_u16(raw, base.saturating_add(GROUP_NUMBER_AT))
            .map_or(0, usize::from);
        let first = number.saturating_mul(PROCESSORS_PER_GROUP);
        for bit in 0..PROCESSORS_PER_GROUP {
            if mask
                .checked_shr(u32::try_from(bit).unwrap_or(u32::MAX))
                .is_some_and(|m| m & 1 == 1)
            {
                threads.push(first.saturating_add(bit));
            }
        }
    }
    threads
}

/// A little-endian `u16` at `at`, if there.
fn read_u16(raw: &[u8], at: usize) -> Option<u16> {
    let bytes = raw.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

/// A little-endian `u32` at `at`, if there.
fn read_u32(raw: &[u8], at: usize) -> Option<u32> {
    let bytes = raw.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// A little-endian `u64` at `at`, if there.
fn read_u64(raw: &[u8], at: usize) -> Option<u64> {
    let bytes = raw.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

/// Throughput in kilobits per second between two readings of a byte
/// counter. `None` if the counter went backwards (an adapter was reset
/// or removed) or no time passed.
pub fn rate_kbps(before: u64, after: u64, elapsed: Duration) -> Option<f64> {
    let seconds = elapsed.as_secs_f64();
    let bytes = after.checked_sub(before)?;
    (seconds > 0.0)
        .then(|| u64_to_f64(bytes) * BITS_PER_BYTE / BITS_PER_KILOBIT / seconds)
}

/// Formats an adapter LUID the way GPU counter instance names do,
/// lowercased: `luid_0x00000000_0x0000d1a2`.
pub fn luid_key(high: i32, low: u32) -> String {
    format!("luid_0x{:08x}_0x{low:08x}", high.cast_unsigned())
}

/// Pulls the LUID out of a GPU counter instance name:
/// `pid_1_luid_0x0_0x1A_phys_0_eng_0_engtype_3D` → `luid_0x0_0x1a`.
pub fn luid_from_instance(instance: &str) -> Option<String> {
    let start = instance.find("luid_")?;
    let rest = instance.get(start..)?;
    let end = rest.find("_phys_")?;
    rest.get(..end).map(str::to_ascii_lowercase)
}

/// GPU use per adapter, by Task Manager's method: add up every process
/// per engine type, then take the busiest engine type. Takes
/// `(instance name, % used)` pairs from `\GPU Engine(*)\Utilization
/// Percentage`; returns % used keyed by [`luid_key`].
pub fn gpu_usage_by_luid<'a, I>(engines: I) -> BTreeMap<String, f64>
where
    I: IntoIterator<Item = (&'a str, f64)>,
{
    let mut per_engine: BTreeMap<(String, &str), f64> = BTreeMap::new();
    for (instance, value) in engines {
        let Some(luid) = luid_from_instance(instance) else {
            continue;
        };
        let engine = instance
            .rsplit_once("_engtype_")
            .map_or(instance, |(_, engine)| engine);
        *per_engine.entry((luid, engine)).or_insert(0.0) += value;
    }

    let mut usage: BTreeMap<String, f64> = BTreeMap::new();
    for ((luid, _), value) in per_engine {
        let busiest = usage.entry(luid).or_insert(0.0);
        *busiest = busiest.max(value.min(PERCENT_MAX));
    }
    usage
}

/// Adds up `(instance name, bytes)` pairs from `\GPU Adapter
/// Memory(*)\Dedicated Usage` per adapter, keyed by [`luid_key`].
pub fn vram_by_luid<'a, I>(instances: I) -> BTreeMap<String, u64>
where
    I: IntoIterator<Item = (&'a str, u64)>,
{
    let mut used: BTreeMap<String, u64> = BTreeMap::new();
    for (instance, bytes) in instances {
        if let Some(luid) = luid_from_instance(instance) {
            let total = used.entry(luid).or_insert(0);
            *total = total.saturating_add(bytes);
        }
    }
    used
}

/// Byte offset of the "incurs seek penalty" flag in a
/// `DEVICE_SEEK_PENALTY_DESCRIPTOR`.
const SEEK_PENALTY_FLAG_POS: usize = 8;
/// Byte offset of the sensor count in a
/// `STORAGE_TEMPERATURE_DATA_DESCRIPTOR`.
const TEMP_COUNT_POS: usize = 12;
/// Byte offset of the first sensor's temperature in a
/// `STORAGE_TEMPERATURE_DATA_DESCRIPTOR` (first entry at 24, its
/// temperature 2 bytes in).
const FIRST_TEMP_POS: usize = 26;
/// Byte offset of the product name's offset in a
/// `STORAGE_DEVICE_DESCRIPTOR`.
const PRODUCT_ID_OFFSET_POS: usize = 16;

/// Reads `N` bytes at `pos`, or `None` if the buffer is too short.
fn bytes_at<const N: usize>(raw: &[u8], pos: usize) -> Option<[u8; N]> {
    raw.get(pos..pos.checked_add(N)?)?.try_into().ok()
}

/// Whether a drive has a seek penalty (it's a spinning hard drive), from
/// its `DEVICE_SEEK_PENALTY_DESCRIPTOR` bytes.
pub fn has_seek_penalty(raw: &[u8]) -> Option<bool> {
    bytes_at::<1>(raw, SEEK_PENALTY_FLAG_POS).map(|[flag]| flag != 0)
}

/// A drive's overall temperature in °C, from its
/// `STORAGE_TEMPERATURE_DATA_DESCRIPTOR` bytes. The first sensor is the
/// composite one.
pub fn composite_temperature(raw: &[u8]) -> Option<i16> {
    let count = u16::from_le_bytes(bytes_at(raw, TEMP_COUNT_POS)?);
    if count == 0 {
        return None;
    }
    Some(i16::from_le_bytes(bytes_at(raw, FIRST_TEMP_POS)?))
}

/// A drive's product name, from its `STORAGE_DEVICE_DESCRIPTOR` bytes.
pub fn product_name(raw: &[u8]) -> Option<String> {
    let offset = u32::from_le_bytes(bytes_at(raw, PRODUCT_ID_OFFSET_POS)?);
    let offset = usize::try_from(offset).ok().filter(|&o| o > 0)?;
    let tail = raw.get(offset..)?;
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    let name = String::from_utf8_lossy(tail.get(..end)?).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        composite_temperature, cores_from_topology, cpu_clock_ghz,
        gpu_usage_by_luid, has_seek_penalty, luid_from_instance, luid_key,
        memory_modules, processor_loads, product_name, rate_kbps, vram_by_luid,
    };

    /// A memory device structure (type 17) with `size`, and one string.
    fn memory_device(size: u16) -> Vec<u8> {
        let mut s = vec![17, 0x15, 0, 0];
        s.resize(0x15, 0);
        s[0x0C..0x0E].copy_from_slice(&size.to_le_bytes());
        s.extend_from_slice(b"DIMM\0\0");
        s
    }

    /// `RawSMBIOSData` around `table`.
    fn raw(table: &[u8]) -> Vec<u8> {
        let mut raw = vec![0, 3, 4, 0];
        let len = u32::try_from(table.len()).unwrap();
        raw.extend_from_slice(&len.to_le_bytes());
        raw.extend_from_slice(table);
        raw
    }

    #[test]
    fn counts_installed_sticks() {
        // A BIOS structure (no strings), two sticks, an empty slot, one
        // more stick, then the end marker.
        let mut table = vec![0, 4, 0, 0, 0, 0];
        for size in [8_192, 8_192, 0, 0x7FFF] {
            table.extend(memory_device(size));
        }
        table.extend_from_slice(&[127, 4, 0, 0, 0, 0]);
        assert_eq!(memory_modules(&raw(&table)), 3);
    }

    #[test]
    fn bad_smbios_data_counts_what_it_can() {
        assert_eq!(memory_modules(&[]), 0);
        let mut table = memory_device(4_096);
        table.extend_from_slice(&[17, 2]); // too short to be a structure
        assert_eq!(memory_modules(&raw(&table)), 1);
        // Strings that never end.
        let mut cut = memory_device(4_096);
        cut.truncate(cut.len() - 2);
        assert_eq!(memory_modules(&raw(&cut)), 1);
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn clock_from_nominal_and_performance() {
        let ghz = cpu_clock_ghz(3_600.0, 120.0);
        assert!(ghz.is_some_and(|g| close(g, 4.32)));
        assert_eq!(cpu_clock_ghz(0.0, 120.0), None);
    }

    #[test]
    fn processors_sorted_numerically_and_capped() {
        let items = [("10", 5.0), ("_Total", 40.0), ("2", 150.0), ("1", -1.0)]
            .map(|(n, v)| (n.to_owned(), v));
        let mut loads = Vec::new();
        let total = processor_loads(items, &mut loads);
        assert_eq!(total, Some(40.0));
        assert_eq!(loads[..3], [0.0, 0.0, 100.0]);
        assert_eq!(loads.len(), 11);
        assert!(close(loads[10], 5.0));
        // A reading missing processor 10 leaves it where it was.
        let next = [("2", 30.0)].map(|(n, v)| (n.to_owned(), v));
        let _ = processor_loads(next, &mut loads);
        assert!(close(loads[10], 5.0) && close(loads[2], 30.0));
        assert_eq!(loads.len(), 11);
    }

    /// A core record with `masks` (group, mask) as Windows lays it out:
    /// relationship and size, flags and padding to byte 30, the group
    /// count, then 16 bytes per group (mask, group number, padding).
    fn core_record(masks: &[(u16, u64)]) -> Vec<u8> {
        let mut groups = Vec::new();
        for (group, mask) in masks {
            groups.extend_from_slice(&mask.to_le_bytes());
            groups.extend_from_slice(&group.to_le_bytes());
            groups.extend_from_slice(&[0; 6]);
        }
        let size =
            u32::try_from(groups.len().checked_add(32).unwrap()).unwrap();
        let mut r = vec![0_u8; 4];
        r.extend_from_slice(&size.to_le_bytes());
        r.extend_from_slice(&[0; 22]);
        r.extend_from_slice(&u16::try_from(masks.len()).unwrap().to_le_bytes());
        r.extend(groups);
        r
    }

    #[test]
    fn reads_the_core_layout() {
        // Two two-thread cores (given out of order) and one efficiency
        // core with a single thread.
        let mut raw = core_record(&[(0, 0b1100)]);
        raw.extend(core_record(&[(0, 0b0011)]));
        raw.extend(core_record(&[(0, 0b1_0000)]));
        assert_eq!(
            cores_from_topology(&raw),
            vec![vec![0, 1], vec![2, 3], vec![4]]
        );
        let none: Vec<Vec<usize>> = Vec::new();
        assert_eq!(cores_from_topology(&[]), none);
        assert_eq!(cores_from_topology(&raw[..20]), none);
    }

    #[test]
    fn network_rate() {
        // 125,000 bytes in 1 s = 1,000 kbit/s.
        let rate = rate_kbps(1_000, 126_000, Duration::from_secs(1));
        assert!(rate.is_some_and(|r| close(r, 1_000.0)));
        assert_eq!(rate_kbps(500, 100, Duration::from_secs(1)), None);
        assert_eq!(rate_kbps(0, 100, Duration::ZERO), None);
    }

    #[test]
    fn luid_formats_and_parses() {
        assert_eq!(luid_key(0, 0xD1A2), "luid_0x00000000_0x0000d1a2");
        assert_eq!(luid_key(-1, 1), "luid_0xffffffff_0x00000001");
        assert_eq!(
            luid_from_instance(
                "pid_1_luid_0x00000000_0x0000D1A2_phys_0_eng_0_engtype_3D"
            ),
            Some("luid_0x00000000_0x0000d1a2".to_owned())
        );
        assert_eq!(luid_from_instance("nothing here"), None);
    }

    #[test]
    fn gpu_usage_takes_busiest_engine_type() {
        let luid = "luid_0x00000000_0x00000001";
        let engines = [
            (format!("pid_1_{luid}_phys_0_eng_0_engtype_3D"), 20.0),
            (format!("pid_2_{luid}_phys_0_eng_0_engtype_3D"), 15.0),
            (format!("pid_1_{luid}_phys_0_eng_3_engtype_Copy"), 30.0),
            (format!("pid_3_{luid}_phys_0_eng_5_engtype_Video"), 90.0),
            (format!("pid_4_{luid}_phys_0_eng_5_engtype_Video"), 90.0),
        ];
        let usage =
            gpu_usage_by_luid(engines.iter().map(|(n, v)| (n.as_str(), *v)));
        // Video: 180% summed, capped at 100.
        assert_eq!(usage.get(luid), Some(&100.0));

        let usage = gpu_usage_by_luid(
            engines.iter().take(3).map(|(n, v)| (n.as_str(), *v)),
        );
        // 3D: 20 + 15 = 35 beats Copy's 30.
        assert_eq!(usage.get(luid), Some(&35.0));
    }

    #[test]
    fn vram_summed_per_adapter() {
        let used = vram_by_luid([
            ("luid_0x0_0x1_phys_0_part_0", 100),
            ("luid_0x0_0x1_phys_0_part_1", 50),
            ("luid_0x0_0x2_phys_0_part_0", 7),
        ]);
        assert_eq!(used.get("luid_0x0_0x1"), Some(&150));
        assert_eq!(used.get("luid_0x0_0x2"), Some(&7));
    }

    #[test]
    fn storage_descriptors() {
        // Flag byte at offset 8.
        let ssd = [0_u8; 12];
        let hdd = [[0_u8; 8].as_slice(), &[1, 0, 0, 0]].concat();
        assert_eq!(has_seek_penalty(&ssd), Some(false));
        assert_eq!(has_seek_penalty(&hdd), Some(true));
        assert_eq!(has_seek_penalty(&[0; 4]), None);

        // Sensor count at 12..14, first temperature at 26..28.
        assert_eq!(composite_temperature(&[0; 40]), None);
        let temps = [
            [0_u8; 12].as_slice(),
            &1_u16.to_le_bytes(),
            &[0; 12],
            &38_i16.to_le_bytes(),
            &[0; 12],
        ]
        .concat();
        assert_eq!(composite_temperature(&temps), Some(38));

        // Name offset at 16..20, pointing past the 24-byte header.
        let device = [
            [0_u8; 16].as_slice(),
            &24_u32.to_le_bytes(),
            &[0; 4],
            b" WD_BLACK SN850X \0junk",
        ]
        .concat();
        assert_eq!(product_name(&device), Some("WD_BLACK SN850X".to_owned()));
        assert_eq!(product_name(&[0; 8]), None);
    }
}
