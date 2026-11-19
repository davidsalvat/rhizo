fn main() {
    // Static executables with no runtime dependencies: entry `_start`, no
    // PIE, segments page-aligned so every page has one set of permissions.
    for arg in [
        "-e_start",
        "-static",
        "--no-pie",
        "-zseparate-loadable-segments",
        "-zmax-page-size=4096",
        "--strip-all",
    ] {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
