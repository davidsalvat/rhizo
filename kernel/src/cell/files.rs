//! Cells' file systems: the empty one every cell without files
//! shares, and those `xtask` packs for the runtimes (CPython, dynamically
//! linked programs, packages, Node.js), decoded once and shared.

use super::*;

/// The file system of a cell given no files, and its encoding: built once,
/// then shared copy-on-write by every such cell, so spawning or resuming one
/// builds no table.
static NO_FILES: Global<Option<(Fs, Vec<u8>)>> = Global::new(None);

fn with_no_files<R>(f: impl FnOnce(&Fs, &[u8]) -> R) -> R {
    NO_FILES.with(|slot| {
        let (fs, bytes) = slot.get_or_insert_with(|| {
            let fs = Fs::new();
            let mut bytes = Vec::new();
            fs.encode(&mut bytes);
            (fs, bytes)
        });
        f(fs, bytes)
    })
}

/// An empty file system for a new cell (shared until the cell writes).
pub fn no_files() -> Fs {
    with_no_files(|fs, _| fs.clone())
}

/// The file system a snapshot holds, shared with [`NO_FILES`] when it has
/// no files.
pub(super) fn files_of(snapshot: &Snapshot) -> Fs {
    with_no_files(|fs, bytes| (snapshot.files_bytes() == bytes).then(|| fs.clone()))
        .unwrap_or_else(|| snapshot.files())
}

/// The files a CPython cell starts with (`cpython-files`, packed by `xtask`:
/// the standard library under `/usr/lib/python3.13` and `/app/agent_demo.py`;
/// design D2 of `add-cpython`), decoded once and published, then shared
/// copy-on-write by every CPython cell; `None` when CPython was not built.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn cpython_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    let bytes = test_image("cpython-files");
    if bytes.is_empty() {
        return None;
    }
    Some(FILES.with(|files| files.get_or_insert_with(|| published(bytes)).clone()))
}

/// The host's file set `bytes`, decoded and published (design D21 of
/// `add-cell-hierarchy`): public, the pages of its executables and
/// libraries loaded now and kept as long as the kernel runs, shared by
/// every domain.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
fn published(bytes: &[u8]) -> Fs {
    let mut fs = Fs::decode(bytes).expect("xtask packs a valid file system");
    publish_files(&mut fs);
    fs
}

/// The files of the dynamically linked `hello` (`dynamic-files`, packed by
/// `xtask` with its interpreter and libraries; change
/// `add-dynamic-linking`), decoded once and published, then shared
/// copy-on-write by every cell given them; `None` when they were not
/// packed.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn dynamic_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    decoded(&FILES, "dynamic-files")
}

/// The files of the distribution's programs (`distro-files`: `ls`, `sort`,
/// `python3`, their interpreter, libraries and data), as
/// [`dynamic_files`]; packed natively only, and embedded in test kernels
/// only.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn distro_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    decoded(&FILES, "distro-files")
}

/// The files of the dynamically linked CPython with packages from PyPI
/// (`packages-files`: numpy, requests and what they need, change
/// `add-pip-packages`), as [`dynamic_files`]; packed natively only, with
/// CPython, and embedded in the `packages` kernels only.
#[cfg_attr(
    not(any(feature = "packages", feature = "bench-cell")),
    allow(dead_code, reason = "used by the package tests and benchmark")
)]
pub fn packages_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    decoded(&FILES, "packages-files")
}

/// The files of Node.js (`node-files`: the official build, its interpreter
/// and libraries, `/app/demo.js`; change `add-node-workload`), as
/// [`dynamic_files`]; embedded in the `node` kernels only.
#[cfg_attr(
    not(any(feature = "node", feature = "bench-cell")),
    allow(dead_code, reason = "used by the Node.js tests and benchmark")
)]
pub fn node_files() -> Option<Fs> {
    static FILES: Global<Option<Fs>> = Global::new(None);
    decoded(&FILES, "node-files")
}

/// The file system packed as test image `name`, decoded into `slot` the
/// first time; `None` when it was not packed.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
fn decoded(slot: &Global<Option<Fs>>, name: &str) -> Option<Fs> {
    let bytes = test_image(name);
    if bytes.is_empty() {
        return None;
    }
    Some(slot.with(|files| files.get_or_insert_with(|| published(bytes)).clone()))
}
