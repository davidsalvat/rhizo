//! Several CPUs (spec `multi-core`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(every_cpu_has_its_number_and_own_stack),
    ktest_case!(kernel_globals_are_safe_across_cpus),
    ktest_case!(timed_waits_last_as_long_on_every_cpu),
    ktest_case!(idle_cpus_wake_only_for_deadlines),
    ktest_case!(a_run_with_a_time_limit_ends_on_time),
    ktest_case!(cells_run_on_every_cpu),
    ktest_case!(busy_cpus_do_not_starve_a_cell),
    ktest_case!(cells_in_parallel_stay_isolated),
    ktest_case!(unmapped_page_is_gone_on_every_cpu),
    ktest_case!(a_cell_on_several_cpus_survives_suspend_and_fork),
    ktest_case!(host_operations_leave_other_cells_running),
    ktest_case!(mixed_host_operations_during_a_run_are_invisible),
    ktest_case!(threads_of_a_cell_run_on_several_cpus_at_once),
];

/// The current stack pointer, roughly: the address of a local.
fn stack_address(_: u64) -> u64 {
    let local = 0u8;
    core::hint::black_box(&local) as *const u8 as u64
}

fn cpu_number(_: u64) -> u64 {
    arch::cpu::index() as u64
}

fn every_cpu_has_its_number_and_own_stack() {
    let mut stacks = Vec::new();
    stacks.push(stack_address(0));
    assert_eq!(arch::cpu::index(), 0, "tests run on the boot CPU");
    for cpu in 1..crate::smp::count() {
        crate::smp::post(cpu, cpu_number, 0);
        assert_eq!(crate::smp::join(cpu), cpu as u64);
        crate::smp::post(cpu, stack_address, 0);
        stacks.push(crate::smp::join(cpu));
    }
    for (cpu, &sp) in stacks.iter().enumerate() {
        let guard = crate::smp::stack_guard(cpu);
        assert!(
            sp > guard && sp - guard <= 0x1_1000,
            "CPU {cpu}: stack {sp:#x} is not above its guard {guard:#x}"
        );
        assert!(
            memory::translate(guard).is_none(),
            "CPU {cpu}: guard mapped"
        );
    }
}

/// Shared by the CPUs in `kernel_globals_are_safe_across_cpus`.
static COUNTER: crate::sync::Global<(u64, u64)> = crate::sync::Global::new((0, 0));

/// Increments both halves of [`COUNTER`] `rounds` times, allocating and
/// freeing kernel heap in between; returns the CPU's number.
fn hammer(rounds: u64) -> u64 {
    for i in 0..rounds {
        COUNTER.with(|(a, b)| {
            *a += 1;
            core::hint::black_box(&*a);
            *b += 1;
        });
        let boxed = Box::new([i; 16]);
        assert_eq!(core::hint::black_box(&boxed)[15], i);
    }
    arch::cpu::index() as u64
}

fn kernel_globals_are_safe_across_cpus() {
    const ROUNDS: u64 = 20_000;
    let cpus = crate::smp::count();
    let before = COUNTER.with(|c| *c);
    let heap_before = crate::heap::stats();
    for cpu in 1..cpus {
        crate::smp::post(cpu, hammer, ROUNDS);
    }
    assert_eq!(hammer(ROUNDS), 0);
    for cpu in 1..cpus {
        assert_eq!(crate::smp::join(cpu), cpu as u64);
    }
    let added = ROUNDS * cpus as u64;
    assert_eq!(COUNTER.with(|c| *c), (before.0 + added, before.1 + added));
    assert_eq!(
        crate::heap::stats().live,
        heap_before.live,
        "every allocation freed"
    );
}

/// A 20 ms wait on each CPU's own timer lasts 20 ms by the counter, on
/// the secondary CPUs as on the boot CPU (spec `multi-core`: x86_64
/// secondary CPUs once kept the local APIC timer's reset divider, and
/// such a wait ended after 1.3 ms on QEMU). The longest of three tries, as
/// another interrupt may end a wait early.
fn timed_waits_last_as_long_on_every_cpu() {
    const WAIT_NS: u64 = 20_000_000;
    for cpu in 0..crate::smp::count() {
        let took = (0..3)
            .map(|_| {
                if cpu == 0 {
                    crate::cell::timed_wait(WAIT_NS)
                } else {
                    crate::smp::post(cpu, crate::cell::timed_wait, WAIT_NS);
                    crate::smp::join(cpu)
                }
            })
            .max()
            .unwrap_or(0);
        assert!(
            (WAIT_NS * 9 / 10..WAIT_NS * 10).contains(&took),
            "CPU {cpu}: a {WAIT_NS} ns wait took {took} ns"
        );
    }
}

/// Every CPU idles while one cell sleeps 300 ms: the CPUs leave their
/// waits a handful of times in all, not at every slice (spec `multi-core`,
/// idle CPUs use no time: each idle CPU once looked again every 10 ms while
/// a cell slept), and the sleeper still ends on time.
fn idle_cpus_wake_only_for_deadlines() {
    use crate::prof::{self, Count};
    let now = || arch::timer::cycles_to_ns(arch::timer::Counter.cycles()).unwrap_or(0);
    let mut host = Host::new();
    let id = spawn(&mut host, "sleeper", &[b"300"]);
    let (w0, t0) = (prof::get(Count::IdleWakeups), now());
    host.run(|h| h.finished(id).is_some());
    let (wakeups, took) = (prof::get(Count::IdleWakeups) - w0, now() - t0);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "slept ok\n".into()));
    let cpus = crate::smp::count() as u64;
    assert!(
        wakeups <= 4 * cpus,
        "{cpus} CPUs left their waits {wakeups} times in 300 ms"
    );
    assert!(took < 400_000_000, "the 300 ms sleeper took {took} ns");
}

/// Cells that sleep an hour while every CPU idles: a run limited to 50 ms
/// ends soon after its limit (500 ms leaves room for an emulated machine on
/// a busy host, far below the hardware's longest wait, 2 s or more), again
/// and again, whichever CPU's timer holds the hour (spec `scheduling`, change `fix-lost-wakeup-smp`). Before,
/// on several CPUs the boot CPU waited for a wake-up only, when another
/// CPU's timer held the hour, and the run outlived its limit by that hour.
fn a_run_with_a_time_limit_ends_on_time() {
    let now = || arch::timer::cycles_to_ns(arch::timer::Counter.cycles()).unwrap_or(0);
    let mut host = Host::new();
    let ids: Vec<u32> = (0..crate::smp::count())
        .map(|_| spawn(&mut host, "sleeper", &[b"3600000"]))
        .collect();
    for round in 0..20 {
        let t0 = now();
        host.run_until(t0 + 50_000_000, |_| false);
        let took = now() - t0;
        assert!(
            took < 500_000_000,
            "round {round}: a run limited to 50 ms took {took} ns"
        );
    }
    for id in ids {
        assert!(host.asleep(id), "cell {id} still sleeps its hour");
        assert!(host.kill(id));
    }
}

/// Every CPU online, as a mask.
fn all_cpus() -> u32 {
    u32::MAX >> (32 - crate::smp::count())
}

/// Two CPU-bound cells per CPU, started together: they all finish, and
/// between them they ran on every CPU (spec `multi-core`, cells in
/// parallel).
fn cells_run_on_every_cpu() {
    let mut host = Host::new();
    let ids: Vec<u32> = (0..2 * crate::smp::count())
        .map(|_| spawn(&mut host, "spin", &[b"20"]))
        .collect();
    host.run(|_| false);
    let mut cpus = 0;
    for id in ids {
        assert_eq!(ended(&host, id).0, Ending::Exit(0));
        cpus |= host.finished(id).expect("ended").cpus;
    }
    assert_eq!(cpus, all_cpus(), "the cells used every CPU");
}

/// More cells spinning for ever than there are CPUs: a cell that prints
/// and exits still gets its turn (spec `scheduling`, preemption, on every
/// CPU).
fn busy_cpus_do_not_starve_a_cell() {
    let mut host = Host::new();
    let spinners: Vec<u32> = (0..crate::smp::count() + 1)
        .map(|_| spawn(&mut host, "spin", &[]))
        .collect();
    let hello = spawn(&mut host, "hello", &[]);
    host.run(|h| h.finished(hello).is_some());
    assert_eq!(
        ended(&host, hello),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    assert_eq!(host.live(), spinners.len(), "the spinners still run");
    for id in spinners {
        assert!(host.kill(id));
    }
}

/// Four copies each of programs that use memory, files, threads and time,
/// all at once on every CPU: each prints exactly what it prints alone, and
/// all memory comes back.
fn cells_in_parallel_stay_isolated() {
    let programs = ["counter", "filer", "libc_threads", "chatter", "threads"];
    let round = || {
        let mut host = Host::new();
        let mut ids = Vec::new();
        for _ in 0..4 {
            for name in programs {
                ids.push((name, spawn(&mut host, name, &[])));
            }
        }
        host.run(|_| false);
        for (name, id) in ids {
            assert_eq!(
                ended(&host, id),
                (Ending::Exit(0), expected_output(name).into()),
                "{name}"
            );
        }
        drop(host);
        crate::cell::drop_images(); // free memory with no image kept loaded
    };
    // The first round grows the kernel heap, which keeps the frames it grows
    // into (as `assert_no_leak` does), so the check does not depend on which
    // tests ran before this one.
    round();
    let before = free_or_in_heap();
    round();
    assert_eq!(free_or_in_heap(), before);
}

/// `parsum` with `threads` threads summing `millions` million rounds,
/// alone on every CPU: its output, and the most CPUs that ran its threads
/// at the same time.
fn parsum(threads: &[u8], millions: &[u8]) -> (alloc::string::String, u32) {
    let mut host = Host::new();
    let id = spawn(&mut host, "parsum", &[threads, millions]);
    host.run(|h| h.finished(id).is_some());
    let (ending, output) = ended(&host, id);
    assert_eq!(ending, Ending::Exit(0), "{output}");
    (output, host.finished(id).expect("ended").peak)
}

/// Four threads of one cell on four CPUs: the same sum as one thread (and
/// as Linux, `cells/expected.rs`), with threads of the cell running on
/// several CPUs at the same time (spec `threads`, threads of a cell in
/// parallel; `scheduling`, idle CPUs run threads of running cells). The
/// speed-up itself is measured by `cell.thread_scaling_pct`, not here: it
/// depends on the host's spare cores.
fn threads_of_a_cell_run_on_several_cpus_at_once() {
    run_expected("parsum");
    let (one, _) = parsum(b"1", b"24");
    let (four, peak) = parsum(b"4", b"24");
    assert_eq!(four, one, "the sum does not depend on the threads");
    assert_eq!(one, "sum 2868a16807fecd08\n");
    if crate::smp::count() >= 2 {
        assert!(peak >= 2, "its threads ran on {peak} CPU at a time");
    } else {
        assert_eq!(peak, 1, "one CPU: one thread at a time");
    }
}

/// A thread keeps reading a page that another unmaps on another CPU: once
/// the unmap has returned, the reader's next read faults; it never reads
/// the page after it saw the flag raised after the unmap (spec
/// `multi-core`, TLB coherence; on x86_64 a shootdown). Several rounds, the
/// race being the point; after the first (which caches the executable),
/// every frame comes back.
fn unmapped_page_is_gone_on_every_cpu() {
    run_expected("shootdown");
    let before = free_or_in_heap();
    for _ in 0..4 {
        run_expected("shootdown");
    }
    assert_eq!(free_or_in_heap(), before, "every frame returned");
}

/// `parsum` computing with 4 threads on every CPU, stopped after each slice
/// of the boot CPU, then suspended and resumed from its snapshot's bytes,
/// or once forked into two children, until it ends: the resumed cell and
/// both children print the uninterrupted sum (spec `multi-core`, cells
/// stopped before host operations). Every cell ends; the frames of cells
/// that ran on several CPUs are checked by `unmapped_page_is_gone_on_every_cpu`
/// and `cells_in_parallel_stay_isolated`.
fn a_cell_on_several_cpus_survives_suspend_and_fork() {
    const SUM: &str = "sum c75f7d3df19f452d\n";
    let mut host = Host::new();
    let mut id = spawn(&mut host, "parsum", &[b"4", b"96"]);
    let mut children = Vec::new();
    let mut output = Vec::new();
    let mut round = 0;
    loop {
        // Asked before the run starts, then after each slice.
        let mut started = false;
        host.run(|_| core::mem::replace(&mut started, true));
        if host.finished(id).is_some() {
            break;
        }
        round += 1;
        if round == 1 {
            children = host.fork(id, 2).expect("fork");
            continue;
        }
        let snapshot = host.suspend(id).expect("live");
        output.extend_from_slice(&host.finished(id).expect("suspended").output);
        let snapshot = Snapshot::from_bytes(
            snapshot.export(&DirectMap, crate::arch::mem::crc32c),
            crate::arch::user::MACHINE,
            crate::arch::mem::crc32c,
        )
        .expect("valid");
        id = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume");
    }
    assert!(round >= 2, "stopped only {round} times");
    host.run(|_| false);
    let (ending, last) = ended(&host, id);
    assert_eq!(ending, Ending::Exit(0));
    output.extend_from_slice(last.as_bytes());
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), SUM);
    for id in children {
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), SUM.into()),
            "child {id}"
        );
    }
    assert_eq!(host.live(), 0);
}

/// Starts `progress` and watches its two words ([`Host::watch`]): rounds
/// made and longest gap.
fn watched_progress(host: &mut Host) -> (u32, crate::cell::Watched) {
    // The boot CPU looks often even while the other CPUs run every cell.
    host.look_every(Some(1_000_000));
    let id = spawn(host, "progress", &[]);
    host.run(|h| h.output(id).is_some_and(|o| o.ends_with(b"\n")));
    let line = alloc::string::String::from_utf8_lossy(host.output(id).expect("live")).into_owned();
    let hex = line.trim().strip_prefix("at 0x").expect("its address");
    let addr = u64::from_str_radix(hex, 16).expect("hexadecimal");
    (id, host.watch(id, addr, 2).expect("its own page"))
}

/// `progress` next to a spinning cell on every CPU, while the host forks
/// the spinner, kills the child, suspends the spinner and resumes it, over
/// and over from the run's stop condition, never ending the run:
/// `progress` keeps counting through every batch of these operations (spec
/// `multi-core`, a host operation stops only the CPUs running its cell).
/// With one or two CPUs nothing else can run while the boot CPU works for
/// the host; the operations still work.
fn host_operations_leave_other_cells_running() {
    use core::sync::atomic::Ordering;
    let mut host = Host::new();
    let (watcher, watched) = watched_progress(&mut host);
    let mut spinner = spawn(&mut host, "spin", &[]);
    let mut progress = Vec::new();
    let mut started = false;
    host.run(|h| {
        // Asked once before the other CPUs start.
        if !core::mem::replace(&mut started, true) {
            return false;
        }
        let before = watched.word(0).load(Ordering::Relaxed);
        let t0 = crate::cell::now_ns();
        let mut rounds = 0;
        while rounds < 4 || crate::cell::now_ns() - t0 < 50_000_000 {
            let children = h.fork(spinner, 1).expect("fork the spinner");
            assert!(h.kill(children[0]));
            let snapshot = h.suspend(spinner).expect("suspend the spinner");
            spinner = h
                .resume(snapshot, Resume::Lazy, false, false)
                .expect("resume the spinner");
            rounds += 1;
        }
        progress.push(watched.word(0).load(Ordering::Relaxed) - before);
        progress.len() == 5
    });
    if crate::smp::count() >= 3 {
        assert!(
            progress.iter().all(|&rounds| rounds > 0),
            "progress stood still during host operations: {progress:?}"
        );
    }
    assert!(host.kill(spinner));
    assert!(host.kill(watcher));
    assert_eq!(host.live(), 0);
}

/// Host operations mixed from the run's stop condition while cells run on
/// every CPU, each at its own pace: `counter` suspended and resumed from
/// its snapshot's bytes every fourth slice of the boot CPU, `parsum`
/// computing with 4 threads (on several CPUs at once) suspended and
/// resumed lazily then forked, then suspended and resumed eagerly from
/// its snapshot's bytes, and a spinner killed and replaced, until
/// `counter`, `parsum` and the fork end: every output is the uninterrupted
/// one, and no cell is lost (spec `multi-core`, cells stopped before host
/// operations). A resumed cell joins the back of the queue: with one CPU
/// and at most four cells queued, moving a cell there at most every fourth
/// slice lets it reach the front in between, so every cell gets to run.
fn mixed_host_operations_during_a_run_are_invisible() {
    const SUM: &str = "sum c75f7d3df19f452d\n";
    let mut host = Host::new();
    host.look_every(Some(1_000_000));
    let mut counter = spawn(&mut host, "counter", &[]);
    let mut parsum = spawn(&mut host, "parsum", &[b"4", b"96"]);
    let mut spinner = spawn(&mut host, "spin", &[]);
    let (mut counted, mut summed) = (Vec::new(), Vec::new());
    let mut child = None;
    let mut calls = 0u32;
    let through_bytes = |snapshot: Snapshot| {
        Snapshot::from_bytes(
            snapshot.export(&DirectMap, crate::arch::mem::crc32c),
            crate::arch::user::MACHINE,
            crate::arch::mem::crc32c,
        )
        .expect("valid")
    };
    host.run(|h| {
        calls += 1;
        // A cell may end on another CPU just before the host acts on it:
        // its record is there then.
        if calls % 4 == 1 && h.finished(counter).is_none() {
            match h.suspend(counter) {
                Some(snapshot) => {
                    counted.extend_from_slice(&h.finished(counter).expect("suspended").output);
                    counter = h
                        .resume(through_bytes(snapshot), Resume::Lazy, true, false)
                        .expect("resume counter");
                }
                None => assert!(h.finished(counter).is_some(), "counter not suspended"),
            }
        }
        // Forked once the other CPUs run, then every fourth time.
        if (calls == 2 || calls % 4 == 3) && h.finished(parsum).is_none() {
            match (child, h.suspend(parsum)) {
                (_, None) => assert!(h.finished(parsum).is_some(), "parsum not suspended"),
                (None, Some(snapshot)) => {
                    // Its fork comes from the resumed cell.
                    summed.extend_from_slice(&h.finished(parsum).expect("suspended").output);
                    parsum = h
                        .resume(snapshot, Resume::Lazy, true, false)
                        .expect("resume parsum");
                    child = h.fork(parsum, 1).map(|ids| ids[0]);
                }
                (Some(_), Some(snapshot)) => {
                    summed.extend_from_slice(&h.finished(parsum).expect("suspended").output);
                    parsum = h
                        .resume(through_bytes(snapshot), Resume::Eager, true, false)
                        .expect("resume parsum");
                }
            }
        }
        if calls.is_multiple_of(5) {
            assert!(h.kill(spinner));
            spinner = spawn(h, "spin", &[]);
        }
        let ended = |id: Option<u32>| id.is_some_and(|id| h.finished(id).is_some());
        ended(Some(counter)) && ended(Some(parsum)) && (child.is_none() || ended(child))
    });
    for (id, output, expected) in [
        (counter, &mut counted, expected_output("counter")),
        (parsum, &mut summed, SUM),
    ] {
        let (ending, last) = ended(&host, id);
        assert_eq!(ending, Ending::Exit(0));
        output.extend_from_slice(last.as_bytes());
        assert_eq!(alloc::string::String::from_utf8_lossy(output), expected);
    }
    let child = child.expect("parsum was forked");
    assert_eq!(ended(&host, child), (Ending::Exit(0), SUM.into()));
    assert!(calls > 3, "only {calls} rounds of operations");
    assert!(host.kill(spinner));
    assert_eq!(host.live(), 0);
}
