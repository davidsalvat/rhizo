//! The cell hierarchy as plain data (design D1 to D4, D7 to D10, D19 and
//! D24 of `add-cell-hierarchy`): the ownership tree, the budgets children
//! carve from their parent, each cell's place in its domain, egress
//! policies, handles as grants with their rights, derivation, revocation
//! and exposure check, channels' queues, accelerator handles with their
//! budgets, and the bounds of every structure.
//!
//! The crate never touches hardware, reads no clock and has no
//! dependencies: the kernel keeps one [`Tree`] and passes in what only it
//! knows (what a cell uses, fresh domain identifiers), so every operation
//! is tested on the host.
//!
//! **Domains.** A child of the host is a root, and a root with all its
//! descendants is a domain, the unit the host isolates. Each domain has an
//! identifier ([`Node::domain`]) that never changes while the domain lives
//! and is never 0, so other parts of the kernel key what they share by it
//! (0 is free for objects every domain shares). Inside its domain each cell
//! has its own number ([`Node::pid`], what `getpid` returns), counted from 1
//! for the root.
//!
//! **Failures.** Every operation checks and allocates before it changes
//! anything, so a refused or failed one leaves the tree as it was (design
//! D19).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod channels;
pub mod egress;
mod grants;
pub mod limits;
mod tree;

pub use channels::{Channels, ROOM, Recv};
pub use egress::{Egress, Proto, Rule};
pub use grants::{AccelBudget, AccelTerms, AccelUse, Ask, Gone, Grant, Grants, Object, Rights};
pub use tree::{Actor, Ended, HOST, Label, Limits, Mode, Node, OnParentEnd, Spec, Tree};

#[cfg(test)]
mod tests;

/// Why an operation was refused. It changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// No such cell in the tree.
    NotFound,
    /// The identifier is in the tree already.
    Exists,
    /// The actor has no authority over the cell, or the request would
    /// widen a policy or a limit it may only narrow (`EACCES`).
    Denied,
    /// Deeper than [`limits::DEPTH_MAX`] below the host, or a derivation
    /// chain longer than [`limits::CHAIN_MAX`] (`EMLINK`).
    TooDeep,
    /// More than [`limits::CHILDREN_MAX`] children (`EMLINK`).
    TooManyChildren,
    /// The parent's free budget cannot hold the child's limits (`EDQUOT`).
    Budget,
    /// The parent's memory, socket or descendant limit is unlimited, so it
    /// may have no children (`EDQUOT`): a subtree holds at most what its
    /// root was given.
    Unlimited,
    /// The new parent is in the cell's own subtree (`EINVAL`).
    Cycle,
    /// A value outside its bounds (`EINVAL`).
    Invalid,
    /// Memory for the crate's own records ran out (`ENOMEM`).
    NoMemory,
    /// More than [`limits::HANDLES_MAX`] handles or
    /// [`limits::CHANNELS_MAX`] channel endpoints held by one cell
    /// (`EMFILE`).
    TooManyHandles,
    /// The peer's queue cannot take the message (`EAGAIN`, or block).
    Full,
    /// The peer endpoint is closed (`EPIPE`).
    Pipe,
    /// A message over [`limits::MESSAGE_MAX`] bytes (`EMSGSIZE`).
    TooBig,
    /// A message with more than [`limits::MESSAGE_HANDLES_MAX`] handles
    /// (`ETOOMANYREFS`).
    TooManyRefs,
}

#[cfg(test)]
std::thread_local! {
    /// Test builds: the number of allocations to let through before the
    /// next one fails (`None`: never fail), to check that every operation
    /// leaves the tree unchanged when one of its allocations fails.
    static FAIL_IN: core::cell::Cell<Option<u32>> = const { core::cell::Cell::new(None) };
    /// Test builds: whether an allocation was made to fail.
    static FAILED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// A place where the crate allocates: in test builds it may be made to fail.
fn alloc_point() -> Result<(), Error> {
    #[cfg(test)]
    if FAIL_IN.with(|f| match f.get() {
        Some(0) => {
            f.set(None);
            true
        }
        Some(n) => {
            f.set(Some(n - 1));
            false
        }
        None => false,
    }) {
        FAILED.with(|f| f.set(true));
        return Err(Error::NoMemory);
    }
    Ok(())
}

/// Room for `n` more items in `v`, or [`Error::NoMemory`].
fn reserve<T>(v: &mut alloc::vec::Vec<T>, n: usize) -> Result<(), Error> {
    alloc_point()?;
    v.try_reserve(n).map_err(|_| Error::NoMemory)
}
