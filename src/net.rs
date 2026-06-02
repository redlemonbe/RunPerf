//! net.rs — TCP throughput and UDP packet-rate engines.
//!
//! TCP: multi-stream, each stream a pinned thread doing tight write/read.
//! UDP: client uses sendmmsg (batched) across pinned threads; server uses
//!      recvmmsg and derives loss from per-stream sequence gaps.
//!
//! UDP payload layout (first 16 bytes): [u64 stream_id LE][u64 seq LE], rest pad.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::affinity::pin_to_cpu;

pub const BATCH: usize = 64; // mmsg batch size
const HDR: usize = 16; // stream_id + seq

// ── Shared counters ────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Counters {
    pub bytes: AtomicU64,
    pub packets: AtomicU64,
    pub drops: AtomicU64,
}

pub struct Summary {
    pub bytes: u64,
    pub packets: u64,
    pub drops: u64,
    pub secs: f64,
}
impl Summary {
    pub fn gbps(&self) -> f64 {
        (self.bytes as f64 * 8.0) / self.secs / 1e9
    }
    pub fn mpps(&self) -> f64 {
        (self.packets as f64) / self.secs / 1e6
    }
    pub fn loss_pct(&self) -> f64 {
        let total = self.packets + self.drops;
        if total == 0 {
            0.0
        } else {
            self.drops as f64 * 100.0 / total as f64
        }
    }
}

/// Background reporter: prints instantaneous rate each second until `run` clears.
pub fn spawn_reporter(c: Arc<Counters>, run: Arc<AtomicBool>, udp: bool) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut last_b = 0u64;
        let mut last_p = 0u64;
        let mut t = 0u64;
        while run.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(1));
            let b = c.bytes.load(Ordering::Relaxed);
            let p = c.packets.load(Ordering::Relaxed);
            let gbps = (b - last_b) as f64 * 8.0 / 1e9;
            t += 1;
            if udp {
                let pps = (p - last_p) as f64 / 1e6;
                eprintln!("[{t:>3}s] {gbps:7.3} Gb/s  {pps:7.3} Mpps");
            } else {
                eprintln!("[{t:>3}s] {gbps:7.3} Gb/s");
            }
            last_b = b;
            last_p = p;
        }
    })
}

fn pin(cpus: &[usize], i: usize) {
    if !cpus.is_empty() {
        pin_to_cpu(cpus[i % cpus.len()]);
    }
}

/// Socket buffer size requested on each socket (kernel may cap at *mem_max).
const SOCK_BUF: libc::c_int = 8 * 1024 * 1024; // 8 MiB

fn set_opt_int(fd: RawFd, level: libc::c_int, opt: libc::c_int, val: libc::c_int) {
    unsafe {
        libc::setsockopt(
            fd,
            level,
            opt,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

/// Build an IPv4 sockaddr_in from "host:port" (IPv4 only — REUSEPORT bind path).
fn sockaddr_v4(addr: &str) -> std::io::Result<(libc::sockaddr_in, libc::socklen_t)> {
    use std::net::{SocketAddr, ToSocketAddrs};
    let sa = addr
        .to_socket_addrs()?
        .find(|s| matches!(s, SocketAddr::V4(_)))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "need an IPv4 addr"))?;
    let v4 = match sa {
        SocketAddr::V4(v4) => v4,
        _ => unreachable!(),
    };
    let s = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: v4.port().to_be(),
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes(v4.ip().octets()),
        },
        sin_zero: [0; 8],
    };
    Ok((s, std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t))
}

/// A TCP listener with SO_REUSEPORT (N can bind the same port → kernel
/// load-balances accepts across pinned workers) + large RX buffer.
fn reuseport_tcp_listener(addr: &str) -> std::io::Result<TcpListener> {
    let (sa, len) = sockaddr_v4(addr)?;
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        set_opt_int(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);
        set_opt_int(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1);
        // Do NOT set SO_RCVBUF here: a fixed value is clamped by net.core.rmem_max
        // (often ~208 KiB) AND disables receive-window autotuning, so the server
        // advertises a tiny window (wscale 2, ~256 KiB) and the sender ends up
        // rwnd-limited (~40% of the time). Autotuning grows to tcp_rmem max (MBs)
        // like iperf3 does — the difference between ~8.2 and ~9.8 Gb/s here.
        if libc::bind(fd, &sa as *const _ as *const libc::sockaddr, len) < 0
            || libc::listen(fd, 1024) < 0
        {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        Ok(TcpListener::from_raw_fd(fd))
    }
}

/// A UDP socket with SO_REUSEPORT (kernel flow-hashes RX across pinned workers)
/// + large RX buffer.
fn reuseport_udp_socket(addr: &str) -> std::io::Result<UdpSocket> {
    let (sa, len) = sockaddr_v4(addr)?;
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        set_opt_int(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);
        set_opt_int(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1);
        set_opt_int(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, SOCK_BUF);
        if libc::bind(fd, &sa as *const _ as *const libc::sockaddr, len) < 0 {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        Ok(UdpSocket::from_raw_fd(fd))
    }
}

// ── TCP ─────────────────────────────────────────────────────────────────────────

pub fn tcp_server(addr: &str, cpus: &[usize]) -> std::io::Result<()> {
    let counters = Arc::new(Counters::default());
    let run = Arc::new(AtomicBool::new(true));
    spawn_reporter(counters.clone(), run.clone(), false);

    let workers = cpus.len().max(1);
    eprintln!("runperf TCP server on {addr} ({workers} SO_REUSEPORT workers)");

    // One REUSEPORT listener per CPU, each pinned — the kernel load-balances
    // incoming connections across them (dnsmark model).
    //
    // SO_REUSEPORT hashes by the 4-tuple; with all flows sharing src/dst IP it
    // distributes unevenly, and pinning each reader to *its listener's* core
    // piled multiple flows onto one core (measured: core 0 at 71% while others
    // idled, vs iperf3's even ~30%). Spread readers round-robin across all cores
    // with a shared accept counter so the receive work balances like iperf3.
    let next = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for w in 0..workers {
        let listener = reuseport_tcp_listener(addr)?;
        let c = counters.clone();
        let cpus = cpus.to_vec();
        let next = next.clone();
        handles.push(thread::spawn(move || {
            pin(&cpus, w);
            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let c = c.clone();
                let cpus = cpus.to_vec();
                let rcpu = next.fetch_add(1, Ordering::Relaxed);
                thread::spawn(move || {
                    pin(&cpus, rcpu);
                    let mut s = stream;
                    let mut buf = vec![0u8; 256 * 1024];
                    loop {
                        match s.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                c.bytes.fetch_add(n as u64, Ordering::Relaxed);
                            }
                        }
                    }
                });
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

pub fn tcp_client(
    addr: &str,
    threads: usize,
    duration: u64,
    cpus: &[usize],
) -> std::io::Result<Summary> {
    let counters = Arc::new(Counters::default());
    let run = Arc::new(AtomicBool::new(true));
    let rep = spawn_reporter(counters.clone(), run.clone(), false);
    let deadline = Instant::now() + Duration::from_secs(duration);
    let start = Instant::now();

    let mut handles = Vec::new();
    for t in 0..threads {
        let c = counters.clone();
        let addr = addr.to_string();
        let cpus = cpus.to_vec();
        handles.push(thread::spawn(move || {
            pin(&cpus, t);
            // Stagger flow starts. Launching N flows at the same instant makes
            // their slow-starts overshoot the bottleneck together, take a
            // synchronized loss, and back off together ("global synchronization")
            // — N flows then sum to *less* than one clean flow and converge only
            // over tens of seconds. A few ms of offset per flow desynchronizes
            // them; measured here it takes 8-flow TCP from ~9.0 to ~9.9 Gb/s.
            if t > 0 {
                thread::sleep(Duration::from_millis(t as u64 * 25));
            }
            let mut s = match TcpStream::connect(&addr) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("connect failed: {e}");
                    return;
                }
            };
            // TCP_NODELAY is load-bearing (without it Nagle + delayed-ACK stalls
            // the stream). But do NOT fix SO_SNDBUF: like SO_RCVBUF it is clamped
            // by net.core.wmem_max and disables send-window autotuning. Let the
            // kernel grow the send window to tcp_wmem max, as iperf3 does.
            let _ = s.set_nodelay(true);
            let buf = vec![0xABu8; 256 * 1024];
            while Instant::now() < deadline {
                match s.write(&buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        c.bytes.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    run.store(false, Ordering::Relaxed);
    let _ = rep.join();
    Ok(Summary {
        bytes: counters.bytes.load(Ordering::Relaxed),
        packets: 0,
        drops: 0,
        secs: start.elapsed().as_secs_f64(),
    })
}

// ── UDP (sendmmsg / recvmmsg) ────────────────────────────────────────────────────

pub fn udp_client(
    addr: &str,
    threads: usize,
    duration: u64,
    len: usize,
    target_pps: u64,
    cpus: &[usize],
) -> std::io::Result<Summary> {
    let counters = Arc::new(Counters::default());
    let run = Arc::new(AtomicBool::new(true));
    let rep = spawn_reporter(counters.clone(), run.clone(), true);
    let deadline = Instant::now() + Duration::from_secs(duration);
    let start = Instant::now();
    let len = len.max(HDR);
    // per-thread pps budget (0 = blast)
    let pps_per_thread = if target_pps == 0 { 0 } else { (target_pps / threads as u64).max(1) };

    let mut handles = Vec::new();
    for t in 0..threads {
        let c = counters.clone();
        let addr = addr.to_string();
        let cpus = cpus.to_vec();
        handles.push(thread::spawn(move || {
            pin(&cpus, t);
            let sock = match UdpSocket::bind("0.0.0.0:0") {
                Ok(s) => s,
                Err(e) => { eprintln!("udp bind: {e}"); return; }
            };
            if sock.connect(&addr).is_err() {
                eprintln!("udp connect failed");
                return;
            }
            let fd = sock.as_raw_fd();
            // BATCH buffers, each stamped with stream id + seq
            let mut bufs: Vec<Vec<u8>> = (0..BATCH).map(|_| vec![0u8; len]).collect();
            for b in bufs.iter_mut() {
                b[0..8].copy_from_slice(&(t as u64).to_le_bytes());
            }
            let mut seq: u64 = 0;
            // pacing
            let mut sent_this_sec: u64 = 0;
            let mut sec_start = Instant::now();

            // Build iovecs + mmsghdrs ONCE (raw pointers into stable bufs/iov; the
            // Vecs never realloc, so reuse them every batch — no hot-loop alloc).
            let mut iov: Vec<libc::iovec> = bufs
                .iter_mut()
                .map(|b| libc::iovec {
                    iov_base: b.as_mut_ptr() as *mut libc::c_void,
                    iov_len: b.len(),
                })
                .collect();
            let mut msgs: Vec<libc::mmsghdr> = (0..BATCH)
                .map(|i| unsafe {
                    let mut m: libc::mmsghdr = std::mem::zeroed();
                    m.msg_hdr.msg_iov = &mut iov[i] as *mut libc::iovec;
                    m.msg_hdr.msg_iovlen = 1;
                    m
                })
                .collect();

            while Instant::now() < deadline {
                // stamp seqs for this batch (iovecs already point at these bufs)
                for b in bufs.iter_mut() {
                    b[8..16].copy_from_slice(&seq.to_le_bytes());
                    seq += 1;
                }
                let ret = unsafe {
                    libc::sendmmsg(fd, msgs.as_mut_ptr(), BATCH as _, 0)
                };
                if ret > 0 {
                    let n = ret as u64;
                    c.packets.fetch_add(n, Ordering::Relaxed);
                    c.bytes.fetch_add(n * len as u64, Ordering::Relaxed);
                    sent_this_sec += n;
                }
                // pacing: cap per-second if target set
                if pps_per_thread > 0 && sent_this_sec >= pps_per_thread {
                    let el = sec_start.elapsed();
                    if el < Duration::from_secs(1) {
                        thread::sleep(Duration::from_secs(1) - el);
                    }
                    sent_this_sec = 0;
                    sec_start = Instant::now();
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    run.store(false, Ordering::Relaxed);
    let _ = rep.join();
    Ok(Summary {
        bytes: counters.bytes.load(Ordering::Relaxed),
        packets: counters.packets.load(Ordering::Relaxed),
        drops: 0,
        secs: start.elapsed().as_secs_f64(),
    })
}

pub fn udp_server(addr: &str, len: usize, cpus: &[usize]) -> std::io::Result<()> {
    let counters = Arc::new(Counters::default());
    let run = Arc::new(AtomicBool::new(true));
    spawn_reporter(counters.clone(), run, true);

    let workers = cpus.len().max(1);
    eprintln!("runperf UDP server on {addr} ({workers} SO_REUSEPORT workers)");
    let len = len.max(HDR);

    // One REUSEPORT socket per CPU, each pinned — the kernel flow-hashes incoming
    // datagrams across them, so RX scales with cores (v0.1 was single-threaded).
    let mut handles = Vec::new();
    for w in 0..workers {
        let sock = reuseport_udp_socket(addr)?;
        let counters = counters.clone();
        let cpus = cpus.to_vec();
        handles.push(thread::spawn(move || {
            pin(&cpus, w);
            udp_server_worker(sock, len, counters);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

/// One UDP RX worker: recvmmsg loop + per-stream loss detection. A given stream
/// always REUSEPORT-hashes to the same socket, so per-worker last_seq is correct.
fn udp_server_worker(sock: UdpSocket, len: usize, counters: Arc<Counters>) {
    let fd = sock.as_raw_fd();
    let mut bufs: Vec<Vec<u8>> = (0..BATCH).map(|_| vec![0u8; len.max(2048)]).collect();
    use std::collections::HashMap;
    let mut last_seq: HashMap<u64, u64> = HashMap::new();

    let mut iov: Vec<libc::iovec> = bufs
        .iter_mut()
        .map(|b| libc::iovec {
            iov_base: b.as_mut_ptr() as *mut libc::c_void,
            iov_len: b.len(),
        })
        .collect();
    let mut msgs: Vec<libc::mmsghdr> = (0..BATCH)
        .map(|i| unsafe {
            let mut m: libc::mmsghdr = std::mem::zeroed();
            m.msg_hdr.msg_iov = &mut iov[i] as *mut libc::iovec;
            m.msg_hdr.msg_iovlen = 1;
            m
        })
        .collect();

    loop {
        for m in msgs.iter_mut() {
            m.msg_len = 0;
        }
        let ret = unsafe {
            libc::recvmmsg(
                fd,
                msgs.as_mut_ptr(),
                BATCH as _,
                libc::MSG_WAITFORONE as _,
                std::ptr::null_mut(),
            )
        };
        if ret <= 0 {
            continue;
        }
        for i in 0..ret as usize {
            let n = msgs[i].msg_len as u64;
            counters.bytes.fetch_add(n, Ordering::Relaxed);
            counters.packets.fetch_add(1, Ordering::Relaxed);
            if (n as usize) >= HDR {
                let b = &bufs[i];
                let sid = u64::from_le_bytes(b[0..8].try_into().unwrap());
                let seq = u64::from_le_bytes(b[8..16].try_into().unwrap());
                if let Some(prev) = last_seq.insert(sid, seq) {
                    if seq > prev + 1 {
                        counters.drops.fetch_add(seq - prev - 1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}
