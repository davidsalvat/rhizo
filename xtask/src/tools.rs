//! External programs and guest images downloaded on demand into
//! `target/tools/`, pinned by version and SHA-256 (design decision D9).
//!
//! Overrides: `RHIZO_FIRECRACKER`, `RHIZO_CLOUD_HYPERVISOR` and
//! `RHIZO_REFERENCE_KERNEL` point at local files instead of downloading.

use crate::Result;
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
const FIRECRACKER: Artifact = Artifact {
    file: "firecracker-v1.15.1-x86_64.tgz",
    url: "https://github.com/firecracker-microvm/firecracker/releases/download/v1.15.1/firecracker-v1.15.1-x86_64.tgz",
    sha256: "",
};

const CLOUD_HYPERVISOR: Artifact = Artifact {
    file: "cloud-hypervisor-v54.0",
    url: "https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v54.0/cloud-hypervisor-static",
    sha256: "",
};

/// Firecracker's published CI guest kernel (Linux 6.1, PVH and initrd enabled).
const REFERENCE_KERNEL: Artifact = Artifact {
    file: "vmlinux-6.1.155",
    url: "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/x86_64/vmlinux-6.1.155",
    sha256: "e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2",
};

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
    let binary = dir.join(format!("firecracker-{FIRECRACKER_VERSION}"));
    if !binary.exists() {
        let archive = fetch(&FIRECRACKER)?;
        run(Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir))?;
        let extracted = dir
            .join(format!("release-{FIRECRACKER_VERSION}-x86_64"))
            .join(format!("firecracker-{FIRECRACKER_VERSION}-x86_64"));
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
    let binary = fetch(&CLOUD_HYPERVISOR)?;
    make_executable(&binary)?;
    Ok(binary)
}

/// Path to the reference Linux guest kernel.
pub fn reference_kernel() -> Result<PathBuf> {
    if let Some(path) = env::var_os("RHIZO_REFERENCE_KERNEL") {
        return Ok(PathBuf::from(path));
    }
    fetch(&REFERENCE_KERNEL)
}

/// Downloads `artifact` unless already present, and verifies its checksum.
fn fetch(artifact: &Artifact) -> Result<PathBuf> {
    let path = dir()?.join(artifact.file);
    if !path.exists() {
        println!("[xtask] downloading {}", artifact.url);
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
        println!(
            "[xtask] {} is not pinned yet; sha256 = {actual} (add it to xtask/src/tools.rs)",
            artifact.file
        );
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
