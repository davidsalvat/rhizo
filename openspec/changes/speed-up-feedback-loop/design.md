# Design: speed up the feedback loop

## D1: build first, then run steps at once
cargo builds one thing at a time per target directory, so builds stay
sequential: cells, every kernel feature set (each image copied aside so the
next build does not replace it), and the host tests with `--no-run`. Then
a pool runs the steps. Each step reserves host CPUs equal to its VM's
virtual CPUs (2 for `cargo test`), capped at half the host so a 4-CPU VM
runs beside the single-CPU ones. Results are reported as each step ends,
with its time; the first failure in step order is returned once all ended.

Not parallel: Firecracker and Cloud Hypervisor share one tap device each
and the VMM scratch directory, and `--gdb` waits for a debugger.

## D2: splitting the in-kernel tests (`shard=I/N`)
The kernel runs the tests whose position across the suites is `I` modulo
`N`, so each runs in exactly one VM (host test of the runner). Splitting
is opt-in: running a test without the ones before it found
- two tests measuring free memory before the first host brought up the
  network card (fixed: the card comes up at boot);
- `cells_in_parallel_stay_isolated` without a warm-up round (fixed);
- `memory_pressure_fails_cells_not_the_kernel` refusing all twenty `filer`
  cells on 4 CPUs when run after the dynamic-linking tests (open);
- `a_woken_cell_runs_at_once` hanging on 4 CPUs under load, a lost wake-up
  already seen without splitting (open, handed to the change on cell
  wake-ups).
Splitting becomes the default once the open ones are fixed.

## D3: missing figures fail the gate
A gated `rhizo` metric present in the baseline for a VMM that produced
other figures in this run, but absent from the results, is a failure that
lists the skipped steps and their reasons. VMMs that did not run at all
are not compared, so a narrower local run still passes.

## D4: memory of the cell benchmark VM
Forking a 50 MiB cell whose child writes every page needs 100 MiB of cell
memory. The default 128 MiB VM left 106 MiB usable once the kernel image
carried the dynamic-linking files and the relinked CPython, and the child
faulted. The cell benchmark VM gets 256 MiB; the boot benchmarks keep
128 MiB. Timings measured in the cell VM do not depend on its size.
