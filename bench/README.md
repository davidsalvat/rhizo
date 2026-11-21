# Benchmarks

`cargo xtask bench` builds the release kernel and boots it repeatedly on every
virtual machine monitor available on the host (QEMU `microvm`, QEMU `q35`,
Firecracker, Cloud Hypervisor), next to a reference Linux guest booted the same
way. Firecracker and Cloud Hypervisor need KVM.

## Known issues

- The arm64 reference kernel (Firecracker's CI build) has no PL011 driver,
  which is the UART of QEMU `virt` and Cloud Hypervisor, so on AArch64 the
  reference Linux guest is measured on Firecracker only. A reference kernel
  with both drivers would fix this.
- No GitHub-hosted ARM runner accelerates a guest (see "AArch64 in CI"
  below), so AArch64 CI runs are emulated: CI gates AArch64 on instruction
  counts only, and AArch64 timings need the `dedicated` environment.
- The reference Linux guest is not measured on QEMU `microvm`: it resets right
  after KASLR setup, before `/init`, both with and without KVM. It is measured
  on QEMU `q35`, Firecracker and Cloud Hypervisor.

## Metrics

| Metric | Kind | Meaning |
|---|---|---|
| `boot.time_to_ready_ms/<guest>/<vmm>` | timing | Host wall clock from launching the VMM to the `rhizo-ready` line |
| `boot.kernel_cycles/rhizo/<vmm>` | timing | TSC cycles from the first kernel instruction to ready |
| `image.size_bytes` | deterministic | Size of the release kernel image |
| `memory.kernel_pt_frames` | deterministic | Frames used by the kernel's page tables |
| `cell.spawn_us/rhizo/<vmm>` | timing | Time to create the `hello` cell (tables, image, stack; the image copied from its executable kept loaded, see below), measured inside the kernel over 101 runs per boot (`bench-cell` build) |
| `cell.run_us/rhizo/<vmm>` | timing | Median time from spawn to the cell's exit and teardown (every frame returned) |
| `cell.pt_frames` | deterministic | Page-table frames of the `hello` cell's own half |
| `cell.switch_ns/rhizo/<vmm>` | timing | Time per switch between two cells yielding to each other (system call in, scheduler, system call out), with address-space tags (ASID/PCID) |
| `cell.suspend_us/rhizo/<vmm>` | timing | Suspending the `ballast` cell (8 MiB written) into a snapshot, all its memory freed |
| `cell.resume_us/rhizo/<vmm>` | timing | Lazy resume of that snapshot until the cell can run (pages come back on first touch) |
| `cell.resume_eager_us/rhizo/<vmm>` | timing | Eager resume: every page restored before the cell runs (the baseline lazy resume is measured against) |
| `cell.resume_touch_us/rhizo/<vmm>` | timing | Lazy resume plus the cell reading every page and exiting |
| `cell.snapshot_bytes` | deterministic | Size of that snapshot |
| `cell.fork_us/rhizo/<vmm>` | timing | Forking a cell with 50 MiB written (frozen as a template): shares its page tables, copies nothing |
| `cell.fork_touch_us/rhizo/<vmm>` | timing | That fork plus the child writing every page (copy-on-write of all 50 MiB) until it exits |
| `cell.fork_pt_frames` | deterministic | Frames a fork allocates (the child's root only) |
| `cell.spawn_from_template_us/rhizo/<vmm>` | timing | Starting `hello` from a frozen template (a fork) and running it to its exit, against `cell.run_us` from the executable |
| `workload.cold_us/rhizo/<vmm>` | timing | A real static glibc program (`libc_warm`: prime sieve warm-up, then one answer) started from its executable and run to its exit |
| `workload.warm_us/rhizo/<vmm>` | timing | The same answer from a cell forked off a template frozen after the warm-up |
| `workload.python_cold_us/rhizo/<vmm>` | timing | MicroPython (static, pinned v1.24.1) answering one request (`cells/python/rhizo_demo.py`, imported from the cell's files) started from its executable: interpreter start, imports, warm-up, answer. Measured only where MicroPython is built (`RHIZO_MICROPYTHON_SRC`, set in CI) |
| `workload.python_warm_us/rhizo/<vmm>` | timing | The same answer from a cell forked off MicroPython frozen after its warm-up |
| `workload.cpython_cold_us/rhizo/<vmm>` | timing | CPython (static, pinned 3.13.18) answering one agent tool call (`cells/python/agent_demo.py`: JSON, regular expressions, dates, a CRC) started from its executable, with the standard library it imports as compiled modules in the cell's files: interpreter start, imports, warm-up, answer, exit. Measured only where CPython is built (`RHIZO_CPYTHON_SRC`, set in CI) |
| `workload.cpython_warm_us/rhizo/<vmm>` | timing | The same answer from a cell forked off CPython frozen after its warm-up |
| `workload.agent_cpython_turn_us/rhizo/<vmm>` | timing | One turn of the CPython agent (`cells/python/agent_loop.py`) as it times itself: the conversation sent over a kept-alive HTTP connection to the host's fake model (`llm=` on the command line, answering at once), the JSON answer parsed, the tool run (a computation, a file written and read back), the result appended. Median of turns 2 to 8 of 3 runs (the first turn connects and runs cold code). Needs CPython and a network card |
| `workload.agent_cpython_model_us/rhizo/<vmm>` | timing | The part of those turns spent on the model: from sending the request to having read the whole answer (network card, TCP, the host's server), as the agent times it |
| `workload.agent_suspend_resume_us/rhizo/<vmm>` | timing | In the same runs, the agent suspended in its pause after a turn and resumed eagerly (every page back, nothing left to page faults), its connection to the model parked in the kernel meanwhile |
| `workload.agent_fork_us/rhizo/<vmm>` | timing | In the same runs, the agent frozen at its decision point as a template and three cells started from it, each told its branch in `/branch`, until every branch has printed its first line |
| `workload.dynamic_run_us/rhizo/<vmm>` | timing | The dynamically linked `hello` (`cells/c/dynamic/hello.c`, glibc) started from the cell's files and run to its exit: the kernel maps it and `ld.so`, `ld.so` maps `libc.so.6` with `mmap` of files, relocates and runs `main`; file pages come from frames kept by content after a first run (change `add-dynamic-linking`). Robust figure over 25 runs; compare with `cell.run_us` (a static `hello`) |
| `workload.dynamic_template_us/rhizo/<vmm>` | timing | The same program forked from a template frozen once it printed `ready` (`hello wait`), its libraries mapped and relocated: what a dynamically linked program costs once a template absorbs the dynamic linking |
| `workload.numpy_cold_us/rhizo/<vmm>` | timing | numpy from its manylinux wheel in a dynamically linked CPython (pinned 3.13.18, change `add-pip-packages`): the interpreter started from the cell's files, `import numpy` (its extension modules and OpenBLAS opened with `dlopen`), array operations, a linear solve, a ufunc and a 64x64 matrix product (`cells/python/numpy_demo.py`), exit. Robust figure over 10 runs after a first that fills the page cache. Measured only where CPython is built (`RHIZO_CPYTHON_SRC`, set in CI) |
| `workload.numpy_warm_us/rhizo/<vmm>` | timing | The same computation from a cell forked off a template frozen once numpy was imported (`numpy_demo.py wait`): import once, fork many |
| `cell.switch_untagged_ns/rhizo/<vmm>` | timing | The same without tags (TLB flushed on every switch): the baseline the tags are measured against |
| `cell.file_write_read_us/rhizo/<vmm>` | timing | The `filer` cell creating a file, writing 1 MiB in 64 KiB writes and reading it back, spawn to exit |
| `cell.fork_with_files_us/rhizo/<vmm>` | timing | Forking a cell (frozen as a template) that holds a 16 MiB file: the file blocks are shared, not copied |
| `cell.pipe_rtt_ns/rhizo/<vmm>` | timing | The `piper` cell writing one byte into a pipe and reading it back (two system calls), per round trip over 2000 |
| `cell.loopback_rtt_us/rhizo/<vmm>` | timing | Two `loopback` cells exchanging a 64-byte message over TCP on 127.0.0.1, per round trip over 200 (each side blocks in `read`, wakes, and the other runs) |
| `cell.tcp_resume_us/rhizo/<vmm>` | timing | A `loopback twice` client suspended right after its first round trip with a `loopback` server, its connection parked in the kernel: lazy resume until it has made its second 64-byte round trip on the same connection and exited (against `cell.resume_us` and `cell.loopback_rtt_us`) |
| `net.tcp_rtt_us/rhizo/<vmm>` | timing | A `loopback` cell exchanging a 64-byte message with a TCP echo server on the host through the virtio-net card (virtio-mmio on `microvm` and Firecracker, virtio-pci on `q35` and Cloud Hypervisor; QEMU user network, Firecracker and Cloud Hypervisor on a tap device each), per round trip over 100. Skipped, with the reason, without password-less `sudo` for a tap device |
| `cell.idle_bytes` | deterministic | Memory each of 1000 idle cells costs (forks of a template of `idle`, each run into its sleep): what "thousands of mostly idle cells per node" costs |
| `cell.suspended_bytes` | deterministic | Snapshot size of one idle cell |
| `cell.thread_create_us/rhizo/<vmm>` | timing | A thread of the `threads` cell creating a thread (raw `clone`, as `pthread_create` does) that exits at once, and joining it (futex wait on its clear-tid word); measured by the cell with `clock_gettime`, robust figure over 105 runs |
| `cell.parallel_us/rhizo/<vmm>` | timing | With 4 vCPUs: sixteen CPU-bound cells (`spin 5`, four per CPU) run on every CPU, wall time per cell (`bench-cell` booted with 4 vCPUs) |
| `cell.scaling_pct/rhizo/<vmm>` | timing | That batch's time on every CPU, times the number of CPUs, over its time stepping on one CPU, in percent: 100 is perfect scaling (lower is better) |
| `cell.ipi_wake_us/rhizo/<vmm>` | timing | Handing work to an idle CPU: post it, wake the CPU with an inter-processor interrupt, see it done (robust figure over 105) |
| `cell.thread_scaling_pct/rhizo/<vmm>` | timing | With 4 vCPUs: `parsum` summing 24 million rounds with 4 threads of one cell, times 4, over the same sum with 1 thread, in percent (each the time the cell reports, median of 3): 100 is perfect scaling (lower is better) |
| `cell.shootdown_us/rhizo/<vmm>` | timing | With 4 vCPUs: one `munmap` of a written page by a thread of the `shootdown` cell while its three other threads run on the other CPUs, median of 200 (x86_64: interrupts the other CPUs and waits until they left the cell's code; AArch64: broadcast invalidation, no interrupt) |
| `cell.syscall_scaling_pct/rhizo/<vmm>` | rate | With 4 vCPUs: four `piper` cells per CPU (each writes and reads a byte through its pipe 20,000 times) run on every CPU, times the number of CPUs, over their time on one CPU after the other, in percent: 100 is perfect scaling, 400 means the calls took turns. Calls that use no socket take no global lock (change `split-sockets-lock`) |
| `cell.futex_wake_ns/rhizo/<vmm>` | timing | One futex hand-off between two threads of a cell (wake the other, wait; the other runs): half a ping-pong round trip, best of five rounds of 200 |
| `cell.syscall_ns/rhizo/<vmm>` | timing | One system call round trip: `yielder getpid 20000` run to its exit minus `yielder getpid 0`, per call (robust figure) |
| `cell.cow_fault_ns/rhizo/<vmm>` | timing | The kernel's side of one copy-on-write fault of a 4 KiB page (frame, copy, entry, TLB flush; the trap itself is about `cell.syscall_ns`), accounted alone (`profile` build) while a fork of a cell holding 1 MiB writes every page |
| `<metric>_insns/rhizo/<vmm>-icount` | instructions | Every timed figure above, and every breakdown, counted in retired instructions under QEMU with `-icount` (see below) |
| `<metric>_insns/rhizo/<vmm>`, `<metric>_cycles/rhizo/<vmm>` | counts | Every single-CPU timed figure counted by the guest's PMU in retired instructions and core cycles, where the guest has one (see below) |
| `breakdown.<workload>.<part>_us/rhizo/<vmm>` | timing, not gated | Where a workload's time goes (see below) |
| `lock.<name>_waits/rhizo/<vmm>` | count, not gated | During the four-CPU breakdown: acquisitions that found the lock held by another CPU, for `frames` (frame allocator), `heap`, `refs` (frame reference counts), `sharing` (memory shared by forks), `net` (sockets), `sched` (run queues) |
| `control.roundtrip_us/rhizo/<vmm>` | timing | The control plane (spec `control-plane`): a `list` request from the host to a control kernel and its reply back, as the host times it (frame written to the VMM's standard input, the serial line both ways, the kernel's poll of its UART, the reply read from the VMM's standard output). Median of 200 in a row on one boot, 1 CPU, 128 MiB |
| `control.run_us/rhizo/<vmm>` | timing | On the same boot, as the host times them: `run` of the `idle` cell (the reply comes once it is created) |
| `control.suspend_us/rhizo/<vmm>` | timing | `suspend` of that cell (snapshot kept in the kernel) |
| `control.resume_us/rhizo/<vmm>` | timing | `resume` of it (lazy: pages come back on first touch) |
| `control.fork_us/rhizo/<vmm>` | timing | `fork` of it into one cell |
| `control.kill_us/rhizo/<vmm>` | timing | `kill` of it. Each of these five over 20 rounds; each reply also carries the kernel's own time for the operation, which `rhizo` prints next to the round trip |
| `chat.turn_us/rhizo/<vmm>` | timing | The Rhizo side of one interactive turn (change `add-interactive-chat`): on the same boot, when CPython is built, the chat agent (`cells/python/agent_chat.py`) against the host's fake model with no delay; each turn timed by the host from writing the `input` request (one line) to the VMM's standard input until it reads the agent's end-of-turn line back, less the time the agent waited for the model (its requests sent until their answers were read). What is left: the request on the serial line, waking the agent's read, CPython's work (JSON, one `run_python` snippet, printing), the answer's output and the serial line back. Median of 20 turns; `rhizo chat` prints the same split after every turn |

The kernel keeps the executables it spawned loaded (spec `cells`): from the
second spawn of an executable on, a cell starts on a copy of its loaded
segments (own tables and writable pages, shared read-only pages) instead of
loading the file again. The figures started from the executable
(`cell.spawn_us`, `cell.run_us`, the `workload.*cold_us` metrics) measure
that, as the Linux processes they are compared with start from an
executable in the page cache.

## Reference workloads

Two workloads run the kernel the way its two markets will
(`docs/vision.md`, change `add-reference-workloads`). `cargo xtask bench`
runs both on every VMM after the cell figures; `--workload laptop` or
`--workload server` runs one alone, `--soak` the long versions (the weekly
`soak` job), `--cells N` changes the server's target and `--llm-delay-ms N`
the fake model's delay (0 by default, so a turn measures Rhizo).

**Laptop** (B2C, 2 vCPUs, 512 MiB): an `agent` template (1 MiB of history
warmed up), 50 agents opened from it, 45 of them suspended; agents ask a
fake model server on the host (`xtask`, plain HTTP through the network
card). There is no virtio-fs or vsock yet, so the shared folder is a file
(`/shared/notes.txt`, 64 KiB) seeded into the template's files at boot and
shared copy-on-write by the agents, not with the host; a user's message is
the host waking the agent (`Host::wake`). Skipped, with the reason, where
the VMM has no network card.

| Metric | Kind | Meaning |
|---|---|---|
| `laptop.open_us/rhizo/<vmm>` | timing | Opening an agent: fork from the template until it opened its session (wrote its history file, printed `open`) |
| `laptop.suspend_us/rhizo/<vmm>` | timing | Suspending an open agent into a snapshot |
| `laptop.resume_us/rhizo/<vmm>` | timing | Lazy resume of a suspended agent until it can run |
| `workload.agent_turn_us/rhizo/<vmm>` | timing | One turn of a live agent: woken by its user, HTTP request to the model, answer, tool action (history appended, shared notes read and checked) |
| `laptop.resume_turn_us/rhizo/<vmm>` | timing | The same turn right after a lazy resume (its pages come back as it touches them) |
| `laptop.memory_bytes/rhizo/<vmm>` | size | Memory the template and the 50 agents take (45 snapshots in the kernel heap included) |
| `laptop.idle_wakeups_per_s/rhizo/<vmm>` | rate | While every agent waits (2 s, 60 s in the soak): returns of the guest's CPUs from waiting for an interrupt, per second |
| `laptop.idle_host_cpu_pct/rhizo/<vmm>` | rate | Over the same window, read on the host: the VMM process's CPU time (all threads, `/proc/PID/task/*/schedstat`) over wall time, in percent of one CPU |

**Server** (B2B, 1 vCPU, 1 GiB): up to 10,000 cells forked from four
`worker` templates, in groups of ten: 4 computing (2,000 xorshift rounds),
2 writing and reading back 4 KiB of a file, and 2 pairs over loopback TCP
(the servers share one listener). Each runs one operation every 200 ms for
3 s (600 s in the soak): 50,000 operations per second offered at 10,000
cells. Every 10 ms the host suspends and lazily resumes the next worker
(its connections stay open) and forks a computing one, killing the
previous child (a fork's connections are reset). With several
CPUs every host operation would stop them all (a run ends to make one), so
the workload uses one vCPU. Then the out-of-memory push: idle cells forked
until the kernel refuses one; spawn, fork and eager resume must be
refused, `hog` must get `ENOMEM`, nothing may panic and every byte must
come back, or the benchmark fails.

| Metric | Kind | Meaning |
|---|---|---|
| `density.cells/rhizo/<vmm>` | deterministic, higher is better | Workers made: the target, or fewer when memory keeps less than 32 MiB of headroom (`limit=` in the `rhizo-server` line says which) |
| `density.ops_per_s/rhizo/<vmm>` | rate, higher is better | Operations of the workers that exited with 0, over the load's wall time |
| `density.cell_bytes/rhizo/<vmm>` | size | Most memory in use during the load, per cell (pages, tables, files, sockets, kernel record) |
| `density.sched_p50_us`, `_p99_us`, `_p999_us` | timing | Scheduling latency: a cell joined a run queue (preempted, yielded, new, or its deadline passed) until it runs |
| `density.wake_p50_us`, `density.wake_p99_us`, `_p999_us` | timing | Wake-up latency: its event (deadline, or a socket change by another cell's system call) until it runs |
| `density.syscall_p50_ns`, `_p99_ns`, `_p999_ns` | timing | Time in the system-call handler, locks included |
| `density.wasted_wakeups_pct/rhizo/<vmm>` | rate | Of the cells a socket change woke from waiting on descriptors, the share whose next system call waited again: a wake-up that found nothing (each socket wakes only the cells waiting on it, change `add-socket-wait-queues`) |
| `density.oom_cells/rhizo/<vmm>` | count, higher is better | Idle cells memory held before the kernel refused one |

In the `ci` environment (from two CI runs that agreed within the
threshold) every workload metric is gated except these, recorded but
listed under `ungated.ci`: `laptop.idle_host_cpu_pct` (the host's
own load on a shared runner), the p99.9 latencies (`*_p999_*`: a
handful of samples per run), and `density.sched_p99_us` and
`density.wake_p99_us`: the server load runs close to saturation (every
socket change wakes every cell waiting on a socket), so its tails moved
up to 5x between runners of the same code. They are gated once that is
fixed. The agent turns are gated since `fix-ephemeral-ports`: they took
1 s in most runs on Firecracker and Cloud Hypervisor, because each boot
started its ephemeral ports at 32768, so its first connections reused
4-tuples the host still kept in `TIME_WAIT` from the previous run, and
smoltcp retried the refused SYN after 1 s.

The `rhizo-server-breakdown` line printed with the results says where the
load's time went (idle, system calls, scheduling decisions, host
operations; runs, socket wake-ups, faults): simple counters in
`kernel/src/prof.rs`, until the permanent per-workload breakdown replaces
them.

Figures the kernel repeats a hundred times or more (`cell.spawn_us`,
`cell.run_us`, `cell.spawn_from_template_us`, the file and network
metrics) are the
minimum over five consecutive rounds of each round's median, so one noisy
moment of the host does not decide them.

Each timing is the median (and p90) of `--runs` boots after one discarded
warm-up boot. Lower is better for every metric not marked "higher is better".

## Where the time goes (spec `profiling`)

### Breakdowns

A second kernel, built with the `profile` feature, has hooks at every
transition of the kernel and charges each CPU's time to one part at a time
(`crates/rhizo-prof`). The parts are exclusive: a part nested in another (a
copy inside a copy-on-write fault) is not counted in it, and the parts add
up to `total`. `cargo xtask bench` boots it three times per VMM and
publishes the medians as `breakdown.<workload>.<part>_us`:

| Workload | What |
|---|---|
| `cell_loop` | The cell benchmark loop: `hello` spawned and run to its exit 101 times |
| `fork_touch` | A fork of a cell holding 50 MiB written (a template), the child writing every page until it exits |
| `cpython_cold` | CPython answering one request from cold (where CPython is built) |
| `parallel` | With 4 vCPUs: sixteen CPU-bound cells (`spin 5`) on every CPU |

| Part | Time spent |
|---|---|
| `user` | In the cell's own code |
| `entry_exit` | Entering and leaving cell code: entries times the cost of one, measured on a cell that only calls `getpid` (an estimate; the rest of the cell's time is `user`) |
| `syscall` | Handling system calls other than socket calls |
| `fault` | Page faults: lazily resumed pages coming back, stacks growing |
| `cow` | Resolving copy-on-write faults |
| `frames` | The frame allocator and frame reference counts |
| `copy` | Copying and zeroing memory through the direct map |
| `net` | Socket calls and polling the network card |
| `sched` | Taking a cell to run and giving it back |
| `idle` | Waiting for work |
| `lock` | Waiting for a lock another CPU holds |
| `spawn` | Cell operations: spawn (loading included), fork, suspend, resume |
| `teardown` | Ending a cell: its address space and frames freed |
| `other` | The rest of the kernel |
| `total` | All of the above, summed over the CPUs |

The kernel also prints the scopes entered per part
(`rhizo-breakdown-counts`) and the eight system calls that took most time
(`rhizo-breakdown-calls`, `name=count/ns`). Without `profile` the hooks
compile to nothing, so no other figure is measured with them.

### Sampling: `cargo xtask profile`

```bash
cargo xtask --arch x86_64 profile [--vmm <name>] [--accel <a>] [--out <dir>] [--per-cell]
```

runs a kernel built with `sampling` (which includes `profile`): the same
workloads (five rounds each, CPython where built), accounted as above and
sampled. The PMU counts cycles of cell code only and interrupts every
100,000 (x86_64 counter 2 on vector 34; AArch64 event counter 1 on the PMU's
PPI); without a PMU the timer samples every 100 µs instead. Each sample is
the cell and its program counter; kernel addresses are not sampled (the
kernel runs with interrupts masked), its time is the accounted parts. It
writes to `target/profile` (or `--out`):

- `<workload>.folded`: `<workload>;<image>[;cell <id>];<function> <ns>`,
  the samples sharing the workload's `user` time by count, and
  `<workload>;kernel;<part> <ns>`;
- `<workload>.calls.folded`: `<workload>;<system call> <ns>` (each call's
  own time, nested parts included).

Functions come from `nm` on the image (CPython: the interpreter before it
was stripped); the Rust test cells are stripped, so their addresses fold by
4 KiB page. Render with `flamegraph.pl`, `inferno-flamegraph` or speedscope.

### Instructions and cycles

Where the guest has performance counters (`rhizo-pmu` in the output), the
`bench-cell` kernel runs its single-CPU benchmarks again with the PMU as
their clock (all but the fork benchmarks, which need memory the first pass
keeps), once counting retired instructions and once core cycles:
`<metric>_insns` and `<metric>_cycles`. Where it has none, these are listed
as skipped (`pmu/<vmm>`).

`cargo xtask bench` also runs both kernels once under QEMU's TCG with
`-icount shift=0,sleep=off` (`--accel icount`): the guest's clock advances
one nanosecond per instruction, so every figure the kernel times is a count
of retired instructions, the same on every run and every host (without the
network card, the only input from outside). These are the
`<metric>_insns/rhizo/<vmm>-icount` metrics, gated at +2 %
(`thresholds_pct.instructions`). `--no-icount` skips them; `--icount-only`
measures nothing else (the AArch64 CI job, below). Where the guest has a
PMU that counts instructions (QEMU's AArch64 CPU), its passes give the
figures they cover, exactly, and the clock the rest (the fork benchmarks;
AArch64's generic timer ticks every 16 instructions under `-icount`).

| VMM | Performance counters in the guest |
|---|---|
| QEMU TCG, x86_64 | None (QEMU emulates no x86 PMU): `-icount` gives the instruction counts |
| QEMU TCG, AArch64 | PMUv3: cycles; instructions only under `-icount` |
| QEMU (`microvm`, `q35`) and Cloud Hypervisor with KVM | What KVM gives the guest: on GitHub-hosted x86_64 runners it depends on the machine: some give Intel architectural performance monitoring with 8 counters, others none. So the PMU counts are recorded but not in the `ci` baseline (a gated figure missing on a VMM that ran fails the gate) |
| Firecracker with KVM | None: Firecracker hides the PMU from its guests (CPUID leaf 0xA cleared on x86_64) |

## Comparisons with existing mechanisms

Measured in the same `cargo xtask bench` run, on the same machine and with the
same clock as the cells (spec `comparisons`):

| Metric | What |
|---|---|
| `snapshot.restore_ms/linux/firecracker` | A Linux guest snapshotted through Firecracker's API (`Full`), restored by a new Firecracker process until the VM runs again: how Firecracker-based sandboxes resume and clone. First restore (cold page cache) dropped |
| `snapshot.restore_api_ms/linux/firecracker` | The `PUT /snapshot/load` request alone |
| `process.fork_us/linux-process/host` | `fork` of a host process holding 50 MiB written, child exits, parent waits (`bench/host/linux_process.c`) |
| `process.fork_call_us/linux-process/<host or firecracker>` | The parent's `fork()` call alone, holding 50 MiB written (pairs with `cell.fork_us`) |
| `process.fork_touch_us/linux-process/<host or firecracker>` | `fork`, the child writes every page of the 50 MiB and exits, the parent waits (pairs with `cell.fork_touch_us`) |
| `process.<metric>/linux-process/firecracker` | The same program as the `/init` of a Linux guest in Firecracker: the VM the cell figures come from, so both sides pay the same virtualization (spec `comparisons`) |
| `snapshot.restore_ms/criu/host` | `criu restore -d` of a dumped process holding 8 MiB written (`bench/host/criu_target.c`), as long as CRIU's log says it took (from its start to "Restore finished"). First restore dropped |
| `workload.cpython_cold_us/linux-process/firecracker` | The same static CPython, files and script in a Linux guest in Firecracker: `fork` and `execve` of `python3 /app/agent_demo.py`, output read from a pipe, until it exits (`bench/host/linux_cpython.c` as `/init`). Pairs with `workload.cpython_cold_us/rhizo/firecracker` |
| `workload.cpython_warm_us/linux-process/firecracker` | In the same guest, a CPython process that imported and warmed up the script forks one child per request; the child answers and exits normally, the parent waits (`bench/host/cpython_forkserver.py`, the pre-fork pattern). Pairs with `workload.cpython_warm_us/rhizo/firecracker` |
| `workload.numpy_cold_us/linux-process/firecracker` | The packages cell's files (the dynamically linked CPython, numpy and the libraries) in a Linux guest in Firecracker: `fork` and `execve` of `python3 /app/numpy_demo.py`, output read from a pipe, until it exits (`bench/host/linux_numpy.c` as `/init`). Pairs with `workload.numpy_cold_us/rhizo/firecracker` |
| `workload.numpy_warm_us/linux-process/firecracker` | In the same guest, a CPython process that imported numpy forks one child per request; the child runs the computation and exits normally (`bench/host/numpy_forkserver.py`). Pairs with `workload.numpy_warm_us/rhizo/firecracker` |
| `workload.agent_cpython_turn_us/linux-process/firecracker` | The same agent as a Linux process in that guest, which brings its network card up on the Firecracker tap device and reaches the same fake model: its turns, timed the same way. Pairs with `workload.agent_cpython_turn_us/rhizo/firecracker` |
| `workload.agent_cpython_model_us/linux-process/firecracker` | The part of its turns spent on the model, timed the same way. Pairs with `workload.agent_cpython_model_us/rhizo/firecracker` |
| `workload.agent_fork_us/linux-process/firecracker` | Its `os.fork` into the same three branches, from the first fork until every branch has written its first line to its pipe. Pairs with `workload.agent_fork_us/rhizo/firecracker`. The Linux process has no suspend-and-resume figure: CRIU is not in the guest (design D5 of `add-agent-workload`) |
| `process.spawn_us/linux-process/host` | `posix_spawn` of the same static `hello` the cell benchmark runs, until it exits |
| `workload.dynamic_run_us/linux-process/firecracker` | `posix_spawn` of the same dynamically linked `hello` the cell benchmark runs, with the same `ld.so` and `libc.so.6` at their paths in the guest's initramfs, until it exits (`bench/host/linux_process.c` as `/init`). Pairs with `workload.dynamic_run_us/rhizo/firecracker` |
| `workload.dynamic_run_us/linux-process/host` | The same on the benchmark host, with the host's own `ld.so` and `libc` |
| `sandbox.run_ms/gvisor/runsc` | `runsc run` of a gVisor sandbox (systrap platform, no network) running the same static `hello`, until it exits. First run dropped |
| `snapshot.restore_ms/gvisor/runsc` | `runsc restore --detach` of a checkpointed sandbox running the idle cell program, until `runsc state` reports it running. First restore dropped |

Firecracker needs KVM for the guest's architecture, gVisor needs `runsc`
(the benchmark job installs a pinned release, checksum verified) and CRIU
needs `criu` and root (the benchmark job installs it); elsewhere each is
listed as skipped with the reason. Process baselines need a Linux host of the target
architecture. The README table puts each cell operation next to these figures;
a column not measured in that run shows "—", never a quoted number.

Rhizo's cell figures in that table are the ones measured with Rhizo itself
running in Firecracker, so both sides pay the same virtualization. GitHub's
runners are shared virtual machines (nested virtualization: every VM exit is
expensive), so publishable numbers come from the `dedicated` environment below.

## Environments and thresholds

`baseline.json` stores, per environment, the median of each Rhizo metric.
`cargo xtask bench --env <env> --check` fails when a metric is worse than its
baseline by more than its threshold:

| Environment | Where | Timing threshold | Deterministic threshold |
|---|---|---|---|
| `ci` | GitHub-hosted runners (shared, noisy) | +100 % on time to ready; TSC cycles recorded but not gated | +1 %; instructions under `-icount` +2 % |
| `ci-arm` | The AArch64 CI job (`ubuntu-24.04-arm`, no KVM): `--icount-only`, instructions on `qemu-virt-icount` | — (no timings) | instructions under `-icount` +2 % |
| `dedicated` | A self-hosted runner labelled `rhizo-bench`: bare metal or a pinned VM with KVM, fixed CPU frequency, nothing else running | +5 % | +1 %; instructions under `-icount` +2 % |

Breakdowns (`breakdown.`) and lock waits (`lock.`) are recorded but not gated
(entries of `ungated` that end in `.` cover every metric they start).

Why so wide: two CI runs of identical code differed by up to +41 % in time to
ready and +53 % in kernel cycles. Hosted runners land on different physical
machines whose TSC frequencies differ, so cycle counts are not comparable
between runs there. The `ci` gate therefore only catches gross regressions
(for example, a boot that doubles); the `dedicated` run is the precise one. It runs from the `bench-dedicated` workflow, started manually
(Actions → bench-dedicated → Run workflow) once such a runner is registered.

## AArch64 in CI: instruction counts, no timings

No GitHub-hosted ARM runner a public repository can use accelerates an
AArch64 guest. A probe on every free ARM label (2026-11-21, runs
44855293209 and 44855941291 of this repository; design D1 of
`add-aarch64-bench`):

| Runner | Machine | Result |
|---|---|---|
| `ubuntu-22.04-arm`, `ubuntu-24.04-arm`, `ubuntu-26.04-arm` | Azure VM, Neoverse-N2 (Cobalt 100) | no `/dev/kvm`; the kernel logs `kvm [1]: HYP mode not available` (it starts at EL1) |
| `macos-15`, `macos-26` | Apple M1 (Virtual) | `kern.hv_support: 0`; QEMU `-accel hvf` fails with `HV_UNSUPPORTED` |
| `windows-11-arm` | Azure VM, Cobalt 100 | no virtualization extensions in the guest; and Rhizo's VMMs need KVM |

GitHub's documentation says the same for macOS ("Nested-virtualization is
not supported due to the limitation of Apple's Virtualization Framework",
[GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
also for the M2 larger runner); larger ARM runners are billed for public
repositories ([Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions))
and run on the same Azure Cobalt 100 sizes, which do not support nested
virtualization ([Dpsv6 series](https://learn.microsoft.com/azure/virtual-machines/sizes/general-purpose/dpsv6-series)).
The AArch64 job's step "Report whether this runner offers KVM" keeps
watching for a change.

Timings under emulation would measure QEMU's translator, so the AArch64
job (`ci-arm64` in `.github/workflows/ci.yml`) runs, after its checks:

```bash
cargo xtask --arch aarch64 bench --icount-only --env ci-arm --check --output bench-results-aarch64.json
```

the same instruction counts as x86_64's, on `qemu-virt-icount`, gated at
+2 % against the `ci-arm` baseline (breakdowns and lock waits ungated). It
publishes them as `bench-aarch64-medians` and `bench-aarch64-results-N`
annotations, apart from the x86_64 job's `bench-results-N`.

## Measuring on real ARM64 hardware (`dedicated`)

Any ARM64 Linux machine with KVM works; the benchmarks only need QEMU,
Firecracker and Cloud Hypervisor to run guests with hardware acceleration.

| Machine | Notes |
|---|---|
| Raspberry Pi 5 (8 GB) | Cortex-A76; use a 64-bit OS with KVM enabled (Raspberry Pi OS 64-bit or Ubuntu Server). Cheapest option; thermal throttling: add a fan and fix the CPU governor |
| Orange Pi 5 / RK3588 boards | Cortex-A76 + A55: pin the benchmark to the big cores (`taskset -c 4-7`) |
| Cloud bare metal | AWS Graviton `*.metal` (e.g. `c7g.metal`), Ampere Altra servers (Oracle A1 bare metal, Hetzner RX): closest to production |

Setup, once:

```bash
sudo apt-get install -y qemu-system-arm
```

```bash
ls -l /dev/kvm
```

```bash
sudo cpupower frequency-set -g performance
```

Then register the machine as a self-hosted GitHub Actions runner with the
labels `self-hosted`, `rhizo-bench` and `arm64`, and start the
`bench-dedicated` workflow with `arch: aarch64`. The first run downloads the
AArch64 Firecracker and Cloud Hypervisor releases, which are not pinned yet:
check them, add their SHA-256 to `xtask/src/tools.rs` (the run prints them),
and commit the result before trusting the numbers. To try once before
pinning: `RHIZO_ALLOW_UNPINNED=1 cargo xtask bench`.

## Updating a baseline

A baseline changes only on purpose, in a pull request that explains why:

```bash
cargo xtask bench --env dedicated --update-baseline
```

CI publishes its measured results as an annotation named `bench-results` on
the `bench` job; to adopt them as the `ci` baseline, save that JSON to a file
and run:

```bash
cargo xtask bench --from results.json --env ci --update-baseline
```

The AArch64 job's `bench-aarch64-results-N` annotations, joined in order,
are the `ci-arm` results:

```bash
cargo xtask bench --from results-aarch64.json --env ci-arm --update-baseline
```

## README chart

```bash
cargo xtask bench --readme
```

regenerates the chart in the README from a fresh measurement (or from stored
results with `--from`). Prefer numbers from the `dedicated` environment.
A second `--from` with the AArch64 job's results adds a table of
instructions per operation on both architectures (a metric in both files
is an error); `python3 docs/img/gen.py bench-results.json
bench-results-aarch64.json` draws the same as `docs/img/arches.svg`.
