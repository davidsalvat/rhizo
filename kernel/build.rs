use std::path::Path;

/// Cells built only when their source is given, or by `xtask` only.
const OPTIONAL: [&str; 7] = [
    "micropython",
    "cpython",
    "cpython-files",
    "dynamic-files",
    "distro-files",
    "packages-files",
    "node-files",
];
/// Cells embedded in test kernels only (`ktest`): elsewhere they read as
/// not built, and take no memory from the cells a benchmark runs (its guest
/// has 128 MiB, and the 50 MiB fork benchmark needs most of it). The C
/// cells here are those no benchmark runs.
const TEST_ONLY: [&str; 9] = [
    "distro-files",
    "libc_alloc",
    "libc_deep_stack",
    "libc_files",
    "libc_float",
    "libc_hwcap",
    "libc_hello",
    "libc_net",
    "libc_threads",
];

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
/// the build output and generates `cells.rs`, a `test_image(name)` lookup
/// with one `include_bytes!` per program in `cells/src/bin`. Without
/// `RHIZO_CELLS_DIR` (a plain `cargo clippy`), empty files stand in and
/// spawning them fails with `NotElf`, which the in-kernel tests report.
fn embed_cells() {
    println!("cargo:rerun-if-env-changed=RHIZO_CELLS_DIR");
    // An optional cell that is not built is not watched (cargo would run
    // this script again on every build for a missing file): `xtask` builds
    // it only once its source is given, which these variables say (change
    // `fix-cli-nits`).
    println!("cargo:rerun-if-env-changed=RHIZO_MICROPYTHON_SRC");
    println!("cargo:rerun-if-env-changed=RHIZO_CPYTHON_SRC");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    // Rust cells (cells/src/bin/*.rs) and C cells (cells/c/*.c).
    let mut names: Vec<String> = Vec::new();
    for (dir, suffix) in [("../cells/src/bin", ".rs"), ("../cells/c", ".c")] {
        let dir = Path::new(&manifest).join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        names.extend(
            std::fs::read_dir(&dir)
                .expect("cells directory exists")
                .filter_map(|e| {
                    e.ok()?
                        .file_name()
                        .to_str()?
                        .strip_suffix(suffix)
                        .map(String::from)
                }),
        );
    }
    // MicroPython and CPython (its executable and its files), built by
    // `xtask` only when their source is given (`RHIZO_MICROPYTHON_SRC`,
    // `RHIZO_CPYTHON_SRC`), and the files of the dynamically linked
    // programs; an empty image means "not built".
    names.extend(OPTIONAL.iter().map(|n| n.to_string()));
    names.sort();

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let out = Path::new(&out_dir).join("cells");
    std::fs::create_dir_all(&out).expect("create OUT_DIR/cells");
    let from = std::env::var("RHIZO_CELLS_DIR").ok();
    let mut lookup = String::from(
        "/// The test cells built by `cargo xtask` and embedded at build time.\n\
         pub fn test_image(name: &str) -> &'static [u8] {\n    \
         find_image(name.as_bytes()).unwrap_or_else(|| panic!(\"no test cell named {name}\"))\n}\n\n\
         /// The embedded cell named `name`, if any (empty: not built).\n\
         pub fn find_image(name: &[u8]) -> Option<&'static [u8]> {\n    Some(match name {\n",
    );
    let ktest = std::env::var_os("CARGO_FEATURE_KTEST").is_some();
    // Node.js (over 100 MiB) only in the kernels built to run it, whose VMs
    // get the memory it needs (change `add-node-workload`).
    let node = std::env::var_os("CARGO_FEATURE_NODE").is_some();
    // The packages' CPython (65 MiB) likewise (change `add-pip-packages`).
    let packages = std::env::var_os("CARGO_FEATURE_PACKAGES").is_some();
    for name in &names {
        let data = match &from {
            Some(_) if TEST_ONLY.contains(&name.as_str()) && !ktest => Vec::new(),
            Some(_) if name == "node-files" && !node => Vec::new(),
            Some(_) if name == "packages-files" && !packages => Vec::new(),
            Some(dir) => {
                let path = Path::new(dir).join(name);
                match std::fs::read(&path) {
                    Ok(data) => {
                        println!("cargo:rerun-if-changed={}", path.display());
                        data
                    }
                    Err(_) if OPTIONAL.contains(&name.as_str()) => Vec::new(),
                    Err(e) => panic!("{}: {e}", path.display()),
                }
            }
            None => Vec::new(),
        };
        std::fs::write(out.join(name), data).expect("write embedded cell");
        lookup += &format!(
            "        b{name:?} => include_bytes!(concat!(env!(\"OUT_DIR\"), \"/cells/{name}\")) as &[u8],\n"
        );
    }
    lookup += "        _ => return None,\n    })\n}\n";
    std::fs::write(Path::new(&out_dir).join("cells.rs"), lookup).expect("write cells.rs");
}
