# Design

## Context

The control kernel (`kernel/src/control.rs`, feature `control`) serves
the host over its serial line, or over the host's vsock connection to
port 1024 while there is one (`control-over-vsock`). The device is polled
(`kernel/src/vsock.rs`); the protocol under the frames is the `Stream` of
`rhizo_virtio::vsock`, which holds one connection, made by the first
request it gets while it holds none.

On QEMU the host end is the host kernel's `AF_VSOCK`. Any local process
may `connect` to `(guest CID, 1024)` without privilege, and the CID is
`3 + (pid << 6) + n`, with the `xtask` or `rhizo` process identifier
(finding O8 of `docs/security.md`). On Firecracker and Cloud Hypervisor
the host end is the VMM's Unix socket, guarded by its file mode (the
VMM's umask).

The serial line is a pipe between the VMM and the process that started
it: no other user can read it (reading another process's pipe needs
`ptrace` rights over it, that is, the same user or root). It is the
channel the host already trusts, and the one vsock replaced.

## D1. One secret per boot, given on the serial line

`vsock::init` draws 32 bytes from the kernel's generator
(`rhizo_core::random`, ChaCha20 seeded from the CPU's random instruction
by `cell::init`; `cell::random_bytes`) and prints the console line
`vsock: secret <64 lowercase hex digits>` before the hello frame.

- **Why the serial line**: it is the one channel the host process alone
  reads, and it is already there on every VMM; no new device, no file.
- **Why not the command line**: `/proc/<pid>/cmdline` of the VMM is
  readable by every user.
- **Why not a file the host writes for the guest** (a disk, `fw_cfg`,
  the device tree): each is a new device or a VMM-specific path, and the
  host would have to choose the secret and keep it from other users on
  disk; drawing it in the guest needs nothing.
- **Why text, not a frame**: a new frame kind, or a longer hello, changes
  the request protocol, whose version 4 is being written by another
  change. Console text between frames is already part of the serial
  line's format (`Item::Text`). A cell cannot print on that console: its
  output reaches the host only inside frames (B8), so no cell can forge
  or move the line.
- **No hardware random source** (an AArch64 host without FEAT_RNG under
  KVM): the generator is seeded from the cycle counter, and the secret
  could be searched for. The kernel then does not listen on vsock
  (`vsock: no hardware random source: control stays on the serial line`)
  and prints no secret; the host, seeing none, stays on the serial line.
  Failing closed costs speed on such hosts, never access.

## D2. The gate: 32 bytes before anything else

`rhizo_virtio::vsock::Gate` wraps the `Stream`: the stream keeps its
packet protocol (credit, resets, connection numbers) unchanged, and the
gate decides which connection is served.

- A connection is **admitted** once its first `SECRET_LEN` (32) bytes are
  the secret. They are compared only when all 32 have come, with an OR of
  XORs passed through `black_box`: a wrong byte gives no early answer, so
  neither the reset's timing nor its moment tells which byte was wrong
  (with a byte-by-byte check the secret could be found one byte at a time,
  256 tries each).
- Until then, `connection()` is `None`: the control kernel neither greets
  it nor reads from it, `write` refuses it (no hello, no reply, no cell
  output reaches it; the frames keep going to the serial line), and
  `has_input` is false. Bytes after the secret, in the same packet or
  later, are the connection's first frames.
- **Refusal**: a wrong secret is reset at once. The kernel keeps
  listening, counts the refusal (`Gate::refused`) and prints
  `vsock: refused a connection without the secret (N so far)`.
- **Bounds**: at most 32 bytes are ever read from a connection that is
  not admitted (more is then a wrong secret); one that does not finish
  within `PROVE_WITHIN_NS` (2 s) of the first poll that saw it is reset
  (`Gate::expire(now)`: the time is passed in, never read, as in the rest
  of the library crates). The host sends the secret in the same burst as
  its connection, so 2 s is far beyond any legitimate delay, even under
  TCG.
- **Replacement**: a request from another peer while the connection held
  is not admitted resets that connection (a refusal) and is accepted.
  Without it a stranger could hold the only connection for the whole
  bound, again and again. An admitted connection is never replaced: a
  second request gets a reset, as before.
- **Reconnects**: admission belongs to a connection number
  (`fix-vsock-reconnect`). A new connection, even from the same peer
  after a drop, starts unadmitted and presents the secret again.

Alternatives that lost:

- *A challenge and response (HMAC of a nonce)*: protects the secret from
  someone who can read the connection, but whoever can read a vsock
  connection on the host is root or the VMM, both trusted. It costs a
  round trip and a MAC in the kernel for nothing here.
- *Checking the source CID or port*: every local process connects from
  CID 2, the host's, and picks no privileged port; the VMMs do not tell
  users apart.
- *The gate inside `Stream`*: the stream's tests and invariants (credit,
  resets, connection numbers) would all change; the gate is a layer on
  its public interface plus `Stream::peer` and `Stream::reset`.

## D3. The host presents the secret

- `xtask::control::read_frames` reads the VMM's output through a
  `SecretFilter`: a line is held back only while it may still be the
  secret line (the prefix, then up to 64 hex digits and a carriage
  return), dropped when it is one, and passed on as text otherwise. The
  first secret is kept, in memory only, when the next frame (the hello)
  arrives. The console text, `Vm::console`, error reports and `rhizo
  up`'s `console.log` never hold it.
- `Vm::start` is `boot` (to the serial hello) and `attach`: `connect`
  writes the secret as the first bytes of the connection, on QEMU's
  `AF_VSOCK` and on the VMMs' Unix sockets alike (defence in depth there:
  their socket's mode follows the VMM's umask). A VM with a socket device
  whose kernel gave no secret is driven over the serial line, and
  `channel` says why.
- `rhizo up`: the relay is `Vm`, so it presents the secret the same way.
  Nothing secret is persisted. Its state directory holds the control
  socket, which gives every cell to whoever connects: `Dir::create` makes
  it `0700` (narrowing one that existed), and the socket is made `0600`
  after `bind`, because it lives in the temporary directory when the
  state's path is too long.

## D4. The request protocol is unchanged

The secret is 32 raw bytes before the first frame of a vsock connection;
the frames, the hello and `VERSION` are as they were. The serial line
carries one more console line. No coordination with the protocol's
version 4 is needed.

## D5. What is left

A local user can still keep the host out: claim the guest's CID before
QEMU starts (O8 already), or keep sending requests so that each replaces
the host's connection before its secret arrives. Both are denial of
service by a local user (attacker 5 of the audit scope), with no access
to any cell. `xtask` reports the failed hello and stops the VM.

## D6. The vsock path runs in pull requests

Before this change CI never drove a control kernel over vsock in a pull
request: `vhost_vsock_usable` looked for `/dev/vhost-vsock` before loading
the module that creates it, so QEMU fell back to the serial line, and the
KVM job ran no control step. A fallback is silent by design (a user
without the device still gets a working channel), which hid it.

- On a CI runner the module is loaded (`modprobe vhost_vsock`, `udevadm
  settle`) before the device is looked for.
- The `control` step prints the channel it used. On a CI runner, a VMM
  that has a socket device there (Firecracker, Cloud Hypervisor, QEMU
  with KVM) must use vsock: the stranger check and `rhizo up`'s
  `console.log` line (`[rhizo] control frames over ...`) both fail the
  step on a fallback.
- The KVM job runs the step on Firecracker and Cloud Hypervisor; the
  x86_64 job already runs it on `qemu-microvm` with KVM.

## Measurements

Not on the per-request path: one 32-byte write when the host connects,
and in the kernel two comparisons per byte read from an admitted
connection (`Gate::connection` before `Stream::read`). The `control.*`
metrics (timings on KVM, `bench.yml`) see the connect only in the VM's
start, which they do not time. `cargo xtask bench --icount-only` on both
architectures: no metric moves (control builds are not among the
instruction-count kernels, and other kernels are unchanged).
