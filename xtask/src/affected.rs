//! `test --affected`: the host test crates and VM steps that the files
//! changed against `origin/main` can reach, through the crate dependency
//! table of [`crate::deps`] (change `faster-ci-2`). For the local loop
//! only: CI runs every step.

use crate::Result;
use crate::deps::ALLOWED;
use std::process::Command;

/// What a set of changed paths reaches.
#[derive(Debug, PartialEq, Eq)]
pub enum Affected {
    /// A path of `xtask` (the harness) or one no crate owns (Cargo files,
    /// the toolchain, CI...): every step.
    Everything,
    /// These workspace crates, with every crate that depends on them.
    Crates(Vec<&'static str>),
}

impl Affected {
    /// The crates whose own files `paths` change, with their dependents.
    pub fn of(paths: &[String]) -> Self {
        let mut crates: Vec<&'static str> = Vec::new();
        for path in paths {
            match owner(path) {
                Owner::Docs => {}
                Owner::Everything => return Affected::Everything,
                Owner::Crate(name) => add_with_dependents(&mut crates, name),
            }
        }
        Affected::Crates(crates)
    }

    fn reaches(&self, name: &str) -> bool {
        match self {
            Affected::Everything => true,
            Affected::Crates(crates) => crates.contains(&name),
        }
    }

    /// Whether `test` runs `step`: every step that boots a kernel (`boot`,
    /// the `ktest*` steps and self-checks) needs the kernel or the cells.
    pub fn wants(&self, step: &str) -> bool {
        let kernel = self.reaches("rhizo-kernel") || self.reaches("rhizo-cells");
        match step {
            "host" => !self.host_crates().is_empty() || *self == Affected::Everything,
            "differential" => self.reaches("rhizo-cells"),
            "control" => self.reaches("rhizo-kernel") || self.reaches("rhizo-cli"),
            s if s == "boot" || s.starts_with("ktest") => kernel,
            _ => true,
        }
    }

    /// The host crates whose tests run (`None`: the whole workspace).
    pub fn host_crates(&self) -> Vec<&'static str> {
        match self {
            Affected::Everything => Vec::new(),
            Affected::Crates(crates) => crates
                .iter()
                .copied()
                .filter(|c| !matches!(*c, "rhizo-kernel" | "rhizo-cells"))
                .collect(),
        }
    }
}

enum Owner {
    Docs,
    Crate(&'static str),
    Everything,
}

/// The crate a path belongs to: its directory, or none for documentation.
fn owner(path: &str) -> Owner {
    let docs = ["docs/", "openspec/", ".agents/"];
    if path.ends_with(".md")
        || docs.iter().any(|d| path.starts_with(d))
        || (path.starts_with("bench/") && path.ends_with(".svg"))
    {
        return Owner::Docs;
    }
    let dir = path.split('/').take(2).collect::<Vec<_>>();
    let name = match dir.as_slice() {
        ["crates", name, ..] => *name,
        ["kernel", ..] => "rhizo-kernel",
        ["cells", ..] => "rhizo-cells",
        ["cli", ..] => "rhizo-cli",
        // The harness itself: what any step runs may have changed.
        _ => return Owner::Everything,
    };
    match ALLOWED.iter().find(|(c, _)| *c == name) {
        Some((c, _)) => Owner::Crate(c),
        None => Owner::Everything,
    }
}

fn add_with_dependents(crates: &mut Vec<&'static str>, name: &'static str) {
    if crates.contains(&name) {
        return;
    }
    crates.push(name);
    for (dependent, deps) in ALLOWED {
        if deps.contains(&name) {
            add_with_dependents(crates, dependent);
        }
    }
}

/// Paths changed since the merge base with `origin/main`: committed, staged,
/// unstaged and untracked.
pub fn changed_paths() -> Result<Vec<String>> {
    let git = |args: &[&str]| -> Result<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(crate::workspace_root())
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let base = git(&["merge-base", "origin/main", "HEAD"])
        .map_err(|e| format!("--affected compares with origin/main: {e}"))?;
    let mut paths: Vec<String> = git(&["diff", "--name-only", base.trim()])?
        .lines()
        .chain(git(&["ls-files", "--others", "--exclude-standard"])?.lines())
        .map(String::from)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(paths: &[&str]) -> Affected {
        Affected::of(&paths.iter().map(|p| p.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn documentation_reaches_nothing() {
        let a = of(&[
            "README.md",
            "openspec/changes/x/design.md",
            "docs/img/a.svg",
        ]);
        assert_eq!(a, Affected::Crates(Vec::new()));
        for step in ["host", "boot", "ktest", "control", "differential"] {
            assert!(!a.wants(step), "{step}");
        }
    }

    #[test]
    fn a_library_crate_reaches_its_dependents_and_the_kernel() {
        let a = of(&["crates/rhizo-fs/src/lib.rs"]);
        let host = a.host_crates();
        for c in ["rhizo-fs", "rhizo-abi", "rhizo-snapshot"] {
            assert!(host.contains(&c), "{c}");
        }
        assert!(!host.contains(&"rhizo-mm"));
        assert!(!host.contains(&"rhizo-kernel"));
        for step in [
            "host",
            "boot",
            "ktest",
            "ktest-smp",
            "ktest-hang",
            "control",
        ] {
            assert!(a.wants(step), "{step}");
        }
        assert!(!a.wants("differential"));
    }

    #[test]
    fn the_command_line_tool_reaches_only_its_tests_and_the_control_plane() {
        let a = of(&["cli/src/main.rs"]);
        assert_eq!(a.host_crates(), ["rhizo-cli"]);
        assert!(a.wants("control") && a.wants("host"));
        assert!(!a.wants("ktest") && !a.wants("ktest-page-fault"));
    }

    #[test]
    fn cells_reach_the_kernel_steps_and_the_differential_check() {
        let a = of(&["cells/c/libc_net.c"]);
        assert!(a.wants("differential") && a.wants("ktest") && a.wants("boot"));
        assert!(!a.wants("host"));
    }

    #[test]
    fn unowned_paths_and_the_harness_reach_everything() {
        for path in [
            "Cargo.lock",
            "rust-toolchain.toml",
            "xtask/src/vmm.rs",
            ".github/x",
        ] {
            assert_eq!(of(&[path]), Affected::Everything, "{path}");
        }
        assert!(of(&["Cargo.toml"]).wants("differential"));
    }
}
