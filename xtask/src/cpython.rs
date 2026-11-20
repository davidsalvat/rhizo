//! CPython as a cell (spec `workloads`, change `add-cpython`): a pinned
//! release built as a static executable (design D1) and the files it runs
//! with (D2), the same for the kernel, the differential check on Linux and
//! the Linux guest of the comparison.
//!
//! Built only from a given source tarball (`RHIZO_CPYTHON_SRC`, fetched by
//! CI) and only natively; otherwise every CPython check says it did not run.

use crate::Result;
use crate::report::{self, Failure};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned release (PSF License 2.0, permissive).
pub const VERSION: &str = "3.13.18";
/// SHA-256 of `Python-3.13.18.tar.xz` as python.org publishes it.
pub const SHA256: &str = "abb0102e6d4ae027509bcb82327d9dd85515b44a3ba8ad538cfc5e9847029f68";
/// Where the cell finds the standard library: `<prefix>/lib/python3.X`,
/// found by CPython from its executable `/bin/python3` (D2).
pub const STDLIB: &str = "lib/python3.13";

/// C extension modules built into the executable. The standard library's
/// other C modules are not built at all (`make python` builds no shared
/// module): what is left out needs what cells do not have (sockets,
/// processes, `dlopen`) or is not used by the workload.
const SETUP_LOCAL: &str = "\
*static*
_bisect _bisectmodule.c
_datetime _datetimemodule.c
_heapq _heapqmodule.c
_json _json.c
_random _randommodule.c
_struct _struct.c
binascii binascii.c
math mathmodule.c
zlib zlibmodule.c
";
const CONFIGURE: [&str; 3] = [
    "--disable-shared",
    "--without-ensurepip",
    "--disable-test-modules",
];

/// The build directory of the selected architecture.
fn dir() -> PathBuf {
    crate::target_dir()
        .join("cpython")
        .join(crate::arch::current().id())
}

/// The tree a CPython cell sees (`bin/python3`, the standard library under
/// [`STDLIB`], `app/agent_demo.py`), as a directory on the host; present
/// once [`build`] has built CPython.
pub fn root() -> PathBuf {
    dir().join("root")
}

/// Builds CPython into `out` as `cpython` (the stripped static executable)
/// and `cpython-files` (the cell's files, encoded as a cell file system)
/// when `RHIZO_CPYTHON_SRC` names the pinned tarball and the host can run
/// the result; otherwise removes them, so the kernel embeds nothing.
pub fn build(out: &Path) -> Result {
    let (exe, files) = (out.join("cpython"), out.join("cpython-files"));
    let skip = |why: String| {
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::remove_file(&files);
        let _ = std::fs::remove_dir_all(root());
        if let Some(why) = why.strip_prefix('!') {
            report::text(&format!("[xtask] CPython not built: {why}"));
        }
        Ok(())
    };
    let Some(tarball) = env::var_os("RHIZO_CPYTHON_SRC") else {
        return skip(String::new());
    };
    if let Some(why) = crate::differential::unavailable_reason() {
        return skip(format!("!it is built natively only ({why})"));
    }
    let python = build_interpreter(Path::new(&tarball))?;
    let root = root();
    let _ = std::fs::remove_dir_all(&root);
    let stdlib = root.join(STDLIB);
    for d in [
        root.join("bin"),
        root.join("app"),
        stdlib.join("lib-dynload"),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    let script = crate::workspace_root().join("cells/python/agent_demo.py");
    std::fs::copy(&script, root.join("app/agent_demo.py")).map_err(|e| e.to_string())?;
    let pack = crate::workspace_root().join("cells/python/pack_stdlib.py");
    let mut cmd = Command::new(&python);
    cmd.env_clear().arg(&pack).arg(&script).arg(&stdlib);
    run(
        &mut cmd,
        "pack the standard library",
        &dir().join("pack.log"),
    )?;
    let mut cmd = Command::new("strip");
    cmd.arg("-o").arg(root.join("bin/python3")).arg(&python);
    run(&mut cmd, "strip python", &dir().join("strip.log"))?;
    let bin = std::fs::read(root.join("bin/python3")).map_err(|e| e.to_string())?;
    write_if_changed(&exe, &bin)?;
    write_if_changed(&files, &encode(&root)?)
}

/// Builds `python` from the tarball unless the build directory already
/// holds one built with the same recipe; returns its path.
fn build_interpreter(tarball: &Path) -> Result<PathBuf> {
    let dir = dir();
    let src = dir.join(format!("Python-{VERSION}"));
    let python = src.join("python");
    let recipe = format!("{VERSION} {}\n{SETUP_LOCAL}", CONFIGURE.join(" "));
    let stamp = dir.join("recipe");
    if python.exists() && std::fs::read_to_string(&stamp).ok().as_deref() == Some(&recipe) {
        return Ok(python);
    }
    check_sha256(tarball)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut tar = Command::new("tar");
    tar.arg("-xf").arg(tarball).arg("-C").arg(&dir);
    run(&mut tar, "unpack CPython", &dir.join("tar.log"))?;
    std::fs::write(src.join("Modules/Setup.local"), SETUP_LOCAL).map_err(|e| e.to_string())?;
    let mut configure = Command::new("./configure");
    configure
        .args(CONFIGURE)
        .env("LDFLAGS", "-static")
        .current_dir(&src);
    run(
        &mut configure,
        "configure CPython",
        &dir.join("configure.log"),
    )?;
    let jobs = std::thread::available_parallelism().map_or(2, |n| n.get());
    let mut make = Command::new("make");
    make.arg(format!("-j{jobs}"))
        .args(["python", "pybuilddir.txt", "LINKFORSHARED="])
        .current_dir(&src);
    run(&mut make, "build CPython", &dir.join("make.log"))?;
    std::fs::write(&stamp, recipe).map_err(|e| e.to_string())?;
    Ok(python)
}

/// Checks the tarball against [`SHA256`].
fn check_sha256(tarball: &Path) -> Result {
    let out = Command::new("sha256sum")
        .arg(tarball)
        .output()
        .map_err(|e| format!("cannot run sha256sum: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let got = text.split_whitespace().next().unwrap_or_default();
    if got == SHA256 {
        return Ok(());
    }
    Err(
        Failure::new("build", "the CPython tarball is not the pinned release")
            .location(tarball.display().to_string())
            .expected(format!("Python-{VERSION}.tar.xz, SHA-256 {SHA256}"))
            .actual(got.to_string())
            .hint("download the pinned release from python.org, or update VERSION and SHA256 on purpose")
            .reproduce(format!("sha256sum {}", tarball.display()))
            .into_error(),
    )
}

/// Runs `cmd` with its output in `log`; on failure, reports the log's end.
fn run(cmd: &mut Command, what: &str, log: &Path) -> Result {
    let file = std::fs::File::create(log).map_err(|e| e.to_string())?;
    let status = cmd
        .stdout(file.try_clone().map_err(|e| e.to_string())?)
        .stderr(file)
        .status()
        .map_err(|e| format!("cannot {what}: {e}"))?;
    if status.success() {
        return Ok(());
    }
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(40).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(Failure::new("build", format!("cannot {what}"))
        .location(log.display().to_string())
        .expected("success")
        .actual(format!("{status}; end of the log:\n{}", tail.join("\n")))
        .reproduce(format!("{cmd:?}"))
        .into_error())
}

fn write_if_changed(path: &Path, data: &[u8]) -> Result {
    if std::fs::read(path).ok().as_deref() == Some(data) {
        return Ok(()); // unchanged: the kernel is not rebuilt for nothing
    }
    std::fs::write(path, data).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The cell's files: `root` without `bin` (the executable is the image
/// the cell is spawned from), encoded as a cell file system (D2).
fn encode(root: &Path) -> Result<Vec<u8>> {
    let mut fs = rhizo_fs::Fs::new();
    for top in ["lib", "app"] {
        add_tree(&mut fs, &root.join(top), &format!("/{top}"))?;
    }
    let mut out = Vec::new();
    fs.encode(&mut out);
    Ok(out)
}

/// Copies directory `dir` into `fs` at `at`, in name order.
fn add_tree(fs: &mut rhizo_fs::Fs, dir: &Path, at: &str) -> Result {
    let failed = |e: rhizo_fs::Error| format!("cannot add {at} to the cell's files: {e:?}");
    fs.mkdir(None, at.as_bytes()).map_err(failed)?;
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = format!("{at}/{}", entry.file_name().to_string_lossy());
        let path = entry.path();
        if path.is_dir() {
            add_tree(fs, &path, &name)?;
        } else {
            let data = std::fs::read(&path).map_err(|e| e.to_string())?;
            fs.put_file(name.as_bytes(), &data)
                .map_err(|e| format!("cannot add {name} to the cell's files: {e:?}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_becomes_a_cell_file_system() {
        let root = crate::target_dir().join(format!("cpython-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib/python3.13/json/empty")).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("lib/python3.13/os.pyc"), b"os").unwrap();
        std::fs::write(root.join("lib/python3.13/json/__init__.pyc"), [7u8; 5000]).unwrap();
        std::fs::write(root.join("app/agent_demo.py"), b"print(1)\n").unwrap();
        std::fs::write(root.join("bin/python3"), b"ELF").unwrap();
        let bytes = encode(&root).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        let mut fs = rhizo_fs::Fs::decode(&bytes).expect("a valid encoding");
        let read = |fs: &mut rhizo_fs::Fs, path: &[u8]| {
            let fd = fs
                .open(
                    None,
                    path,
                    rhizo_fs::OpenFlags {
                        read: true,
                        ..Default::default()
                    },
                    0,
                )
                .unwrap();
            let mut buf = vec![0; 8192];
            let n = fs.read(fd, &mut buf, None).unwrap();
            fs.close(fd).unwrap();
            buf.truncate(n);
            buf
        };
        assert_eq!(read(&mut fs, b"/lib/python3.13/os.pyc"), b"os");
        assert_eq!(
            read(&mut fs, b"/lib/python3.13/json/__init__.pyc"),
            [7u8; 5000]
        );
        assert_eq!(read(&mut fs, b"/app/agent_demo.py"), b"print(1)\n");
        assert_eq!(
            fs.stat(None, b"/lib/python3.13/json/empty").unwrap().kind,
            rhizo_fs::Kind::Dir
        );
        assert!(
            fs.stat(None, b"/bin").is_err(),
            "the executable is not a file"
        );
    }
}
