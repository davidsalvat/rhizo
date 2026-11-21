//! The kernel's audit records (design D18 of `add-cell-hierarchy`): one
//! ring of 64 KiB for all CPUs (`rhizo_control::audit::Ring`), which the
//! control kernel empties into `audit` frames for the host. Only a denial
//! or an administrative action records anything, so the hot paths pay
//! nothing; recording never waits for the host (a full ring drops and
//! counts). Cells are named by kernel identifier here; the control kernel
//! gives the host its own when the record leaves.

use crate::sync::Global;
pub use rhizo_control::audit::Kind;
use rhizo_control::audit::{Record, Ring};

static RING: Global<Ring> = Global::new(Ring::new());

/// Records `kind` about kernel cell `cell` of the domain whose root is
/// kernel cell `domain` (0: none), with `detail` for people (cut at
/// `DETAIL_MAX` bytes). Repeated denials of one kind by one cell fold
/// into one record per second.
pub fn record(kind: Kind, cell: u32, domain: u32, detail: &[u8]) {
    let now = crate::cell::now_ns();
    RING.with(|r| r.record(now, kind, cell, domain, detail));
}

/// Records denial `kind` (see [`record`]): the hook for refusals by egress
/// policies and the floor, scopes, tree checks and publication.
pub fn deny(kind: Kind, cell: u32, domain: u32, detail: &[u8]) {
    debug_assert!(kind.is_denial());
    record(kind, cell, domain, detail);
}

/// Whether a record waits to leave.
#[cfg_attr(not(feature = "control"), allow(dead_code, reason = "control kernel"))]
pub fn pending() -> bool {
    let now = crate::cell::now_ns();
    RING.with(|r| r.pending(now))
}

/// The oldest record waiting to leave.
#[cfg_attr(
    not(any(feature = "control", feature = "ktest")),
    allow(dead_code, reason = "control kernel and in-kernel tests")
)]
pub fn take() -> Option<Record> {
    let now = crate::cell::now_ns();
    RING.with(|r| r.take(now))
}
