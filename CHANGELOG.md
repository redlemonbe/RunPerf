# Changelog

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
