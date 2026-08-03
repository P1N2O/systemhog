//! CPU and RAM sampling, one implementation per OS.
//!
//! Linux  : /proc/stat + /proc/meminfo — zero dependencies, no libc.
//! macOS  : host_statistics64 + sysconf via `libc`.
//! Windows: GetSystemTimes + GlobalMemoryStatusEx via `windows-sys`.
//! Other  : metrics report None; the maintainer still runs, it just logs warnings.

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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
mod imp {
    use super::{CpuSample, MemInfo};

    // libc marks mach_host_self as deprecated ("use mach2"); it is a
    // trivial thunk, so declare it directly to keep the dependency set
    // at zero for this platform too.
    unsafe extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
    }

    pub fn cpu_sample() -> Option<CpuSample> {
        unsafe {
            let host = mach_host_self();
            let mut ticks = [0i32; 4]; // user, system, idle, nice
            let mut count = 4u32;
            let r = libc::host_statistics64(
                host,
                libc::HOST_CPU_LOAD_INFO,
                ticks.as_mut_ptr(),
                &mut count,
            );
            if r != 0 {
                return None;
            }
            let idle = ticks[2].max(0) as u64;
            let total = ticks.into_iter().map(|t| t.max(0) as u64).sum();
            Some(CpuSample { total, idle })
        }
    }

    pub fn mem_info() -> Option<MemInfo> {
        unsafe {
            let pagesize = libc::sysconf(libc::_SC_PAGESIZE) as u64;
            let total = libc::sysconf(libc::_SC_PHYS_PAGES) as u64 * pagesize;
            let host = mach_host_self();
            let mut stats: libc::vm_statistics64 = std::mem::zeroed();
            let mut count =
                (std::mem::size_of::<libc::vm_statistics64>() / std::mem::size_of::<libc::integer_t>())
                    as u32;
            let r = libc::host_statistics64(
                host,
                libc::HOST_VM_INFO64,
                &mut stats as *mut _ as *mut libc::integer_t,
                &mut count,
            );
            if r != 0 {
                return None;
            }
            let avail =
                (stats.free_count + stats.inactive_count + stats.speculative_count) as u64 * pagesize;
            Some(MemInfo {
                total_kb: total / 1024,
                avail_kb: avail / 1024,
            })
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{CpuSample, MemInfo};
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    fn ft_to_u64(t: FILETIME) -> u64 {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    }

    pub fn cpu_sample() -> Option<CpuSample> {
        let mut idle: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
        let mut user: FILETIME = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        let (idle, kernel, user) = (ft_to_u64(idle), ft_to_u64(kernel), ft_to_u64(user));
        Some(CpuSample {
            total: kernel + user,
            idle,
        })
    }

    pub fn mem_info() -> Option<MemInfo> {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
        if ok == 0 {
            return None;
        }
        Some(MemInfo {
            total_kb: m.ullTotalPhys / 1024,
            avail_kb: m.ullAvailPhys / 1024,
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::{CpuSample, MemInfo};

    pub fn cpu_sample() -> Option<CpuSample> {
        None
    }

    pub fn mem_info() -> Option<MemInfo> {
        None
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
