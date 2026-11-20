// What each test cell prints and how it ends. Shared by the in-kernel tests
// (`kernel/src/tests.rs`) and the differential check on a Linux host
// (`xtask`), so both compare against the same expectations.

/// How a test cell ends.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// `exit_group(status)`.
    Exit(i32),
    /// Killed by a fault (on Linux: `SIGSEGV`).
    Fault,
}

/// `(program, standard output, ending)` for every test cell.
#[allow(dead_code)]
pub const PROGRAMS: &[(&str, &str, Ending)] = &[
    ("hello", "hello from a cell\n", Ending::Exit(0)),
    ("exit3", "", Ending::Exit(3)),
    (
        "abi",
        "brk ok\nmmap ok\nefault ok\nenosys ok\nmisc ok\n",
        Ending::Exit(0),
    ),
    ("fault_null", "", Ending::Fault),
    ("fault_kernel", "", Ending::Fault),
    ("chatter", "x\nx\nx\n", Ending::Exit(0)),
    ("sleeper", "slept ok\n", Ending::Exit(0)),
    ("same_addr", "own value ok\n", Ending::Exit(0)),
    ("peek", "", Ending::Fault),
    ("yielder", "", Ending::Exit(0)),
    ("ballast", "filled\nchecked\n", Ending::Exit(0)),
    ("forkme", "ready\nown ok\n", Ending::Exit(0)),
    ("diary", "ready\nown ok\n", Ending::Exit(0)),
    ("filer", "files ok\n", Ending::Exit(0)),
    (
        "libc_hello",
        "hello from libc, 1 argument, first libc_hello\n\
         sorted: min 0 max 1008 weighted sum 336699270\n\
         strtod 3.141593, scaled 3.142e+10, 0.448799\n\
         0314.159|rhizo |beef (20 chars)\n",
        Ending::Exit(0),
    ),
    ("libc_alloc", "allocator checksum 154285459\n", Ending::Exit(0)),
    ("libc_float", "fp ok: 20100.0\n", Ending::Exit(0)),
    (
        "libc_files",
        "cwd ends in /work\n\
         notes: 100 lines, 2200 bytes, digest 716f37cd630517f4, regular 1\n\
         pread: f2 XYZ 0e\n\
         hole: 0 0 !, size 30001\n\
         dup offset: 2\n\
         truncated to 10\n\
         appended to 14\n\
         exclusive: -1 errno 17\n\
         missing: -1 errno 2\n\
         not a dir: -1 errno 20\n\
         access: 0, x -1 errno 13\n\
         .: data.bin sub\n\
         sub: moved.txt\n\
         rmdir non-empty: -1 errno 39\n\
         after unlink: \"line 000\", stat -1 errno 2\n\
         .:\n\
         files ok\n",
        Ending::Exit(0),
    ),
    (
        "libc_warm",
        "warm\nprimes below 2000000: 148933, below 1000: 168\n",
        Ending::Exit(0),
    ),
    ("loopback", "loopback ok\n", Ending::Exit(0)),
    ("piper", "pipes ok\n", Ending::Exit(0)),
    (
        "netstate",
        "ready\npipe kept\nconnection open\nlistener ok\n",
        Ending::Exit(0),
    ),
    (
        "libc_net",
        "pipe poll: 1 ready, read end empty, write end writable\n\
         empty read: -1 errno 11\n\
         pipe: through a pipe\n\
         closed pipe: revents 0x10, read 0\n\
         eventfd: 7\n\
         drained: -1 errno 11\n\
         poll timeout: 0\n\
         listening on 127.0.0.1: a port\n\
         connect: started\n\
         epoll: 1 ready, data 42\n\
         connected: writable 1, SO_ERROR 0\n\
         nothing yet: -1 errno 11\n\
         epoll: 1 ready, data 7, events 0x1\n\
         echo: HELLO, SERVER\n\
         peer is 127.0.0.1: 1, ports match: 1\n\
         after shutdown: 1 ready, events 0x2001, read 0\n\
         nobody listens: -1 errno 111\n\
         select: 1, readable 1\n\
         udp: datagram (cut), from 127.0.0.1: 1\n\
         udp recvmsg: 12 bytes, datag|ram two, flags 0\n\
         unix: socket file 1\n\
         unix: over unix\n\
         unix again: -1 errno 98\n\
         socketpair: pair\n\
         net ok\n",
        Ending::Exit(0),
    ),
    (
        "threads",
        "tids ok\nfutex ok\nmutex ok\nping-pong ok\nregs ok\n",
        Ending::Exit(0),
    ),
    ("libc_deep_stack", "deep 130560\nbuffer ok\n", Ending::Exit(0)),
    (
        "libc_threads",
        "ready\n\
         pool: 500500 2001000 4501500\n\
         queue: 400 items, sum 240200\n\
         mutex: 400 increments\n\
         fp: 210.0 20190.0\n\
         threads: 13, distinct ids yes\n",
        Ending::Exit(0),
    ),
    (
        "counter",
        "step 07 digest 6b4e24cdafe14e6b\nstep 15 digest c93f1c055e5410cf\nstep 23 digest 65632246df41d107\nstep 31 digest a7c1ff878b0627df\nstep 39 digest 9a1ea881edd8f792\n",
        Ending::Exit(0),
    ),
    // Threads on several CPUs at once (spec `threads`, `multi-core`).
    ("parsum", "sum 15df5a63211010ed\n", Ending::Exit(0)),
    ("shootdown", "", Ending::Fault),
    // Hostile cells (spec `isolation`) whose answers Linux gives too.
    (
        "hostile_ptrs",
        "efault ok\nkernel ranges ok\nhuge ranges ok\nthread pointer ok\n",
        Ending::Exit(0),
    ),
    ("poke_kernel", "", Ending::Fault),
    ("scrub", "ready\nzero ok\n", Ending::Exit(0)),
    ("rewrite", "ready\nown ok\n", Ending::Exit(0)),
];

/// Hostile test cells run only in the kernel (spec `isolation`): random
/// system calls, or taking all memory, are not for the host that runs the
/// differential check.
#[allow(dead_code)]
pub const KERNEL_ONLY: &[(&str, &str, Ending)] = &[
    ("fuzz", "fuzz ok\n", Ending::Exit(0)),
    ("hog", "exhausted\n", Ending::Exit(0)),
    ("namer", "names full\n", Ending::Exit(0)),
];

/// Test cells that never end on their own (they are killed by the tests):
/// not run by the differential check.
#[allow(dead_code)]
pub const ENDLESS: &[&str] = &["spin", "mapper", "idle", "storm"];

/// MicroPython (`cells/python`, built only when its source is given): the
/// command it runs, with `rhizo_demo.py` in `lib/` relative to the working
/// directory, and what it prints. The module pauses after `warm`, where a
/// host may freeze it as a template.
#[allow(dead_code)]
pub const PYTHON_COMMAND: &str =
    "import sys; sys.path.insert(0, 'lib'); import rhizo_demo; rhizo_demo.main()";
#[allow(dead_code)]
pub const PYTHON_STDOUT: &str = "warm\n\
     primes below 10000: 1229, last 9973; top words: agent=70, kernel=69, fork=67\n\
     python ok\n";

/// CPython (`cells/python/agent_demo.py`, built only when its source is
/// given): its command line, in a cell holding the files `xtask` packs
/// (`/usr/lib/python3.13`, `/app`) and on Linux in a directory laid out the same
/// way, and what it prints. The script pauses after `warm`, where a host may
/// freeze it as a template.
#[allow(dead_code)]
pub const CPYTHON_ARGV: [&str; 2] = ["/usr/bin/python3", "/app/agent_demo.py"];
#[allow(dead_code)]
pub const CPYTHON_STDOUT: &str = "warm\n\
     {\"contacts\": 24, \"counts\": [82, 71, 71], \"dates_since\": 24, \
     \"digest\": \"fa17d1e499110000\", \"first_contact\": \"Agent@Team0\", \
     \"gap_spread\": 7.62, \"last_date\": \"22 Dec 2026\", \"mean_gap_days\": 8.6087, \
     \"tool\": \"summarize\", \"top_words\": [\"KERNEL\", \"FORK\", \"PAGE\"]}\n\
     cpython ok\n";

/// The CPython agent (`cells/python/agent_loop.py`, change
/// `add-agent-workload`) after [`AGENT_TURNS`] turns with the host's fake
/// model: its transcript, the same as a Linux process (`linux` mode) and as
/// a cell suspended after every turn and forked at its decision point
/// (`cell` mode). `conn_requests` counts the requests on the agent's
/// connection: it keeps counting across suspends, and each branch has a
/// connection of its own.
#[allow(dead_code)]
pub const AGENT_TURNS: usize = 3;
#[allow(dead_code)]
pub const AGENT_TRANSCRIPT: &str = "\
turn 1 call_37dd1708 n=804 seed=565 value=209594 notes=1 sha=8834d66e conn_requests=1
turn 2 call_87b7d3a8 n=624 seed=963 value=305024 notes=2 sha=9fd0492f conn_requests=2
turn 3 call_4801f3f6 n=898 seed=808 value=334680 notes=3 sha=2e6a15fb conn_requests=3
fork
branch 0
branch 0 value=334680
branch 0 turn call_6094ead6 n=942 seed=305 value=121891 notes=5 sha=2374c347 conn_requests=1
branch 1
branch 1 value=320113
branch 1 turn call_49db5b55 n=797 seed=645 value=254942 notes=5 sha=ba5adce1 conn_requests=1
branch 2
branch 2 value=360156
branch 2 turn call_9848ba8f n=635 seed=988 value=305130 notes=5 sha=fb723766 conn_requests=1
";
