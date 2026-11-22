//! Memory: the kernel's page tables, physical frames and the heap.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(lower_half_is_unmapped),
    ktest_case!(kernel_code_is_executable_not_writable),
    ktest_case!(kernel_data_is_writable_not_executable),
    ktest_case!(stack_guard_page_is_unmapped),
    ktest_case!(direct_map_uses_largest_pages),
    ktest_case!(frames_are_unique_and_aligned),
    ktest_case!(contiguous_frames),
    ktest_case!(page_zero_and_copy_match_byte_loops),
    ktest_case!(hardware_checksum_matches_the_table),
    ktest_case!(memory_routines_match_a_byte_model),
    ktest_case!(map_use_and_unmap_a_page),
    ktest_case!(double_mapping_is_rejected),
    ktest_case!(box_and_vec_work),
    ktest_case!(heap_alignment_is_honored),
    ktest_case!(heap_memory_is_reused),
    ktest_case!(large_heap_allocations_survive_fragmentation),
    ktest_case!(every_cpu_flushes_between_runs),
];

fn lower_half_is_unmapped() {
    assert_eq!(memory::translate(0), None);
    let (kernel_phys, _) = arch::boot::layout::kernel_phys();
    assert_eq!(
        memory::translate(kernel_phys),
        None,
        "identity map must be gone"
    );
    assert_eq!(memory::translate(0x0000_7fff_ffff_f000), None);
}

fn kernel_code_is_executable_not_writable() {
    let code = kernel_code_is_executable_not_writable as *const () as u64;
    let t = memory::translate(code).expect("kernel code is mapped");
    assert!(t.flags.is_executable());
    assert!(!t.flags.is_writable());
}

fn kernel_data_is_writable_not_executable() {
    use core::sync::atomic::{AtomicU64, Ordering};
    static DATA: AtomicU64 = AtomicU64::new(7);
    assert_eq!(DATA.load(Ordering::Relaxed), 7);
    let t = memory::translate(&DATA as *const _ as u64).expect("kernel data is mapped");
    assert!(t.flags.is_writable());
    assert!(!t.flags.is_executable());
}

fn stack_guard_page_is_unmapped() {
    let guard = arch::boot::layout::stack_guard();
    assert_eq!(memory::translate(guard), None);
    assert!(memory::translate(guard + FRAME_SIZE).is_some());
}

fn direct_map_uses_largest_pages() {
    use rhizo_mm::paging::PageSize;
    // Any RAM frame will do; RAM is not at the same addresses everywhere.
    let frame = memory::with_frames(|f| f.alloc())
        .flatten()
        .expect("free frame");
    let t = memory::translate(DirectMap::virt(frame) + 8).expect("direct map present");
    memory::with_frames(|f| f.free(frame));
    assert_eq!(t.phys, frame + 8);
    let expected = if arch::cpu::supports_1gib_pages() {
        PageSize::Huge
    } else {
        PageSize::Large
    };
    assert_eq!(t.size, expected);
    assert!(t.flags.is_writable() && !t.flags.is_executable());
}

fn frames_are_unique_and_aligned() {
    let frames: Vec<u64> = (0..64)
        .map(|_| {
            memory::with_frames(|f| f.alloc())
                .flatten()
                .expect("free frame")
        })
        .collect();
    let (kernel_start, kernel_end) = arch::boot::layout::kernel_phys();
    for (i, &frame) in frames.iter().enumerate() {
        assert_eq!(frame % FRAME_SIZE, 0);
        assert!(frame >= 1024 * 1024, "first MiB must stay reserved");
        assert!(frame + FRAME_SIZE <= kernel_start || frame >= kernel_end);
        assert!(!frames[..i].contains(&frame), "frame returned twice");
    }
    for frame in frames {
        memory::with_frames(|f| f.free(frame));
    }
}

/// Pseudo-random bytes (xorshift64), the same on every run.
fn noise(state: &mut u64, out: &mut [u8]) {
    for b in out {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *b = *state as u8;
    }
}

/// The architecture's page primitives (change `fast-mem-primitives`), as
/// the direct map uses them, agree with byte loops for random contents:
/// a zeroed frame, frame-to-frame copies of one page and of several, and
/// copies of whole pages to and from kernel buffers at every alignment.
fn page_zero_and_copy_match_byte_loops() {
    const PAGES: usize = 3;
    const PAGE: usize = FRAME_SIZE as usize;
    const LEN: usize = PAGES * PAGE;
    let run = memory::with_frames(|f| f.alloc_contiguous(2 * PAGES))
        .flatten()
        .expect("free run");
    let (a, b) = (run, run + LEN as u64);
    let mut state = 0x9e37_79b9_7f4a_7c15;
    let mut want = alloc::vec![0u8; LEN + 16];
    let mut got = alloc::vec![0u8; LEN + 16];
    for round in 0..16 {
        noise(&mut state, &mut want);
        DirectMap.write_pages(a, &want[..LEN]);
        for (i, byte) in want[..LEN].iter().enumerate() {
            let word = DirectMap.read_u64(a + (i as u64 & !7));
            assert_eq!(word.to_le_bytes()[i % 8], *byte, "round {round}: write");
        }
        DirectMap.copy(a, b, LEN as u64);
        DirectMap.read_pages(b, &mut got[..LEN]);
        assert_eq!(got[..LEN], want[..LEN], "round {round}: frame copy");
        DirectMap.copy(a + FRAME_SIZE, b, FRAME_SIZE);
        DirectMap.read_pages(b, &mut got[..PAGE]);
        assert_eq!(got[..PAGE], want[PAGE..2 * PAGE], "round {round}: one page");
        // To and from buffers at every offset modulo 16.
        let (from, to) = (round, 15 - round);
        DirectMap.write_pages(b, &want[from..from + LEN]);
        DirectMap.read_pages(b, &mut got[to..to + LEN]);
        assert_eq!(
            got[to..to + LEN],
            want[from..from + LEN],
            "round {round}: offsets"
        );
        DirectMap.zero_frame(b + FRAME_SIZE);
        DirectMap.read_pages(b, &mut got[..LEN]);
        assert!(
            got[PAGE..2 * PAGE].iter().all(|&x| x == 0),
            "round {round}: zeroed"
        );
        assert_eq!(
            got[..PAGE],
            want[from..from + PAGE],
            "the page before is untouched"
        );
        assert_eq!(
            got[2 * PAGE..LEN],
            want[from + 2 * PAGE..from + LEN],
            "and the one after"
        );
    }
    for i in 0..2 * PAGES as u64 {
        memory::with_frames(|f| f.free(run + i * FRAME_SIZE));
    }
}

// The kernel's memory routines (its own on AArch64, change
// `fast-thread-switch-aarch64`; `compiler_builtins`' on x86_64), called by
// symbol: a constant length would let the compiler inline the copy.
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

/// Byte `i` of the pattern `seed`: the reference the routines' results are
/// checked against, byte by byte (never with a routine under test).
fn pattern(seed: u64, i: usize) -> u8 {
    ((i as u64 ^ seed).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 56) as u8
}

/// Bytes `range` of `buf` as the pattern `seed`.
fn fill(buf: &mut [u8], seed: u64, range: core::ops::Range<usize>) {
    for i in range {
        buf[i] = pattern(seed, i);
    }
}

/// `start..end` and 16 bytes each side, within a buffer of `len` bytes.
fn near(start: usize, end: usize, len: usize) -> core::ops::Range<usize> {
    start.saturating_sub(16)..(end + 16).min(len)
}

/// `memcpy`, `memmove`, `memset` and `memcmp` give what a byte loop gives,
/// and touch nothing else: every length 0–160 at every pair of offsets
/// modulo 16, overlapping moves at every distance up to 72 bytes in both
/// directions, and random lengths and offsets up to 4,096 (change
/// `fast-thread-switch-aarch64`).
fn memory_routines_match_a_byte_model() {
    const LEN: usize = 4096;
    const ROOM: usize = 2 * LEN + 64;
    let (a_seed, b_seed) = (0x1234_5678_9abc_def0, 0x0fed_cba9_8765_4321);
    let mut a = alloc::vec![0u8; ROOM];
    let mut b = alloc::vec![0u8; ROOM];
    fill(&mut a, a_seed, 0..ROOM);
    // `n` bytes from `a[s..]` to `b[d..]`, with `b` as `b_seed` around
    // them (16 bytes each side are checked).
    let copy = |a: &[u8], b: &mut [u8], s: usize, d: usize, n: usize| {
        let near = near(d, d + n, b.len());
        fill(b, b_seed, near.clone());
        // SAFETY: both ranges are inside their buffers, which are distinct.
        let ret = unsafe { memcpy(b.as_mut_ptr().add(d), a.as_ptr().add(s), n) };
        assert_eq!(ret, b[d..].as_mut_ptr(), "memcpy returns its destination");
        for j in near {
            let want = if (d..d + n).contains(&j) {
                pattern(a_seed, s + j - d)
            } else {
                pattern(b_seed, j)
            };
            assert_eq!(b[j], want, "memcpy of {n} from {s} to {d}: byte {j}");
        }
    };
    // `n` bytes within `b`, from `s` to `d`.
    let mov = |b: &mut [u8], s: usize, d: usize, n: usize| {
        let near = near(s.min(d), s.max(d) + n, b.len());
        fill(b, b_seed, near.clone());
        // SAFETY: both ranges are inside `b`; `memmove` allows overlap.
        let ret = unsafe { memmove(b.as_mut_ptr().add(d), b.as_ptr().add(s), n) };
        assert_eq!(ret, b[d..].as_mut_ptr(), "memmove returns its destination");
        for j in near {
            let at = if (d..d + n).contains(&j) {
                s + j - d
            } else {
                j
            };
            assert_eq!(
                b[j],
                pattern(b_seed, at),
                "memmove of {n} from {s} to {d}: byte {j}"
            );
        }
    };
    let set = |b: &mut [u8], d: usize, n: usize, c: u8| {
        let near = near(d, d + n, b.len());
        fill(b, b_seed, near.clone());
        // SAFETY: the range is inside `b`. The high bits of `c` are ignored.
        let ret = unsafe { memset(b.as_mut_ptr().add(d), 0x7700 | c as i32, n) };
        assert_eq!(ret, b[d..].as_mut_ptr(), "memset returns its destination");
        for j in near {
            let want = if (d..d + n).contains(&j) {
                c
            } else {
                pattern(b_seed, j)
            };
            assert_eq!(b[j], want, "memset of {n} at {d}: byte {j}");
        }
    };
    // `a[s..s + n]` against a copy of it in `b[d..]` with byte `k` changed
    // by `delta` (none if `k >= n`), and every byte after `k` scrambled.
    let cmp = |a: &[u8], b: &mut [u8], s: usize, d: usize, n: usize, k: usize, delta: u8| {
        for j in 0..n {
            b[d + j] = match j.cmp(&k) {
                core::cmp::Ordering::Less => a[s + j],
                core::cmp::Ordering::Equal => a[s + j].wrapping_add(delta),
                core::cmp::Ordering::Greater => pattern(b_seed, j),
            };
        }
        // SAFETY: both ranges are inside their buffers.
        let got = unsafe { memcmp(a.as_ptr().add(s), b.as_ptr().add(d), n) };
        let want = if k < n {
            a[s + k] as i32 - b[d + k] as i32
        } else {
            0
        };
        assert_eq!(
            got.signum(),
            want.signum(),
            "memcmp of {n} at {s} and {d}, differing at {k}"
        );
    };
    for n in 0..=160 {
        for (s, d) in (0..16).flat_map(|s| (0..16).map(move |d| (s, d))) {
            copy(&a, &mut b, 16 + s, 16 + d, n);
            cmp(&a, &mut b, s, d, n, n, 0);
            if n > 0 {
                cmp(&a, &mut b, s, d, n, (s * 16 + d) % n, 1 + (n as u8 % 255));
            }
        }
        for d in 0..16 {
            set(&mut b, 16 + d, n, n as u8);
        }
        for dist in 0..=72 {
            mov(&mut b, 100, 100 + dist, n);
            mov(&mut b, 100 + dist, 100, n);
        }
    }
    // Explicit edges: a whole page, page-aligned or not, and a byte more.
    for (s, d, n) in [(0, 0, LEN), (0, LEN, LEN), (1, 7, LEN), (3, 0, LEN + 1)] {
        copy(&a, &mut b, s, d, n);
        mov(&mut b, s, d, n);
        mov(&mut b, d, s, n);
        set(&mut b, d, n, 0);
        cmp(&a, &mut b, s, d, n, n - 1, 0x80);
    }
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = |below: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % below as u64) as usize
    };
    for _ in 0..300 {
        let n = next(LEN + 1);
        let (s, d) = (next(LEN + 1), next(LEN + 1));
        copy(&a, &mut b, s, d, n);
        mov(&mut b, s, d, n);
        set(&mut b, d, n, next(256) as u8);
        cmp(&a, &mut b, s, d, n, next(n + 1), 1 + next(255) as u8);
    }
}

/// The kernel's CRC-32C (the CPU's instructions where it has them) gives
/// the bytes of the portable table for any length and alignment, so
/// snapshot files stay the same (change `fast-mem-primitives`).
fn hardware_checksum_matches_the_table() {
    if !arch::mem::has_crc32() {
        arch::console::BootConsole.write_bytes(b"(no CRC32 instructions on this CPU) ");
    }
    let mut bytes = alloc::vec![0u8; 9000];
    noise(&mut 0x2545_f491_4f6c_dd1d, &mut bytes);
    assert_eq!(arch::mem::crc32c(b"123456789"), 0xe306_9283);
    for len in (0..64).chain([4095, 4096, 4097, 8990]) {
        for skip in 0..8 {
            let part = &bytes[skip..skip + len];
            let want = rhizo_snapshot::crc32c(part);
            assert_eq!(arch::mem::crc32c(part), want, "{len} bytes at {skip}");
        }
    }
}

fn contiguous_frames() {
    let run = memory::with_frames(|f| f.alloc_contiguous(4))
        .flatten()
        .expect("free run");
    for i in 0..4 {
        assert!(memory::with_frames(|f| f.is_allocated(run + i * FRAME_SIZE)).unwrap());
    }
    memory::with_frames(|f| f.free_contiguous(run, 4));
}

const TEST_PAGE: u64 = 0xffff_c000_0000_0000;

fn map_use_and_unmap_a_page() {
    let frame = memory::with_frames(|f| f.alloc())
        .flatten()
        .expect("free frame");
    memory::map_page(
        TEST_PAGE,
        frame,
        PageFlags::WRITABLE | PageFlags::NO_EXECUTE,
    )
    .expect("map test page");

    // SAFETY: TEST_PAGE was just mapped writable to a frame we own.
    unsafe { (TEST_PAGE as *mut u64).write_volatile(0x5a5a_1234) };
    assert_eq!(DirectMap.read_u64(frame), 0x5a5a_1234);

    assert_eq!(memory::unmap_page(TEST_PAGE), Ok(frame));
    assert_eq!(memory::translate(TEST_PAGE), None);
    memory::with_frames(|f| f.free(frame));
}

fn double_mapping_is_rejected() {
    use rhizo_mm::paging::MapError;
    let frame = memory::with_frames(|f| f.alloc())
        .flatten()
        .expect("free frame");
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    memory::map_page(TEST_PAGE, frame, flags).expect("map test page");
    assert_eq!(
        memory::map_page(TEST_PAGE, frame, flags),
        Err(MapError::AlreadyMapped(TEST_PAGE))
    );
    assert_eq!(memory::unmap_page(TEST_PAGE), Ok(frame));
    memory::with_frames(|f| f.free(frame));
}

fn box_and_vec_work() {
    let boxed = Box::new(0xfeed_u64);
    assert_eq!(*boxed, 0xfeed);
    let v: Vec<u64> = (0..20_000).collect();
    assert_eq!(v.iter().sum::<u64>(), 19_999 * 20_000 / 2);
}

fn heap_alignment_is_honored() {
    for align in [1, 8, 64, 4096] {
        let layout = Layout::from_size_align(40, align).unwrap();
        // SAFETY: non-zero size; freed right below with the same layout.
        let p = unsafe { alloc::alloc::alloc(layout) };
        assert!(!p.is_null());
        assert_eq!(p as usize % align, 0, "alignment {align}");
        // SAFETY: allocated above with this layout.
        unsafe { alloc::alloc::dealloc(p, layout) };
    }
}

fn heap_memory_is_reused() {
    let first = Box::new([1u8; 200]);
    let addr = &*first as *const _ as usize;
    drop(first);
    let free_before = memory::free_bytes();
    let second = Box::new([2u8; 200]);
    assert_eq!(&*second as *const _ as usize, addr);
    assert_eq!(
        memory::free_bytes(),
        free_before,
        "reuse must not take new frames"
    );
}

/// With no two free frames next to each other, a large heap allocation
/// still succeeds (mapped from single frames), holds what is written, and
/// gives every frame back.
fn large_heap_allocations_survive_fragmentation() {
    let before = memory::free_bytes();
    let mut taken: Vec<u64> = Vec::with_capacity((before / FRAME_SIZE) as usize);
    let mut kept = Vec::with_capacity(taken.capacity() / 2 + 1);
    while let Some(phys) = memory::with_frames(|f| f.alloc()).flatten() {
        taken.push(phys);
    }
    // Give back every other frame: plenty of memory, no run of two.
    taken.sort_unstable();
    for (i, phys) in taken.into_iter().enumerate() {
        if i % 2 == 0 {
            memory::with_frames(|f| f.free(phys));
        } else {
            kept.push(phys);
        }
    }
    let mut big = alloc::vec![0u64; (1 << 20) / 8];
    for (i, word) in big.iter_mut().enumerate() {
        *word = i as u64 * 3;
    }
    assert!(big.iter().enumerate().all(|(i, &w)| w == i as u64 * 3));
    drop(big);
    for phys in kept {
        memory::with_frames(|f| f.free(phys));
    }
    // All back, the window's tables included (change `add-heap-trim`):
    // those its setup left are freed too, the first time.
    let free = memory::free_bytes();
    assert!(free >= before, "{} bytes not returned", before - free);
}

/// Between host runs, every CPU can be made to flush its translations and
/// paging-structure caches (what freeing the window's tables waits for,
/// change `add-heap-trim`); while another CPU runs a job, it cannot.
fn every_cpu_flushes_between_runs() {
    assert!(crate::smp::flush_everywhere());
    if crate::smp::count() > 1 && !crate::arch::user::TLB_FLUSH_IS_BROADCAST {
        fn spin(rounds: u64) -> u64 {
            (0..rounds).fold(0, |a, i| core::hint::black_box(a ^ i))
        }
        crate::smp::post(1, spin, 1 << 24);
        let busy = crate::smp::flush_everywhere();
        crate::smp::join(1);
        assert!(!busy, "a busy CPU is not asked");
        assert!(crate::smp::flush_everywhere());
    }
}
