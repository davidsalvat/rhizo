//! The Rhizo kernel.
//!
//! Boot flow: the hypervisor enters the PVH trampoline in `arch::boot`, which
//! switches to 64-bit mode in the higher half and calls [`kernel_main`].

#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod heap;
mod memory;
mod sync;
#[cfg(feature = "ktest")]
mod tests;

use arch::console::BootConsole;
use arch::power::Machine;
use memory::DirectMap;
use rhizo_core::options::{BootOptions, read_cmdline};
use rhizo_core::panic::SourceLocation;
use rhizo_hal::{ExitStatus, Power};

/// First Rust code to run, called by the boot trampoline on the kernel stack.
///
/// `start_info` is the physical address of the PVH `hvm_start_info`, and
/// `entry_tsc` the TSC value read at the first kernel instruction.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(start_info: u64, entry_tsc: u64) -> ! {
    arch::boot::record(start_info, entry_tsc);
    // Stage-1 tables map physical memory below 4 GiB at the direct map, so the
    // command line can be read before memory setup.
    let mut cmdline = [0u8; 256];
    let options = BootOptions::parse(read_cmdline(&DirectMap, start_info, &mut cmdline));
    arch::console::init(options.console);
    arch::power::init(options.exit);
    let mut console = BootConsole;
    rhizo_core::boot::print_banner(&mut console);

    arch::cpu::init();
    let summary = memory::init(start_info);
    rhizo_core::boot::report_memory(
        &mut console,
        summary.usable,
        summary.free,
        summary.page_table_frames,
    );

    let ready = {
        use rhizo_hal::CycleCounter;
        arch::tsc::Tsc.cycles()
    };
    rhizo_core::boot::report_ready(&mut console, entry_tsc, ready);

    #[cfg(feature = "ktest")]
    let status = rhizo_core::ktest::run(&mut console, tests::ALL);

    #[cfg(not(feature = "ktest"))]
    let status = {
        rhizo_core::boot::report_boot_complete(&mut console, entry_tsc, ready);
        ExitStatus::Success
    };

    Machine.exit(status)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // A fresh handle is safe here: the console is stateless after init and
    // the kernel runs on a single CPU with interrupts disabled.
    let mut console = BootConsole;
    let location = info.location().map(|loc| SourceLocation {
        file: loc.file(),
        line: loc.line(),
        column: loc.column(),
    });
    rhizo_core::panic::report_panic(&mut console, info.message(), location);
    Machine.exit(ExitStatus::Failure)
}
