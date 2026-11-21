# Proposal

## Why

PoC line F.2.3: snapshots and templates stay inside the VM that made them.
A warmed agent dies with its VM, and nothing can ship one: the thesis of
"downloadable agents" (`docs/vision.md`) needs a suspended cell, or a
frozen template, to be saved to a file on the host and loaded into a fresh
Rhizo VM (another boot, possibly another machine of the same
architecture), where it resumes or forks.

`Snapshot::export` already writes self-contained bytes and
`Snapshot::from_bytes` validates them, but no production path calls
`from_bytes` (audit scope, boundary B3), and the control plane has no
templates: it can fork a running cell only.

A known gap from `fast-node-template` also lives in the format: a thread
waiting on descriptors is recorded without its time left, so a resumed
cell starts that call's timeout over. Since the format changes here, it is
fixed here.

## What Changes

- **Snapshot format version 10.** A thread waiting on descriptors is
  recorded as such (wait kind 3) with the time its call had left, and a
  runnable thread with the time left of the call it makes again; a resumed
  cell's calls end when they would have.
- **Snapshot files.** The exported form ends with a CRC-32C of everything
  before it. `from_bytes` checks the header (magic, version, architecture),
  then the checksum, then everything it holds, and names the reason of a
  refusal: another version, another architecture, a damaged or cut file.
- **Templates in the control plane.** `freeze` makes a running cell a
  template; `fork` of a template makes cells from it (they keep no time);
  `kill` drops it; `list` shows it.
- **Save and load.** Protocol version 3: `save` streams the snapshot of a
  suspended cell or a template in chunks of 64 KiB; `load` and `load-data`
  send one in chunks, and the last chunk makes it a suspended cell or a
  template, through `from_bytes`, with quotas no higher than a new cell's.
  `rhizo freeze <id>`, `rhizo save <id> <file>` and
  `rhizo load <file> [--template]`.
- **Transport: the serial line, measured.** The control channel carries
  the chunks; the design gives the measured rates and the faster path
  (a virtio-blk exchange disk) as follow-up.
- **Tests**: format v10 and the checksum on the host, the hostile-snapshot
  property tests on the sealed (import) form, two in-kernel tests (a wait
  on descriptors keeps its time left across suspend; a template travels as
  bytes), and the `control` step: the CPython chat agent frozen into a
  template and another suspended, saved, `rhizo down`, `rhizo up`, loaded;
  two cells of the template and the resumed cell answer right; damaged,
  cut and foreign files are refused with the reason.
- **Benchmarks**: `snapshot.save_us`, `snapshot.load_us`,
  `snapshot.file_bytes`, and the first answer of a cell of the loaded
  template on a fresh VM (`snapshot.loaded_answer_us`) next to a cold start
  (`snapshot.cold_answer_us`), on QEMU.

### Out of scope

A faster transport (follow-up, design D2), signed or authenticated files
(the checksum catches damage, not forgery: forged bytes are contained by
validation, spec `isolation`), files that refer to executables or file
sets by content instead of carrying them, and loading a file across
architectures.

## Capabilities

### Modified Capabilities

- `snapshots`: format version 10 (time left of waits on descriptors), the
  exported form with its checksum, files across boots.
- `control-plane`: templates, `save` and `load` (protocol version 3), the
  `rhizo` commands.
- `isolation`: snapshots from files go through validation with their
  checksum, refer to no frame, and get no more quota than a new cell.
- `benchmarks`: snapshot file metrics.

## Impact

`crates/rhizo-snapshot` (version 10, checksum, `cap_quota`, `deadlines`,
`export(&self)`), `crates/rhizo-control` (version 3), `kernel/src/cell.rs`
(`Host::load_template`, `template_snapshot`, `drop_template`, deadlines
across suspend and resume), `kernel/src/control.rs`, `kernel/src/tests.rs`,
`cli/`, `xtask/src/control.rs`, `docs/audit-scope.md`, `docs/security.md`.
Suspend and resume do one more lookup per thread; `export` now copies the
bytes of a snapshot that shares no frame (only exports, which leave the
kernel, pay it).
