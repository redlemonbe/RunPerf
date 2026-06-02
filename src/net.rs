//! net.rs — TCP throughput and UDP packet-rate engines.
//!
//! TCP: multi-stream, each stream a pinned thread doing tight write/read.
//! UDP: client uses sendmmsg (batched) across pinned threads; server uses
//!      recvmmsg and derives loss from per-stream sequence gaps.
//!
//! UDP payload layout (first 16 bytes): [u64 stream_id LE][u64 seq LE], rest pad.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::affinity::pin_to_cpu;

const BATCH: usize = 64; // mmsg batch size
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

// ── TCP ─────────────────────────────────────────────────────────────────────────

pub fn tcp_server(addr: &str, cpus: &[usize]) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("runperf TCP server on {addr}");
    let counters = Arc::new(Counters::default());
    let run = Arc::new(AtomicBool::new(true));
    spawn_reporter(counters.clone(), run.clone(), false);

    let mut i = 0;
    for stream in listener.incoming() {
        let stream = stream?;
        let c = counters.clone();
        let cpus = cpus.to_vec();
        let idx = i;
        i += 1;
        thread::spawn(move || {
            pin(&cpus, idx);
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
            let mut s = match TcpStream::connect(&addr) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("connect failed: {e}");
                    return;
                }
            };
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
    let sock = UdpSocket::bind(addr)?;
    eprintln!("runperf UDP server on {addr}");
    let fd = sock.as_raw_fd();
    let counters = Arc::new(Counters::default());
    let run = Arc::new(AtomicBool::new(true));
    spawn_reporter(counters.clone(), run, true);
    pin(cpus, 0);

    let len = len.max(HDR);
    let mut bufs: Vec<Vec<u8>> = (0..BATCH).map(|_| vec![0u8; len.max(2048)]).collect();
    // per-stream last-seq for loss detection
    use std::collections::HashMap;
    let mut last_seq: HashMap<u64, u64> = HashMap::new();

    // Build iovecs + mmsghdrs ONCE; recvmmsg refills the same bufs each call.
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
        // reset msg_len before each recvmmsg call
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
