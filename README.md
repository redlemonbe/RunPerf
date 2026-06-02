# RunPerf

![Version](https://img.shields.io/badge/version-v0.1.0-blue)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
![Arch](https://img.shields.io/badge/arch-x86__64%20%7C%20aarch64-lightgrey)

**A network throughput & packet-rate benchmark — like `iperf3`, but built for the
packet rates `iperf3` can't reach.**

RunPerf measures TCP throughput and **UDP packet rate (Mpps)** with a multi-threaded,
CPU-pinned, NUMA-aware engine. The UDP path uses `sendmmsg`/`recvmmsg` batching so a
real NIC — not the tool — is the bottleneck. Built in Rust, single static binary, no
runtime dependencies (musl targets).

> Why not just `iperf3`? `iperf3` is single-threaded for UDP and is not a packet
> generator — it tops out well below line rate on small frames. RunPerf pins N
> threads to N cores and batches sends, which is what you need to push toward the
> 14.88 Mpps small-frame ceiling of a 10 GbE link.

## Install

Download the release bundle (inside the host/VM that will run the test):

```bash
ver=v0.1.0
curl -fsSLO https://github.com/redlemonbe/RunPerf/releases/download/$ver/runperf-$ver-x86_64-linux-gnu.tar.gz
tar -xzf runperf-$ver-x86_64-linux-gnu.tar.gz
./runperf --help
```

Or build it: `cargo build --release` (only dependency: `libc`).

## Usage

One side runs the **server**, the other the **client**.

### TCP throughput

```bash
# receiver
runperf server --bind 0.0.0.0:5201

# sender (4 pinned streams, 10 s)
runperf client --connect HOST:5201 --duration 10 --threads 4 --cpus 0-3
```

### UDP packet rate (the point of this tool)

```bash
# receiver — reports received pps and loss (from per-stream sequence gaps)
runperf server --udp --bind 0.0.0.0:5201 --len 64

# sender — blast 64-byte packets from 8 cores pinned to NUMA node 0
runperf client --connect HOST:5201 --udp --len 64 --duration 10 --threads 8 --numa 0
```

`--target-pps N` caps the send rate (default 0 = blast). `--json` prints a
machine-readable summary for scripting.

## Metrics

- **TCP**: throughput (Gb/s), per second and total.
- **UDP**: packet rate (Mpps), throughput (Gb/s), and **loss %** — the server
  derives loss from gaps in each stream's sequence numbers (no control channel).

## Options

| Flag | Meaning |
|---|---|
| `--udp` | UDP packet-rate test (default: TCP throughput) |
| `--threads N` | parallel streams/sockets (client) |
| `--cpus LIST` | pin worker threads to CPUs (`0-3,8`) |
| `--numa N` | pin worker threads to NUMA node N's CPUs |
| `--len B` | UDP payload size (bytes) |
| `--target-pps N` | cap send rate (0 = blast) |
| `--duration S` | client run time |
| `--json` | machine-readable client summary |

## Status

**v0.1.0 — functional.** TCP throughput, UDP pps with `sendmmsg`/`recvmmsg`,
CPU pinning, NUMA pinning, sequence-gap loss, JSON output — all working and
self-tested on loopback. Real per-NIC numbers come from running it on real
hardware. Loopback UDP pps is kernel-bound, not a tool limit.

Roadmap (v0.2): multi-threaded UDP server (`SO_REUSEPORT`), latency/RTT mode with
P99, optional `io_uring` send path.

## License

AGPL-3.0 — see [LICENSE](LICENSE). Contact: redlemonbe@codix.be
