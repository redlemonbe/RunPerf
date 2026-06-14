# RunPerf
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/redlemonbe/RunPerf)](https://github.com/redlemonbe/RunPerf/releases/latest)
[![GitHub Sponsors](https://img.shields.io/github/sponsors/redlemonbe?style=flat&logo=github&label=Sponsor)](https://github.com/sponsors/redlemonbe)

> Read [ACCEPTABLE_USE.md](ACCEPTABLE_USE.md) before use. At full throttle RunPerf
> generates **millions of packets per second** — only point it at networks you own
> or are authorized to test.

A network throughput and packet-rate benchmark — like `iperf3`, but built to
**scale with the cores you give it**: auto per-CPU pinned workers, multiqueue,
SSE2/AVX2 hot path, and an optional **AF_XDP kernel-bypass** datapath.

## What you get

- **TCP throughput** (Gb/s), multi-stream, `SO_REUSEPORT` servers.
- **UDP packet rate** (Mpps) via `sendmmsg`/`recvmmsg` batching.
- **Auto per-CPU scaling** — detects online CPUs and runs **one pinned worker per
  core** by default (the dnsmark model). No flags needed; adapts to the hardware.
- **Multiqueue aware** — one worker per NIC RX queue, capped at the CPU count.
- **AF_XDP datapath** (`--xdp`, optional `xdp` feature) — kernel-bypass TX
  generator + RX sink (zero-copy where the driver supports it).
- **SIMD hot path** — SSE2/AVX2 (`core::arch::asm!`) memory copy, runtime CPU
  dispatch.
- **Loss** measured server-side from per-stream sequence gaps (no control channel).
- **Anti-OOM guard**, human + **JSON** output. Single static binary (libc-only
  unless `xdp` is enabled).

## Performance & scaling — *match the model to the metric*

The two metrics scale differently, and RunPerf defaults to the right model for
each:

- **TCP throughput → a single flow.** One TCP stream already saturates a 10 GbE
  path: the kernel + NIC do the transport, not userspace, so a single core is
  never the bottleneck at 10 G. Stacking N concurrent flows only adds cubic
  contention and synchronised-startup loss — *N flows sum to less than one clean
  flow* on a single bottleneck. So `runperf client` defaults to **one TCP flow**
  and hits **line rate, identical to `iperf3`**. (Use `--threads N` for TCP only
  on links a single core can't fill — 25/40/100 GbE — where the flows are then
  started staggered to avoid global synchronisation.)
- **UDP packet-rate → one pinned worker per CPU.** Here the bottleneck *is*
  per-core packet processing, so throughput scales with cores/queues. A generic
  emulated NIC caps small-packet rate low (every packet is a VM exit); RunPerf
  scales out across cores and queues instead of fighting it with a faster single
  thread.

Measured VM-to-VM, two hypervisor hosts, 10 GbE direct link, RunPerf v0.2:

| Setup | UDP generation (Mpps) | TCP, 1 flow (Gb/s) |
|---|---|---|
| emulated NIC, **2 queues / 2 vCPU** | ~0.05 | — |
| paravirt + vhost, **8 queues / 8 vCPU** | **3.57** | **9.88** (= `iperf3` 9.88) |

UDP: same code, ≈ **70×** the packet rate from cores/queues alone — **give it
more CPUs, it goes faster.** TCP: a single flow reaches the physical port ceiling
(10 GbE − framing ≈ 9.88 Gb/s), matching `iperf3` exactly; two independent tools
hitting the same number *is* the proof it's the link, not the tool. (On real
hardware, the `--xdp` path pushes the UDP rate further by bypassing the stack.)

## Install

```bash
ver=$(curl -fsSL https://api.github.com/repos/redlemonbe/RunPerf/releases/latest | grep -oP '"tag_name":\s*"\K[^"]+')
base=https://github.com/redlemonbe/RunPerf/releases/download/$ver
curl -fsSL $base/runperf-x86_64-unknown-linux-musl -o runperf && chmod +x runperf  # static
./runperf --help
```

## Quick start

```bash
# TCP throughput — receiver, then sender (auto: one pinned worker per core)
runperf server --bind 0.0.0.0:5201
runperf client --connect HOST:5201 --duration 10

# UDP packet rate (small frames) — receiver reports Mpps + loss
runperf server --udp --bind 0.0.0.0:5201 --len 64
runperf client --connect HOST:5201 --udp --len 64 --duration 10

# Pin explicitly / cap workers
runperf client --connect HOST:5201 --threads 8 --cpus 0-7
```

## AF_XDP datapath (kernel-bypass)

Build with the `xdp` feature (needs `clang` + `libbpf-dev` for the eBPF program;
pulls `aya`). The feature embeds a small XDP program; the generator **attaches it
to arm the driver's zero-copy TX queue** — `i40e`/`ixgbe` (and most drivers) only
set up the AF_XDP zero-copy datapath when a program is bound to the netdev. Without
the feature, or if the attach fails, the generator runs in **copy mode** (libc-only,
CPU-bound, ≈ socket speed); the RX sink uses the same program to redirect UDP into
the XSK. The program detaches automatically on exit (RAII).

```bash
cargo build --release --features xdp

# generator (TX): zero-copy AF_XDP, 10 GbE line rate on a real NIC
runperf client --xdp --iface eth1 --connect 10.0.0.2:5201 --udp --len 64 --cpus 0-7

# sink (RX): XDP redirects all UDP into the XSK, counted kernel-bypass
runperf server --xdp --iface eth1 --udp
```

On a real NIC (`ixgbe`/`i40e` PF) the generator reaches line rate in zero-copy
(≈ 8.8 Mpps at 64 B), where a kernel-socket datapath is CPU-bound below the wire — the
difference is the architecture, laid out with measurements in
[docs/WHITEPAPER.md](docs/WHITEPAPER.md). On an emulated NIC it falls back to copy mode
and the device emulation is the ceiling.

AF_XDP shines on real NICs (ixgbe/i40e PF, line-rate). On emulated NICs it falls
back to copy mode and the device emulation is the ceiling.

## Flags

| Flag | Meaning |
|---|---|
| `--bind ADDR:PORT` / `--connect HOST:PORT` | server listen / client target |
| `--udp` | UDP packet-rate test (default: TCP throughput) |
| `--threads N` | worker count (default: number of online CPUs) |
| `--cpus LIST` / `--numa N` | pin workers to CPUs / a NUMA node |
| `--len B` | UDP payload size |
| `--target-pps N` | cap send rate (0 = blast, default) |
| `--duration S` | client run time |
| `--xdp` | AF_XDP datapath (needs `--iface`; build `--features xdp` for the sink) |
| `--iface NIC` | NIC for the AF_XDP datapath |
| `--json` | machine-readable summary |

## Build from source

```bash
cargo build --release                 # socket datapath, libc-only, single static binary
cargo build --release --features xdp  # + AF_XDP (needs clang + libbpf-dev, pulls aya)
```

Cross-compiles to `x86_64`/`aarch64` × `gnu`/`musl`.

## Status

**v0.3 — zero-copy generator validated.** Auto per-CPU pinned workers, multiqueue,
`SO_REUSEPORT` servers, SSE2/AVX2 SIMD. On TCP, one flow matches `iperf3` exactly
(9.88 Gb/s — it's the link). For small-frame packet rate, the **AF_XDP zero-copy TX
generator reaches 10 GbE line rate** (≈ 8.8 Mpps at 64 B), measured at the NIC counter
on an Intel X710, where a kernel-socket datapath is CPU-bound; server-side sequence-gap
loss is reported live. Roadmap: latency/RTT (P99), `io_uring`, >10 GbE validation.

Design and measured results: **[docs/WHITEPAPER.md](docs/WHITEPAPER.md)**.

## Support the project

[![GitHub Sponsors](https://img.shields.io/github/sponsors/redlemonbe?style=flat&logo=github&label=Sponsor%20on%20GitHub)](https://github.com/sponsors/redlemonbe)

**Bitcoin** — `3FP8hkkiu4kwCD1PDFgAv2oq1ZTyXwy3yy`  
**Ethereum** — `0xB5eEAf89edA4204Aa9305B068b37A93439cBb680`

---


## License

AGPL v3 — see [LICENSE](LICENSE).

---

## Part of RunASM

RunPerf is part of **[RunASM](https://www.runasm.com)** — high-performance infrastructure in Rust, with benchmarks measured at the NIC hardware counters, not asserted.
