//! Hardware-independent logic of the Rhizo kernel.
//!
//! Everything here is written against the interfaces in [`rhizo_hal`], so it
//! runs unchanged inside the kernel and in host tests.

#![cfg_attr(not(test), no_std)]

pub mod boot;
pub mod exception;
pub mod ktest;
pub mod memory;
pub mod options;
pub mod panic;

/// The kernel version, taken from the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
