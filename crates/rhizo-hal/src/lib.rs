//! Hardware interfaces for the Rhizo kernel.
//!
//! Kernel core logic reaches hardware only through the traits in this crate.
//! The bootable kernel provides real implementations; host tests use the
//! in-memory doubles from the [`testing`] module (feature `testing`). This is
//! what lets the core run, and later be simulated deterministically, on the
//! host.

#![cfg_attr(not(any(test, feature = "testing")), no_std)]

use core::fmt;

/// A byte-oriented output device, such as a serial port.
pub trait Console {
    /// Writes all bytes to the device, blocking until they are accepted.
    fn write_bytes(&mut self, bytes: &[u8]);
}

/// Adapter that lets any [`Console`] be used with `core::fmt` macros
/// (`write!`, `writeln!`).
#[derive(Debug)]
pub struct ConsoleWriter<'a, C: Console + ?Sized>(pub &'a mut C);

impl<C: Console + ?Sized> fmt::Write for ConsoleWriter<'_, C> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_bytes(s.as_bytes());
        Ok(())
    }
}

/// How the machine (or virtual machine) should end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    /// Everything went as expected.
    Success,
    /// Something failed (a test, a panic, a fatal error).
    Failure,
}

/// Control over stopping the machine.
pub trait Power {
    /// Stops the machine, reporting `status` to whoever started it.
    fn exit(&mut self, status: ExitStatus) -> !;
}

/// A monotonically increasing counter of CPU cycles.
pub trait CycleCounter {
    /// Returns the current cycle count.
    fn cycles(&self) -> u64;
}

/// Access to physical memory by physical address.
///
/// The kernel implements this through its direct map; host tests use
/// [`testing::FakePhysicalMemory`]. Addresses passed to `read_u64` and
/// `write_u64` must be 8-byte aligned.
pub trait PhysicalMemory {
    /// Reads the 64-bit value stored at physical address `phys`.
    fn read_u64(&self, phys: u64) -> u64;

    /// Writes `value` at physical address `phys`.
    fn write_u64(&mut self, phys: u64, value: u64);

    /// Fills the 4 KiB frame starting at `frame` with zeros.
    fn zero_frame(&mut self, frame: u64) {
        for offset in (0..4096).step_by(8) {
            self.write_u64(frame + offset, 0);
        }
    }

    /// Copies `out.len()` bytes starting at physical address `phys` (any
    /// alignment) into `out`.
    fn read_bytes(&self, phys: u64, out: &mut [u8]) {
        for (i, byte) in out.iter_mut().enumerate() {
            let p = phys + i as u64;
            *byte = (self.read_u64(p & !7) >> ((p & 7) * 8)) as u8;
        }
    }

    /// Copies `data` to physical address `phys` (any alignment).
    fn write_bytes(&mut self, phys: u64, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let p = phys + i as u64;
            let shift = (p & 7) * 8;
            let word = self.read_u64(p & !7);
            self.write_u64(p & !7, (word & !(0xff << shift)) | ((byte as u64) << shift));
        }
    }
}

/// The processor's cache of address translations.
pub trait Tlb {
    /// Invalidates any cached translation for the page containing `virt`.
    fn flush_page(&mut self, virt: u64);
}

/// In-memory test doubles for host tests.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::{Console, CycleCounter, PhysicalMemory, Tlb};
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::string::String;
    use std::vec::Vec;

    /// A console that records everything written to it.
    #[derive(Debug, Default)]
    pub struct RecordingConsole {
        bytes: Vec<u8>,
    }

    impl RecordingConsole {
        /// Creates an empty recording console.
        pub fn new() -> Self {
            Self::default()
        }

        /// Returns the raw bytes written so far.
        pub fn bytes(&self) -> &[u8] {
            &self.bytes
        }

        /// Returns the output written so far as text (lossy UTF-8).
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.bytes).into_owned()
        }
    }

    impl Console for RecordingConsole {
        fn write_bytes(&mut self, bytes: &[u8]) {
            self.bytes.extend_from_slice(bytes);
        }
    }

    /// A cycle counter whose value is set by the test.
    #[derive(Debug, Default)]
    pub struct ManualCycleCounter {
        now: Cell<u64>,
    }

    impl ManualCycleCounter {
        /// Creates a counter starting at `start`.
        pub fn new(start: u64) -> Self {
            Self {
                now: Cell::new(start),
            }
        }

        /// Moves the counter forward by `delta` cycles.
        pub fn advance(&self, delta: u64) {
            self.now.set(self.now.get() + delta);
        }
    }

    impl CycleCounter for ManualCycleCounter {
        fn cycles(&self) -> u64 {
            self.now.get()
        }
    }

    /// Sparse physical memory: every address reads as zero until written.
    #[derive(Debug, Default)]
    pub struct FakePhysicalMemory {
        words: BTreeMap<u64, u64>,
    }

    impl FakePhysicalMemory {
        /// Creates an empty (all-zero) physical memory.
        pub fn new() -> Self {
            Self::default()
        }

        /// Number of 64-bit words holding a non-zero value.
        pub fn nonzero_words(&self) -> usize {
            self.words.values().filter(|&&v| v != 0).count()
        }
    }

    impl PhysicalMemory for FakePhysicalMemory {
        fn read_u64(&self, phys: u64) -> u64 {
            assert_eq!(phys % 8, 0, "unaligned physical read at {phys:#x}");
            self.words.get(&phys).copied().unwrap_or(0)
        }

        fn write_u64(&mut self, phys: u64, value: u64) {
            assert_eq!(phys % 8, 0, "unaligned physical write at {phys:#x}");
            if value == 0 {
                self.words.remove(&phys);
            } else {
                self.words.insert(phys, value);
            }
        }
    }

    /// A TLB that records every flushed page.
    #[derive(Debug, Default)]
    pub struct RecordingTlb {
        /// Pages flushed so far, in order.
        pub flushed: Vec<u64>,
    }

    impl Tlb for RecordingTlb {
        fn flush_page(&mut self, virt: u64) {
            self.flushed.push(virt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{FakePhysicalMemory, ManualCycleCounter, RecordingConsole, RecordingTlb};
    use super::*;
    use core::fmt::Write;

    #[test]
    fn byte_copies_work_at_any_alignment() {
        let mut mem = FakePhysicalMemory::new();
        mem.write_bytes(0x1003, b"rhizo cells!");
        let mut back = [0u8; 12];
        mem.read_bytes(0x1003, &mut back);
        assert_eq!(&back, b"rhizo cells!");
        assert_eq!(
            mem.read_u64(0x1000) >> 24,
            u64::from_le_bytes(*b"rhizo\0\0\0")
        );
    }

    #[test]
    fn recording_console_reads_back_exact_bytes() {
        let mut console = RecordingConsole::new();
        console.write_bytes(b"hello\n");
        console.write_bytes(&[0x00, 0xff]);
        assert_eq!(console.bytes(), b"hello\n\x00\xff");
    }

    #[test]
    fn console_writer_supports_formatting() {
        let mut console = RecordingConsole::new();
        write!(ConsoleWriter(&mut console), "{}-{:x}", 42, 255).unwrap();
        assert_eq!(console.text(), "42-ff");
    }

    #[test]
    fn fake_physical_memory_reads_back_and_zeroes_frames() {
        let mut mem = FakePhysicalMemory::new();
        assert_eq!(mem.read_u64(0x1000), 0);
        mem.write_u64(0x1008, 0xdead_beef);
        mem.write_u64(0x2000, 7);
        assert_eq!(mem.read_u64(0x1008), 0xdead_beef);
        mem.zero_frame(0x1000);
        assert_eq!(mem.read_u64(0x1008), 0);
        assert_eq!(mem.read_u64(0x2000), 7);
        assert_eq!(mem.nonzero_words(), 1);
    }

    #[test]
    fn recording_tlb_records_flushes() {
        let mut tlb = RecordingTlb::default();
        tlb.flush_page(0x4000);
        tlb.flush_page(0x8000);
        assert_eq!(tlb.flushed, vec![0x4000, 0x8000]);
    }

    #[test]
    fn manual_cycle_counter_advances() {
        let counter = ManualCycleCounter::new(10);
        assert_eq!(counter.cycles(), 10);
        counter.advance(5);
        assert_eq!(counter.cycles(), 15);
    }
}
