# Changelog

## v0.2.0 — scaling datapath

- Auto per-CPU pinned workers (one per online core); `--threads`/`--cpus`/`--numa` overrides.
- Multiqueue-aware: one worker per NIC RX queue, capped at the CPU count.
- TCP: `SO_REUSEPORT` servers; single-flow default reaching 10 GbE line rate (= `iperf3`).
- UDP: `sendmmsg`/`recvmmsg` batching; packet-rate scales with cores/queues (~70x from 2->8 queues, VM-to-VM 10 GbE).
- SIMD hot path: SSE2/AVX2 memcpy, runtime CPU dispatch.
- AF_XDP datapath (`--xdp`, `xdp` feature): kernel-bypass UDP TX generator (libc-only) + XDP-redirect RX sink (via aya).
- Design + measured results: docs/WHITEPAPER.md.


## v0.1.0

Initial release.

- TCP throughput benchmark (multi-stream, CPU-pinned).
- UDP packet-rate benchmark using `sendmmsg`/`recvmmsg` batching — built for high
  pps where `iperf3` falls short.
- Per-stream sequence-gap loss detection (server side, no control channel).
- CPU pinning (`--cpus`) and NUMA-node pinning (`--numa`).
- Human and JSON (`--json`) output.
- Single static binary (musl), only dependency: `libc`.

Self-tested on loopback (TCP + UDP + JSON + pinning). Real per-NIC numbers require
running on real hardware; loopback UDP pps is kernel-bound, not a tool limit.

Roadmap: multi-threaded UDP server (`SO_REUSEPORT`), latency/RTT mode (P99),
optional `io_uring` send path.
