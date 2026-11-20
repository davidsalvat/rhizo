//! Crate dependency rules (see AGENTS.md, "Code map").
//!
//! Every workspace crate must appear in [`ALLOWED`], and may depend only on
//! the workspace crates listed for it. A milestone that adds a crate adds its
//! row here in the same pull request.

use crate::Result;
use crate::report::Failure;
use serde_json::Value;
use std::process::Command;

/// Allowed workspace-internal dependencies, per crate.
pub const ALLOWED: &[(&str, &[&str])] = &[
    ("rhizo-hal", &[]),
    ("rhizo-core", &["rhizo-hal"]),
    ("rhizo-mm", &["rhizo-hal"]),
    ("rhizo-cell", &["rhizo-hal", "rhizo-mm"]),
    ("rhizo-fs", &[]),
    ("rhizo-net", &["rhizo-hal"]),
    ("rhizo-virtio", &["rhizo-hal"]),
    (
        "rhizo-abi",
        &[
            "rhizo-hal",
            "rhizo-mm",
            "rhizo-cell",
            "rhizo-fs",
            "rhizo-net",
            "rhizo-virtio",
        ],
    ),
    ("rhizo-sched", &[]),
    (
        "rhizo-snapshot",
        &[
            "rhizo-hal",
            "rhizo-mm",
            "rhizo-cell",
            "rhizo-fs",
            "rhizo-net",
            "rhizo-virtio",
        ],
    ),
    (
        "rhizo-kernel",
        &[
            "rhizo-hal",
            "rhizo-core",
            "rhizo-mm",
            "rhizo-cell",
            "rhizo-abi",
            "rhizo-sched",
            "rhizo-snapshot",
            "rhizo-fs",
            "rhizo-net",
            "rhizo-virtio",
        ],
    ),
    ("rhizo-cells", &[]),
    ("xtask", &["rhizo-fs"]),
];

/// Workspace-internal edges `(from, to)` found in `cargo metadata` output.
pub fn workspace_edges(metadata: &Value) -> Vec<(String, String)> {
    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut edges = Vec::new();
    for p in &packages {
        let from = p["name"].as_str().unwrap_or_default();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let to = d["name"].as_str().unwrap_or_default();
            // Dev-dependencies on a crate's own test doubles are allowed as
            // long as the normal edge is allowed; check them the same way.
            if names.contains(&to)
                && d["path"].is_string()
                && !edges.contains(&(from.into(), to.into()))
            {
                edges.push((from.to_string(), to.to_string()));
            }
        }
    }
    edges
}

/// Rule violations for the given workspace crates and edges.
pub fn violations(crates: &[String], edges: &[(String, String)]) -> Vec<Failure> {
    let mut out = Vec::new();
    for name in crates {
        if !ALLOWED.iter().any(|(c, _)| c == name) {
            out.push(
                Failure::new(
                    "deps",
                    format!("crate `{name}` is not in the dependency table"),
                )
                .location("xtask/src/deps.rs")
                .hint("add a row for the new crate to ALLOWED and to the code map in AGENTS.md")
                .reproduce("cargo xtask check"),
            );
        }
    }
    for (from, to) in edges {
        let allowed = ALLOWED
            .iter()
            .find(|(c, _)| c == from)
            .is_some_and(|(_, deps)| deps.contains(&to.as_str()));
        if !allowed && ALLOWED.iter().any(|(c, _)| c == from) {
            out.push(
                Failure::new("deps", format!("`{from}` must not depend on `{to}`"))
                    .location(format!("{from}/Cargo.toml"))
                    .expected(format!(
                        "dependencies of `{from}` within {:?}",
                        ALLOWED.iter().find(|(c, _)| c == from).map(|(_, d)| *d).unwrap_or(&[])
                    ))
                    .actual(format!("`{from}` -> `{to}`"))
                    .hint("move the shared code to a lower crate (usually rhizo-hal) or change the design")
                    .reproduce("cargo xtask check"),
            );
        }
    }
    out
}

/// Runs `cargo metadata` and checks the rules.
pub fn check() -> Result {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(crate::workspace_root())
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let metadata: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad cargo metadata: {e}"))?;
    let crates: Vec<String> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    let found = violations(&crates, &workspace_edges(&metadata));
    match found
        .into_iter()
        .map(Failure::into_error)
        .reduce(|a, b| a + "\n" + &b)
    {
        None => Ok(()),
        Some(errors) => Err(errors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn current_layout_is_valid() {
        let crates = strings(&[
            "rhizo-hal",
            "rhizo-core",
            "rhizo-mm",
            "rhizo-kernel",
            "xtask",
        ]);
        let edges = vec![
            ("rhizo-mm".into(), "rhizo-hal".into()),
            ("rhizo-kernel".into(), "rhizo-mm".into()),
        ];
        assert!(violations(&crates, &edges).is_empty());
    }

    #[test]
    fn forbidden_edge_names_both_crates() {
        let crates = strings(&["rhizo-mm", "rhizo-kernel"]);
        let edges = vec![("rhizo-mm".into(), "rhizo-kernel".into())];
        let v = violations(&crates, &edges);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].what, "`rhizo-mm` must not depend on `rhizo-kernel`");
        assert_eq!(v[0].actual.as_deref(), Some("`rhizo-mm` -> `rhizo-kernel`"));
    }

    #[test]
    fn unknown_crate_must_be_added_to_the_table() {
        let v = violations(&strings(&["rhizo-unlisted"]), &[]);
        assert_eq!(v.len(), 1);
        assert!(v[0].what.contains("rhizo-unlisted"));
    }

    #[test]
    fn edges_come_from_path_dependencies_only() {
        let meta = json!({"packages": [
            {"name": "rhizo-mm", "dependencies": [
                {"name": "rhizo-hal", "path": "/w/crates/rhizo-hal"},
                {"name": "proptest"}
            ]},
            {"name": "rhizo-hal", "dependencies": []}
        ]});
        assert_eq!(
            workspace_edges(&meta),
            vec![("rhizo-mm".to_string(), "rhizo-hal".to_string())]
        );
    }
}
