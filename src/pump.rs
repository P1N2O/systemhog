//! The maintainer itself: duty-cycled CPU worker threads and a RAM block
//! pool, steered by a proportional controller toward the configured band.

use crate::config::Config;
use crate::log::Logger;
use crate::metrics;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BLOCK_MB: usize = 10;
const MAX_BLOCKS_PER_STEP: usize = 20;

/// One worker burns `duty` percent of one core using a 100 ms busy/sleep
/// cycle. Equal duties across N workers (N = core count) mean the summed
/// contribution to system-wide CPU% equals the duty value itself.
struct CpuWorker {
    stop: Arc<AtomicBool>,
    duty: Arc<AtomicUsize>,
    join: Option<JoinHandle<()>>,
}

impl CpuWorker {
    fn spawn() -> CpuWorker {
        let stop = Arc::new(AtomicBool::new(false));
        let duty = Arc::new(AtomicUsize::new(0));
        let (s, d) = (stop.clone(), duty.clone());
        let join = std::thread::spawn(move || worker_loop(s, d));
        CpuWorker { stop, duty, join: Some(join) }
    }

    fn set_duty(&self, duty: usize) {
        self.duty.store(duty, Ordering::Relaxed);
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn join(&mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn worker_loop(stop: Arc<AtomicBool>, duty: Arc<AtomicUsize>) {
    let cycle = Duration::from_millis(100);
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let d = duty.load(Ordering::Relaxed).min(100);
        let busy = cycle.mul_f32(d as f32 / 100.0);
        let t0 = Instant::now();
        while t0.elapsed() < busy {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::hint::spin_loop();
        }
        if d < 100 {
            std::thread::sleep(cycle - busy);
        }
    }
}

/// A pool of 10 MiB blocks. Blocks are zero-filled on allocation so pages
/// are actually committed, and large enough (> mmap threshold) that the
/// allocator returns them to the kernel on free.
struct RamPump {
    blocks: Vec<Vec<u8>>,
}

impl RamPump {
    fn new() -> RamPump {
        RamPump { blocks: Vec::new() }
    }

    fn mb(&self) -> usize {
        self.blocks.len() * BLOCK_MB
    }

    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// One 10 MiB block with every page committed.
    /// A plain `vec![0u8; N]` memset gets dead-store eliminated (the buffer
    /// is never read again), so the kernel never faults in real pages and
    /// memory usage stays ~0. Volatile stores cannot be removed: one per
    /// 4 KiB page forces the kernel to commit each page.
    fn push_block(&mut self) {
        let v = vec![0u8; BLOCK_MB * 1024 * 1024];
        unsafe {
            for page in v.chunks(4096) {
                std::ptr::write_volatile(page.as_ptr().cast_mut(), 0u8);
            }
        }
        self.blocks.push(v);
    }

    fn pop_block(&mut self) {
        self.blocks.pop();
    }
}

/// Main control loop; returns the process exit code. Runs until `stop` is set.
pub fn run(cfg: &Config, cfg_path: &Path, log: &mut Logger, stop: &'static AtomicBool) -> i32 {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let mut workers: Vec<CpuWorker> = (0..n).map(|_| CpuWorker::spawn()).collect();
    let mut ram = RamPump::new();
    let mut cur = cfg.clone();
    let mut duty = 0usize;
    let mut warn_metrics = true;
    let mut warn_reload = true;

    log.info(&format!(
        "started: {} cpu workers, cpu {}..{}%, ram target {}%, interval {}s",
        n, cur.cpu_min, cur.cpu_max, cur.ram_target, cur.check_interval
    ));

    while !stop.load(Ordering::Relaxed) {
        // Hot-reload: pick up config edits on the next interval.
        match crate::config::load(cfg_path) {
            Ok(next) if next != cur => {
                log.info(&format!(
                    "configuration reloaded: cpu {}..{}%, ram {}%, interval {}s",
                    next.cpu_min, next.cpu_max, next.ram_target, next.check_interval
                ));
                cur = next;
                warn_reload = true;
            }
            Err(_) if warn_reload => {
                log.warn("config parse failed; keeping previous settings");
                warn_reload = false;
            }
            _ => {}
        }

        // CPU: two samples ~1s apart.
        let a = metrics::cpu_sample();
        std::thread::sleep(Duration::from_secs(1));
        let b = metrics::cpu_sample();
        match (a, b) {
            (Some(a), Some(b)) => {
                let c = metrics::cpu_percent(&a, &b);
                if c < cur.cpu_min as f64 {
                    // Step is bounded by the deficit, so one adjustment can
                    // at most reach the band (plus measurement lag).
                    let step = ((cur.cpu_min as f64 - c).ceil() as usize).clamp(5, 30);
                    duty = (duty + step).min(100);
                    for w in &workers {
                        w.set_duty(duty);
                    }
                    log.info(&format!(
                        "cpu {c:.1}% below min {}%, raising load to {duty}%",
                        cur.cpu_min
                    ));
                } else if c > cur.cpu_max as f64 {
                    let step = ((c - cur.cpu_max as f64).ceil() as usize).clamp(5, 30);
                    duty = duty.saturating_sub(step);
                    for w in &workers {
                        w.set_duty(duty);
                    }
                    log.info(&format!(
                        "cpu {c:.1}% above max {}%, lowering load to {duty}%",
                        cur.cpu_max
                    ));
                } else {
                    log.info(&format!("cpu {c:.1}% within band (load {duty}%)"));
                }
            }
            _ => {
                if warn_metrics {
                    log.warn("cpu metrics unavailable on this platform");
                    warn_metrics = false;
                }
            }
        }

        // RAM: steer *system used* (not our allocation alone) toward the
        // target percentage, with one-block hysteresis. Re-reading meminfo
        // inside the loop keeps the estimate accurate as we allocate/free.
        let mut steps = 0usize;
        let mut phase = 0u8; // 0 = none, 1 = pushed, 2 = popped (no flip-flop in one batch)
        while steps < MAX_BLOCKS_PER_STEP {
            let Some(m) = metrics::mem_info() else { break };
            let target = m.total_kb * cur.ram_target as u64 / 100;
            let used = m.total_kb.saturating_sub(m.avail_kb);
            let block_kb = (BLOCK_MB * 1024) as u64;
            if used + block_kb <= target {
                if phase == 2 {
                    break;
                }
                ram.push_block();
                steps += 1;
                phase = 1;
            } else if used > target + block_kb && !ram.is_empty() {
                if phase == 1 {
                    break;
                }
                ram.pop_block();
                steps += 1;
                phase = 2;
            } else {
                break;
            }
        }
        if steps > 0 {
            log.info(&format!("ram adjusted ({} MB held)", ram.mb()));
        } else if let Some(m) = metrics::mem_info() {
            let used = m.total_kb.saturating_sub(m.avail_kb);
            log.info(&format!(
                "ram {:.1}% used ({} MB held, target {}%)",
                used as f64 * 100.0 / m.total_kb as f64,
                ram.mb(),
                cur.ram_target
            ));
        }

        std::thread::sleep(Duration::from_secs(cur.check_interval as u64));
    }

    let held = ram.mb();
    for w in &workers {
        w.stop();
    }
    for w in &mut workers {
        w.join();
    }
    drop(ram);
    log.info(&format!(
        "stopped cleanly ({} workers joined, {held} MB released)",
        workers.len()
    ));
    0
}
