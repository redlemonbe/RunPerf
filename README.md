# RunPerf
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/redlemonbe/RunPerf)](https://github.com/redlemonbe/RunPerf/releases/latest)
[![GitHub Sponsors](https://img.shields.io/github/sponsors/redlemonbe?style=flat&logo=github&label=Sponsor)](https://github.com/sponsors/redlemonbe)

> Read [ACCEPTABLE_USE.md](ACCEPTABLE_USE.md) before use. At full throttle RunPerf
> generates millions of packets per second — only point it at networks you own or
> are authorized to test.

A network throughput and packet-rate benchmark — like `iperf3`, but built for the
packet rates `iperf3` can't reach.

## What you get

- **TCP throughput** (Gb/s), multi-stream.
- **UDP packet rate** (Mpps) via `sendmmsg`/`recvmmsg` batching — so a real NIC,
  not the tool, is the bottleneck. `iperf3` is single-threaded for UDP and tops out
  well below line rate on small frames; RunPerf pins N threads to N cores.
- **Loss** measured server-side from per-stream sequence gaps (no control channel).
- **CPU pinning** (`--cpus`) and **NUMA-node pinning** (`--numa`).
- **Anti-OOM guard**: refuses absurd buffer allocations (won't OOM the box).
- Human and **JSON** output. Single static binary, only dependency `libc`.

## Install

```bash
ver=v0.1.0
base=https://github.com/redlemonbe/RunPerf/releases/download/$ver

# x86_64 static (musl — no dependencies)
curl -fsSL $base/runperf-x86_64-unknown-linux-musl -o runperf && chmod +x runperf
# x86_64 glibc (servers with glibc >= 2.17)
curl -fsSL $base/runperf-x86_64-unknown-linux-gnu  -o runperf && chmod +x runperf
# aarch64 static (Graviton, Raspberry Pi 4/5 — musl)
curl -fsSL $base/runperf-aarch64-unknown-linux-musl -o runperf && chmod +x runperf
# aarch64 glibc
curl -fsSL $base/runperf-aarch64-unknown-linux-gnu  -o runperf && chmod +x runperf

./runperf --help
```

## Quick start

One side runs the **server**, the other the **client**.

```bash
# TCP throughput — receiver, then sender (4 pinned streams, 10 s)
runperf server --bind 0.0.0.0:5201
runperf client --connect HOST:5201 --duration 10 --threads 4 --cpus 0-3

# UDP packet rate — receiver (reports pps + loss), then blast 64 B from NUMA node 0
runperf server --udp --bind 0.0.0.0:5201 --len 64
runperf client --connect HOST:5201 --udp --len 64 --duration 10 --threads 8 --numa 0

# Capped rate + JSON (CI/CD)
runperf client --connect HOST:5201 --udp --len 1400 --target-pps 1000000 --json
```

## Output

- **TCP**: throughput (Gb/s) per second and total.
- **UDP**: packet rate (Mpps), throughput (Gb/s), and **loss %** (server side, from
  sequence gaps).

## Flags

| Flag | Meaning |
|---|---|
| `--bind ADDR:PORT` | server listen address (default `0.0.0.0:5201`) |
| `--connect HOST:PORT` | client target |
| `--udp` | UDP packet-rate test (default: TCP throughput) |
| `--threads N` | parallel streams/sockets (client) |
| `--cpus LIST` | pin worker threads to CPUs (e.g. `0-3,8`) |
| `--numa N` | pin worker threads to NUMA node N's CPUs |
| `--len B` | UDP payload size in bytes |
| `--target-pps N` | cap send rate (0 = blast, default) |
| `--duration S` | client run time |
| `--json` | machine-readable client summary |

## Build from source

```bash
cargo build --release        # only dependency: libc
```

Cross-compiles cleanly to `x86_64`/`aarch64` × `gnu`/`musl` (no C dependencies).

## Status

**v0.1.0 — functional**, self-tested on loopback (TCP, UDP, JSON, pinning, anti-OOM).
Real per-NIC numbers come from running on real hardware; loopback UDP pps is
kernel-bound, not a tool limit. Roadmap (v0.2): multi-threaded UDP server
(`SO_REUSEPORT`), latency/RTT mode with P99, optional `io_uring` send path.

## Contributing

Issues and PRs welcome. New code paths should keep the anti-OOM guard intact.

## Support the project

[![Sponsor](https://img.shields.io/github/sponsors/redlemonbe?style=flat&logo=github&label=Sponsor)](https://github.com/sponsors/redlemonbe)

---

AGPL-3.0 — see [LICENSE](LICENSE). Contact: redlemonbe@codix.be
