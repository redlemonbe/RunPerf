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

- **TX generator** (`src/xdp/gen.rs`) — TX-only AF_XDP needs **no** BPF program (XDP is an
  RX-side concept), so the generator stays libc-only: it crafts Ethernet/IPv4/UDP frames into
  the umem (payload copied with the SIMD routine) and pushes them through the AF_XDP TX ring via
  `sendto`, one pinned worker per queue.
- **RX sink** (`ebpf/runperf_xdp.c`, loaded via `aya`) — an XDP program redirects matching UDP
  traffic straight into the XSK (`XDP_REDIRECT`), where it is counted without traversing the
  kernel network stack; everything else is `XDP_PASS`ed.

AF_XDP is zero-copy where the driver supports it (`ixgbe`/`i40e` PF, line rate); on an emulated
NIC it falls back to copy mode and the device emulation becomes the ceiling.

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

Honest scope: these are the socket-datapath numbers on virtualised NICs. The `--xdp` path is
expected to push the UDP rate further on a real `ixgbe`/`i40e` PF by bypassing the stack, but
that has not yet been benchmarked on real hardware here — it must be measured, not extrapolated.
The v0.1 release was self-tested on loopback only (where UDP pps is kernel-bound, not a tool
limit); real per-NIC numbers require real hardware.

## 8. Test rig

VM-to-VM across two hypervisor hosts over a 10 GbE direct link; x86_64; the high-rate row used a
paravirtualised NIC with `vhost`, 8 RX queues bound to 8 vCPUs. Generators pinned one worker per
vCPU. Loss measured server-side from per-stream sequence gaps. TCP compared head-to-head with
`iperf3` on the identical path.

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
