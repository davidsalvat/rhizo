# Proposal

## Why

Inside the kernel a cell starts in 7.3 µs, but nobody outside the VM sees
that number. What a user and the market compare is the end-to-end control
path: a request from the host (`rhizo` or `xtask/src/control.rs`), the
relay server, the serial line, the control kernel's request loop
(`kernel/src/control.rs`), the cell's first instruction, and its first
output back to the host. Measured independently: E2B creates a sandbox in
717 ms, Freestyle forks in under 50 ms, GKE Agent Sandbox starts in under
200 ms. Today `cargo xtask bench` times single requests
(`control.roundtrip_us`, `control.run_us`, ...) but not a request to the
first output, and nothing says where the time goes, so nothing tells us
what to fix first.

## What Changes

- **Two end-to-end metrics** in `cargo xtask bench`, timed by the host
  from writing the request to reading the first byte the cell printed:
  `control.spawn_to_first_output_us` (`run` of `hello`, then `output`)
  and `control.fork_to_first_output_us` (`fork` of a template frozen in
  its sleep, whose child prints at once, then `output`). The existing
  `control.roundtrip_us` stays the no-op request.
- **A breakdown, not gated** (`breakdown.control.*`): each start's reply
  round trip, the kernel's own time for it and the output watch; the
  kernel's time for a `list`; what one byte costs on the serial line each
  way (requests the kernel refuses at once, 1 KiB in, or in and out); and
  a `list` after 300 ms of quiet, when the kernel looks at the line less
  often.
- **`cargo xtask bench --workload control`** measures the control plane's
  figures alone, as `--workload node` does for Node.js.
- **One cheap win, measured** (design D3): the x86_64 16550 driver fills
  the transmit FIFO it turned on (16 bytes) after one look at the line
  status instead of looking before every byte. Each register access
  traps to the VMM; this removes nearly half of them for every byte the
  kernel sends.
- **Bottleneck and follow-up written down** (design *Measurements*,
  D4): the serial line's trapped register accesses dominate the path;
  the fix, a virtio-vsock control channel, is a follow-up change
  `control-over-vsock`, not this one.

## Impact

- Specs: `benchmarks` (control path to first output, its breakdown,
  `--workload control`).
- Code: `xtask/src/control.rs` (the measurements), `xtask/src/bench.rs`
  and `xtask/src/lib.rs` (`--workload control`),
  `kernel/src/arch/x86_64/serial.rs` (transmit bursts). The control
  protocol and its version do not change.
- `bench/baseline.json`: entries added for the two new gated metrics in
  the `ci` environment, from a `bench` run of this branch; no existing
  value changes.
- `bench/README.md`: the new metrics and the breakdown.
