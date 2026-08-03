//! CPU and RAM sampling. Linux only: /proc/stat + /proc/meminfo —
//! zero dependencies, no libc.

/// Cumulative CPU ticks, system-wide.
#[derive(Debug, Clone, Copy)]
pub struct CpuSample {
    pub total: u64,
    pub idle: u64,
}

pub struct MemInfo {
    pub total_kb: u64,
    pub avail_kb: u64,
}

/// Busy fraction between two samples, in percent (0..=100).
pub fn cpu_percent(a: &CpuSample, b: &CpuSample) -> f64 {
    let dt = b.total.saturating_sub(a.total);
    let di = b.idle.saturating_sub(a.idle);
    if dt == 0 {
        0.0
    } else {
        (1.0 - di as f64 / dt as f64) * 100.0
    }
}

mod imp {
    use super::{CpuSample, MemInfo};

    pub fn cpu_sample() -> Option<CpuSample> {
        let s = std::fs::read_to_string("/proc/stat").ok()?;
        let mut fields = s.lines().next()?.split_whitespace();
        fields.next()?; // "cpu"
        // user nice system idle iowait irq softirq steal (guest is already
        // folded into user/nice, per proc(5)).
        let mut vals = [0u64; 8];
        for v in vals.iter_mut() {
            *v = fields.next()?.parse().ok()?;
        }
        Some(CpuSample {
            total: vals.iter().sum(),
            idle: vals[3] + vals[4],
        })
    }

    pub fn mem_info() -> Option<MemInfo> {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let mut total = 0u64;
        let mut avail = 0u64;
        let mut free = 0u64;
        let mut buffers = 0u64;
        let mut cached = 0u64;
        for line in s.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            let kb: u64 = v
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            match k {
                "MemTotal" => total = kb,
                "MemAvailable" => avail = kb,
                "MemFree" => free = kb,
                "Buffers" => buffers = kb,
                "Cached" => cached = kb,
                _ => {}
            }
        }
        if avail == 0 {
            // Pre-3.14 kernels without MemAvailable.
            avail = free + buffers + cached;
        }
        (total > 0).then_some(MemInfo {
            total_kb: total,
            avail_kb: avail,
        })
    }
}

pub use imp::{cpu_sample, mem_info};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_math() {
        let a = CpuSample { total: 100, idle: 80 };
        let b = CpuSample { total: 200, idle: 140 };
        assert!((cpu_percent(&a, &b) - 40.0).abs() < 1e-9);
        assert_eq!(cpu_percent(&a, &a), 0.0);
    }
}
