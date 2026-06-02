//! sink.rs — AF_XDP RX sink (feature `xdp`), kernel-bypass receive.
//!
//! Loads the XDP redirect program (aya), attaches it, and registers one XSK per
//! queue in the XSKS map. The kernel redirects incoming UDP frames straight into
//! the XSK RX ring; the sink reads them (zero-copy where supported), counts
//! packets/bytes and derives loss from per-stream sequence gaps, then recycles
//! frames to the fill ring.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::loader::XdpHandle;
use super::socket::{create_xsk_socket, get_rx_queue_count, iface_index, XskSocket};

const HDRS: usize = 42; // Eth(14) + IPv4(20) + UDP(8)

#[allow(clippy::too_many_arguments)]
pub fn xdp_udp_sink(
    iface: &str,
    duration: u64,
    cpus: &[usize],
    bytes: Arc<AtomicU64>,
    pkts: Arc<AtomicU64>,
    drops: Arc<AtomicU64>,
) -> Result<(), String> {
    let ifidx = iface_index(iface).ok_or_else(|| format!("iface {iface} not found"))?;
    let mut handle = XdpHandle::load(iface)?;
    let queues = get_rx_queue_count(iface).max(1);
    let workers = (cpus.len() as u32).min(queues).max(1);
    eprintln!("runperf AF_XDP sink: {iface} q={workers} (kernel-bypass RX)");

    let run = Arc::new(AtomicBool::new(true));
    let deadline = Instant::now() + Duration::from_secs(duration.max(1));

    let mut socks = Vec::new();
    for q in 0..workers {
        let sock = unsafe { create_xsk_socket(ifidx, q, true) }
            .or_else(|_| unsafe { create_xsk_socket(ifidx, q, false) })?;
        handle.register_socket(q, sock.fd)?;
        socks.push(sock);
    }

    let mut handles = Vec::new();
    for (w, sock) in socks.into_iter().enumerate() {
        let cpus = cpus.to_vec();
        let run = run.clone();
        let bytes = bytes.clone();
        let pkts = pkts.clone();
        let drops = drops.clone();
        handles.push(thread::spawn(move || {
            crate::affinity::pin_to_cpu(cpus[w % cpus.len()]);
            rx_loop(sock, &run, deadline, &bytes, &pkts, &drops);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    drop(handle); // detach the XDP program
    Ok(())
}

fn rx_loop(
    sock: XskSocket,
    run: &Arc<AtomicBool>,
    deadline: Instant,
    bytes: &Arc<AtomicU64>,
    pkts: &Arc<AtomicU64>,
    drops: &Arc<AtomicU64>,
) {
    let mut last_seq: HashMap<u64, u64> = HashMap::new();
    while run.load(Ordering::Relaxed) && Instant::now() < deadline {
        let mut pfd = libc::pollfd { fd: sock.fd, events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut pfd, 1, 100) };

        let descs = sock.rx.consume_rx();
        if descs.is_empty() {
            continue;
        }
        let mut recycle: Vec<u64> = Vec::with_capacity(descs.len());
        for desc in &descs {
            let len = desc.len as usize;
            let frame = unsafe { std::slice::from_raw_parts(sock.umem.ptr_at(desc.addr), len) };
            bytes.fetch_add(len as u64, Ordering::Relaxed);
            pkts.fetch_add(1, Ordering::Relaxed);
            if len >= HDRS + 16 {
                let p = &frame[HDRS..];
                let sid = u64::from_le_bytes(p[0..8].try_into().unwrap());
                let seq = u64::from_le_bytes(p[8..16].try_into().unwrap());
                if let Some(prev) = last_seq.insert(sid, seq) {
                    if seq > prev + 1 {
                        drops.fetch_add(seq - prev - 1, Ordering::Relaxed);
                    }
                }
            }
            recycle.push(desc.addr);
        }
        sock.umem.fill.enqueue_batch(&recycle);
    }
}
