//! The small local state that lets later commands find the VM `rhizo up`
//! started: a directory with the state file, the control socket and logs.

use std::fs;
use std::path::{Path, PathBuf};

/// The state directory and the files in it.
#[derive(Debug, Clone)]
pub struct Dir(pub PathBuf);

impl Dir {
    /// `--state`, else `$RHIZO_STATE`, else `~/.rhizo`.
    pub fn resolve(given: Option<PathBuf>) -> Self {
        let dir = given
            .or_else(|| std::env::var_os("RHIZO_STATE").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".rhizo")))
            .unwrap_or_else(|| PathBuf::from("/tmp/rhizo"));
        Dir(dir)
    }

    pub fn state(&self) -> PathBuf {
        self.0.join("state")
    }
    /// The control socket: in the state directory, or in the system's
    /// temporary directory when that path is too long for a Unix socket.
    pub fn socket(&self) -> PathBuf {
        let path = self.0.join("control.sock");
        if path.as_os_str().len() < 100 {
            return path;
        }
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut hasher);
        std::env::temp_dir().join(format!("rhizo-{:016x}.sock", hasher.finish()))
    }
    /// The kernel's console text.
    pub fn console(&self) -> PathBuf {
        self.0.join("console.log")
    }
    /// The background server's own output.
    pub fn log(&self) -> PathBuf {
        self.0.join("server.log")
    }
    /// The kernel image the VM booted (a copy: later builds replace the
    /// original).
    pub fn kernel(&self) -> PathBuf {
        self.0.join("kernel")
    }
}

/// What `rhizo up` records about the VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The background server's process.
    pub pid: u32,
    /// The VMM's process.
    pub vmm_pid: u32,
    pub vmm: String,
    pub arch: String,
    pub memory_mib: u32,
    pub cpus: u32,
    /// Milliseconds from launching the VMM to the kernel's hello.
    pub boot_ms: u64,
}

impl Info {
    pub fn encode(&self) -> String {
        format!(
            "pid={}\nvmm_pid={}\nvmm={}\narch={}\nmemory_mib={}\ncpus={}\nboot_ms={}\n",
            self.pid, self.vmm_pid, self.vmm, self.arch, self.memory_mib, self.cpus, self.boot_ms
        )
    }

    pub fn decode(text: &str) -> Option<Self> {
        let field = |name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
        };
        Some(Info {
            pid: field("pid")?.parse().ok()?,
            vmm_pid: field("vmm_pid")?.parse().ok()?,
            vmm: field("vmm")?.into(),
            arch: field("arch")?.into(),
            memory_mib: field("memory_mib")?.parse().ok()?,
            cpus: field("cpus")?.parse().ok()?,
            boot_ms: field("boot_ms")?.parse().ok()?,
        })
    }

    /// The state of a VM that is up, if one is.
    pub fn read(dir: &Dir) -> Option<Self> {
        let info = Self::decode(&fs::read_to_string(dir.state()).ok()?)?;
        alive(info.pid).then_some(info)
    }

    /// Writes the state file at once (a reader never sees half of it).
    pub fn write(&self, dir: &Dir) -> Result<(), String> {
        let tmp = dir.0.join("state.new");
        fs::write(&tmp, self.encode())
            .and_then(|()| fs::rename(&tmp, dir.state()))
            .map_err(|e| format!("cannot write {}: {e}", dir.state().display()))
    }
}

/// Whether process `pid` exists.
pub fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_round_trips() {
        let info = Info {
            pid: 10,
            vmm_pid: 11,
            vmm: "qemu-microvm".into(),
            arch: "x86_64".into(),
            memory_mib: 256,
            cpus: 2,
            boot_ms: 40,
        };
        assert_eq!(Info::decode(&info.encode()), Some(info));
        assert_eq!(Info::decode("pid=1\n"), None);
        assert_eq!(Info::decode("garbage"), None);
    }

    #[test]
    fn this_process_is_alive() {
        assert!(alive(std::process::id()));
    }
}
