# Design

## D1. The official build, unmodified, in kernels of its own

Node.js is the official Linux build of a pinned LTS release (v24.22.3,
"Krypton"), downloaded by `xtask` from nodejs.org into `target/tools`
(`tools::node`) and checked against the SHA-256 nodejs.org publishes, like
the VMMs `xtask` downloads; only `bin/node` and the release's `LICENSE`
are kept. It is a dynamically linked executable (glibc 2.28 or later,
`libstdc++`, `libgcc_s`, `libm`, `libdl`, `libpthread`), so it is packed
like the distribution's programs (`dynamic.rs`): `/usr/bin/node`,
`/app/demo.js`, the interpreter at the path `PT_INTERP` names and the
libraries under `/lib/<triple>`, from the host's system directories, or
from the cross toolchain's when the cells' architecture is not the host's
(so an x86_64 host also runs the AArch64 Node.js cell under QEMU). Only
with `RHIZO_NODE` set: the download is 30 MiB and the cells 150 MiB, which
not every `cargo xtask test` should pay; CI sets it and caches
`target/tools`.

The files are over 100 MiB: embedded in a kernel and decoded into a cell
file system they take twice that, and the file pages a cell maps a third
time, before Node.js allocates anything. The kernels that run cells today
have 128 MiB (tests) and 256 MiB (benchmarks, whose 50 MiB fork benchmark
needs most of it). So `node-files` is embedded only in kernels built with
the new `node` feature: with `ktest` it runs the Node.js tests alone
(`tests::ALL` is the Node.js suite, as a self-check build has its own),
with `bench-cell` it prints the Node.js figures alone; both VMs get
1 GiB. Every other kernel embeds an empty `node-files` and is unchanged.

Alternatives: embedding Node.js in every test kernel and giving every test
VM 1 GiB (the tests that fill memory would fill eight times more, and
every kernel would carry 120 MiB it does not use); the distribution's
`nodejs` package (Node 18 on Ubuntu 24.04, split into `libnode.so`,
`libuv`, ICU, OpenSSL: as large, older, and different on each runner); a
stripped binary (8% smaller, but no longer the file nodejs.org signs).

## D2. What the kernel lacked

Running Node.js (`strace` on Linux, then the kernel's `unimplemented
syscall` reports) found, beyond what dynamic linking brought:

- `gettimeofday`: libuv's `uv_gettimeofday`; Node.js asserts it succeeds.
- `clock_getres`: one nanosecond for every clock `clock_gettime` reads
  (they count nanoseconds).
- `capget`: Node.js asks it before trusting the environment. A cell holds
  no capabilities: there is nothing outside it to be privileged over.
  Versions 1 to 3 of the header are read; any other is answered with
  version 3 and `EINVAL`, as Linux does.
- `prctl`: `PR_SET_NAME` (V8 and libuv name their threads) is accepted
  and the name not kept (a name is cell state the snapshot would carry for
  nothing; `PR_GET_NAME` reads it empty); other options are `EINVAL`.
- `sched_getparam`, `sched_getscheduler`: abseil's mutexes ask a thread's
  policy under contention and print an error when the call fails. Every
  thread has the normal policy and priority 0.
- `io_uring_setup`: declined with `ENOSYS` on purpose, like `rseq`, and
  not reported as missing: libuv falls back to `epoll`. A ring shared with
  the kernel is state the snapshot format would have to carry.

`rt_sigaction` and `rt_sigprocmask` already succeed without delivering;
`/proc/self/maps` (glibc's `pthread_getattr_np` for the main thread; V8
falls back to `__libc_stack_end`) and `/proc/self/exe` (Node.js falls back
to `argv[0]`) do not exist and are not needed.

## D3. Room for Node.js's files

A cell's file data was bounded at 64 MiB, and so was a file. The `node`
executable alone is 121 MiB, so the bound becomes 256 MiB (spec `files`).
It stays a bound on the kernel memory a cell's files take (1/16 overhead
at most), and blocks shared with other cells still count for each. The
kernel also kept at most 64 MiB of mapped file pages, starting over past
that: one Node.js cell maps 107 MiB of its executable, so every spawn
copied every page again. The page cache keeps up to 256 MiB, as much as
one cell's files; it still gives way to cells under memory pressure, with
the kept executables, before any cell is refused memory.

## D4. Tests

`node_answers_as_on_linux` spawns Node.js with `--jitless` on
`/app/demo.js`: it warms up, prints `warm`, answers a tool call (JSON,
regular expressions, a fold through composed closures) and exits; its
output is `NODE_STDOUT` (`cells/expected.rs`), which the differential
check holds the same files to on Linux. Run again with the URL of the
host's fake model, it asks the model with `node:http` through the network
card and prints the tool call it got (`NODE_MODEL_LINE`). After the first
run every frame comes back; heap allocations too, but for the few the
card's own traffic holds at any moment (DHCP, ARP), so the heap is
compared within four allocations. After the second, the cell's connection
is closed.

`a_node_cell_suspends_resumes_and_forks` runs the script with `wait` (a
50 ms pause after `warm`): suspended there, resumed lazily from the
snapshot's bytes, forked twice, and frozen as a template forked twice,
every copy prints the answer and exits with 0. Node.js runs seven
threads, so this exercises threads across suspend, fork and templates on
the two CPUs of the VM.

## D5. Metrics

`workload.node_cold_us`: spawning Node.js on the script from its files and
running it to its exit (`ld.so`, Node.js and V8 start-up, warm-up, answer,
exit), median of seven after a first spawn that fills the page cache.
`workload.node_warm_us`: the same answer forked from a template frozen
after `warm`, median of seven; the template's pause ends before the first
fork, so no fork waits for it (its deadline is absolute). Both from the
`node` benchmark kernel (`cargo xtask bench --workload node` measures
them alone), with `--jitless`; without a JIT there is no figure with one.

On Linux (spec `comparisons`), `bench/host/linux_node.c` is `/init` of a
Firecracker guest whose initramfs holds the same files; it forks and execs
`node --jitless /app/demo.js` until it exits: `workload.node_cold_us` for
guest `linux-process`. Node.js cannot fork itself (`child_process.fork`
starts a new process), so Linux has no warm figure: forking a warm
JavaScript runtime is what a cell adds.

## D6. The JIT under W^X: the next change

V8 in this release never changes the permissions of its code: without
memory protection keys it reserves its code range (512 MiB) and makes all
of it readable, writable and executable once
(`mprotect(PROT_READ|PROT_WRITE|PROT_EXEC)`), then writes and runs code in
it freely; the flag that made it flip pages between writable and
executable (`--write-protect-code-memory`) was removed. Cells refuse W+X,
so Node.js runs with `--jitless` here. The options considered:

- **Honour `mprotect` RW → RX transitions with shootdowns**: correct, and
  what the kernel would do anyway, but V8 never asks for them.
- **Memory protection keys** (`--memory-protection-keys`, V8's default
  where the CPU has them): the pages are W+X in the page tables and only
  the thread's `PKRU` register keeps them from being written; that is not
  W^X, and AArch64 has no keys.
- **Two mappings of the same frames** (writable at one address,
  executable at another, through `memfd_create`): V8 does this only on
  Apple platforms; on Linux it would need a patched V8, and `memfd` and
  shared mappings in cells.
- **Write-or-execute pages, toggled on faults** (chosen): `mmap` and
  `mprotect` accept `PROT_WRITE|PROT_EXEC` on anonymous private memory,
  but no page is ever both in the page tables. Such a page carries a
  software bit (x86_64 bits 9–11 and AArch64 bits 55–58 are the
  software's) and is mapped writable and not executable. An instruction
  fetch from it faults; the kernel, holding the cell's lock, makes it
  executable and read-only, flushes it from the TLB of every CPU running
  the cell's threads (the x86_64 shootdowns of `add-parallel-threads`,
  broadcast TLB invalidation on AArch64) and, on AArch64, cleans the data
  cache and invalidates the instruction cache for the page; then the
  thread resumes. A write to it faults and flips it back the same way.
  V8 runs unmodified and the page tables never hold W+X. The bit is cell
  state: the snapshot format records it per page (a new version), forks
  keep it (a write to a copy-on-write page resolves the copy, then makes
  it writable), and restored pages keep it.

  Costs and risks to measure in that change: two faults per page each
  time code is written into a page that runs (V8 allocates new code next
  to running code); a page written by one thread while another runs it
  (concurrent Sparkplug batches) bounces between the two states, each
  flip a fault and a shootdown, and progress relies on the faulting
  thread retrying its instruction right after its flip (if it does not
  hold in practice, V8's `--no-concurrent-sparkplug` avoids the pattern).
  Its tests: a C cell that maps W+X memory, writes a function, calls it,
  rewrites and calls it again from two threads on two CPUs, and checks
  `mincore`-free observable behavior only; Node.js without `--jitless`
  printing the same answer; `workload.node_jit_cold_us` and
  `workload.node_jit_warm_us` next to the `--jitless` figures.

The kernel work (fault kinds from both architectures' exception
syndromes, the software bit in `EntryFormat`, the toggle in
`rhizo_cell::space`, the snapshot version, instruction-cache maintenance)
is larger than everything this change needed, and touches the snapshot
format; it is its own change.

## D8. Cells read the counter

Node.js's abseil reads the virtual counter (`MRS CNTVCT_EL0`) directly
for its cycle clock, on some paths only (mutexes under contention). The
kernel never set `CNTKCTL_EL1`, whose reset value is UNKNOWN: on QEMU it
leaves EL0 access off, so the read trapped and the cell was killed
(`system register trap`, class 0x18), and only now and then, which is why
the first runs passed. Every CPU now sets `CNTKCTL_EL1` to `EL0VCTEN`
alone in `user::init_cpu` (the boot CPU through `init_tags`, the others
at bring-up), as Linux does: cells may read `CNTVCT_EL0` and
`CNTFRQ_EL0`; the physical counter (`EL0PCTEN`) and both timers
(`EL0VTEN`, `EL0PTEN`) stay the kernel's. x86_64 already lets cells use
`rdtsc` (`CR4.TSD` clear). The test cell `ticks` reads the counter on
both architectures, in each architecture's tests and natively.

The counter tells a cell nothing `clock_gettime(CLOCK_MONOTONIC)` does not
already tell it, in finer steps. It is not cell state: a suspended cell's
counter keeps running, a resumed or forked cell reads the machine's, and a
snapshot resumed on another machine sees that machine's count, as
`CLOCK_MONOTONIC` does. Emulating the read in the trap handler would keep
the counter's value under the kernel's control (a per-cell offset across
suspend), but costs a trap per read on a path libraries use because it is
cheap, and nothing needs that offset.

Its test, the first arch test to run a cell, ran after the shared suite's
last test (`samples_show_where_a_cell_spins`) and hung on four CPUs: the
sampler's period is one global, and stopping it on the boot CPU cleared
it first, so the other CPUs' `stop_sampling` returned early and left their
counter counting and its overflow interrupt on. The next cell to run on
such a CPU overflowed it, and the interrupt fired without end (nothing
cleared the overflow), so that CPU never came back from its run. Each CPU
now disables its own counter and interrupt whatever the shared period
says, on both architectures.

## D7. Known limits

Every thread stack Node.js creates (six of 8 MiB, reserved with
`PROT_NONE` and then made writable) is backed by zeroed frames at once
(spec `linux-abi`, reserved mappings): a Node.js cell holds 48 MiB of
stacks it barely touches, and a fork shares them copy-on-write.
Anonymous memory backed on first touch would remove that; it is not
needed to run Node.js. `madvise` stays accepted without effect, so the
memory glibc gives back from its thread stacks (`MADV_DONTNEED`) stays the
cell's until it exits; V8 gives memory back with `mprotect(PROT_NONE)`,
which frees it. `MADV_DONTFORK` is about `fork(2)`, which cells do not
have: a cell's fork copies the whole cell.
