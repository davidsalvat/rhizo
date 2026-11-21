//! Cross-cell leaks (change `close-cross-cell-leaks`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(sysinfo_shows_a_cell_only_itself),
    ktest_case!(ephemeral_ports_do_not_show_other_cells_binds),
    ktest_case!(memory_pressure_keeps_the_pages_other_cells_map),
    ktest_case!(cells_run_alike_with_and_without_kpti),
    ktest_case!(threads_racing_on_several_cpus_cannot_hurt_the_kernel, 42),
    ktest_case!(flags_a_cell_sets_stay_in_the_cell),
    ktest_case!(one_page_mapped_without_end_cannot_stop_the_kernel, 41),
    ktest_case!(hardware_entropy_works_where_present),
    ktest_case!(no_two_cells_draw_the_same_random_bytes),
];

/// Every other test runs with KPTI on (design D1 of `add-kpti`); the
/// kernel without it, as most CPUs run it, behaves the same: system calls,
/// faults, forks (copy-on-write flushes) and switches between cells.
fn cells_run_alike_with_and_without_kpti() {
    for kpti in [false, true] {
        arch::user::set_kpti(kpti);
        run_expected("abi");
        run_expected("fault_kernel");
        let mut host = Host::new();
        let parent = spawn(&mut host, "rewrite", &[]);
        let other = spawn(&mut host, "chatter", &[b"c"]);
        while host.output(parent) != Some(b"ready\n") {
            host.step();
        }
        let mut ids = host.fork(parent, 2).expect("fork");
        ids.push(parent);
        host.run(|_| false);
        for id in ids {
            let (ending, out) = ended(&host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            assert!(out.ends_with("own ok\n"), "{out}");
        }
        assert_eq!(ended(&host, other), (Ending::Exit(0), "c\nc\nc\n".into()));
    }
}

/// Hostile (finding S24): `sysinfo` shows a cell its own memory limit and
/// use, and its own threads; a neighbour taking 32 MiB changes nothing in
/// it (it showed the kernel's free memory before).
fn sysinfo_shows_a_cell_only_itself() {
    use rhizo_cell::quota::Quota;
    let mut host = Host::new();
    let observer = spawn(&mut host, "observer", &[b"memory"]);
    let quota = Quota {
        memory: 64 << 20,
        ..Quota::NONE
    };
    assert!(host.set_quota(observer, quota));
    let neighbour = neighbour_meanwhile(&mut host, observer, &[b"neighbour", b"32", b"0"]);
    assert_eq!(
        ended(&host, neighbour),
        (Ending::Exit(0), "holding\n".into())
    );
    assert_eq!(
        ended(&host, observer),
        (
            Ending::Exit(0),
            "ready\nmemory total=64 change=0 procs=1\n".into()
        )
    );
}

/// Runs `observer` to its `ready`, then a neighbour `observer` with `argv`
/// to its `holding`, then the observer to its end while the neighbour
/// holds what it took, then the neighbour to its end; returns the
/// neighbour. The host takes turns: cells share no loopback network to
/// meet on (design D5 of `add-cell-hierarchy`).
fn neighbour_meanwhile(host: &mut Host, observer: u32, argv: &[&[u8]]) -> u32 {
    while host.output(observer) != Some(b"ready\n") {
        host.step();
    }
    let neighbour = spawn(host, "observer", argv);
    while host.output(neighbour) != Some(b"holding\n") {
        host.step();
    }
    assert!(host.wake(observer));
    host.run(|h| h.finished(observer).is_some());
    assert!(host.wake(neighbour));
    host.run(|_| false);
    neighbour
}

/// Hostile (finding S25): the ephemeral ports a cell gets do not show how
/// many other cells took. A neighbour binds 100 between the observer's
/// two binds; the observer's ports are one apart (102 with the one counter
/// all cells shared before).
fn ephemeral_ports_do_not_show_other_cells_binds() {
    let mut host = Host::new();
    let observer = spawn(&mut host, "observer", &[b"ports"]);
    let neighbour = neighbour_meanwhile(&mut host, observer, &[b"neighbour", b"0", b"100"]);
    assert_eq!(
        ended(&host, neighbour),
        (Ending::Exit(0), "holding\n".into())
    );
    assert_eq!(
        ended(&host, observer),
        (Ending::Exit(0), "ready\nports +1\n".into())
    );
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Hostile (finding S26): a cell that takes memory until refused makes the
/// kernel drop the kept file pages no cell maps, but not the page another
/// cell maps (it emptied the whole page cache before: a flush any cell
/// could make). Every frame comes back.
fn memory_pressure_keeps_the_pages_other_cells_map() {
    let before = memory_balance();
    assert_eq!(crate::cell::kept_pages(), 0);
    let mut host = Host::new();
    let user = spawn(&mut host, "observer", &[b"page"]);
    while host.output(user) != Some(b"mapped\n") {
        host.step();
    }
    assert_eq!(crate::cell::kept_pages(), 1);
    let hog = spawn(&mut host, "hog", &[]);
    while host.output(hog) != Some(b"exhausted\n") {
        host.step();
    }
    assert_eq!(
        crate::cell::kept_pages(),
        1,
        "the page another cell maps stays kept"
    );
    assert!(host.wake(user));
    host.run(|_| false);
    assert_eq!(ended(&host, hog), (Ending::Exit(0), "exhausted\n".into()));
    assert_eq!(
        ended(&host, user),
        (Ending::Exit(0), "mapped\npage ok\n".into())
    );
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// Hostile threads on several CPUs (spec `isolation`, `multi-core`):
/// `racer` cells unmap memory other threads are copying through the
/// kernel, unmap a word a thread waits on, end while their threads are in
/// system calls, and flip a page read-only under a writer. Each ends as its
/// race allows, a cell next to them finishes, the kernel keeps running and
/// every frame comes back.
fn threads_racing_on_several_cpus_cannot_hurt_the_kernel() {
    let before = memory_balance();
    let mut host = Host::new();
    let races = ["io", "futex", "exit", "protect"].map(|race| {
        let id = spawn(&mut host, "racer", &[race.as_bytes()]);
        (race, id)
    });
    let other = spawn(&mut host, "hello", &[]);
    host.run(|_| false);
    for (race, id) in races {
        let (ending, out) = ended(&host, id);
        match race {
            "exit" => assert_eq!(ending, Ending::Exit(3), "{race}: {out}"),
            // The writer may hit the page while it is read-only.
            "protect" if ending == Ending::Fault => {}
            _ => assert_eq!(
                (ending, out.as_str()),
                (Ending::Exit(0), alloc::format!("{race} ok\n").as_str()),
                "{race}"
            ),
        }
    }
    assert_eq!(ended(&host, other).0, Ending::Exit(0));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// A cell that spins with the direction and alignment-check flags set
/// (x86_64) is interrupted many times: the kernel's own code runs with
/// them clear, so its copies go forwards and SMAP stays on, and the cells
/// around it, which the kernel copies data for, are unharmed.
fn flags_a_cell_sets_stay_in_the_cell() {
    let mut host = Host::new();
    let flags = spawn(&mut host, "flags", &[]);
    let others: Vec<u32> = (0..3).map(|_| spawn(&mut host, "abi", &[])).collect();
    let before = memory::free_bytes();
    host.run(|_| false);
    assert_eq!(ended(&host, flags), (Ending::Exit(0), "flags ok\n".into()));
    for id in others {
        let (ending, out) = ended(&host, id);
        assert_eq!(
            (ending, out.as_str()),
            (Ending::Exit(0), expected_output("abi"))
        );
    }
    assert!(memory::free_bytes() >= before);
}

/// Finding S14 (spec `isolation`): a cell maps one page of a file of its
/// own 70000 times, more references to the page cache's frame than 16 bits
/// count. The kernel keeps running (it panicked), every mapping reads the
/// file, a cell next to it finishes, and every frame comes back.
fn one_page_mapped_without_end_cannot_stop_the_kernel() {
    let before = memory_balance();
    let mut host = Host::new();
    let sharer = spawn(&mut host, "sharer", &[]);
    let other = spawn(&mut host, "hello", &[]);
    host.run(|_| false);
    assert_eq!(
        ended(&host, sharer),
        (Ending::Exit(0), "shares ok\n".into())
    );
    assert_eq!(ended(&host, other).0, Ending::Exit(0));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// The CPU's random-number instruction, where it has one, gives bytes that
/// differ from one read to the next (QEMU's `max` CPU has it on both
/// architectures; without it the boot says `getrandom` is predictable).
fn hardware_entropy_works_where_present() {
    use rhizo_hal::Entropy;
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    if !arch::random::HardwareEntropy.fill(&mut a) {
        arch::console::BootConsole.write_bytes(b"(no hardware random source on this CPU) ");
        return;
    }
    assert!(arch::random::HardwareEntropy.fill(&mut b));
    assert_ne!(a, b, "two reads gave the same bytes");
    assert_ne!(a, [0; 32]);
}

/// `getrandom` is a private stream per cell (spec `isolation`): independent
/// cells, forks, copies of a template and two resumes of one snapshot all
/// draw different bytes after the point they share.
fn no_two_cells_draw_the_same_random_bytes() {
    fn ready(host: &mut Host) -> u32 {
        let id = spawn(host, "dice", &[]);
        while host.output(id) != Some(b"ready\n") {
            host.step();
        }
        id
    }
    let mut host = Host::new();
    // Each is used as soon as it is ready (stepping may end the others).
    let mut ids = alloc::vec![ready(&mut host)];
    ids.extend(host.fork(ids[0], 2).expect("fork"));
    ids.push(ready(&mut host));
    let template = ready(&mut host);
    assert!(host.freeze(template));
    for _ in 0..2 {
        ids.push(
            host.spawn_from_template(template, true, false)
                .expect("copy"),
        );
    }
    let suspended = ready(&mut host);
    let bytes = host
        .suspend(suspended)
        .expect("suspend")
        .export(&DirectMap, crate::arch::mem::crc32c);
    for mode in [Resume::Eager, Resume::Lazy] {
        let snapshot =
            Snapshot::from_bytes(bytes.clone(), arch::user::MACHINE, crate::arch::mem::crc32c)
                .expect("valid");
        ids.push(host.resume(snapshot, mode, true, false).expect("resume"));
    }
    host.run(|_| false);
    let mut draws: Vec<alloc::string::String> = ids
        .iter()
        .map(|&id| {
            let (ending, out) = ended(&host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            out.trim_start_matches("ready\n").trim_end().into()
        })
        .collect();
    assert!(draws.iter().all(|d| d.len() == 32), "{draws:?}");
    draws.sort();
    draws.dedup();
    assert_eq!(draws.len(), ids.len(), "two cells drew the same bytes");
}
