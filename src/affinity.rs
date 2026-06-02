//! affinity.rs — CPU pinning and NUMA topology helpers.
//!
//! `pin_to_cpu` binds the calling thread to a single CPU via sched_setaffinity.
//! `numa_cpus` reads the CPU list of a NUMA node from sysfs.

use std::fs;

/// CPUs this process is allowed to run on (sched_getaffinity). One worker per
/// entry = the dnsmark model: a pinned worker per detected core.
pub fn online_cpus() -> Vec<usize> {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        let mut v = Vec::new();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) == 0 {
            for c in 0..(libc::CPU_SETSIZE as usize) {
                if libc::CPU_ISSET(c, &set) {
                    v.push(c);
                }
            }
        }
        if v.is_empty() {
            v.push(0);
        }
        v
    }
}

/// Number of CPUs available to this process (>= 1).
pub fn online_cpu_count() -> usize {
    online_cpus().len().max(1)
}

/// Pin the calling thread to `cpu`. Best-effort; logs nothing on failure.
pub fn pin_to_cpu(cpu: usize) -> bool {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        // 0 = calling thread
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0
    }
}

/// Parse a Linux cpulist string ("0-3,8,10-11") into individual CPU ids.
pub fn parse_cpulist(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                for c in a..=b {
                    out.push(c);
                }
            }
        } else if let Ok(c) = part.parse::<usize>() {
            out.push(c);
        }
    }
    out
}

/// CPUs belonging to a NUMA node, read from
/// /sys/devices/system/node/node<N>/cpulist.
pub fn numa_cpus(node: usize) -> Vec<usize> {
    let path = format!("/sys/devices/system/node/node{node}/cpulist");
    match fs::read_to_string(&path) {
        Ok(s) => parse_cpulist(s.trim()),
        Err(_) => Vec::new(),
    }
}

/// Resolve the effective CPU list for `--cpus`/`--numa`, falling back to none.
pub fn resolve_cpus(cpus: &Option<String>, numa: &Option<usize>) -> Vec<usize> {
    if let Some(list) = cpus {
        parse_cpulist(list)
    } else if let Some(n) = numa {
        numa_cpus(*n)
    } else {
        Vec::new()
    }
}
