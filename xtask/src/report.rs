//! Failure reports and progress output, for humans and for agents
//! (`--json`, one JSON object per line on standard output).

use serde_json::{Value, json};
use std::io::{self, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

static JSON: AtomicBool = AtomicBool::new(false);

/// Switches every report to JSON lines. Other output then goes to stderr.
pub fn set_json(enabled: bool) {
    JSON.store(enabled, Ordering::Relaxed);
}

/// Whether reports are JSON lines.
pub fn json() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// Where child processes and console echo should write: stdout for humans,
/// stderr in JSON mode so stdout carries only JSON lines.
pub fn child_stdout() -> Stdio {
    if json() {
        Stdio::from(io::stderr())
    } else {
        Stdio::inherit()
    }
}

/// Writes free-form progress text (to stderr in JSON mode).
pub fn text(s: &str) {
    if json() {
        let _ = writeln!(io::stderr(), "{s}");
    } else {
        println!("{s}");
    }
}

/// Raw bytes from a child (console echo), routed like [`text`].
pub fn raw(bytes: &[u8]) {
    if json() {
        let _ = io::stderr().write_all(bytes);
    } else {
        let mut out = io::stdout();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }
}

fn emit(value: &Value) {
    let mut out = io::stdout();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// Announces the start of a step.
pub fn step(name: &str) {
    if json() {
        emit(&json!({"event": "step", "name": name}));
    } else {
        println!("\n==> {name}");
    }
}

/// A structured result value (benchmark rows, timings).
pub fn data(kind: &str, value: &Value) {
    if json() {
        emit(&json!({"event": "data", "kind": kind, "value": value}));
    }
}

/// Final line of a command.
pub fn summary(ok: bool, message: &str) {
    if json() {
        emit(&json!({"event": "summary", "ok": ok, "message": message}));
    } else if ok {
        println!("\n{message}");
    }
}

/// Everything an agent needs to act on a failure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Failure {
    /// Kind of check: `fmt`, `clippy`, `host-test`, `boot`, `ktest`,
    /// `self-check`, `bench`, `deps`, `tool`.
    pub kind: String,
    /// What failed, in one sentence.
    pub what: String,
    /// `file:line`, an address, or the command that failed.
    pub location: Option<String>,
    pub expected: Option<String>,
    pub actual: Option<String>,
    /// What usually fixes it.
    pub hint: Option<String>,
    /// Exact command that reproduces it.
    pub reproduce: Option<String>,
}

impl Failure {
    pub fn new(kind: &str, what: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            what: what.into(),
            ..Self::default()
        }
    }

    pub fn location(mut self, v: impl Into<String>) -> Self {
        self.location = Some(v.into());
        self
    }

    pub fn expected(mut self, v: impl Into<String>) -> Self {
        self.expected = Some(v.into());
        self
    }

    pub fn actual(mut self, v: impl Into<String>) -> Self {
        self.actual = Some(v.into());
        self
    }

    pub fn hint(mut self, v: impl Into<String>) -> Self {
        self.hint = Some(v.into());
        self
    }

    pub fn reproduce(mut self, v: impl Into<String>) -> Self {
        self.reproduce = Some(v.into());
        self
    }

    /// Human rendering.
    pub fn render(&self) -> String {
        let mut s = format!("FAILED {}: {}", self.kind, self.what);
        for (label, value) in [
            ("where", &self.location),
            ("expected", &self.expected),
            ("actual", &self.actual),
            ("hint", &self.hint),
            ("reproduce", &self.reproduce),
        ] {
            if let Some(v) = value {
                s += &format!("\n  {label:<10} {v}");
            }
        }
        s
    }

    /// JSON rendering.
    pub fn to_json(&self) -> Value {
        json!({
            "event": "failure",
            "kind": self.kind,
            "what": self.what,
            "where": self.location,
            "expected": self.expected,
            "actual": self.actual,
            "hint": self.hint,
            "reproduce": self.reproduce,
        })
    }

    /// Emits the failure (JSON mode) and returns the human text as the error.
    pub fn into_error(self) -> String {
        if json() {
            emit(&self.to_json());
        }
        self.render()
    }
}

/// Extracts `file:line:column` and the message from a kernel panic report.
pub fn panic_location(output: &str) -> Option<(String, String)> {
    let line = output.lines().find(|l| l.starts_with("PANIC at "))?;
    let rest = line.strip_prefix("PANIC at ")?;
    let (location, message) = rest.split_once(": ")?;
    Some((location.to_string(), message.to_string()))
}

/// Extracts the exception summary and faulting instruction address.
pub fn exception_location(output: &str) -> Option<(String, String)> {
    let line = output.lines().find(|l| l.starts_with("EXCEPTION: "))?;
    // x86_64 reports "at rip <addr>", AArch64 "at pc <addr>".
    let rip = line
        .rsplit(" at ")
        .next()?
        .split_whitespace()
        .nth(1)?
        .to_string();
    Some((line.trim_start_matches("EXCEPTION: ").to_string(), rip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_rendering_lists_only_present_fields() {
        let f = Failure::new("self-check", "ktest-page-fault must end with Failed")
            .expected("Failed")
            .actual("Passed")
            .reproduce("cargo xtask test --only ktest-page-fault");
        assert_eq!(
            f.render(),
            "FAILED self-check: ktest-page-fault must end with Failed\n  \
             expected   Failed\n  \
             actual     Passed\n  \
             reproduce  cargo xtask test --only ktest-page-fault"
        );
    }

    #[test]
    fn json_rendering_has_every_field() {
        let f = Failure::new("ktest", "in-kernel test failed").location("kernel/src/tests.rs:42:5");
        let v = f.to_json();
        assert_eq!(v["event"], "failure");
        assert_eq!(v["where"], "kernel/src/tests.rs:42:5");
        for key in ["kind", "what", "expected", "actual", "hint", "reproduce"] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        // One line, parseable.
        let line = v.to_string();
        assert!(!line.contains('\n'));
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), v);
    }

    #[test]
    fn kernel_reports_are_parsed() {
        let out =
            "test x ... \nPANIC at kernel/src/tests.rs:42:5: assertion `left == right` failed\n";
        assert_eq!(
            panic_location(out),
            Some((
                "kernel/src/tests.rs:42:5".into(),
                "assertion `left == right` failed".into()
            ))
        );
        let out = "\nEXCEPTION: page fault (vector 14, error 0x2) at rip 0xffffffff80111220\n";
        assert_eq!(
            exception_location(out),
            Some((
                "page fault (vector 14, error 0x2) at rip 0xffffffff80111220".into(),
                "0xffffffff80111220".into()
            ))
        );
        let out = "\nEXCEPTION: data abort (class 0x25, ESR 0x96000044) at pc 0xffffffff80003194\n";
        assert_eq!(exception_location(out).unwrap().1, "0xffffffff80003194");
        assert_eq!(panic_location("all good"), None);
    }
}
