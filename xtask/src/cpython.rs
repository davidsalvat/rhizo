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
/// found by CPython from its executable `/usr/bin/python3` (D2).
pub const STDLIB: &str = "usr/lib/python3.13";

/// C extension modules built into the executable. The standard library's
/// other C modules are not built at all (`make python` builds no shared
/// module): what is left out needs what cells do not have (processes,
/// `dlopen`) or is not used by the workloads. For the agent (design D2 of
/// `add-agent-workload`): sockets, `select` and `array` (imported by
/// `socket`) for its connection to its model, `unicodedata` for the `idna`
/// codec `getaddrinfo` encodes host names with, and the hash modules
/// `hashlib` imports (HACL* and BLAKE2 code in CPython's tree, no OpenSSL).
const SETUP_LOCAL: &str = "\
*static*
_bisect _bisectmodule.c
_blake2 _blake2/blake2module.c _blake2/blake2b_impl.c _blake2/blake2s_impl.c
_datetime _datetimemodule.c
_heapq _heapqmodule.c
_json _json.c
_md5 md5module.c -I$(srcdir)/Modules/_hacl/include _hacl/Hacl_Hash_MD5.c -D_BSD_SOURCE -D_DEFAULT_SOURCE
_random _randommodule.c
_sha1 sha1module.c -I$(srcdir)/Modules/_hacl/include _hacl/Hacl_Hash_SHA1.c -D_BSD_SOURCE -D_DEFAULT_SOURCE
_sha2 sha2module.c -I$(srcdir)/Modules/_hacl/include _hacl/Hacl_Hash_SHA2.c -D_BSD_SOURCE -D_DEFAULT_SOURCE
_sha3 sha3module.c -I$(srcdir)/Modules/_hacl/include _hacl/Hacl_Hash_SHA3.c -D_BSD_SOURCE -D_DEFAULT_SOURCE
_socket socketmodule.c
_struct _struct.c
array arraymodule.c
binascii binascii.c
math mathmodule.c
select selectmodule.c
unicodedata unicodedata.c
zlib zlibmodule.c
";
/// The scripts a CPython cell runs, from `cells/python` to `/app`.
pub const SCRIPTS: [&str; 3] = ["agent_demo.py", "agent_loop.py", "agent_chat.py"];
/// Linked statically, with its segments on 2 MiB boundaries so the kernel
/// maps whole 2 MiB pieces of them with 2 MiB pages (design D2 of
/// `add-large-image-pages`).
const LDFLAGS: &str = "-static -Wl,-z,max-page-size=0x200000";
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

/// The tree a CPython cell sees (`usr/bin/python3`, the standard library under
/// [`STDLIB`], the [`SCRIPTS`] in `app`), as a directory on the host; present
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
        root.join("usr/bin"),
        root.join("app"),
        stdlib.join("lib-dynload"),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    let scripts = crate::workspace_root().join("cells/python");
    for script in SCRIPTS {
        std::fs::copy(scripts.join(script), root.join("app").join(script))
            .map_err(|e| e.to_string())?;
    }
    let pack = scripts.join("pack_stdlib.py");
    let mut cmd = Command::new(&python);
    // From the build directory: a script may write files where it runs.
    cmd.env_clear()
        .current_dir(dir())
        .arg(&pack)
        .arg(&stdlib)
        .args(SCRIPTS.map(|s| scripts.join(s)));
    run(
        &mut cmd,
        "pack the standard library",
        &dir().join("pack.log"),
    )?;
    let mut cmd = Command::new("strip");
    cmd.arg("-o").arg(root.join("usr/bin/python3")).arg(&python);
    run(&mut cmd, "strip python", &dir().join("strip.log"))?;
    let bin = std::fs::read(root.join("usr/bin/python3")).map_err(|e| e.to_string())?;
    write_if_changed(&exe, &bin)?;
    write_if_changed(&files, &encode(&root)?)
}

/// The interpreter as built, with its symbols (the cell runs a stripped
/// copy; spec `profiling` names the functions of its samples from this).
pub fn interpreter() -> PathBuf {
    dir().join(format!("Python-{VERSION}")).join("python")
}

/// Builds `python` from the tarball unless the build directory already
/// holds one built with the same recipe; returns its path.
fn build_interpreter(tarball: &Path) -> Result<PathBuf> {
    let dir = dir();
    let src = dir.join(format!("Python-{VERSION}"));
    let python = interpreter();
    let recipe = format!("{VERSION} {}\n{SETUP_LOCAL}", CONFIGURE.join(" ")) + LDFLAGS;
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
        .env("LDFLAGS", LDFLAGS)
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

/// The SHA-256 of `file`, in hexadecimal.
pub(crate) fn sha256(file: &Path) -> Result<String> {
    let out = Command::new("sha256sum")
        .arg(file)
        .output()
        .map_err(|e| format!("cannot run sha256sum: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.split_whitespace().next().unwrap_or_default().into())
}

/// Checks the tarball against [`SHA256`].
pub(crate) fn check_sha256(tarball: &Path) -> Result {
    let got = sha256(tarball)?;
    if got == SHA256 {
        return Ok(());
    }
    Err(
        Failure::new("build", "the CPython tarball is not the pinned release")
            .location(tarball.display().to_string())
            .expected(format!("Python-{VERSION}.tar.xz, SHA-256 {SHA256}"))
            .actual(got)
            .hint("download the pinned release from python.org, or update VERSION and SHA256 on purpose")
            .reproduce(format!("sha256sum {}", tarball.display()))
            .into_error(),
    )
}

/// Runs `cmd` with its output in `log`; on failure, reports the log's end.
pub(crate) fn run(cmd: &mut Command, what: &str, log: &Path) -> Result {
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

pub(crate) fn write_if_changed(path: &Path, data: &[u8]) -> Result {
    if std::fs::read(path).ok().as_deref() == Some(data) {
        return Ok(()); // unchanged: the kernel is not rebuilt for nothing
    }
    std::fs::write(path, data).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The cell's files: `root` without `usr/bin` (the executable is the
/// image the cell is spawned from), and an empty `/tmp` for programs that
/// write scratch files there, encoded as a cell file system (D2).
fn encode(root: &Path) -> Result<Vec<u8>> {
    let mut fs = rhizo_fs::Fs::new();
    fs.mkdir(None, b"/usr")
        .map_err(|e| format!("cannot make /usr: {e:?}"))?;
    for top in ["usr/lib", "app"] {
        add_tree(&mut fs, &root.join(top), &format!("/{top}"))?;
    }
    fs.mkdir(None, b"/tmp")
        .map_err(|e| format!("cannot make /tmp: {e:?}"))?;
    let mut out = Vec::new();
    fs.encode(&mut out);
    Ok(out)
}

/// Copies directory `dir` into `fs` at `at`, in name order.
pub(crate) fn add_tree(fs: &mut rhizo_fs::Fs, dir: &Path, at: &str) -> Result {
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
        std::fs::create_dir_all(root.join("usr/lib/python3.13/json/empty")).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/lib/python3.13/os.pyc"), b"os").unwrap();
        std::fs::write(
            root.join("usr/lib/python3.13/json/__init__.pyc"),
            [7u8; 5000],
        )
        .unwrap();
        std::fs::write(root.join("app/agent_demo.py"), b"print(1)\n").unwrap();
        std::fs::write(root.join("usr/bin/python3"), b"ELF").unwrap();
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
        assert_eq!(read(&mut fs, b"/usr/lib/python3.13/os.pyc"), b"os");
        assert_eq!(
            read(&mut fs, b"/usr/lib/python3.13/json/__init__.pyc"),
            [7u8; 5000]
        );
        assert_eq!(read(&mut fs, b"/app/agent_demo.py"), b"print(1)\n");
        assert_eq!(
            fs.stat(None, b"/usr/lib/python3.13/json/empty")
                .unwrap()
                .kind,
            rhizo_fs::Kind::Dir
        );
        assert!(
            fs.stat(None, b"/usr/bin").is_err(),
            "the executable is not a file"
        );
        assert!(fs.stat(None, b"/tmp").is_ok(), "a /tmp for scratch files");
    }
}
