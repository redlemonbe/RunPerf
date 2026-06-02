//! runperf — high-rate network throughput & packet-rate benchmark.
//!
//!   runperf server  [--bind 0.0.0.0:5201] [--udp] [--len 1400] [--cpus L|--numa N]
//!   runperf client  --connect HOST:PORT  [--udp] [--duration 10] [--threads 1]
//!                   [--len 1400] [--target-pps N] [--cpus L|--numa N] [--json]
//!
//! TCP measures throughput (Gb/s). UDP measures packet rate (Mpps), throughput and
//! loss (derived from per-stream sequence gaps). Threads can be pinned to CPUs or a
//! NUMA node — this is the difference from a single-threaded iperf3 UDP run.

mod affinity;
mod net;

use affinity::resolve_cpus;

fn usage() -> ! {
    eprintln!(
        "runperf {}\n\
         \n\
         SERVER:\n\
         \x20 runperf server [--bind 0.0.0.0:5201] [--udp] [--len 1400] [--cpus 0,1|--numa 0]\n\
         \n\
         CLIENT:\n\
         \x20 runperf client --connect HOST:PORT [--udp] [--duration 10] [--threads 1] \\\n\
         \x20                [--len 1400] [--target-pps 0] [--cpus 0,1|--numa 0] [--json]\n\
         \n\
         --udp           UDP packet-rate test (default: TCP throughput)\n\
         --target-pps N  cap send rate (0 = blast, default)\n\
         --cpus LIST     pin worker threads to these CPUs (e.g. 0-3,8)\n\
         --numa N        pin worker threads to NUMA node N's CPUs\n\
         --json          machine-readable summary (client)",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2);
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}
fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Available memory in bytes (MemAvailable from /proc/meminfo), if readable.
fn mem_available_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

/// Anti-OOM safeguard: refuse to allocate packet buffers larger than 25% of
/// available RAM (or 1 GiB if MemAvailable is unreadable). Stops absurd
/// --threads / --len combinations from OOM-ing the host.
fn oom_guard(planned_bytes: u64) {
    let cap = mem_available_bytes().map(|m| m / 4).unwrap_or(1 << 30);
    if planned_bytes > cap {
        eprintln!(
            "error: refusing to allocate {} MiB of buffers (anti-OOM limit {} MiB) — \
             reduce --threads or --len.",
            planned_bytes / (1 << 20),
            cap / (1 << 20)
        );
        std::process::exit(1);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let mode = args[0].clone();
    let udp = has(&args, "--udp");
    let len: usize = flag(&args, "--len").and_then(|s| s.parse().ok()).unwrap_or(1400);
    let cpus = resolve_cpus(&flag(&args, "--cpus"), &flag(&args, "--numa").and_then(|s| s.parse().ok()));

    match mode.as_str() {
        "server" => {
            let bind = flag(&args, "--bind").unwrap_or_else(|| "0.0.0.0:5201".into());
            if udp {
                oom_guard(net::BATCH as u64 * (len.max(2048)) as u64);
            }
            let r = if udp {
                net::udp_server(&bind, len, &cpus)
            } else {
                net::tcp_server(&bind, &cpus)
            };
            if let Err(e) = r {
                eprintln!("server error: {e}");
                std::process::exit(1);
            }
        }
        "client" => {
            let connect = match flag(&args, "--connect") {
                Some(c) => c,
                None => {
                    eprintln!("error: client needs --connect HOST:PORT");
                    usage();
                }
            };
            let duration: u64 = flag(&args, "--duration").and_then(|s| s.parse().ok()).unwrap_or(10);
            let threads: usize = flag(&args, "--threads").and_then(|s| s.parse().ok()).unwrap_or(1);
            let target_pps: u64 = flag(&args, "--target-pps").and_then(|s| s.parse().ok()).unwrap_or(0);
            let json = has(&args, "--json");

            if !cpus.is_empty() {
                eprintln!("pinning {} thread(s) to CPUs {:?}", threads, cpus);
            }
            let planned = if udp {
                threads as u64 * net::BATCH as u64 * len as u64
            } else {
                threads as u64 * 256 * 1024
            };
            oom_guard(planned);
            let summary = if udp {
                net::udp_client(&connect, threads, duration, len, target_pps, &cpus)
            } else {
                net::tcp_client(&connect, threads, duration, &cpus)
            };
            match summary {
                Ok(s) => print_summary(&s, udp, json),
                Err(e) => {
                    eprintln!("client error: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}

fn print_summary(s: &net::Summary, udp: bool, json: bool) {
    if json {
        if udp {
            println!(
                "{{\"proto\":\"udp\",\"seconds\":{:.3},\"bytes\":{},\"packets\":{},\"gbps\":{:.4},\"mpps\":{:.4}}}",
                s.secs, s.bytes, s.packets, s.gbps(), s.mpps()
            );
        } else {
            println!(
                "{{\"proto\":\"tcp\",\"seconds\":{:.3},\"bytes\":{},\"gbps\":{:.4}}}",
                s.secs, s.bytes, s.gbps()
            );
        }
        return;
    }
    println!("──────────────────────────────────────────");
    if udp {
        println!(
            "UDP  {:.3} s  |  {:.3} Gb/s  |  {:.4} Mpps  |  {} packets",
            s.secs, s.gbps(), s.mpps(), s.packets
        );
        println!("(loss is reported by the server via sequence gaps)");
    } else {
        println!("TCP  {:.3} s  |  {:.3} Gb/s  |  {} bytes", s.secs, s.gbps(), s.bytes);
    }
    println!("──────────────────────────────────────────");
}
