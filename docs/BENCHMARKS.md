# RunPerf — Benchmarks

All figures are **first-party, measured at the NIC hardware counter** (`ethtool -S`,
`rx_bytes`/`rx_packets` deltas on the receiver), not the generator's self-report. Back-to-back
on the same rig, same NIC, same frame size.

## Reference rig

- Two Proxmox hosts, **Intel X710 (i40e)**, 10 GbE **direct** link (no switch), MTU 1500.
- Generator host: AMD Threadripper PRO 5995WX. Receiver: Intel Xeon E5-2690 v2 ×2.
- `runperf` built `--features xdp` (embeds + attaches the XDP program to arm zero-copy).
- 64 B UDP payload (≈ 106 B frame, ≈ 130 B on the wire incl. preamble/IFG/FCS → ~9.6 Mpps
  theoretical line rate for this size).

## UDP packet rate — socket vs copy vs zero-copy vs iperf3

| Generator | queues / cores | Mpps (NIC) | Gb/s (NIC) | notes |
|---|---|---|---|---|
| `iperf3 -u -b 0 -l 64 -P 8` | 8 streams | 2.67 | 1.37 | sender count 16.0 M / 6 s |
| RunPerf socket (`sendmmsg`) | 8 cores | 3.76 | 1.93 | CPU-bound |
| RunPerf AF_XDP **copy** | 8 cores | 2.30 | 2.04 | per-packet kernel copy — no win |
| RunPerf AF_XDP **zero-copy** | 1 / 1 | 6.22 | 5.54 | single core |
| RunPerf AF_XDP **zero-copy** | 8 / 8 | **8.3 – 8.8** | **8.46** | ≈ 10 GbE line rate |

Reading: the socket path tops out CPU-bound at ~40 % of line rate and scales with cores;
zero-copy saturates the link and is ~13× more efficient per core (6.22 Mpps from one core vs
0.47 Mpps/core on the socket path). Copy mode adds a per-packet copy and does not beat the
socket path — it is a fallback, not a tier.

## TCP throughput — agreement with iperf3

Single flow, same link: RunPerf **9.88 Gb/s** = `iperf3` **9.88 Gb/s** (10 GbE − framing).
A line-rate number reached by two independent tools is the link ceiling, not the tool.

## Reproduce

Build with the `xdp` feature on both hosts (needs `clang` + `libbpf-dev`):

```bash
cargo build --release --features xdp
```

**Zero-copy generator (TX) at line rate** — on the generator host, `<gen-nic>` is a real
ZC-capable PF (i40e/ixgbe), `<dst-ip>` is the receiver on the direct link:

```bash
# populate ARP once
ping -c2 <dst-ip>
# blast — RunPerf attaches its XDP program to arm zero-copy, then binds ZC
runperf client --connect <dst-ip>:5201 --udp --len 64 --duration 8 --xdp --iface <gen-nic> --cpus 0-7
```

Measure on the **receiver** (NIC truth), over the same window:

```bash
ethtool -S <recv-nic> | awk -F: '/rx_bytes:/{b=$2}/rx_packets:/{p=$2}END{print p, b}'
# delta(rx_packets)/seconds = Mpps ; delta(rx_bytes)*8/seconds = Gb/s
```

**Socket baseline** (no `--xdp`): `runperf server --udp --len 64` on the receiver,
`runperf client --connect <dst>:5201 --udp --len 64 --threads 8` on the generator.

**iperf3 baseline**: `iperf3 -s` on the receiver, `iperf3 -c <dst> -u -b 0 -l 64 -P 8` on the
generator.

> Counter note: when XSK redirect is active the i40e global `rx_packets`/`tx_packets` counter
> may not increment for redirected frames; the `*_bytes` counters do, and are authoritative.
> After any `--xdp` run, detach a residual program with `ip link set <nic> xdp off`.

## Not yet measured

- >10 GbE (25/40/100 G), where the zero-copy advantage is expected to widen.
- Latency/RTT percentiles.
- Receive-side zero-copy sink throughput at line rate.
