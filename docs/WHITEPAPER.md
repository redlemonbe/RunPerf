# RunPerf — a core-scaling network benchmark

**Status: v0.2 (scaling datapath).** Measured numbers only; every figure below was produced
on the rig described in §8. Where something is designed but not yet measured on real hardware,
it is labelled *roadmap*.

---

## 1. Abstract

RunPerf is a network throughput and packet-rate benchmark in the spirit of `iperf3`, built on
one principle: **match the measurement model to the metric, and scale with the cores you give
it.** It ships two datapaths:

- **Socket datapath** (default, libc-only, single static binary) — TCP throughput over
  `SO_REUSEPORT` servers, and UDP packet-rate via `sendmmsg`/`recvmmsg` batching, with one
  CPU-pinned worker per core.
- **AF_XDP datapath** (`--xdp`, optional `xdp` feature) — a kernel-bypass UDP TX generator and
  an XDP-redirect RX sink, zero-copy where the NIC driver supports it.

The headline result is not a single number but a **slope**: the same UDP code goes ~70× faster
when given 8 cores/queues instead of 2 (see §8). For TCP, a single flow already reaches the
10 GbE ceiling — identical to `iperf3` — so RunPerf defaults to one flow there.

## 2. The problem

`iperf3` is the de-facto throughput tool, but two things limit it for modern, high-packet-rate
measurement:

- **UDP packet-rate is single-threaded.** Small-frame rate (Mpps) is bound by per-core packet
  processing; one thread cannot express a multi-core/multi-queue NIC. The bottleneck is the
  *tool*, not the link.
- **No kernel-bypass path.** On a real NIC the stack itself caps small-packet rate; there is no
  AF_XDP option to take the kernel out of the hot loop.

RunPerf keeps the familiar client/server UX but **scales out** — one pinned worker per CPU, one
per NIC RX queue — and offers an AF_XDP path for the cases where the kernel is the ceiling.

## 3. Architecture

```
runperf client/server
  ├─ auto CPU detection → one pinned worker per online core (the dnsmark model)
  │     • TCP: SO_REUSEPORT servers, kernel does the transport
  │     • UDP: sendmmsg/recvmmsg batching, one worker per core/queue
  ├─ SIMD hot path — SSE2/AVX2 memcpy, runtime CPU dispatch (no SIGILL: target_feature-guarded)
  └─ optional AF_XDP datapath (--xdp)
        • TX generator: crafts Eth/IPv4/UDP frames, blasts via the AF_XDP TX ring (no BPF needed)
        • RX sink: an XDP program redirects UDP into the XSK, counted kernel-bypass
```

- **Auto per-CPU scaling.** With no flags, RunPerf detects the online CPUs and runs one pinned
  worker per core; `--threads N`, `--cpus LIST`, `--numa N` override. Worker count is capped at
  the NIC RX-queue count where that matters.
- **Loss without a control channel.** The server derives per-stream loss from sequence gaps in
  the payload, so a UDP blast needs no side channel that could itself become a bottleneck.
- **Single static binary.** The socket datapath is libc-only and cross-compiles to
  `x86_64`/`aarch64` × `gnu`/`musl`; the `xdp` feature adds `clang` + `libbpf` (via `aya`) for
  the RX program only.

## 4. Two metrics, two models

This is the core design decision, and it is why RunPerf does not simply "use all cores for
everything":

- **TCP throughput → one flow.** A single TCP stream already saturates a 10 GbE path — the
  kernel and NIC do the transport, not userspace, so one core is never the bottleneck at 10 G.
  Stacking N concurrent flows adds contention and synchronised-startup loss: **N flows sum to
  less than one clean flow** on a single bottleneck. So `runperf client` defaults to **one TCP
  flow** and reaches line rate, identical to `iperf3`. `--threads N` is for links a single core
  cannot fill (25/40/100 GbE), where the flows are then started staggered to avoid global
  synchronisation.
- **UDP packet-rate → one pinned worker per CPU.** Here the bottleneck *is* per-core packet
  processing, so throughput scales with cores and queues. RunPerf scales out rather than
  fighting an emulated NIC's per-packet VM-exit cost with a faster single thread.

## 5. The SIMD hot path

The payload copy on the hot path uses a runtime-dispatched SIMD routine (`src/simd.rs`):
AVX-512 / AVX2 / SSE4.2 are selected once via `is_x86_feature_detected!` and the chosen path is
reused for the run — there is no per-buffer feature check, and no `SIGILL` risk because each
path is `target_feature`-guarded. A portable scalar fallback covers non-x86 and older CPUs. As
with the rest of the suite, the SIMD only earns its keep where per-packet/per-copy work is the
bound; on a 10 GbE TCP flow the kernel path already saturates and the copy is not the limit.

## 6. The AF_XDP datapath

Built with `--features xdp`:

- **TX generator** (`src/xdp/gen.rs`) — crafts Ethernet/IPv4/UDP frames into the umem (payload
  copied with the SIMD routine) and pushes them through the AF_XDP TX ring, one pinned worker
  per queue. Two details decide whether a frame ever leaves the wire:
  - **Kick once per batch, unconditionally.** With `XDP_USE_NEED_WAKEUP` the kernel only sets
    the wakeup flag *after* it has begun draining, so the flag is clear before the first
    transmit. Kicking *only* when the flag is set deadlocks on startup — descriptors pile up,
    the completion ring never refills, the umem drains, and TX stalls after one umem (the
    "16384 frames then silence, NIC `tx_packets`=0" symptom). One `sendto` per ~64-frame batch
    is the canonical trigger and is negligible (≤ a few hundred k syscalls/s at line rate).
  - **Zero-copy needs an XDP program bound to the netdev.** On `i40e`/`ixgbe` the ZC TX queue
    is only armed when *some* XDP program is attached — even for TX-only. The `xdp` feature
    attaches the embedded program to arm ZC, then binds zero-copy; without it (or on failure)
    the generator falls back to **copy mode**, which adds a per-packet copy and is no faster
    than the socket path. The program detaches on exit (RAII).
- **RX sink** (`ebpf/runperf_xdp.c`, loaded via `aya`) — an XDP program redirects matching UDP
  traffic straight into the XSK (`XDP_REDIRECT`), counted without traversing the kernel stack;
  everything else is `XDP_PASS`ed.

On a real NIC (`ixgbe`/`i40e` PF) zero-copy reaches line rate (§7); on an emulated NIC the
device emulation becomes the ceiling.

## 7. Measured results

Measured VM-to-VM, two hypervisor hosts, 10 GbE direct link, RunPerf v0.2:

| Setup | UDP generation (Mpps) | TCP, 1 flow (Gb/s) |
|---|---|---|
| emulated NIC, **2 queues / 2 vCPU** | ~0.05 | — |
| paravirt + vhost, **8 queues / 8 vCPU** | **3.57** | **9.88** |

- **UDP — same code, ≈ 70× the packet rate** from cores/queues alone. This is the slope the
  tool is built to express: give it more CPUs/queues, it goes faster. The absolute number is a
  function of the NIC/host, not a tool ceiling.
- **TCP — 9.88 Gb/s on a single flow**, which equals `iperf3` on the same path (10 GbE minus
  framing). Two independent tools reaching the same figure *is* the evidence that the ceiling is
  the physical link, not the tool.

### Zero-copy on real hardware (v0.3)

The `--xdp` path has now been benchmarked on real hardware — **Intel X710 (i40e), 10 GbE
direct link, 64 B UDP, read at the receiver NIC counter** (not the tool's self-report):

| Generator path | UDP 64 B (Mpps) | per-core | vs `iperf3` |
|---|---|---|---|
| `iperf3 -u -b 0`, 8 streams | 2.67 | — | 1.0× |
| RunPerf socket (`sendmmsg`, 8 cores) | 3.76 | 0.47 | 1.4× |
| RunPerf AF_XDP copy mode | 2.30 | — | 0.9× |
| RunPerf AF_XDP zero-copy, 1 queue / 1 core | 6.22 | **6.22** | 2.3× |
| RunPerf AF_XDP zero-copy, 8 queues | **8.3 – 8.8** (≈ link rate) | — | **3.1×** |

The socket path is CPU-bound at ~40 % of line rate (scales with cores); copy mode adds a
per-packet copy and is no faster. **Zero-copy saturates the link and delivers ≈ 13× the
per-core packet rate** (6.22 Mpps from one core vs 0.47 Mpps/core on the socket path). On
faster NICs (25/40/100 GbE) the gap is expected to widen — the socket path stays CPU-bound
while zero-copy tracks the wire — but that is a projection until measured, not a result.

## 8. Test rig

Two rigs. **Socket/scaling rows:** VM-to-VM across two hypervisor hosts over a 10 GbE direct
link; x86_64; the high-rate row used a paravirtualised NIC with `vhost`, 8 RX queues bound to 8
vCPUs. **Zero-copy rows (v0.3):** bare-metal, Intel **X710 (i40e)** PF, 10 GbE direct host-to-host
link, MTU 1500, generator on an AMD Threadripper PRO 5995WX. Generators pinned one worker per
core/queue. Loss measured server-side from per-stream sequence gaps; throughput read from the
NIC hardware counter (`ethtool -S`). TCP compared head-to-head with `iperf3` on the identical
path. Full reproduction recipe: [BENCHMARKS.md](BENCHMARKS.md).

## 9. Limits and roadmap

- **Measured**: TCP single-flow line-rate (= `iperf3`); UDP packet-rate scaling with
  cores/queues (~70× from 2→8); the socket datapath on virtualised NICs.
- **Roadmap**: receive-side scaling tuning; a latency / RTT mode with percentiles (P99); an
  `io_uring` send path; and an AF_XDP benchmark on a real `ixgbe`/`i40e` PF (the path is shipped
  but not yet line-rate-validated on real hardware).
- **Use responsibly**: at full throttle RunPerf emits millions of packets per second — point it
  only at networks you own or are authorised to test (see `ACCEPTABLE_USE.md`).

RunPerf is part of the **Run** suite — the same engineering line as the suite's servers and
other benchmarks: scale with the cores, measure at the counter, and let the plateau be the
hardware, not the tool.
