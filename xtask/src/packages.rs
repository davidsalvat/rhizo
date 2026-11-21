//! Python packages from PyPI in a cell (spec `workloads`, change
//! `add-pip-packages`): a dynamically linked CPython, built from the same
//! pinned tarball as the static one (design D1), with pinned wheels
//! installed into its `site-packages` as pip installs them (D2), packed with
//! the standard library and packages the scripts use and the shared
//! libraries they need into `packages-files`, a cell file system.
//!
//! Built only with `RHIZO_CPYTHON_SRC` and only natively, as CPython; the
//! wheels are downloaded from PyPI once and checked against their SHA-256.
//! They are test inputs, never copied into the repository: numpy
//! (BSD-3-Clause, with OpenBLAS: BSD-3-Clause, and libgfortran: GPL-3.0
//! with the GCC runtime library exception), requests (Apache-2.0), urllib3
//! (MIT), idna (BSD-3-Clause), charset-normalizer (MIT) and certifi
//! (MPL-2.0, a data file of certificates).

use crate::Result;
use crate::cpython;
use crate::report::Failure;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A pinned wheel: its file name on PyPI, where to download it, its
/// SHA-256, and the architecture it is for (`None`: pure Python).
struct Wheel {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
    arch: Option<&'static str>,
}

const PYPI: &str = "https://files.pythonhosted.org/packages";

/// The packages and their pinned releases, each built for CPython 3.13 and
/// glibc 2.28 or later where it has C code (manylinux_2_28).
const WHEELS: [Wheel; 7] = [
    Wheel {
        file: "numpy-2.5.4-cp313-cp313-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl",
        url: "da/24/3bd070f3269dc609d8f26b2643f62ef91bb415841c0b294805aaf7fe06da",
        sha256: "6ffa07666f8da0eef81d149934a626d0d95fbd6838432a33e66245423a9062c0",
        arch: Some("x86_64"),
    },
    Wheel {
        file: "numpy-2.5.4-cp313-cp313-manylinux_2_27_aarch64.manylinux_2_28_aarch64.whl",
        url: "db/b6/135bb0953b61dc21c6cafa14b424ae666944e4899cf140e00c2b322a1a45",
        sha256: "1c2e71b04c6cad90026e544501bbe0ab9290fa8a4d845e7e8c0d124fb429c988",
        arch: Some("aarch64"),
    },
    Wheel {
        file: "requests-2.34.2-py3-none-any.whl",
        url: "a0/f4/c67b0b3f1b9245e8d266f0f112c500d50e5b4e83cb6f3b71b6528104182a",
        sha256: "2a0d60c172f83ac6ab31e4554906c0f3b3588d37b5cb939b1c061f4907e278e0",
        arch: None,
    },
    Wheel {
        file: "urllib3-2.8.0-py3-none-any.whl",
        url: "92/9d/c4e665119135114480843e7ab388fa94d8480650450e6f8e26b70d323a4c",
        sha256: "0cf3cae568d36aa9576b28dfb35f11328f1cb974ca7647d9475ebb86c75ac6e3",
        arch: None,
    },
    Wheel {
        file: "idna-3.20-py3-none-any.whl",
        url: "58/a2/bb081bab032533a855d44de1d56f8e8426114ff1ba5d1f07a438a0a654f8",
        sha256: "ab7ae7122974553370f0bdb919e1a960b2cd1bc1ef0276416d896db81c14582c",
        arch: None,
    },
    Wheel {
        file: "charset_normalizer-3.5.2-py3-none-any.whl",
        url: "fc/ad/d07d7862a62ffa6d79d68074d14823243dd235a77c45262acbf6adeb28bf",
        sha256: "b6b751274acb69d77b3323d6b7dbaa3c7fdfc1eb829b7eb61d262f32e1af9685",
        arch: None,
    },
    Wheel {
        file: "certifi-2026.10.23-py3-none-any.whl",
        url: "32/d1/a3652fe2c19134d0d15922177821d863cbd7e842d53a21bbe7e8e3a1b5ac",
        sha256: "e9b54de76e530bb6cf0ccd036787103acb58fd42ba930ea8bf948ddbaa2f61d1",
        arch: None,
    },
];

/// The scripts a packages cell runs, from `cells/python` to `/app`.
pub const SCRIPTS: [&str; 2] = ["numpy_demo.py", "requests_demo.py"];

/// Configured like the static CPython but linked dynamically: extension
/// modules are shared objects in `lib-dynload`, and the executable exports
/// the C API they link against (CPython's default `-export-dynamic`).
const CONFIGURE: [&str; 4] = [
    "--prefix=/usr",
    "--disable-shared",
    "--without-ensurepip",
    "--disable-test-modules",
];

/// The build directory of the selected architecture.
fn dir() -> PathBuf {
    crate::target_dir()
        .join("cpython")
        .join(format!("{}-dynamic", crate::arch::current().id()))
}

/// The tree a packages cell sees, as a directory on the host: present once
/// [`build`] has built it.
pub fn root() -> PathBuf {
    dir().join("root")
}

/// Builds the dynamically linked CPython, installs the wheels and writes
/// the cell's files to `out/packages-files` when CPython is built (see
/// [`cpython::build`]); otherwise removes them.
pub fn build(out: &Path) -> Result {
    let files = out.join("packages-files");
    let tarball = std::env::var_os("RHIZO_CPYTHON_SRC");
    let (Some(tarball), None) = (tarball, crate::differential::unavailable_reason()) else {
        let _ = std::fs::remove_file(&files);
        let _ = std::fs::remove_dir_all(root());
        return Ok(());
    };
    let stage = install(Path::new(&tarball))?;
    let lib = stage.join(cpython::STDLIB);
    let python = stage.join("usr/bin/python3.13");
    let site = lib.join("site-packages");
    let _ = std::fs::remove_dir_all(&site);
    std::fs::create_dir_all(&site).map_err(|e| e.to_string())?;
    let arch = crate::arch::current().id();
    for wheel in WHEELS.iter().filter(|w| w.arch.is_none_or(|a| a == arch)) {
        // A wheel is a zip archive laid out as installed (PEP 427).
        let mut cmd = Command::new(&python);
        cmd.env_clear()
            .args(["-I", "-m", "zipfile", "-e"])
            .arg(fetch(wheel)?)
            .arg(&site);
        cpython::run(&mut cmd, "install a wheel", &dir().join("wheel.log"))?;
    }
    let root = root();
    let _ = std::fs::remove_dir_all(&root);
    for d in [root.join("usr/bin"), root.join("app")] {
        std::fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    let scripts = crate::workspace_root().join("cells/python");
    for script in SCRIPTS {
        std::fs::copy(scripts.join(script), root.join("app").join(script))
            .map_err(|e| e.to_string())?;
    }
    // The requests script needs a server: the host's fake model.
    let server = crate::hostnet::llm_server("127.0.0.1", 0)?;
    // `-s`, as the scripts run: never a user's `site-packages`.
    let mut cmd = Command::new(&python);
    cmd.env_clear()
        .current_dir(dir())
        .arg("-s")
        .arg(scripts.join("pack_stdlib.py"))
        .arg(root.join(cpython::STDLIB))
        .arg(scripts.join(SCRIPTS[0]))
        .arg(format!(
            "{}\nhttp://127.0.0.1:{server}",
            scripts.join(SCRIPTS[1]).display()
        ));
    cpython::run(
        &mut cmd,
        "pack the standard library and packages",
        &dir().join("pack.log"),
    )?;
    let mut cmd = Command::new("strip");
    cmd.arg("-o").arg(root.join("usr/bin/python3")).arg(&python);
    cpython::run(&mut cmd, "strip python", &dir().join("strip.log"))?;
    // The standard library's extension modules carry debug information
    // (half their size); the wheels' are stripped already.
    let dynload = root.join(cpython::STDLIB).join("lib-dynload");
    let mut cmd = Command::new("strip");
    cmd.args(crate::dynamic::files(&dynload)?);
    cpython::run(
        &mut cmd,
        "strip extension modules",
        &dir().join("strip.log"),
    )?;
    crate::dynamic::pack(&root, true, &files)
}

/// Builds and installs (`DESTDIR`) the dynamically linked CPython unless the
/// build directory holds one made with the same recipe; returns where it is
/// installed.
fn install(tarball: &Path) -> Result<PathBuf> {
    let dir = dir();
    let stage = dir.join("stage");
    let recipe = format!("{} {}", cpython::VERSION, CONFIGURE.join(" "));
    let stamp = dir.join("recipe");
    if stage.join("usr/bin/python3.13").exists()
        && std::fs::read_to_string(&stamp).ok().as_deref() == Some(&recipe)
    {
        return Ok(stage);
    }
    cpython::check_sha256(tarball)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut tar = Command::new("tar");
    tar.arg("-xf").arg(tarball).arg("-C").arg(&dir);
    cpython::run(&mut tar, "unpack CPython", &dir.join("tar.log"))?;
    let src = dir.join(format!("Python-{}", cpython::VERSION));
    let mut configure = Command::new("./configure");
    configure.args(CONFIGURE).current_dir(&src);
    cpython::run(
        &mut configure,
        "configure the dynamic CPython",
        &dir.join("configure.log"),
    )?;
    let jobs = std::thread::available_parallelism().map_or(2, |n| n.get());
    let mut make = Command::new("make");
    make.arg(format!("-j{jobs}")).current_dir(&src);
    cpython::run(
        &mut make,
        "build the dynamic CPython",
        &dir.join("make.log"),
    )?;
    let mut make = Command::new("make");
    make.arg("altinstall")
        .arg(format!("DESTDIR={}", stage.display()))
        .current_dir(&src);
    cpython::run(
        &mut make,
        "install the dynamic CPython",
        &dir.join("install.log"),
    )?;
    // Only the installed tree is kept: the sources, objects and static
    // library take 300 MiB more of the cache.
    let _ = std::fs::remove_dir_all(&src);
    for entry in std::fs::read_dir(stage.join(cpython::STDLIB)).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("config-"))
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
    std::fs::write(&stamp, recipe).map_err(|e| e.to_string())?;
    Ok(stage)
}

/// The wheel's file, downloaded once into the CPython build directory and
/// checked against its pinned SHA-256.
fn fetch(wheel: &Wheel) -> Result<PathBuf> {
    let dir = crate::target_dir().join("cpython").join("wheels");
    let path = dir.join(wheel.file);
    if path.exists() && cpython::sha256(&path)? == wheel.sha256 {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let url = format!("{PYPI}/{}/{}", wheel.url, wheel.file);
    let part = dir.join(format!("{}.part", wheel.file));
    let mut curl = Command::new("curl");
    curl.args(["--fail", "--silent", "--show-error", "--location", "-o"])
        .arg(&part)
        .arg(&url);
    cpython::run(&mut curl, "download a wheel", &dir.join("curl.log"))?;
    let got = cpython::sha256(&part)?;
    if got != wheel.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(
            Failure::new("build", "a downloaded wheel is not the pinned one")
                .location(url.clone())
                .expected(format!("SHA-256 {}", wheel.sha256))
                .actual(got)
                .hint("PyPI files never change: check the URL, or update the pin on purpose")
                .reproduce(format!("curl -L {url} | sha256sum"))
                .into_error(),
        );
    }
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}
