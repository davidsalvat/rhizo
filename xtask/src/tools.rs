//! External programs and guest images downloaded on demand into
//! `target/tools/`, pinned by version and SHA-256 (design decision D9).
//!
//! Overrides: `RHIZO_FIRECRACKER`, `RHIZO_CLOUD_HYPERVISOR` and
//! `RHIZO_REFERENCE_KERNEL` point at local files instead of downloading.
//! Node.js is downloaded only when `RHIZO_NODE` is set (change
//! `add-node-workload`).

use crate::Result;
use crate::arch::{self, Arch};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A pinned download.
struct Artifact {
    /// File name inside `target/tools/`.
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const FIRECRACKER_VERSION: &str = "v1.15.1";

fn firecracker_artifact(arch: Arch) -> Artifact {
    match arch {
        Arch::X86_64 => Artifact {
            file: "firecracker-v1.15.1-x86_64.tgz",
            url: "https://github.com/firecracker-microvm/firecracker/releases/download/v1.15.1/firecracker-v1.15.1-x86_64.tgz",
            sha256: "d4a32ab2322d887ca1bc4a4e7afa9cc35393e6362dfc2b3becb389d362e4275a",
        },
        // Pinned on the first verified run on ARM64 hardware with KVM.
        Arch::Aarch64 => Artifact {
            file: "firecracker-v1.15.1-aarch64.tgz",
            url: "https://github.com/firecracker-microvm/firecracker/releases/download/v1.15.1/firecracker-v1.15.1-aarch64.tgz",
            sha256: "",
        },
    }
}

fn cloud_hypervisor_artifact(arch: Arch) -> Artifact {
    match arch {
        Arch::X86_64 => Artifact {
            file: "cloud-hypervisor-v54.0",
            url: "https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v54.0/cloud-hypervisor-static",
            sha256: "4f4f98a3d661a40f3e5272627cc895a1b2e525f2f6bedf5f895f9cd21ba5c65b",
        },
        // Pinned on the first verified run on ARM64 hardware with KVM.
        Arch::Aarch64 => Artifact {
            file: "cloud-hypervisor-v54.0-aarch64",
            url: "https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v54.0/cloud-hypervisor-static-aarch64",
            sha256: "",
        },
    }
}

/// Firecracker's published CI guest kernels (Linux 6.1).
fn reference_kernel_artifact(arch: Arch) -> Artifact {
    match arch {
        Arch::X86_64 => Artifact {
            file: "vmlinux-6.1.155",
            url: "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/x86_64/vmlinux-6.1.155",
            sha256: "e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2",
        },
        Arch::Aarch64 => Artifact {
            file: "vmlinux-6.1.155-aarch64",
            url: "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/aarch64/vmlinux-6.1.155",
            sha256: "e3544b10603acbf3db492cb52e000d22ba202cb4b63b9add027565683e11c591",
        },
    }
}

/// The pinned Node.js release (LTS "Krypton"; MIT license, its bundled
/// dependencies under the licenses its `LICENSE` file lists).
pub const NODE_VERSION: &str = "v24.22.3";

/// The official Linux build of [`NODE_VERSION`] (dynamically linked with
/// glibc), as nodejs.org publishes it with its SHA-256 (`SHASUMS256.txt`).
fn node_artifact(arch: Arch) -> Artifact {
    match arch {
        Arch::X86_64 => Artifact {
            file: "node-v24.22.3-linux-x64.tar.xz",
            url: "https://nodejs.org/dist/v24.22.3/node-v24.22.3-linux-x64.tar.xz",
            sha256: "4f2b175a6d53a5c617a80dba2676238a0688cef28c6da4eba35c7d844e66fcfb",
        },
        Arch::Aarch64 => Artifact {
            file: "node-v24.22.3-linux-arm64.tar.xz",
            url: "https://nodejs.org/dist/v24.22.3/node-v24.22.3-linux-arm64.tar.xz",
            sha256: "6e51a2ada00f0011cc266f65d98922b403d7ad88f809ca2aa615a5fe1ce6e86c",
        },
    }
}

/// The `node` executable of [`NODE_VERSION`] for the cells' architecture,
/// unmodified from its archive, with the release's `LICENSE` next to it.
pub fn node() -> Result<PathBuf> {
    let dir = dir()?;
    let arch = arch::current();
    let binary = dir.join(format!("node-{NODE_VERSION}-{}", arch.id()));
    if !binary.exists() {
        let artifact = node_artifact(arch);
        let archive = fetch(&artifact)?;
        let top = artifact.file.trim_end_matches(".tar.xz");
        run(Command::new("tar")
            .arg("-xJf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .arg(format!("{top}/bin/node"))
            .arg(format!("{top}/LICENSE")))?;
        fs::rename(
            dir.join(top).join("LICENSE"),
            dir.join(format!("node-{NODE_VERSION}-LICENSE")),
        )
        .map_err(|e| format!("unexpected Node.js archive layout: {e}"))?;
        fs::rename(dir.join(top).join("bin/node"), &binary)
            .map_err(|e| format!("unexpected Node.js archive layout: {e}"))?;
        let _ = fs::remove_dir_all(dir.join(top));
    }
    Ok(binary)
}

/// The tools directory, created if needed.
pub fn dir() -> Result<PathBuf> {
    let dir = crate::target_dir().join("tools");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Path to the Firecracker binary.
pub fn firecracker() -> Result<PathBuf> {
    if let Some(path) = env::var_os("RHIZO_FIRECRACKER") {
        return Ok(PathBuf::from(path));
    }
    let dir = dir()?;
    let arch = arch::current();
    let binary = dir.join(format!("firecracker-{FIRECRACKER_VERSION}-{}", arch.id()));
    if !binary.exists() {
        let archive = fetch(&firecracker_artifact(arch))?;
        run(Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir))?;
        let extracted = dir
            .join(format!("release-{FIRECRACKER_VERSION}-{}", arch.id()))
            .join(format!("firecracker-{FIRECRACKER_VERSION}-{}", arch.id()));
        fs::rename(&extracted, &binary)
            .map_err(|e| format!("unexpected Firecracker archive layout: {e}"))?;
    }
    Ok(binary)
}

/// Path to the Cloud Hypervisor binary.
pub fn cloud_hypervisor() -> Result<PathBuf> {
    if let Some(path) = env::var_os("RHIZO_CLOUD_HYPERVISOR") {
        return Ok(PathBuf::from(path));
    }
    let binary = fetch(&cloud_hypervisor_artifact(arch::current()))?;
    make_executable(&binary)?;
    Ok(binary)
}

/// Path to the reference Linux guest kernel.
pub fn reference_kernel() -> Result<PathBuf> {
    if let Some(path) = env::var_os("RHIZO_REFERENCE_KERNEL") {
        return Ok(PathBuf::from(path));
    }
    fetch(&reference_kernel_artifact(arch::current()))
}

/// Downloads `artifact` unless already present, and verifies its checksum.
fn fetch(artifact: &Artifact) -> Result<PathBuf> {
    let path = dir()?.join(artifact.file);
    if !path.exists() {
        crate::report::text(&format!("[xtask] downloading {}", artifact.url));
        let partial = path.with_extension("partial");
        run(Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(&partial)
            .arg(artifact.url))?;
        fs::rename(&partial, &path).map_err(|e| format!("cannot store download: {e}"))?;
    }
    let actual = sha256(&path)?;
    if artifact.sha256.is_empty() {
        crate::report::text(&format!(
            "[xtask] {} is not pinned yet; sha256 = {actual} (add it to xtask/src/tools.rs)",
            artifact.file
        ));
        if env::var_os("GITHUB_ACTIONS").is_some() {
            println!("::notice title=unpinned-sha256::{} {actual}", artifact.file);
        }
        if env::var_os("RHIZO_ALLOW_UNPINNED").is_none() {
            return Err(format!(
                "refusing to run unpinned download {}; check it, then pin its sha256 in \
                 xtask/src/tools.rs (or set RHIZO_ALLOW_UNPINNED=1 to run it once)",
                artifact.file
            ));
        }
    } else if actual != artifact.sha256 {
        let _ = fs::remove_file(&path);
        return Err(format!(
            "checksum mismatch for {}: expected {}, got {actual}; the file was deleted, \
             re-run to download it again",
            artifact.file, artifact.sha256
        ));
    }
    Ok(path)
}

fn sha256(path: &Path) -> Result<String> {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("cannot run sha256sum: {e}"))?;
    if !out.status.success() {
        return Err(format!("sha256sum failed for {}", path.display()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string())
}

fn make_executable(path: &Path) -> Result {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot chmod {}: {e}", path.display()))
}

fn run(cmd: &mut Command) -> Result {
    let status = cmd
        .status()
        .map_err(|e| format!("cannot run {:?}: {e}", cmd.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed with {status}", cmd.get_program()))
    }
}
