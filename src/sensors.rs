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

/// Splits `\Processor(*)` counter values into the total and the
/// per-processor values in processor order. Values are capped to
/// 0..=100%.
pub fn split_processors<I>(items: I) -> (Option<f64>, Vec<f64>)
where
    I: IntoIterator<Item = (String, f64)>,
{
    let mut total = None;
    let mut cores: Vec<(u32, f64)> = Vec::new();
    for (name, value) in items {
        let value = value.clamp(0.0, PERCENT_MAX);
        if name == TOTAL_INSTANCE {
            total = Some(value);
        } else if let Ok(index) = name.parse::<u32>() {
            cores.push((index, value));
        }
    }
    cores.sort_by_key(|&(index, _)| index);
    (total, cores.into_iter().map(|(_, value)| value).collect())
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
        composite_temperature, cpu_clock_ghz, gpu_usage_by_luid,
        has_seek_penalty, luid_from_instance, luid_key, product_name,
        rate_kbps, split_processors, vram_by_luid,
    };

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
        let (total, cores) = split_processors(items);
        assert_eq!(total, Some(40.0));
        assert_eq!(cores, vec![0.0, 100.0, 5.0]);
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
