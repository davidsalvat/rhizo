use std::path::Path;

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let script = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "linker-aarch64.ld",
        _ => "linker.ld",
    };
    println!("cargo:rustc-link-arg-bins=-T{dir}/{script}");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=linker-aarch64.ld");
    embed_cells();
}

/// Copies the test cells built by `cargo xtask` (`RHIZO_CELLS_DIR`) next to
/// the build output, where `include_bytes!` finds them. Without it (a plain
/// `cargo clippy`), empty files stand in and spawning them fails with
/// `NotElf`, which the in-kernel tests report.
/// Every program in `cells/src/bin`.
const CELLS: &[&str] = &[
    "hello",
    "exit3",
    "abi",
    "fault_null",
    "fault_kernel",
    "chatter",
    "spin",
    "sleeper",
    "same_addr",
    "mapper",
    "peek",
];

fn embed_cells() {
    println!("cargo:rerun-if-env-changed=RHIZO_CELLS_DIR");
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo")).join("cells");
    std::fs::create_dir_all(&out).expect("create OUT_DIR/cells");
    let from = std::env::var("RHIZO_CELLS_DIR").ok();
    for name in CELLS {
        let data = match &from {
            Some(dir) => {
                let path = Path::new(dir).join(name);
                println!("cargo:rerun-if-changed={}", path.display());
                std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            }
            None => Vec::new(),
        };
        std::fs::write(out.join(name), data).expect("write embedded cell");
    }
}
