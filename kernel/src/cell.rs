//! Running cells: spawn from a static executable, run until exit or fault,
//! then return every frame (specs `cells` and `linux-abi`).

use crate::arch::console::BootConsole;
use crate::arch::cpu::{self, CpuTlb, Trap};
use crate::arch::platform::{self, Format};
use crate::arch::timer::{self, Counter};
use crate::memory::{self, DirectMap};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use rhizo_abi::{Abi, Context, Outcome, Services};
use rhizo_cell::elf::{ElfError, Image};
use rhizo_cell::space::{self, Frames};
use rhizo_hal::{Console, CycleCounter};
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::{PageSize, PageTable};

/// How a cell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It called `exit` or `exit_group` with this status.
    Exit(i32),
    /// It raised an exception and was killed.
    Fault,
}

/// The kernel's frame allocator, as cells see it.
#[derive(Debug)]
struct KernelFrames;

impl Frames for KernelFrames {
    fn alloc(&mut self) -> Option<u64> {
        memory::with_frames(|f| f.alloc()).flatten()
    }

    fn alloc_large(&mut self) -> Option<u64> {
        memory::with_frames(|f| f.alloc_large()).flatten()
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        let frames = (size.bytes() / FRAME_SIZE) as usize;
        memory::with_frames(|f| f.free_contiguous(phys, frames));
    }
}

/// Console output of a cell, copied into `capture` when the caller wants it.
struct KernelServices<'a> {
    capture: Option<&'a mut Vec<u8>>,
    random: u64,
}

impl Console for KernelServices<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        BootConsole.write_bytes(bytes);
        if let Some(capture) = self.capture.as_deref_mut() {
            capture.extend_from_slice(bytes);
        }
    }
}

impl Services for KernelServices<'_> {
    fn monotonic_ns(&mut self) -> u64 {
        timer::cycles_to_ns(Counter.cycles()).unwrap_or(0)
    }

    fn realtime_ns(&mut self) -> u64 {
        // No real-time clock yet: the epoch is the boot.
        self.monotonic_ns()
    }

    fn random(&mut self, out: &mut [u8]) {
        // xorshift64*, seeded from the counter: not for cryptography (design
        // D6); deterministic sources for snapshots come in H4.
        for byte in out {
            self.random ^= self.random >> 12;
            self.random ^= self.random << 25;
            self.random ^= self.random >> 27;
            *byte = (self.random.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56) as u8;
        }
    }
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Spawns a cell from `image` with `argv`, runs it to the end on this CPU and
/// frees it. The cell's output (and a fault report) goes to the console and,
/// when given, to `capture`.
pub fn run(
    image: &[u8],
    argv: &[&[u8]],
    capture: Option<&mut Vec<u8>>,
) -> Result<Ending, ElfError> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let seed = Counter.cycles() | 1;
    let mut random = [0u8; 16];
    let mut services = KernelServices {
        capture,
        random: seed,
    };
    services.random(&mut random);

    let kernel = memory::kernel_tables();
    let kernel_half = platform::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
    let mut cell = rhizo_cell::spawn::<Format>(
        id,
        image,
        platform::MACHINE,
        kernel_half,
        &mut DirectMap,
        &mut KernelFrames,
        argv,
        &[],
        random,
    )?;
    let pt = PageTable::<Format>::from_root(cell.root);
    sync_code(&pt, image);

    let mut abi = Abi::new(platform::ABI);
    let ending = loop {
        match cpu::enter_user(cell.root, &mut cell.regs) {
            Trap::Syscall => {
                let mut cx = Context {
                    pt,
                    mem: &mut DirectMap,
                    tlb: &mut CpuTlb,
                    frames: &mut KernelFrames,
                    services: &mut services,
                };
                if let Outcome::Exit(status) = abi.handle(&mut cell, &mut cx) {
                    break Ending::Exit(status);
                }
            }
            Trap::Fault(fault) => {
                fault.report(&mut services, id);
                break Ending::Fault;
            }
        }
    };
    cpu::release_user();
    space::destroy(pt, &mut DirectMap, &mut KernelFrames);
    Ok(ending)
}

/// Makes the executable pages just written through the direct map visible to
/// instruction fetch (needed on AArch64; nothing to do on x86_64).
fn sync_code(pt: &PageTable<Format>, image: &[u8]) {
    let Ok(image) = Image::parse(image, platform::MACHINE) else {
        return;
    };
    for s in image.segments().filter(|s| s.flags.is_executable()) {
        let first = s.vaddr & !(FRAME_SIZE - 1);
        for page in (first..s.vaddr + s.mem_size).step_by(FRAME_SIZE as usize) {
            if let Some(t) = pt.translate(&DirectMap, page) {
                cpu::sync_code(DirectMap::virt(t.phys), FRAME_SIZE);
            }
        }
    }
}

/// The test cells built by `cargo xtask` and embedded at build time.
pub fn test_image(name: &str) -> &'static [u8] {
    macro_rules! cells {
        ($($name:literal),*) => {
            match name {
                $($name => include_bytes!(concat!(env!("OUT_DIR"), "/cells/", $name)),)*
                _ => panic!("no test cell named {name}"),
            }
        };
    }
    cells!("hello", "exit3", "abi", "fault_null", "fault_kernel")
}
