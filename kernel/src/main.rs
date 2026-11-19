//! The Rhizo kernel.
//!
//! Boot flow: the hypervisor enters the PVH trampoline in
//! `arch::boot`, which switches to 64-bit mode and calls [`kernel_main`].

#![no_std]
#![no_main]

mod arch;
#[cfg(feature = "ktest")]
mod tests;

use arch::qemu::QemuExit;
use arch::serial::Serial;
use rhizo_core::panic::SourceLocation;
use rhizo_hal::{ExitStatus, Power};

/// First Rust code to run, called by the boot trampoline on the boot stack.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    let mut console = Serial::com1();
    console.init();
    rhizo_core::boot::print_banner(&mut console);

    #[cfg(feature = "ktest")]
    let status = rhizo_core::ktest::run(&mut console, tests::ALL);

    #[cfg(not(feature = "ktest"))]
    let status = {
        use rhizo_hal::CycleCounter;
        let end = arch::tsc::Tsc.cycles();
        rhizo_core::boot::report_boot_complete(&mut console, arch::boot::entry_cycles(), end);
        ExitStatus::Success
    };

    QemuExit.exit(status)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // A fresh handle is safe here: the serial port is stateless after init and
    // the kernel runs on a single CPU with interrupts disabled.
    let mut console = Serial::com1();
    let location = info.location().map(|loc| SourceLocation {
        file: loc.file(),
        line: loc.line(),
        column: loc.column(),
    });
    rhizo_core::panic::report_panic(&mut console, info.message(), location);
    QemuExit.exit(ExitStatus::Failure)
}
