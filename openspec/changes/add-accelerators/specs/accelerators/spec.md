# Spec Delta

## ADDED Requirements

### Requirement: Accelerators stay behind a broker
The Rhizo kernel SHALL NOT drive any accelerator device. Every GPU, NPU
and hosted model SHALL be reached through an accelerator broker outside
the Rhizo VM, over one vendor-neutral, versioned protocol that only the
kernel speaks. A frame the kernel cannot decode SHALL end the session with
an error, never the kernel.

#### Scenario: A broker sends garbage
- **WHEN** the broker answers a request with a truncated or unknown frame
- **THEN** the cell's session read fails with `EIO`, other cells keep running, and the kernel does not panic

### Requirement: No accelerator without a grant
A cell SHALL reach an accelerator only through a handle with the right
`infer` (model sessions) or `compute` (compute sessions), granted by the
host or an ancestor and only attenuated down the tree. A new cell SHALL
hold none. Its sessions SHALL appear as channel endpoints under
`/run/rhizo/accel/`.

#### Scenario: A cell without a handle
- **WHEN** a cell with no accelerator handle opens `/run/rhizo/accel/chat` or tries to reach the broker's address
- **THEN** the open fails with `ENOENT`, the connect fails with `EACCES`, and nothing reaches the broker

#### Scenario: A granted model session
- **WHEN** the host runs a cell with `--accel fake:infer` and the cell writes a prompt to `/run/rhizo/accel/chat`
- **THEN** it reads the fake model's tokens back, on both architectures and every VMM

### Requirement: Accelerator budgets per tree
Device memory, KV bytes held in host memory, device time share and open
sessions SHALL be budgets carved from the parent's at creation and
returned at the end, so a subtree never holds more than its root. A
request beyond a budget SHALL fail with `EDQUOT` and change nothing.

#### Scenario: A child over its device memory
- **WHEN** a child with `accel.mem` of 1 GiB opens a session whose KV would need 2 GiB
- **THEN** the session fails with `EDQUOT`, and the parent's remaining budget is unchanged

### Requirement: Forked cells fork their sessions
Forking a cell SHALL fork each of its model sessions so that every copy
continues from the same tokens, sharing the KV already computed; no copy
SHALL prefill the shared prefix again when its broker has the same model.

#### Scenario: Fork without prefill
- **WHEN** a cell with a 4,096-token session on the fake broker is forked into four copies and each generates
- **THEN** the fake reports no tokens prefilled for the fork, and each copy's output equals a fresh session's output on the same tokens

### Requirement: Engine-neutral KV in the cell's frames
KV blocks evicted from the engine SHALL be kept in frames owned by the
cell and charged to its memory, in a container keyed by model fingerprint
and token prefix. Identical blocks SHALL be shared only between cells of
one domain, never across domains.

#### Scenario: Same prompt in two domains
- **WHEN** two cells of one domain and one cell of another evict KV for the same prompt
- **THEN** the two cells of one domain share the frames, and the third cell holds its own copy

### Requirement: Resume without re-prefill
A suspended, saved, loaded or moved cell SHALL resume its sessions from
its KV blocks when the broker serves the same model fingerprint, and SHALL
re-prefill from its token ids otherwise. Either way it SHALL generate the
same tokens as before.

#### Scenario: Saved and loaded across a restart
- **WHEN** a cell with a session is saved, `rhizo down` and `rhizo up` run, and the cell is loaded with its accelerator bound
- **THEN** its next tokens match, and the broker prefilled zero tokens

#### Scenario: Another model
- **WHEN** the same file is loaded against a broker whose model fingerprint differs
- **THEN** the session re-prefills from its token ids and reports it, and no KV block is imported

### Requirement: One domain per device or partition
A device, or a hardware partition of it (MIG instance, compute and memory
partition, SR-IOV function), SHALL serve at most one domain at a time.
Cells of one domain MAY share an engine. Sharing across domains without a
partition SHALL need the operator's explicit, audited override.

#### Scenario: A second domain asks for a bound device
- **WHEN** a domain holds `gpu0` and a cell of another domain opens a session on it, without the override
- **THEN** the open fails with `EBUSY`, and the audit log records the refusal

### Requirement: Scrub and IOMMU
The broker SHALL refuse a device whose DMA is not behind an IOMMU. Before
a device or partition serves another domain, the broker SHALL clear the
device memory and local memory the previous domain could have written.

#### Scenario: Leftover memory
- **WHEN** a domain's kernel writes a pattern into device local memory, its sessions close, and another domain's probe reads uninitialized local memory
- **THEN** the probe reads no byte of the pattern

### Requirement: Hosted models through the gateway
Hosted model APIs SHALL be reached through the broker acting as the
`model` gateway, with API keys kept on the host. Sessions of one fork
family SHALL be routed to the same provider and cache key, and cache keys
SHALL be derived per domain so domains never share a provider cache.

#### Scenario: Ten forks on a hosted model
- **WHEN** a cell with a long prompt on a fake API server with a prefix cache forks into ten copies that each send a turn
- **THEN** the fake records one cache write and ten cache reads, all under one key of that domain

### Requirement: A deterministic fake accelerator
`rhizo-hal` SHALL provide a fake accelerator for host tests, and `xtask` a
fake broker for in-kernel tests, both deterministic: the same requests
SHALL give the same tokens and KV bytes, prefilled tokens SHALL be
counted, and device loss and exhaustion SHALL be injectable.

#### Scenario: Device lost mid-session
- **WHEN** the fake broker reports its device lost while a cell generates
- **THEN** the cell's read fails with `EIO`, its session can be reopened and re-prefilled from its token ids, and other domains are unaffected
