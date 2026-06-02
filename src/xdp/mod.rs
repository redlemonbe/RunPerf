//! AF_XDP datapath — kernel-bypass packet generator.
//!
//! Engine (umem/socket/frame) ported from dnsmark's proven transport/xdp
//! (itself from Runbound). `gen` adds a generic UDP TX generator. The TX path
//! needs no BPF program, so the generator is libc-only; `loader` (aya) is only
//! pulled by the `xdp` feature for the future AF_XDP RX sink.

pub mod frame;
pub mod socket;
pub mod umem;

#[cfg(feature = "xdp")]
pub mod loader;

mod gen;
pub use gen::xdp_udp_blast;
