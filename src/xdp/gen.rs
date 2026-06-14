//! gen.rs — generic AF_XDP UDP generator (TX), kernel-bypass, libc-only.
//!
//! TX-only AF_XDP needs no XDP/BPF program (XDP is an RX concept), so the
//! generator builds without aya/clang. It crafts Ethernet+IPv4+UDP frames in the
//! umem (payload copy via the SIMD memcpy) and pushes them through the TX ring.
//! Zero-copy where the driver supports it, copy-mode fallback otherwise.

use std::net::SocketAddrV4;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::frame::{self, FrameHeader};
use super::socket::{create_xsk_socket, get_rx_queue_count, iface_index};
use super::umem::{AddrRing, DescRing, SockaddrXdp, XdpDesc, FRAME_SIZE};

const AF_XDP: i32 = 44;
const TX_BATCH: usize = 64;

struct TxState {
    fd: i32,
    tx: Mutex<DescRing>,
    comp: Mutex<AddrRing>,
    pool: Mutex<Vec<u64>>,
    hdr: FrameHeader,
    sa: SockaddrXdp,
    area: *mut u8,
}
unsafe impl Send for TxState {}
unsafe impl Sync for TxState {}

fn tx_one(state: &TxState, payload: &[u8]) -> bool {
    let addr = match state.pool.lock().unwrap().pop() {
        Some(a) => a,
        None => return false,
    };
    let len = unsafe {
        let buf = std::slice::from_raw_parts_mut(state.area.add(addr as usize), FRAME_SIZE as usize);
        state.hdr.write_frame(buf, payload)
    };
    let desc = XdpDesc { addr, len: len as u32, options: 0 };
    let n = { state.tx.lock().unwrap().produce_tx(&[desc]) };
    if n == 0 {
        state.pool.lock().unwrap().push(addr);
        return false;
    }
    true
}

/// Wake the kernel TX path. Called once per batch — NOT per packet. With
/// `XDP_USE_NEED_WAKEUP` the kernel only sets the wakeup flag *after* it has
/// started draining, so the flag is clear before the very first transmit and a
/// purely conditional kick deadlocks (frames sit in the ring, nothing is sent,
/// the completion ring never fills, the umem pool drains and TX stalls — the
/// 16384-frames-then-stop symptom). An unconditional `sendto` per batch is the
/// canonical AF_XDP TX trigger; at ~64 frames/kick it is ≤ a few hundred k
/// syscalls/s even at 10 GbE line rate (14.9 Mpps), i.e. negligible.
#[inline]
fn kick(state: &TxState) {
    unsafe {
        libc::sendto(
            state.fd,
            std::ptr::null(),
            0,
            libc::MSG_DONTWAIT,
            &state.sa as *const SockaddrXdp as *const libc::sockaddr,
            std::mem::size_of::<SockaddrXdp>() as libc::socklen_t,
        );
    }
}

/// Blast crafted UDP frames out `iface` to `dst` via AF_XDP TX. One pinned
/// worker per queue (capped by CPU count). Counters accumulate frames/bytes.
#[allow(clippy::too_many_arguments)]
pub fn xdp_udp_blast(
    iface: &str,
    dst: SocketAddrV4,
    payload_len: usize,
    duration: u64,
    target_pps: u64,
    cpus: &[usize],
    bytes: Arc<AtomicU64>,
    pkts: Arc<AtomicU64>,
) -> Result<(), String> {
    let ifidx = iface_index(iface).ok_or_else(|| format!("iface {iface} not found"))?;
    let dst_ip = *dst.ip();
    let src_ip = frame::local_ipv4(iface).ok_or("no IPv4 on iface")?;
    let src_mac = frame::local_mac(iface).ok_or("no MAC on iface")?;
    let dst_mac = frame::resolve_server_mac(dst_ip).ok_or("cannot resolve dst MAC via ARP")?;
    let hdr = FrameHeader::new(src_mac, dst_mac, src_ip, dst_ip, 40000, dst.port());

    let queues = get_rx_queue_count(iface).max(1);
    let workers = (cpus.len() as u32).min(queues).max(1);
    let payload_len = payload_len.max(16);

    // ZERO-COPY arming. On i40e/ixgbe (and most drivers) the AF_XDP zero-copy TX
    // queue is only set up when an XDP program is bound to the netdev — without
    // one the ZC bind "succeeds" but no frame ever leaves (tx stalls after one
    // umem). The `xdp` feature embeds + attaches a pass/redirect program, which
    // arms ZC; we then bind ZC and reach line rate. The default libc-only build
    // has no program, so it uses copy mode (correct, just CPU-bound, ~socket
    // speed). Keep the handle alive for the whole run; it detaches on drop.
    #[cfg(feature = "xdp")]
    let _xdp_handle = match super::loader::XdpHandle::load(iface) {
        Ok(h) => { eprintln!("runperf: XDP program attached on {iface} — zero-copy TX armed"); Some(h) }
        Err(e) => { eprintln!("runperf: could not attach XDP program ({e}) — falling back to copy mode"); None }
    };
    // Only attempt a zero-copy bind when we have a program attached to arm it.
    let try_zc = cfg!(feature = "xdp") && {
        #[cfg(feature = "xdp")] { _xdp_handle.is_some() }
        #[cfg(not(feature = "xdp"))] { false }
    };
    eprintln!(
        "runperf AF_XDP generator: {iface} q={workers} -> {dst} (payload {payload_len}B, {})",
        if try_zc { "zero-copy" } else { "copy mode" }
    );

    // One XSK per queue. extract_tx() pulls the TX+completion rings + frame pool;
    // the socket itself (rx/fill/umem/fd) must stay alive → forget it (generator
    // runs to process exit).
    let mut states = Vec::new();
    for q in 0..workers {
        let mut sock = if try_zc {
            unsafe { create_xsk_socket(ifidx, q, true) }
                .or_else(|_| unsafe { create_xsk_socket(ifidx, q, false) })?
        } else {
            unsafe { create_xsk_socket(ifidx, q, false) }?
        };
        let area = sock.umem.ptr_at(0);
        let fd = sock.fd;
        let sa = SockaddrXdp {
            sxdp_family: AF_XDP as u16,
            sxdp_flags: 0,
            sxdp_ifindex: ifidx,
            sxdp_queue_id: q,
            sxdp_shared_umem_fd: 0,
        };
        let (tx, comp, pool) = sock.extract_tx();
        std::mem::forget(sock);
        states.push(Arc::new(TxState {
            fd,
            tx: Mutex::new(tx),
            comp: Mutex::new(comp),
            pool: Mutex::new(pool),
            hdr: hdr.clone(),
            sa,
            area,
        }));
    }

    let run = Arc::new(AtomicBool::new(true));
    let deadline = Instant::now() + Duration::from_secs(duration);
    let pps_per = if target_pps == 0 { 0 } else { (target_pps / workers as u64).max(1) };

    let mut handles = Vec::new();
    for (w, state) in states.into_iter().enumerate() {
        let cpus = cpus.to_vec();
        let run = run.clone();
        let bytes = bytes.clone();
        let pkts = pkts.clone();
        handles.push(thread::spawn(move || {
            crate::affinity::pin_to_cpu(cpus[w % cpus.len()]);
            tx_loop(&state, w, payload_len, deadline, pps_per, &run, &bytes, &pkts);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn tx_loop(
    state: &TxState,
    worker: usize,
    payload_len: usize,
    deadline: Instant,
    pps_per: u64,
    run: &Arc<AtomicBool>,
    bytes: &Arc<AtomicU64>,
    pkts: &Arc<AtomicU64>,
) {
    let mut payload = vec![0u8; payload_len];
    payload[0..8].copy_from_slice(&(worker as u64).to_le_bytes());
    let mut seq: u64 = 0;
    let frame_total = (frame::OUTER_HDR + payload_len) as u64;
    let mut sent_sec = 0u64;
    let mut sec_start = Instant::now();

    while run.load(Ordering::Relaxed) && Instant::now() < deadline {
        // recycle completed frames back to the pool
        {
            let done = state.comp.lock().unwrap().dequeue_all();
            if !done.is_empty() {
                state.pool.lock().unwrap().extend_from_slice(&done);
            }
        }
        let mut sent = 0u64;
        for _ in 0..TX_BATCH {
            payload[8..16].copy_from_slice(&seq.to_le_bytes());
            seq += 1;
            if tx_one(state, &payload) {
                sent += 1;
            } else {
                break;
            }
        }
        if sent > 0 {
            // One wakeup per batch drives the kernel TX path (see `kick`).
            kick(state);
            pkts.fetch_add(sent, Ordering::Relaxed);
            bytes.fetch_add(sent * frame_total, Ordering::Relaxed);
            sent_sec += sent;
        }
        if pps_per > 0 && sent_sec >= pps_per {
            let el = sec_start.elapsed();
            if el < Duration::from_secs(1) {
                thread::sleep(Duration::from_secs(1) - el);
            }
            sent_sec = 0;
            sec_start = Instant::now();
        }
    }
}
