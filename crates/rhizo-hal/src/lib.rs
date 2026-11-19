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

/// In-memory test doubles for host tests.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::{Console, CycleCounter};
    use std::cell::Cell;
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
}

#[cfg(test)]
mod tests {
    use super::testing::{ManualCycleCounter, RecordingConsole};
    use super::*;
    use core::fmt::Write;

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
    fn manual_cycle_counter_advances() {
        let counter = ManualCycleCounter::new(10);
        assert_eq!(counter.cycles(), 10);
        counter.advance(5);
        assert_eq!(counter.cycles(), 15);
    }
}
