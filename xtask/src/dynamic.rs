//! Dynamically linked programs as cells (spec `workloads`, change
//! `add-dynamic-linking`), each tree packed with the interpreter (`ld.so`)
//! and the shared libraries its programs need into a cell file system, laid
//! out as the distribution lays them out:
//!
//! - `dynamic-files`: the dynamic `hello` C cell (`cells/c/dynamic/hello.c`),
//!   small, for every kernel that runs cells (tests and benchmarks);
//! - `distro-files`: where the cells' architecture is the host's, the
//!   distribution's own `ls`, `sort` and `python3`, embedded in test
//!   kernels only (the benchmark kernel keeps its memory for cells);
//! - `node-files`: where the cells' architecture is the host's and
//!   `RHIZO_NODE` is set, the official Node.js build (`tools::node`) with
//!   `cells/node/demo.js`, embedded only in the kernels built with the
//!   `node` feature (change `add-node-workload`).
//!
//! The interpreter, libraries and programs are the build host's (glibc:
//! LGPL-2.1-or-later; coreutils: GPL-3.0-or-later; libselinux: public
//! domain; PCRE2: BSD-3-Clause; OpenSSL: Apache-2.0; zlib: Zlib; expat:
//! MIT; CPython: PSF-2.0; libstdc++ and libgcc_s: GPL-3.0-or-later with the
//! GCC Runtime Library Exception; Node.js: MIT, with the licenses of its
//! bundled dependencies in the release's `LICENSE`), copied at build time as
//! test inputs and run unmodified. Nothing of them is copied into the
//! repository.

use crate::Result;
use crate::report::Failure;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Files the programs read, from the top of the cell's files.
const DATA: [(&str, &str); 2] = [
    ("data/fruit", "pear\napple\nfig\ncherry\nbanana\n"),
    ("data/numbers", "3\n1\n2\n"),
];

/// Programs of the distribution packed when building natively: where they
/// are on the host and in the cell (`python3` is the interpreter
/// `/usr/bin/python3` names).
const PROGRAMS: [(&str, &str); 3] = [
    ("/usr/bin/ls", "usr/bin/ls"),
    ("/usr/bin/sort", "usr/bin/sort"),
    ("/usr/bin/python3", "usr/bin/python3"),
];

/// Where Node.js and its script are in the cell's files.
pub const NODE: &str = "usr/bin/node";
pub const NODE_SCRIPT: &str = "app/demo.js";

/// The trees the cells' files are made from, as directories on the host:
/// the dynamic `hello`, the distribution's programs, Node.js.
pub fn roots() -> [PathBuf; 3] {
    let dir = crate::target_dir()
        .join("dynamic")
        .join(crate::arch::current().id());
    [dir.join("dynamic"), dir.join("distro"), dir.join("node")]
}

/// GNU triple of the cells' architecture (the distribution's library
/// directory).
fn triple() -> String {
    format!("{}-linux-gnu", crate::arch::current().id())
}

/// Builds the trees ([`roots`]) and writes them, encoded as cell file
/// systems, to `out/dynamic-files` and (natively) `out/distro-files` and,
/// with `RHIZO_NODE`, `out/node-files`.
pub fn build(out: &Path) -> Result {
    let [hello, distro, node] = roots();
    let native = crate::differential::unavailable_reason().is_none();
    fresh(&hello, "usr/bin")?;
    compile_hello(&hello.join("usr/bin/hello"))?;
    pack(&hello, native, &out.join("dynamic-files"))?;
    let node_files = out.join("node-files");
    if native && std::env::var_os("RHIZO_NODE").is_some() {
        fresh(&node, "app")?;
        copy(&crate::tools::node()?, &node.join(NODE))?;
        copy(
            &crate::workspace_root().join("cells/node/demo.js"),
            &node.join(NODE_SCRIPT),
        )?;
        pack(&node, native, &node_files)?;
    } else {
        let _ = std::fs::remove_dir_all(&node);
        let _ = std::fs::remove_file(&node_files);
    }
    let files = out.join("distro-files");
    if !native {
        let _ = std::fs::remove_dir_all(&distro);
        let _ = std::fs::remove_file(&files);
        return Ok(());
    }
    fresh(&distro, "data")?;
    for (path, text) in DATA {
        std::fs::write(distro.join(path), text).map_err(|e| e.to_string())?;
    }
    for (host, cell) in PROGRAMS {
        if Path::new(host).exists() {
            copy(Path::new(host), &distro.join(cell))?;
        }
    }
    if distro.join("usr/bin/python3").exists() {
        pack_python(&distro)?;
    }
    pack(&distro, native, &files)
}

/// Empties the tree `root` and makes its directory `sub`.
fn fresh(root: &Path, sub: &str) -> Result {
    let _ = std::fs::remove_dir_all(root);
    let dir = root.join(sub);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

/// Adds the interpreter and libraries to the tree `root`, then writes it
/// to `file` as a cell file system.
fn pack(root: &Path, native: bool, file: &Path) -> Result {
    add_libraries(root, native)?;
    let mut fs = rhizo_fs::Fs::new();
    let mut tops: Vec<_> = std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .collect();
    tops.sort_by_key(|e| e.file_name());
    for top in tops {
        let at = format!("/{}", top.file_name().to_string_lossy());
        crate::cpython::add_tree(&mut fs, &top.path(), &at)?;
    }
    let mut bytes = Vec::new();
    fs.encode(&mut bytes);
    crate::cpython::write_if_changed(file, &bytes)
}

fn copy(from: &Path, to: &Path) -> Result {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}

/// `cells/c/dynamic/hello.c`, linked dynamically with the system glibc.
fn compile_hello(target: &Path) -> Result {
    let source = crate::workspace_root().join("cells/c/dynamic/hello.c");
    let cc = crate::c_compiler();
    let status = Command::new(cc)
        .args(["-O2", "-o"])
        .arg(target)
        .arg(&source)
        .status();
    if matches!(&status, Ok(s) if s.success()) {
        return Ok(());
    }
    Err(
        Failure::new("build", "cannot compile the dynamic C cell `hello`")
            .location(source.display().to_string())
            .expected("a dynamically linked executable")
            .actual(match status {
                Ok(s) => s.to_string(),
                Err(e) => e.to_string(),
            })
            .reproduce(format!(
                "{cc} -O2 -o {} {}",
                target.display(),
                source.display()
            ))
            .into_error(),
    )
}

/// The standard library `python3` needs for `cells/python/dynamic_demo.py`
/// (as `app/demo.py`), compiled by that interpreter at its relative path
/// under the cell's `usr/lib/python3.X`, with its extension modules
/// (design D5 of `add-dynamic-linking`).
fn pack_python(root: &Path) -> Result {
    let python = root.join("usr/bin/python3");
    let script = crate::workspace_root().join("cells/python/dynamic_demo.py");
    copy(&script, &root.join("app/demo.py"))?;
    let lib = Command::new(&python)
        .args(["-S", "-c", "import os; print(os.path.dirname(os.__file__))"])
        .env_clear()
        .output()
        .map_err(|e| format!("cannot run {}: {e}", python.display()))?;
    let lib = String::from_utf8_lossy(&lib.stdout).trim().to_string();
    let stdlib = root.join(lib.trim_start_matches('/'));
    std::fs::create_dir_all(stdlib.join("lib-dynload")).map_err(|e| e.to_string())?;
    let pack = crate::workspace_root().join("cells/python/pack_stdlib.py");
    let out = Command::new(&python)
        .arg("-S")
        .arg(&pack)
        .arg(&stdlib)
        .arg(&script)
        .env_clear()
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    Err(
        Failure::new("build", "cannot pack the standard library of python3")
            .expected("exit status 0")
            .actual(format!(
                "{}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ))
            .reproduce(format!(
                "{} -S {} {} {}",
                python.display(),
                pack.display(),
                stdlib.display(),
                script.display()
            ))
            .into_error(),
    )
}

/// The interpreter and `DT_NEEDED` libraries of an ELF file (`readelf`).
fn dynamic_info(file: &Path) -> Result<(Option<String>, Vec<String>)> {
    let out = Command::new("readelf")
        .args(["-ldW"])
        .arg(file)
        .output()
        .map_err(|e| format!("cannot run readelf (binutils): {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let between = |line: &str, start: &str| {
        let rest = &line[line.find(start)? + start.len()..];
        Some(rest[..rest.find(']')?].to_string())
    };
    let interp = text
        .lines()
        .find_map(|l| between(l, "[Requesting program interpreter: "));
    let needed = text
        .lines()
        .filter(|l| l.contains("(NEEDED)"))
        .filter_map(|l| between(l, "Shared library: ["))
        .collect();
    Ok((interp, needed))
}

/// Adds, for every ELF file in `root`, its interpreter at the path it names
/// and the libraries it needs (and theirs) under `lib/<triple>`, a
/// directory the interpreter searches by default: from the host's system
/// directories, or from the cross toolchain's when `native` is false.
fn add_libraries(root: &Path, native: bool) -> Result {
    let triple = triple();
    let search: Vec<PathBuf> = if native {
        [
            format!("/lib/{triple}"),
            format!("/usr/lib/{triple}"),
            "/lib".into(),
            "/usr/lib".into(),
        ]
        .map(PathBuf::from)
        .to_vec()
    } else {
        vec![PathBuf::from(format!("/usr/{triple}/lib"))]
    };
    let mut todo = Vec::new();
    for file in elf_files(root)? {
        todo.push((file.clone(), dynamic_info(&file)?));
    }
    // Libraries need the interpreter too, by name: it is loaded already.
    let mut seen: BTreeSet<String> = todo
        .iter()
        .filter_map(|(_, (interp, _))| Some(interp.as_ref()?.rsplit('/').next()?.to_string()))
        .collect();
    while let Some((file, (interp, needed))) = todo.pop() {
        if let Some(interp) = interp {
            let name = interp.rsplit('/').next().unwrap_or_default().to_string();
            let at = root.join(interp.trim_start_matches('/'));
            if !at.exists() {
                let host = search.iter().map(|d| d.join(&name)).find(|p| p.exists());
                let host = if native {
                    Some(PathBuf::from(&interp))
                } else {
                    host
                };
                copy(&host.ok_or(format!("no interpreter {interp}"))?, &at)?;
            }
        }
        for name in needed {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(host) = search.iter().map(|d| d.join(&name)).find(|p| p.exists()) else {
                return Err(format!(
                    "{} needs {name}, found in none of {search:?}",
                    file.display()
                ));
            };
            let at = root.join("lib").join(&triple).join(&name);
            copy(&host, &at)?;
            todo.push((at.clone(), dynamic_info(&at)?));
        }
    }
    Ok(())
}

/// Every ELF file under `dir`.
fn elf_files(dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(files(dir)?
        .into_iter()
        .filter(|p| std::fs::read(p).is_ok_and(|d| d.starts_with(b"\x7fELF")))
        .collect())
}

/// Every file under `dir`, in name order.
pub fn files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    entries.sort();
    let mut out = Vec::new();
    for path in entries {
        if path.is_dir() {
            out.extend(files(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}
