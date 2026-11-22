//! The small local state that lets later commands find the VM `rhizo up`
//! started: a directory with the state file, the control socket and logs.
//! Once the VM is down only the console log stays, the last VM's, until the
//! next `rhizo up` replaces it, and the audit log (`audit/`), which stays
//! for good.

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

    /// Creates the directory if it is missing, and makes it reachable by
    /// its user alone (mode `0700`), as it holds the control socket, which
    /// controls every cell (change `authenticate-control-channel`).
    pub fn create(&self) -> Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.0)
            .and_then(|()| fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)))
            .map_err(|e| format!("cannot create {}: {e}", self.0.display()))
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

    /// Removes what a VM leaves here but the console log, the state file
    /// last (`rhizo down` waits for it to go). Files that are not there
    /// are fine.
    pub fn clean(&self) {
        for file in [
            self.0.join("state.new"),
            self.socket(),
            self.kernel(),
            self.log(),
            self.state(),
        ] {
            let _ = fs::remove_file(file);
        }
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
    /// The control protocol the kernel's hello named; `None` in the state
    /// of a server older than version 4, whose kernel speaks version 3 at
    /// most.
    pub protocol: Option<u8>,
}

impl Info {
    pub fn encode(&self) -> String {
        let mut text = format!(
            "pid={}\nvmm_pid={}\nvmm={}\narch={}\nmemory_mib={}\ncpus={}\nboot_ms={}\n",
            self.pid, self.vmm_pid, self.vmm, self.arch, self.memory_mib, self.cpus, self.boot_ms
        );
        if let Some(p) = self.protocol {
            text += &format!("protocol={p}\n");
        }
        text
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
            protocol: match field("protocol") {
                Some(p) => Some(p.parse().ok()?),
                None => None,
            },
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
            protocol: Some(4),
        };
        assert_eq!(Info::decode(&info.encode()), Some(info.clone()));
        // A version-3 server wrote no protocol.
        let old = Info {
            protocol: None,
            ..info
        };
        assert_eq!(old.encode().lines().count(), 7);
        assert_eq!(Info::decode(&old.encode()), Some(old));
        assert_eq!(Info::decode("pid=1\n"), None);
        assert_eq!(Info::decode("garbage"), None);
    }

    #[test]
    fn cleaning_leaves_only_the_console_and_audit_logs() {
        let dir = Dir(std::env::temp_dir().join(format!("rhizo-clean-{}", std::process::id())));
        let _ = fs::remove_dir_all(&dir.0);
        fs::create_dir_all(&dir.0).unwrap();
        for file in [
            dir.state(),
            dir.0.join("state.new"),
            dir.socket(),
            dir.console(),
            dir.log(),
            dir.kernel(),
        ] {
            fs::write(file, b"x").unwrap();
        }
        let audit = crate::audit::dir(&dir.0);
        fs::create_dir_all(&audit).unwrap();
        fs::write(audit.join("audit-000000.jsonl"), b"x").unwrap();
        dir.clean();
        let mut left: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["audit", "console.log"]);
        dir.clean();
        assert!(dir.console().exists());
        assert!(audit.join("audit-000000.jsonl").exists());
        fs::remove_dir_all(&dir.0).unwrap();
    }

    /// A new state directory, and one that already existed open to
    /// everyone, are both left to their user alone.
    #[test]
    fn the_state_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let root = std::env::temp_dir().join(format!("rhizo-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let new = Dir(root.join("a").join("state"));
        new.create().unwrap();
        assert_eq!(mode(&new.0), 0o700);
        let old = Dir(root.join("old"));
        fs::create_dir_all(&old.0).unwrap();
        fs::set_permissions(&old.0, fs::Permissions::from_mode(0o777)).unwrap();
        old.create().unwrap();
        assert_eq!(mode(&old.0), 0o700);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn this_process_is_alive() {
        assert!(alive(std::process::id()));
    }
}
