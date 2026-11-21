//! Quotas (spec `isolation`, change `add-cell-quotas`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(memory_quota_refuses_the_cell_not_the_others),
    ktest_case!(cpu_quota_ends_the_cell),
    ktest_case!(file_and_socket_quotas_travel_with_the_cell),
    ktest_case!(a_cell_holds_a_bounded_share_of_sockets),
];

/// A cell with a memory quota is refused (`ENOMEM`) once its pages reach
/// it, and keeps running; a cell without one, next to it, still gets the
/// memory it asks for; every frame comes back.
fn memory_quota_refuses_the_cell_not_the_others() {
    // Anonymous memory, then pages of a file past its end (finding S20).
    for args in [&[][..], &[&b"file"[..]]] {
        quota_refuses_the_hog(args);
    }
}

fn quota_refuses_the_hog(args: &[&[u8]]) {
    use rhizo_cell::quota::Quota;
    const QUOTA: u64 = 8 << 20;
    let before = memory_balance();
    let mut host = Host::new();
    let hog = spawn(&mut host, "hog", args);
    let quota = Quota {
        memory: QUOTA,
        ..Quota::NONE
    };
    assert!(host.set_quota(hog, quota));
    while host.output(hog) != Some(b"exhausted\n") {
        host.step();
    }
    let charged = host.charged_frames(hog).expect("still live") * FRAME_SIZE;
    // `mmap` takes 1 MiB at a time, then `brk` 64 KiB: it stops within one
    // brk step of the quota.
    assert!(
        charged <= QUOTA && charged > QUOTA - (128 << 10),
        "{charged}"
    );
    let other = spawn(&mut host, "ballast", &[b"all", b"16"]);
    host.run(|_| false);
    assert_eq!(ended(&host, hog), (Ending::Exit(0), "exhausted\n".into()));
    assert_eq!(
        ended(&host, other),
        (Ending::Exit(0), "filled\nchecked\n".into())
    );
    drop(host);
    assert_eq!(memory_balance(), before, "every frame came back");
}

/// A cell past its CPU time quota is ended (`OutOfCpu`) at the end of the
/// slice that used it up, whether one thread or several used it; the cell
/// next to it runs to its end.
fn cpu_quota_ends_the_cell() {
    use rhizo_cell::quota::Quota;
    let mut host = Host::new();
    let quota = Quota {
        cpu_ns: 30_000_000,
        ..Quota::NONE
    };
    // One spinning thread, and four (on several CPUs at once with SMP: the
    // time of every CPU counts).
    let spin = spawn(&mut host, "spin", &[]);
    let threads = spawn(&mut host, "threads", &[b"spin", b"4"]);
    for id in [spin, threads] {
        assert!(host.set_quota(id, quota));
    }
    let chatter = spawn(&mut host, "chatter", &[b"c"]);
    host.run(|_| false);
    for id in [spin, threads] {
        let (ending, out) = ended(&host, id);
        assert_eq!(ending, Ending::OutOfCpu);
        assert!(out.ends_with("killed: CPU time quota used up\n"), "{out}");
    }
    assert_eq!(ended(&host, chatter), (Ending::Exit(0), "c\nc\nc\n".into()));
}

/// Quotas on descriptors and sockets: `getrlimit` reports them, sockets
/// stop at `ENFILE` and descriptors at `EMFILE`; a fork, a copy of a
/// template and a resumed snapshot of the cell keep them.
fn file_and_socket_quotas_travel_with_the_cell() {
    use rhizo_cell::quota::Quota;
    fn ready(host: &mut Host) -> u32 {
        let id = spawn(host, "limits", &[]);
        let quota = Quota {
            files: 16,
            sockets: 3,
            ..Quota::NONE
        };
        assert!(host.set_quota(id, quota));
        while host.output(id) != Some(b"ready\n") {
            host.step();
        }
        id
    }
    let mut host = Host::new();
    let mut ids = alloc::vec![ready(&mut host)];
    ids.extend(host.fork(ids[0], 1).expect("fork"));
    let template = ready(&mut host);
    assert!(host.freeze(template));
    ids.push(
        host.spawn_from_template(template, true, false)
            .expect("copy"),
    );
    let suspended = ready(&mut host);
    let bytes = host
        .suspend(suspended)
        .expect("suspend")
        .export(&DirectMap, crate::arch::mem::crc32c);
    let snapshot =
        Snapshot::from_bytes(bytes, arch::user::MACHINE, crate::arch::mem::crc32c).expect("valid");
    ids.push(
        host.resume(snapshot, Resume::Lazy, true, false)
            .expect("resume"),
    );
    host.run(|_| false);
    for id in ids {
        let (ending, out) = ended(&host, id);
        assert_eq!(ending, Ending::Exit(0), "{out}");
        assert!(
            out.ends_with("nofile 16 sockets 3 errno 23 files 13 errno 24\n"),
            "{out}"
        );
    }
}

/// Finding S21: without a quota from the host, a cell and its fork stop at
/// the default socket quota (256, `ENFILE`), far below the kernel's 4096;
/// descriptors are not limited beyond the kernel's own.
fn a_cell_holds_a_bounded_share_of_sockets() {
    let mut host = Host::new();
    let id = spawn(&mut host, "limits", &[]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    let mut ids = alloc::vec![id];
    ids.extend(host.fork(id, 1).expect("fork"));
    host.run(|_| false);
    for id in ids {
        let (ending, out) = ended(&host, id);
        assert_eq!(ending, Ending::Exit(0), "{out}");
        assert!(
            out.ends_with("nofile 1024 sockets 256 errno 23 files 1021 errno 24\n"),
            "{out}"
        );
    }
}
