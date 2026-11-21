# Spec Delta

## ADDED Requirements

### Requirement: Modules are connected child cells
Adding a module to a cell SHALL copy the module's template as a child of
that cell, with limits carved from its module budget, joined to it by a
channel at `/run/rhizo/chan/<label>`, with a control handle at
`/run/rhizo/cell/<label>`. Nothing of the module SHALL be mapped into the
cell itself, and killing the child SHALL remove the module and give its
budget back.

#### Scenario: numpy as a capability
- **WHEN** an agent adds `numpy` and sends a computation over the module's `repl:python` channel
- **THEN** it reads the result, its own memory map is unchanged, and killing the child gives back its budget

### Requirement: Module children reseed
A module's server SHALL check its copy generation before each request
and, when it changed since the warm-up, reseed the generators its
interpreter keeps in memory before running anything, so that no two
children of one module template draw the same numbers.

#### Scenario: Two agents add python
- **WHEN** agents in two domains each add `python` and evaluate `random.random()` over its `repl:python` channel
- **THEN** the two answers differ

### Requirement: Module templates are public
A loaded module SHALL be published (`add-cell-hierarchy`, public
objects): shared across domains, its mapped pages loaded at
publication. A builder's per-build token SHALL be dead before the
builder is frozen, and the operator's signature SHALL be the attestation
that the template holds no secret.

#### Scenario: Token after the freeze
- **WHEN** a builder is frozen and a copy of its template sends a request with its build token to the package proxy
- **THEN** the proxy answers `403` and logs it

### Requirement: Sub-templates share their base
A module made from another SHALL be a copy of its base's template,
frozen after its installation, sharing copy-on-write every page it did
not write, and a request for it SHALL make one child from the deepest
template that provides it.

#### Scenario: pandas over numpy over python
- **WHEN** an agent adds `pandas` while `python+numpy+pandas` is loaded on top of `python+numpy` and `python`
- **THEN** one child is made, and its private memory before any work is only what the pandas layer wrote

### Requirement: File-only modules are views
A module made only of files SHALL be given as a read-only snapshot view
mounted at `/opt/modules/<name>`, in the cell or in its child that the
manifest names.

#### Scenario: A dataset module
- **WHEN** an agent adds a file-only module
- **THEN** its files read under `/opt/modules/<name>` and writes there fail with `EROFS`

### Requirement: Module requests
A cell SHALL ask for a module through its kernel endpoint with the
`modules` right. The kernel SHALL refuse a module absent from the cell's
whitelist (`EACCES`), absent from its module table (`ENOENT`), whose
limits do not fit what is left of the module budget (`EDQUOT`), or whose
requested egress exceeds the cell's.

#### Scenario: Refusals
- **WHEN** an agent asks for a module not on its list, one over its budget of children, and one asking for `full` egress while its own is `model`
- **THEN** each is refused with its reason and no cell is made

### Requirement: Module policy
A cell's module policy SHALL hold a budget for all its modules (children,
memory, CPU time, sockets), carved from its limits when it is made, an
approver (parent or host), and entries naming a module, a version prefix,
caps within the budget, whether each add needs approval and whether
egress within the cell's needs none. A child's policy SHALL be a subset
of its parent's.

#### Scenario: Subset for a child
- **WHEN** an agent gives its child a policy naming a module its own policy lacks
- **THEN** the child's creation is refused

### Requirement: Module children start with nothing
A module child SHALL get an empty module policy (no entries, a zero
budget) and a kernel endpoint without the `spawn` right unless the add
request delegates a policy to it. Every `add` it sends with that empty
policy SHALL be refused with `EACCES`, make no cell, and leave an audit
record.

#### Scenario: Module asks the kernel
- **WHEN** a module child added without delegation sends `add` for `python` on its kernel endpoint
- **THEN** it is refused with `EACCES`, no cell is made, the agent's module budget is unchanged, and the audit log records the refusal

### Requirement: The agent adds siblings for its modules
A module child SHALL get further capabilities only through its agent:
the agent MAY add the module as its own child, a sibling of the
requester, carved from its own module budget, and pass the requester a
handle to it derived with fewer rights over the channel joining them.

#### Scenario: Sibling with a message-only handle
- **WHEN** a module asks its agent for `documents` over their channel, and the agent adds it and passes a cell handle derived with only the `message` right
- **THEN** the new module is the agent's child, the requester opens a channel to it through that handle, and observing, writing input to or controlling it fails with `EACCES`

### Requirement: Delegated module policies
An add request MAY delegate a module policy to the module: entries each
within one of the delegator's (version prefix at least as long, caps no
higher, `ask` kept, `egress = "granted"` only where the delegator's has
it) and a budget slice carved from what is left of the delegator's
module budget. The module's own adds SHALL then make its children under
the same rules, the tree depth limit and the approver rules.

#### Scenario: Grandchild within the slice
- **WHEN** an agent adds `coder` delegating entries `python` and `numpy` with a budget of two children and 512 MiB, and `coder` adds `python`, `numpy`, then a third module
- **THEN** `python` and `numpy` run as children of `coder`, the third is refused with `EDQUOT`, and the agent's budget shows the whole slice taken

### Requirement: Delegation never exceeds the delegator
The kernel SHALL refuse, making nothing, a delegation naming a module
absent from the delegator's policy or a looser entry (`EACCES`), a budget
over what is left of the delegator's (`EDQUOT`), or `egress = "granted"`
the delegator lacks (`EACCES`). Each delegation SHALL be an audit record
and SHALL show in `rhizo ps --tree`.

#### Scenario: Delegations past the delegator
- **WHEN** an agent with egress `model` and 512 MiB of module budget left delegates an entry for `requests` it lacks, a 1 GiB slice, and `egress = "granted"` on an entry whose own lacks it
- **THEN** each add is refused with its reason, no cell is made, and the agent's budget is unchanged

#### Scenario: Delegated module asks past its egress
- **WHEN** a delegated module with egress `none` adds a module whose manifest requests egress `model`
- **THEN** the add is refused and no grandchild is made

#### Scenario: Delegation is visible
- **WHEN** an agent delegates a policy to a module
- **THEN** the audit log records the delegated entries and budget, and `rhizo ps --tree` shows them on the module

### Requirement: A module never widens egress
A module child's egress SHALL be the intersection of what its manifest
requests, what its entry grants, and its parent's egress. Adding a module
SHALL grant no network access the cell's tree does not already allow.

#### Scenario: requests without internet
- **WHEN** an agent with egress `model` adds `requests` (approved) and the module connects to an index and to the model endpoint
- **THEN** the first fails with `EACCES` and the second succeeds

### Requirement: Approval of sensitive modules
An add SHALL wait for the approver when its entry asks for it or the
child would get egress its entry does not grant. The approver SHALL be
the parent, through an `approvals` channel the kernel gives it, or the
host; a cell SHALL have at most one pending request, refused after its
deadline or on a suspend. Each answer SHALL be an audit record.

#### Scenario: Approve, deny, time out
- **WHEN** three adds needing approval are answered by approve, by deny, and not at all
- **THEN** the first makes the child, the second and third are refused, the third after its deadline

### Requirement: The operator approves for roots
For a root cell, every add that needs approval, or would give the child
any egress, SHALL wait for the operator (the host), whatever its policy
says: no entry and no approver setting SHALL make it automatic.

#### Scenario: Root agent asks for requests
- **WHEN** a root agent whose policy marks `requests` with `egress = "granted"` and names itself as approver adds `requests`
- **THEN** the request waits until the operator runs `rhizo approve`, and the audit log records the operator's answer

### Requirement: Signed module files
A module file SHALL be a snapshot file followed by a canonical manifest
(name, version, architecture, base digest, interface, entry, requested
egress, minimum limits, lockfile and tree digests, builder), signed in the
minisign format (Ed25519 over BLAKE2b-512). Only the host SHALL hold
keys; no cell and no kernel code SHALL hold or use a secret key.

#### Scenario: Tampered module
- **WHEN** `rhizo module load` is given a module with one byte changed, one signed by an unknown key, and one whose digest is revoked
- **THEN** each is refused before any byte reaches the kernel

### Requirement: Loading modules
The host SHALL load a module only after checking its signature against a
trusted key, the revocation list and, for a delta file, that its base is
loaded with the digest its manifest names. The kernel SHALL validate the
bytes as any snapshot and keep the template in its module table. An
unsigned module SHALL match only entries that allow unsigned modules.

#### Scenario: Delta without its base
- **WHEN** a `python+numpy` delta file is loaded before `python`
- **THEN** it is refused, and once `python` is loaded it loads and shares its base's frames

### Requirement: Builder cell
New catalogue modules SHALL come only from a builder cell: a root of its
own domain with no handle from any agent, whose egress is the host's
package-index rule set, installing from a lockfile with pinned versions
and hashes, recording its installed tree's digest, frozen as a template,
and signed by the host only after the operator approves it.

#### Scenario: Builder confined
- **WHEN** a builder installs from a lockfile and also connects to the model endpoint and to another address
- **THEN** both connections fail with `EACCES`, and a wheel whose hash differs from the lockfile fails the build

### Requirement: Reproducible module trees
Two builds from the same lockfile and base SHALL give the same installed
tree digest, and `rhizo module build --twice` SHALL refuse to go on when
they differ.

#### Scenario: Two builds
- **WHEN** a module is built twice from one lockfile
- **THEN** both tree digests are equal and the manifest records it

### Requirement: Packages installed by an agent stay private
Packages a cell installs itself SHALL stay in its own files: copied only
into its forks, gone when its tree ends, and never loaded into the module
table.

#### Scenario: pip inside an agent
- **WHEN** an agent whose egress allows the index runs `pip install` and its sibling and a cell of another domain look for the package
- **THEN** neither finds it, and nothing of it remains after the agent's tree ends

### Requirement: Signing keys
Secret keys SHALL stay outside cells, the kernel and builders: in a
passphrase-encrypted file of the operator or behind a signing command.
Trusted public keys SHALL carry a purpose and a validity, and `rhizo`
SHALL refuse a trust directory or file that others can write, and a
signature made outside its key's validity.

#### Scenario: Rotation
- **WHEN** a module key is rotated with a grace period of 30 days
- **THEN** modules re-signed by the new key load, and those signed only by the old key load until the grace period ends and not after

### Requirement: Signed revocation list
Revoked keys and module digests SHALL be listed in a file signed by a
revocation key, with a sequence number; `rhizo` SHALL refuse a list with
a bad signature or a lower sequence number than the last it saw, and
SHALL report loaded modules the list revokes.

#### Scenario: Rollback refused
- **WHEN** an older revocation list replaces the current one
- **THEN** `rhizo module load` refuses to run until the current list is back

### Requirement: Package proxy
`rhizo proxy` SHALL serve a builder only the projects, versions and files
the build's lockfile names, fetched from the configured index over
verified TLS, each checked against its pinned SHA-256 before any byte is
served; requests without a live build token SHALL get `403`. It SHALL
listen only where the guest reaches the host and log every request.

#### Scenario: Off-list and tampered files
- **WHEN** a builder asks the proxy for a project outside its lockfile, and upstream serves a pinned file with other bytes
- **THEN** the first gets `404`, the second `502` with no byte served, the build fails, and both are in the proxy's log

### Requirement: Only builders reach the proxy
The proxy's address SHALL stay in every cell's deny floor except for
builder cells, so no agent can reach it.

#### Scenario: Agent tries the proxy
- **WHEN** an agent with egress `full` connects to the proxy's address
- **THEN** it receives `EACCES` and the denial is audited
