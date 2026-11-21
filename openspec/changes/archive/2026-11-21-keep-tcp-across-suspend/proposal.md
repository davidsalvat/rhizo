# Proposal

## Why

An agent cell talks to services over TCP: a model API, a database, a peer
cell. Today a suspend, a resume or a fork resets every established
connection (as CRIU does by default), so suspending an agent mid-request
breaks its request, and its peer sees a reset. Suspending a cell is the
core operation of Rhizo (rule 6); within one kernel it should be invisible
to the cell's connections, as it already is to its memory, threads, files
and pipes. Forks and templates, which make several cells out of one, need
a precise and tested rule for the connections they inherit.

## What Changes

- Suspend and resume in the same kernel keep connected stream sockets
  working: TCP on the loopback network, TCP through the network card
  (`smoltcp`), Unix stream sockets and socket pairs. On suspend a
  connection is *parked*: it stays in the kernel with its peer, sequence
  numbers, windows, buffered and unacknowledged data and options; the
  kernel keeps acknowledging, buffering (up to the receive bound) and
  retransmitting for it. The snapshot records where to find it. On resume
  the cell gets the same connection back: same peer, no byte lost.
  Listeners are parked as well: clients that connect while their server is
  suspended wait in the backlog and are accepted after the resume.
- A socket that cannot be re-attached (a snapshot resumed in another boot,
  resumed a second time, or never parked in this kernel) comes back as
  before: a listener is bound again, a connection is reset: `ECONNRESET` on the next call, then end of stream for reads and
  `EPIPE` for writes, never a wait. A peer that closed or reset while the
  cell was suspended is seen as such after the resume.
- `cell::close_parked`: a snapshot the host drops without resuming releases its
  parked connections (their peers see the close).
- Fork and templates: the rule the kernel already follows is made precise
  and tested: the parent (or template) keeps its connections; the child's
  inherited connections are reset with the sequence above; listening and
  bound datagram sockets are shared; sockets never bound are copied.
- Snapshot format version 6: a socket record may say where it is parked
  (boot identity and key).
- Benchmark: `cell.tcp_resume_us`, resuming a cell suspended with an
  established loopback connection plus the first round trip after it.

### Out of scope

Carrying a connection to another kernel or another boot (TCP repair with
the peer's cooperation, migration of addresses), UDP sockets' queued
datagrams across a suspend, signals (`SIGPIPE`).

## Capabilities

### Modified Capabilities

- `networking`: sockets across suspend and resume, sockets across fork,
  connections to the outside.
- `snapshots`: pipes and sockets in snapshots (format version 6, parked
  connections).
- `benchmarks`: network metrics gain `cell.tcp_resume_us`.

## Impact

`crates/rhizo-net` (parking, records), `crates/rhizo-snapshot` (format
version 6), `kernel/src/cell.rs` (export, import, discard, boot identity,
benchmark), `kernel/src/tests.rs`, `cells/src/bin/loopback.rs`,
`cells/src/bin/netstate.rs`, `xtask/src/bench.rs`, `bench/README.md`. No
new dependency. No benchmarked path gets slower: suspend and resume do one
lookup per connected socket, and cells without sockets do nothing new.
