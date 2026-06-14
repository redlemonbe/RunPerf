//! End-to-end loopback integration tests: spawn the real `runperf` binary as a server and a
//! client on 127.0.0.1 and assert the client's --json summary shows real traffic. No added
//! dependency — the --json output is a single hand-rolled line on stdout (the SIMD banner goes
//! to stderr), so a tiny field extractor is enough.

use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_runperf")
}

/// Pull an unsigned integer field out of the one-line JSON summary.
fn field(json: &str, key: &str) -> u64 {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat).unwrap_or_else(|| panic!("key {key} missing in: {json}")) + pat.len();
    json[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn run_loopback(udp: bool, port: u16) -> String {
    let addr = format!("127.0.0.1:{port}");
    let mut server_args = vec!["server", "--bind", &addr, "--cpus", "0"];
    if udp {
        server_args.push("--udp");
    }
    let mut server = Command::new(bin())
        .args(&server_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");

    // Let the server bind its REUSEPORT socket(s) before the client connects/sends.
    sleep(Duration::from_millis(500));

    let mut client_args = vec![
        "client", "--connect", &addr, "--duration", "1", "--cpus", "0", "--json",
    ];
    if udp {
        client_args.push("--udp");
    }
    let out = Command::new(bin())
        .args(&client_args)
        .output()
        .expect("run client");

    // Always reap the server before asserting (so a failed assert doesn't leak it).
    let _ = server.kill();
    let _ = server.wait();

    assert!(out.status.success(), "client exited non-zero: {:?}", out.status);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn udp_loopback_moves_packets() {
    let json = run_loopback(true, 53111);
    assert!(json.contains("\"proto\":\"udp\""), "unexpected summary: {json}");
    assert!(field(&json, "packets") > 0, "no packets sent: {json}");
    assert!(field(&json, "bytes") > 0, "no bytes sent: {json}");
}

#[test]
fn tcp_loopback_moves_bytes() {
    let json = run_loopback(false, 53112);
    assert!(json.contains("\"proto\":\"tcp\""), "unexpected summary: {json}");
    assert!(field(&json, "bytes") > 0, "no bytes transferred: {json}");
}
