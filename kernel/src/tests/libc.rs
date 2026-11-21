//! Real C programs (static glibc) and their FP/SIMD state.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(real_libc_programs_behave_as_on_linux),
    ktest_case!(cells_learn_the_cpu_as_on_linux),
    ktest_case!(fp_state_is_private_to_each_cell),
    ktest_case!(fp_state_survives_suspend_and_resume),
    ktest_case!(fp_registers_hold_nothing_of_another_cell),
    ktest_case!(fp_state_is_private_to_every_thread_and_cell, 92),
    ktest_case!(fp_state_survives_suspends_resumes_and_forks),
    ktest_case!(warm_template_of_a_real_program),
];

/// Static glibc programs (cells/c): startup, stdio, the allocator, FP.
fn real_libc_programs_behave_as_on_linux() {
    for name in ["libc_hello", "libc_alloc", "libc_float"] {
        run_expected(name);
    }
}

/// What a program learns of the CPU is what Linux tells it (spec
/// `linux-abi`): on AArch64, `AT_HWCAP` with FP, Advanced SIMD and
/// `HWCAP_CPUID`, and ID registers read at EL0 answered by the kernel
/// (`MIDR_EL1` the CPU's, `MPIDR_EL1` only bit 31).
fn cells_learn_the_cpu_as_on_linux() {
    run_expected("libc_hwcap");
}

/// Two FP programs holding values in FP/SIMD registers across yields, run
/// together: each must find its own values (lazy FP switching).
fn fp_state_is_private_to_each_cell() {
    let mut host = Host::new();
    let a = spawn(&mut host, "libc_float", &[b"1"]);
    let b = spawn(&mut host, "libc_float", &[b"1000"]);
    host.run(|_| false);
    assert_eq!(
        ended(&host, a),
        (Ending::Exit(0), "fp ok: 20100.0\n".into())
    );
    assert_eq!(
        ended(&host, b),
        (Ending::Exit(0), "fp ok: 219900.0\n".into())
    );
}

/// The FP state is part of the snapshot: suspended and resumed every few
/// steps (eagerly and lazily), next to another FP cell, the program still
/// gets its exact result.
fn fp_state_survives_suspend_and_resume() {
    for mode in [Resume::Eager, Resume::Lazy] {
        let mut host = Host::new();
        let other = spawn(&mut host, "libc_float", &[b"7"]);
        let mut id = spawn(&mut host, "libc_float", &[b"3"]);
        let mut output = Vec::new();
        for step in 1.. {
            host.step();
            if let Some(done) = host.finished(id) {
                assert_eq!(done.ending, Ending::Exit(0));
                output.extend_from_slice(&done.output);
                break;
            }
            if step % 9 == 0 {
                let snapshot = host.suspend(id).expect("live");
                output.extend_from_slice(&host.finished(id).expect("suspended").output);
                id = host.resume(snapshot, mode, true, true).expect("resume");
            }
        }
        assert_eq!(
            alloc::string::String::from_utf8_lossy(&output),
            "fp ok: 20500.0\n"
        );
        host.run(|_| false);
        assert_eq!(
            ended(&host, other),
            (Ending::Exit(0), "fp ok: 21300.0\n".into())
        );
    }
}

/// Whether `fp` holds the state a program starts with, and so nothing of
/// any thread or cell: on x86_64 the x87 control word and `MXCSR` as
/// Linux sets them, the x87 and XMM registers zero (the rest of the
/// `FXSAVE` image describes the CPU, not a thread); on AArch64 all zero.
fn fp_is_initial(fp: &rhizo_cell::FpState) -> bool {
    let w = &fp.0;
    if cfg!(target_arch = "x86_64") {
        w[0] & 0xffff == 0x37f && w[3] & 0xffff_ffff == 0x1f80 && w[4..52].iter().all(|&x| x == 0)
    } else {
        w.iter().all(|&x| x == 0)
    }
}

/// While a cell runs that has not used FP/SIMD, the CPU's FP/SIMD
/// registers hold the initial state, not the values of the cell that ran
/// before (change `lazy-fp-switch`): FP/SIMD trapping alone would leave
/// them there for code that runs speculatively before the trap (LazyFP,
/// CVE-2018-3665). Read from the CPU after each scheduling step in which
/// such a cell ran, right after `fp_regs` filled every register.
fn fp_registers_hold_nothing_of_another_cell() {
    let mut host = Host::new();
    let secret = spawn(&mut host, "fp_regs", &[b"5", b"100"]);
    let mut checked = 0;
    for tag in [&b"a"[..], b"b", b"c"] {
        let quiet = spawn(&mut host, "chatter", &[tag]);
        let mut seen = 0;
        while host.finished(quiet).is_none() {
            host.step();
            let printed = host.output(quiet).map_or(0, <[u8]>::len);
            if printed > seen {
                seen = printed;
                let mut fp = rhizo_cell::FpState::default();
                arch::user::fp_save(&mut fp);
                assert!(
                    fp_is_initial(&fp),
                    "another cell's FP/SIMD values while `chatter` ran: {:x?}",
                    &fp.0[16..24]
                );
                checked += 1;
            }
        }
        assert_eq!(ended(&host, quiet).0, Ending::Exit(0));
    }
    assert!(
        checked >= 6,
        "`chatter` ran after `fp_regs` {checked} times"
    );
    host.run(|_| false);
    assert_eq!(
        ended(&host, secret),
        (Ending::Exit(0), "fp regs ok\n".into())
    );
}

/// Cells and threads holding distinct values in every FP/SIMD register
/// (and the FP control and status registers) across yields and sleeps, many
/// times over, next to hostile cells that only read their registers: no
/// value leaks to another thread or cell, none is lost, on every CPU
/// (`ktest-smp`: threads of one cell at once on several CPUs).
fn fp_state_is_private_to_every_thread_and_cell() {
    let mut host = Host::new();
    let cells = [
        spawn(&mut host, "fp_regs", &[b"1", b"300", b"3", b"3"]),
        spawn(&mut host, "fp_regs", &[b"2", b"300", b"3", b"2"]),
        spawn(&mut host, "fp_regs", &[b"3", b"300"]),
        spawn(&mut host, "fp_regs", &[b"4", b"300", b"2"]),
    ];
    let peekers = [
        spawn(&mut host, "fp_peek", &[b"300"]),
        spawn(&mut host, "fp_peek", &[b"300"]),
    ];
    let quiet = spawn(&mut host, "yielder", &[]);
    host.run(|_| false);
    for id in cells {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "fp regs ok\n".into()));
    }
    for id in peekers {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "fp clean\n".into()));
    }
    assert_eq!(ended(&host, quiet).0, Ending::Exit(0));
}

/// A cell whose threads hold values in their FP/SIMD registers, suspended
/// and resumed (eagerly and lazily, sometimes twice before it runs again,
/// so a thread that has not touched FP/SIMD since its restore is captured
/// again) and forked mid-run, next to another such cell and a hostile
/// reader: it, its forks and the others all find their own values.
fn fp_state_survives_suspends_resumes_and_forks() {
    let mut host = Host::new();
    let other = spawn(&mut host, "fp_regs", &[b"8", b"200"]);
    let peeker = spawn(&mut host, "fp_peek", &[b"200"]);
    let mut id = spawn(&mut host, "fp_regs", &[b"6", b"120", b"2", b"4"]);
    let mut forks = Vec::new();
    let mut output = Vec::new();
    for step in 1.. {
        host.step();
        if let Some(done) = host.finished(id) {
            assert_eq!(done.ending, Ending::Exit(0));
            output.extend_from_slice(&done.output);
            break;
        }
        if step % 7 == 0 {
            // Every third time twice in a row, before it runs again.
            let times = if step % 21 == 0 { 2 } else { 1 };
            for _ in 0..times {
                let mode = if step % 14 == 0 {
                    Resume::Eager
                } else {
                    Resume::Lazy
                };
                let snapshot = host.suspend(id).expect("live");
                output.extend_from_slice(&host.finished(id).expect("suspended").output);
                id = host.resume(snapshot, mode, true, true).expect("resume");
            }
        }
        if step % 25 == 0 && forks.len() < 4 {
            forks.extend(host.fork(id, 1).expect("fork"));
        }
    }
    assert_eq!(
        alloc::string::String::from_utf8_lossy(&output),
        "fp regs ok\n"
    );
    host.run(|_| false);
    assert!(!forks.is_empty(), "no fork was made mid-run");
    for child in forks {
        assert_eq!(
            ended(&host, child),
            (Ending::Exit(0), "fp regs ok\n".into())
        );
    }
    assert_eq!(
        ended(&host, other),
        (Ending::Exit(0), "fp regs ok\n".into())
    );
    assert_eq!(ended(&host, peeker), (Ending::Exit(0), "fp clean\n".into()));
}

/// A real program frozen after its warm-up: every cell forked from it
/// answers without repeating the warm-up.
fn warm_template_of_a_real_program() {
    run_expected("libc_warm");
    let mut host = Host::new();
    let warm = spawn(&mut host, "libc_warm", &[]);
    host.run_to_pause(warm, b"warm", Pause::Slice)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(warm));
    let ids: Vec<u32> = (0..3)
        .map(|_| host.spawn_from_template(warm, true, true).expect("spawn"))
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(
            ended(&host, id),
            (
                Ending::Exit(0),
                "primes below 2000000: 148933, below 1000: 168\n".into()
            )
        );
    }
}
