# Proposal

## Why

Open finding O8 in `docs/security.md`: on QEMU, the host reaches the
control kernel through `AF_VSOCK`, and Linux lets any local user connect
to a guest's vsock port without privilege, whatever the mode of
`/dev/vhost-vsock`. The guest's CID follows from the process identifier
of `xtask` or `rhizo`, which `ps` shows. The control kernel holds one
connection and takes the first request it gets while it holds none, so a
local user who connects first (before the host does, or after the host's
connection ended) controls every cell: read output, save snapshots of
their memory, kill them, power the VM off. Before `control-over-vsock`
the control line was a serial pipe private to the host process.

## What Changes

- **A secret per boot.** A control kernel with a socket device draws a
  256-bit secret from its generator (seeded from the hardware) and prints
  it on its serial line, before its hello, as the console line
  `vsock: secret <64 hex digits>`. Only the process that started the VMM
  reads that pipe. The secret never goes on the kernel's command line
  (`/proc/<pid>/cmdline` is world-readable). Without a hardware random
  source the kernel does not listen on vsock at all, and the frames stay
  on the serial line.
- **A connection presents it first.** The vsock stream admits a
  connection only once its first 32 bytes are the secret, compared in
  constant time and only once all 32 have come. Until then nothing it
  sends is read and nothing is written to it. A wrong secret, no secret
  within 2 s, or another peer's request resets it; the kernel keeps
  listening, counts each refusal and reports it on its console. Every new
  connection, after a drop too, presents the secret again. The frames and
  the protocol version are unchanged: the secret is 32 raw bytes before
  the first frame.
- **The host presents it.** `xtask`'s `Vm` (and so `rhizo up`'s relay)
  takes the secret line out of the console text, keeps the secret in
  memory only, and writes it first on the vsock connection, on QEMU,
  Firecracker and Cloud Hypervisor alike. A kernel that gave no secret is
  driven over the serial line.
- **The state directory is private.** `rhizo up` makes its state
  directory mode `0700` (created so, or narrowed if it existed), and its
  control socket `0600`. Nothing secret is written to disk.
- **Tests**: host tests of the gate (wrong, missing, slow and split
  secrets, a reconnect after a drop, a stranger that connects first) and
  property tests over arbitrary first bytes and arbitrary packets; host
  tests of the secret line's filtering and of the secret sent first; the
  `control` test step checks, on a VM with a socket device, that
  connections without the secret are reset with nothing sent back and
  cannot power the VM off.
- `docs/security.md`: O8 closed as finding S28. The weekly random-seed run
  also runs the `vsock::` property tests.

### Out of scope

A local user may still keep the host from connecting, by claiming the
guest's CID first or by sending requests that keep replacing the host's
connection before its secret arrives: denial of service by local users
was already possible (O8, *Before the host connects*) and stays attacker
5 of the audit scope.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `control-plane`: vsock connections are admitted only with the boot's
  secret, given on the serial line; `rhizo up`'s state directory is
  private.

## Impact

`crates/rhizo-virtio/src/vsock.rs` (`Gate`, `Stream::peer`,
`Stream::reset` made public) and its tests; `crates/rhizo-control`
(`SECRET_LINE`, `SECRET_LEN`); `kernel/src/vsock.rs`,
`kernel/src/cell.rs` (control builds only); `xtask/src/control.rs`;
`cli/src/state.rs`, `cli/src/main.rs`, `cli/src/serve.rs`;
`.github/workflows/weekly.yml`; `docs/security.md`. One extra write of 32
bytes when the host connects; per request, two comparisons per byte read
from the vsock connection. Kernels without the `control` feature are
unchanged.
