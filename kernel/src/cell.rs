//! Running cells: spawn from a static executable, run until exit or fault,
//! then return every frame (specs `cells` and `linux-abi`).

use crate::arch::console::BootConsole;
use crate::arch::cpu::CpuTlb;
use crate::arch::platform::Format;
use crate::arch::timer::{self, Counter};
use crate::arch::user::{self, Trap};
use crate::memory::{self, DirectMap};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use rhizo_abi::{Abi, Context, Outcome, Services};
use rhizo_cell::Cell;
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

/// Console output of a cell: echoed to the console and/or copied into
/// `capture`.
struct KernelServices<'a> {
    capture: Option<&'a mut Vec<u8>>,
    echo: bool,
    random: u64,
}

impl Console for KernelServices<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) {
        if self.echo {
            BootConsole.write_bytes(bytes);
        }
        if let Some(capture) = self.capture.as_deref_mut() {
            capture.extend_from_slice(bytes);
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

/// A cell created and ready to run.
#[derive(Debug)]
pub struct Spawned {
    cell: Cell,
    seed: u64,
}

impl Spawned {
    /// Page-table frames of the cell's own (lower) half.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark metric")
    )]
    pub fn table_frames(&self) -> usize {
        PageTable::<Format>::from_root(self.cell.root).lower_half_tables(&DirectMap)
    }
}

/// Creates a cell from the static executable `image` with `argv`.
pub fn spawn(image: &[u8], argv: &[&[u8]]) -> Result<Spawned, ElfError> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut services = KernelServices {
        capture: None,
        echo: false,
        random: Counter.cycles() | 1,
    };
    let mut random = [0u8; 16];
    services.random(&mut random);
    let kernel = memory::kernel_tables();
    let kernel_half = user::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
    let cell = rhizo_cell::spawn::<Format>(
        id,
        image,
        user::MACHINE,
        kernel_half,
        &mut DirectMap,
        &mut KernelFrames,
        argv,
        &[],
        random,
    )?;
    sync_code(&PageTable::<Format>::from_root(cell.root), image);
    Ok(Spawned {
        cell,
        seed: services.random,
    })
}

/// Runs a spawned cell on this CPU until it exits or faults, then frees it.
/// Its output (and a fault report) goes to the console when `echo` is set,
/// and to `capture` when given.
#[allow(
    clippy::needless_pass_by_value,
    reason = "taking the cell by value ends it: it cannot be run twice"
)]
pub fn finish(spawned: Spawned, capture: Option<&mut Vec<u8>>, echo: bool) -> Ending {
    let Spawned { mut cell, seed } = spawned;
    let mut services = KernelServices {
        capture,
        echo,
        random: seed,
    };
    let pt = PageTable::<Format>::from_root(cell.root);
    let mut abi = Abi::new(user::ABI);
    let ending = loop {
        match user::enter_user(cell.root, &mut cell.regs) {
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
                fault.report(&mut services, cell.id);
                break Ending::Fault;
            }
        }
    };
    user::release_user();
    space::destroy(pt, &mut DirectMap, &mut KernelFrames);
    ending
}

/// Spawns and runs a cell to its end, echoing its output to the console.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn run(
    image: &[u8],
    argv: &[&[u8]],
    capture: Option<&mut Vec<u8>>,
) -> Result<Ending, ElfError> {
    Ok(finish(spawn(image, argv)?, capture, true))
}

/// `bench-cell`: spawns and runs the hello cell repeatedly and prints the
/// median spawn time, the median time to run it to its exit (spawn
/// included), and its page-table frames.
#[cfg(feature = "bench-cell")]
pub fn bench(console: &mut impl Console) {
    use core::fmt::Write;
    const RUNS: usize = 101;
    let image = test_image("hello");
    let mut spawn_ns = Vec::with_capacity(RUNS);
    let mut run_ns = Vec::with_capacity(RUNS);
    let mut tables = 0;
    let mut out = Vec::with_capacity(64);
    for _ in 0..RUNS {
        out.clear();
        let t0 = Counter.cycles();
        let spawned = spawn(image, &[b"hello"]).expect("the hello cell spawns");
        let t1 = Counter.cycles();
        tables = spawned.table_frames();
        let ending = finish(spawned, Some(&mut out), false);
        let t2 = Counter.cycles();
        assert_eq!(ending, Ending::Exit(0), "the hello cell exits with 0");
        assert_eq!(out, b"hello from a cell\n");
        spawn_ns.push(timer::cycles_to_ns(t1 - t0).unwrap_or(0));
        run_ns.push(timer::cycles_to_ns(t2 - t0).unwrap_or(0));
    }
    spawn_ns.sort_unstable();
    run_ns.sort_unstable();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-cell spawn_ns={} run_ns={} pt_frames={tables}",
        spawn_ns[RUNS / 2],
        run_ns[RUNS / 2]
    );
}

/// Makes the executable pages just written through the direct map visible to
/// instruction fetch (needed on AArch64; nothing to do on x86_64).
fn sync_code(pt: &PageTable<Format>, image: &[u8]) {
    let Ok(image) = Image::parse(image, user::MACHINE) else {
        return;
    };
    for s in image.segments().filter(|s| s.flags.is_executable()) {
        let first = s.vaddr & !(FRAME_SIZE - 1);
        for page in (first..s.vaddr + s.mem_size).step_by(FRAME_SIZE as usize) {
            if let Some(t) = pt.translate(&DirectMap, page) {
                user::sync_code(DirectMap::virt(t.phys), FRAME_SIZE);
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
