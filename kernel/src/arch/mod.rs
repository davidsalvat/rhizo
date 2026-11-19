//! Platform-specific code. Everything that depends on the instruction set
//! lives here; the rest of the kernel uses only this interface:
//!
//! - `boot`: `record`, `entry_cycles`, `boot_arg`, `layout::{kernel_phys,
//!   text, rodata, data, stack_guard, virt_to_phys}`
//! - `platform`: `Format` (page-table entry format), `cmdline`,
//!   `for_each_ram`, `for_each_reserved`, `map_devices`
//! - `console`: `init`, `BootConsole`; `power`: `init`, `Machine`
//! - `cpu`: `init`, `supports_1gib_pages`, `activate`, `CpuTlb`
//! - `timer`: `Counter`, `cycles_to_ns`
//! - `user` (cell builds only): `enter_user`, `Trap`, `release_user`,
//!   `sync_code`; `platform::{MACHINE, ABI, CELL_ROOT_HAS_KERNEL_HALF}`
//! - `tests::ALL` (in-kernel tests specific to the architecture)

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;
