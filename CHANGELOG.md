# Changelog

## v0.3.1 — hardening, tests, positioning

No datapath change — the zero-copy generator is unchanged from v0.3.0. This release adds the
polish that makes the project stand on its own:

- **`--version` / `-V`** (and explicit `-h`/`--help`).
- **End-to-end integration tests** (`tests/loopback.rs`): spawn the real `runperf` server +
  client on loopback (TCP and UDP) and assert the `--json` summary shows real traffic. Uses a
  TCP connect-retry readiness probe (no fragile fixed sleep) and reaps the server before
  asserting. Runs in CI.
- **Honest positioning among generators** — README + WHITEPAPER credit TRex/MoonGen/pktgen-dpdk
  as the DPDK-class high-rate references and frame RunPerf as the bridge between `iperf3`
  ergonomics and a kernel-bypass datapath (AF_XDP, no DPDK). The iperf3 comparison is stated as
  architecture, measured, with no value judgement.
- **Docs/CI**: documented IPv4-only (REUSEPORT bind / UDP client / AF_XDP generator; IPv6 on the
  roadmap), CI status badge, `CONTRIBUTING.md` (incl. the measure-at-the-NIC benchmark policy),
  and a pinned `dtolnay/rust-toolchain` in CI (no pipe-to-sh).

## v0.3.0 — zero-copy generator validated

The AF_XDP zero-copy TX generator now actually works and is validated at the NIC.

- **Fixed the AF_XDP TX generator**, built since v0.2 but never transmitting:
  - it kicked the TX ring only on `NEED_WAKEUP`, but that flag is clear before the first
    transmit → frames sat in the ring, the completion ring never refilled, TX stalled after one
    umem (16384 frames, 0 on the wire). Now kicks once per batch unconditionally.
  - on `i40e`/`ixgbe` the zero-copy TX queue is only armed when an XDP program is bound to the
    netdev. The `xdp` feature now attaches the embedded program to arm zero-copy, binds ZC, and
    falls back to copy mode otherwise. Detaches on exit (RAII).
- **Validated at the NIC counter on an Intel X710 (i40e) 10 GbE link**, 64 B UDP:
  zero-copy **8.3–8.8 Mpps (≈ line rate)**, **6.22 Mpps from a single core**, vs the socket
  path 3.76 Mpps and `iperf3` 2.67 Mpps — ~3× at the wire ceiling, ~13× per core. See
  `docs/WHITEPAPER.md` / `docs/BENCHMARKS.md`.
- **Server-side loss now reported live** (per-stream sequence-gap %) — was computed, never shown.
- `--help` lists `--xdp` / `--iface`; removed dead code. Zero warnings, `clippy -D` clean on both
  the default (libc-only) and `--features xdp` builds. Added a CI workflow (build/clippy/test).

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
