//! The plain-text sensor panel: turns a [`Snapshot`] into lines.

use std::fmt::Write as _;

use crate::sensors::{BYTES_PER_GB, PERCENT_MAX, Snapshot, u64_to_f64};

/// Shown where a value is missing.
const MISSING: &str = "--";
/// Kilobits per second in a megabit per second.
const KBPS_PER_MBPS: f64 = 1_000.0;

/// The panel's lines for one snapshot.
pub fn lines(snapshot: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    let cpu = &snapshot.cpu;
    out.push(format!(
        "CPU   {:>4}  {} GHz",
        percent(cpu.total),
        decimal(cpu.clock_ghz, 2)
    ));
    let mut cores = String::from("CORES");
    for value in &cpu.per_core {
        let _ = write!(cores, " {value:>3.0}");
    }
    out.push(cores);

    out.push(match snapshot.memory {
        Some(m) if m.total_bytes > 0 => {
            let used = u64_to_f64(m.used_bytes);
            let total = u64_to_f64(m.total_bytes);
            format!(
                "RAM   {:>4}  {:.1} / {:.1} GB",
                percent(Some(used / total * PERCENT_MAX)),
                used / BYTES_PER_GB,
                total / BYTES_PER_GB
            )
        }
        _ => format!("RAM   {MISSING}"),
    });

    out.push(match snapshot.network {
        Some(n) => format!(
            "NET   down {}  up {}",
            speed(n.down_kbps),
            speed(n.up_kbps)
        ),
        None => format!("NET   {MISSING}"),
    });

    for (number, gpu) in (1_u32..).zip(&snapshot.gpus) {
        out.push(format!("GPU{number}  {}", gpu.name));
        out.push(format!(
            "      {:>4}  {}/{} °C  fan {}/{} rpm  power {}%  \
             VRAM {}/{} GB",
            percent(gpu.usage),
            decimal(gpu.temp_c, 0),
            decimal(gpu.temp_max_c, 0),
            whole(gpu.fan_rpm),
            whole(gpu.fan_max_rpm),
            decimal(gpu.power_percent, 0),
            gigabytes(gpu.vram_used_bytes),
            gigabytes(gpu.vram_total_bytes)
        ));
    }

    for disk in &snapshot.disks {
        out.push(format!(
            "SSD{}  {} °C  {}",
            disk.disk,
            disk.temp_c,
            disk.name.as_deref().unwrap_or(MISSING)
        ));
    }
    out
}

/// `42%`, right-aligned by the caller, or `--`.
fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| MISSING.to_owned(), |v| format!("{v:.0}%"))
}

/// The value to `places` decimals, or `--`.
fn decimal(value: Option<f64>, places: usize) -> String {
    value.map_or_else(|| MISSING.to_owned(), |v| format!("{v:.places$}"))
}

/// A whole number, or `--`.
fn whole(value: Option<u32>) -> String {
    value.map_or_else(|| MISSING.to_owned(), |v| v.to_string())
}

/// Bytes as GB to 1 decimal, or `--`.
fn gigabytes(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || MISSING.to_owned(),
        |b| format!("{:.1}", u64_to_f64(b) / BYTES_PER_GB),
    )
}

/// A network speed: `850 kbit/s` below 1 Mbit/s, `5.8 Mbit/s` above.
fn speed(kbps: f64) -> String {
    if kbps < KBPS_PER_MBPS {
        format!("{kbps:.0} kbit/s")
    } else {
        format!("{:.1} Mbit/s", kbps / KBPS_PER_MBPS)
    }
}

#[cfg(test)]
mod tests {
    use super::lines;
    use crate::sensors::{Cpu, DiskTemp, Gpu, Memory, Network, Snapshot};

    #[test]
    fn empty_snapshot_shows_placeholders() {
        let text = lines(&Snapshot::default());
        assert_eq!(
            text,
            ["CPU     --  -- GHz", "CORES", "RAM   --", "NET   --"]
        );
    }

    #[test]
    fn full_snapshot() {
        let snapshot = Snapshot {
            cpu: Cpu {
                total: Some(17.1),
                per_core: vec![23.9, 5.0],
                clock_ghz: Some(4.351),
                cores: vec![vec![0, 1]],
            },
            memory: Some(Memory {
                used_bytes: 1 << 30,
                total_bytes: 4 << 30,
                modules: 2,
            }),
            network: Some(Network {
                down_kbps: 5_830.1,
                up_kbps: 369.9,
                link_kbps: None,
            }),
            gpus: vec![Gpu {
                name: "RTX 2080".to_owned(),
                usage: Some(58.5),
                temp_c: Some(38.1),
                temp_max_c: Some(83.0),
                fan_rpm: Some(1307),
                fan_max_rpm: Some(3838),
                power_percent: Some(18.1),
                vram_used_bytes: Some(3 << 29),
                vram_total_bytes: Some(8 << 30),
            }],
            disks: vec![DiskTemp {
                disk: 0,
                name: Some("WD_BLACK".to_owned()),
                temp_c: 38,
            }],
        };
        assert_eq!(
            lines(&snapshot),
            [
                "CPU    17%  4.35 GHz",
                "CORES  24   5",
                "RAM    25%  1.0 / 4.0 GB",
                "NET   down 5.8 Mbit/s  up 370 kbit/s",
                "GPU1  RTX 2080",
                "       58%  38/83 °C  fan 1307/3838 rpm  power 18%  \
                 VRAM 1.5/8.0 GB",
                "SSD0  38 °C  WD_BLACK",
            ]
        );
    }
}
