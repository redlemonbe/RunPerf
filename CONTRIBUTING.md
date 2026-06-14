# Contributing to RunPerf

Thanks for your interest. RunPerf is a small, focused tool — contributions are welcome,
especially measured ones.

## Ground rules

- **Branch from `main`**, open a PR against `main`.
- **CI must be green.** Before pushing, run locally what CI runs:
  ```bash
  cargo build --release
  cargo build --release --features xdp        # needs clang + libbpf-dev
  cargo clippy --release --all-targets -- -D warnings
  cargo clippy --release --features xdp --all-targets -- -D warnings
  cargo test --release
  ```
  Zero warnings is the bar (CI denies them on both the default and `xdp` builds).
- **Commits:** imperative subject (`fix(xdp): ...`, `docs: ...`), a body that explains *why*.

## Performance claims = measurements, or they don't ship

RunPerf is a benchmark; its credibility is its honesty. If a change touches the datapath or you
state a number:

- Measure at the **NIC hardware counter** (`ethtool -S`), not the tool's self-report, on a real
  link — see [docs/BENCHMARKS.md](docs/BENCHMARKS.md) for the method.
- Give the **rig** (NIC, driver, link speed, frame size) and compare back-to-back on the same
  hardware. Note what is *measured* vs *projected*.
- We benchmark against `iperf3` as the reference and report agreement on TCP; we do not frame
  results as a contest.

## Scope

In scope: throughput/packet-rate accuracy, datapath performance, AF_XDP, portability, docs.
Out of scope for now: turning RunPerf into a full traffic-shaping/replay suite.

## License

By contributing you agree your work is licensed under the project's AGPL-3.0.
