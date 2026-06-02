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

## Performance & scaling — *speed is a function of allocated cores*

This is the whole point. A generic, "works-everywhere" emulated NIC layer
(e.g. an emulated `vmxnet3` on a hypervisor) caps small-packet rate low because
every packet costs a VM exit — typically a few-hundred-k pps ceiling. RunPerf
doesn't fight that with a faster single thread; it **scales out across cores and
queues**, so throughput rises with the resources you allocate.

Measured VM-to-VM, two hypervisor hosts, 10 GbE direct link, RunPerf v0.2:

| Setup | UDP generation | TCP |
|---|---|---|
| emulated NIC, **2 queues / 2 vCPU** | ~0.05 Mpps | ~5.9 Gb/s |
| paravirt + vhost, **8 queues / 8 vCPU** | **3.57 Mpps** | **8.48 Gb/s** |

Same code — the difference is the cores/queues handed to it (≈ **70×** the packet
rate). RunPerf auto-detects and pins one worker per core/queue: **give it more
CPUs, it goes faster.** (On real hardware, the `--xdp` path pushes the rate
further by bypassing the kernel stack entirely.)

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
pulls `aya`). The TX generator needs no BPF program (XDP is RX-side) and is
libc-only; the RX sink loads an XDP redirect program via aya.

```bash
cargo build --release --features xdp

# generator (TX): crafts Eth/IPv4/UDP frames, blasts via the AF_XDP TX ring
runperf client --xdp --iface eth1 --connect 10.0.0.2:5201 --len 64

# sink (RX): XDP redirects all UDP into the XSK, counted kernel-bypass
runperf server --xdp --iface eth1
```

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

**v0.2 — scaling datapath.** Auto per-CPU pinned workers, multiqueue, SO_REUSEPORT
servers, SSE2/AVX2 SIMD, AF_XDP TX generator + RX sink. Throughput scales with
allocated cores/queues. Roadmap: receive-side scaling, latency/RTT (P99), `io_uring`.

## Support the project

[![Sponsor](https://img.shields.io/github/sponsors/redlemonbe?style=flat&logo=github&label=Sponsor)](https://github.com/sponsors/redlemonbe)

---

AGPL-3.0 — see [LICENSE](LICENSE). Contact: redlemonbe@codix.be
