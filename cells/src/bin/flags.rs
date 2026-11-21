//! Spins for many time slices with the flags a cell may set for itself
//! that change how the kernel's own code behaves: on x86_64 the direction
//! flag (string instructions run backwards) and the alignment-check flag
//! (which also lifts SMAP in ring 0). The kernel must not inherit them
//! when an interrupt stops the cell (spec `isolation`). On AArch64 it just
//! spins. Prints `flags ok` once it has its direction flag back as it set
//! it.
#![no_std]
#![no_main]

use rhizo_cells::{check, print};

/// Iterations of the spin loop: many time slices at any speed.
const ROUNDS: u64 = 200_000_000;

#[cfg(target_arch = "x86_64")]
fn spin() -> bool {
    let flags: u64;
    // SAFETY: only this program's flags and registers change; DF and AC are
    // cleared again before any Rust code runs (the ABI wants DF clear).
    unsafe {
        core::arch::asm!(
            "pushfq",
            "or qword ptr [rsp], {ac}",
            "popfq",
            "std",
            "2:",
            "dec {n}",
            "jnz 2b",
            "pushfq",
            "pop {flags}",
            "cld",
            "pushfq",
            "and qword ptr [rsp], {not_ac}",
            "popfq",
            ac = const 1u64 << 18,
            not_ac = const !(1u64 << 18),
            n = inout(reg) ROUNDS => _,
            flags = out(reg) flags,
        );
    }
    flags & (1 << 10) != 0
}

#[cfg(target_arch = "aarch64")]
fn spin() -> bool {
    let mut n = ROUNDS;
    // SAFETY: only a register changes.
    unsafe { core::arch::asm!("2:", "subs {n}, {n}, #1", "b.ne 2b", n = inout(reg) n) };
    n == 0
}

fn main() -> i32 {
    check(spin(), "flags lost");
    print(b"flags ok\n");
    0
}

rhizo_cells::entry!(main);
