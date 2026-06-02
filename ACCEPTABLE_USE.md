# Acceptable Use Policy — RunPerf

RunPerf is a network throughput and packet-rate benchmarking tool. At full
throttle it generates **millions of packets per second**. Pointed at a network
or host you do not own, it is a denial-of-service weapon.

## Permitted use

- Network links, hosts, and VMs you own or operate
- Systems for which you hold explicit **written** authorization
- Controlled academic or security research environments
- Isolated test rigs and CI/CD performance gates on your own resources

## Prohibited use

- Any system or network without prior written authorization from its owner
- Denial-of-service attacks or amplification attacks
- Integration into botnets, attack scripts, or automated attack pipelines
- Generating load over networks you do not control (shared/transit/cloud
  fabrics) such that other tenants are affected
- Any activity that violates applicable local, national, or international law

## License and consequences

RunPerf is distributed under the **GNU Affero General Public License v3
(AGPL-3.0-only)**. Any use of RunPerf as part of a network service requires making
the full source code available under the same license.

Unauthorized use voids all liability protections and constitutes a criminal offense
in most jurisdictions, including but not limited to violations of the Computer Fraud
and Abuse Act (US), the Computer Misuse Act (UK), and equivalent legislation worldwide.

The authors bear no responsibility for misuse.

---

*RunPerf is designed to benchmark network datapaths — including
[RunX540](https://github.com/redlemonbe/RunX540) and
[Runbound](https://github.com/redlemonbe/Runbound) — in authorized, isolated
environments.*
