//! What the system calls of a cell ask of the kernel (`rhizo_abi::Services`):
//! console, clocks, memory, FP state and random bytes.

use super::*;

/// Most output kept for one cell when the host captures it (spec
/// `isolation`): a cell printing without end cannot fill the kernel heap.
const CAPTURE_MAX: usize = 1 << 20;

/// Console output of a cell: echoed to the console and/or copied into
/// `capture`.
pub(super) struct KernelServices<'a> {
    pub(super) capture: Option<&'a mut Vec<u8>>,
    pub(super) echo: bool,
    /// The cell's generator, split off the kernel's on first use.
    pub(super) random: &'a mut Option<Generator>,
    /// The calling thread's FP/SIMD state is in the CPU's registers.
    pub(super) fp_in_cpu: bool,
}

impl Console for KernelServices<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        if self.echo {
            BootConsole.write_bytes(bytes);
        }
        // Captured output is kernel heap: bounded per cell, and never grown
        // by an allocation that could fail (and panic); the rest is dropped.
        if let Some(capture) = self.capture.as_deref_mut() {
            let n = bytes.len().min(CAPTURE_MAX.saturating_sub(capture.len()));
            if capture.try_reserve(n).is_ok() {
                capture.extend_from_slice(&bytes[..n]);
            }
        }
    }
}

impl Services for KernelServices<'_> {
    fn log(&mut self, message: core::fmt::Arguments<'_>) {
        use core::fmt::Write;
        let _ = writeln!(rhizo_hal::ConsoleWriter(&mut BootConsole), "{message}");
    }

    fn monotonic_ns(&mut self) -> u64 {
        timer::cycles_to_ns(Counter.cycles()).unwrap_or(0)
    }

    fn realtime_ns(&mut self) -> u64 {
        // No real-time clock yet: the epoch is the boot.
        self.monotonic_ns()
    }

    fn can_store(&mut self, bytes: u64) -> bool {
        cells_may_take(bytes)
    }

    /// The frames cells may have, fixed at boot: nothing another cell does
    /// changes it (design D1 of `close-cross-cell-leaks`).
    fn memory(&mut self) -> u64 {
        memory::with_frames(|f| f.usable_frames() as u64).unwrap_or(0) * FRAME_SIZE
    }

    fn save_fp(&mut self, fp: &mut FpState) {
        if self.fp_in_cpu {
            user::fp_save(fp);
        }
    }

    fn random(&mut self, out: &mut [u8]) {
        self.random
            .get_or_insert_with(|| RANDOM.with(Batched::split))
            .fill(out);
    }
}

/// The 16 random bytes of a new cell's stack (`AT_RANDOM`), from the
/// kernel's generator.
pub(super) fn start_random() -> [u8; 16] {
    let mut random = [0u8; 16];
    RANDOM.with(|g| g.fill(&mut random));
    random
}

/// The TLB of a hierarchy no CPU has used yet: nothing to flush (its tag's
/// old translations go on its first entry on each CPU).
pub(super) struct NewSpace;

impl Tlb for NewSpace {
    fn flush_page(&mut self, _: u64) {}
}
