fn main() {
    if std::env::var("CARGO_FEATURE_XDP").is_ok() {
        compile_xdp_program();
    }
}

fn compile_xdp_program() {
    use std::process::Command;

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let src = "ebpf/runperf_xdp.c";
    let out = format!("{out_dir}/runperf_xdp.o");

    println!("cargo:rerun-if-changed={src}");

    // Resolve the arch-specific include path for asm/types.h and friends.
    // Try common Debian/Ubuntu multiarch paths, then fall back to the sysroot.
    let arch_include = arch_include_path();

    let mut cmd = Command::new("clang");
    cmd.args(["-target", "bpf", "-O2", "-g", "-c", src, "-o", &out]);
    if let Some(ref inc) = arch_include {
        cmd.arg(format!("-I{inc}"));
    }

    let status = cmd.status().unwrap_or_else(|e| {
        panic!(
            "clang not found: {e}\n\
             Install with: apt install clang libbpf-dev"
        )
    });

    if !status.success() {
        panic!(
            "clang failed compiling {src}\n\
             Install build deps: apt install clang libbpf-dev"
        );
    }

    println!("cargo:rustc-env=XDP_BPF_OBJ={out}");
}

fn arch_include_path() -> Option<String> {
    // The BPF object is architecture-neutral: it is the same regardless of the Rust target,
    // so compile it with the BUILD HOST's kernel headers, NOT the (cross-)target's. Using the
    // target arch breaks cross-compilation — building the aarch64 target on an x86_64 CI runner
    // looks for /usr/include/aarch64-linux-gnu (absent) and clang then can't find asm/types.h.
    // `std::env::consts::ARCH` is the host arch (build scripts run on the host); fall back to
    // any present multiarch dir.
    let host = match std::env::consts::ARCH {
        "x86_64"  => "x86_64-linux-gnu",
        "aarch64" => "aarch64-linux-gnu",
        "arm"     => "arm-linux-gnueabihf",
        _         => "",
    };
    for multiarch in [host, "x86_64-linux-gnu", "aarch64-linux-gnu"] {
        if multiarch.is_empty() {
            continue;
        }
        let path = format!("/usr/include/{multiarch}");
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    None
}
