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
deadline or on a suspend.

#### Scenario: Approve, deny, time out
- **WHEN** three adds needing approval are answered by approve, by deny, and not at all
- **THEN** the first makes the child, the second and third are refused, the third after its deadline

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
