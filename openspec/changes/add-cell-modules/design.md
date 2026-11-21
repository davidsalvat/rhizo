# Design

This design builds on `add-cell-hierarchy`. It uses that change's
ownership tree, carved budgets, egress policy, handles, channels, views
and kernel endpoint, and adds no new isolation mechanism. Its decision
numbers (D1 to D16) are cited as `H-Dn`.

The user's decisions, which this design takes as given:

1. Capabilities are connected child cells. The agent's memory is never
   merged with a module's.
2. The agent adds capabilities itself, within a whitelist and a budget
   set when it is created. Sensitive modules need approval. A module
   never gets more than its parent.
3. The shared catalogue holds only signed modules. Promotion into it goes
   through a builder cell, with operator approval. The host holds the
   keys.
4. A small static chat agent, with Python added only as a capability.

Decided after the first review, with the rule **design for production,
not just the proof of concept**:

5. A root agent's sensitive modules always need the operator's approval
   (D2).
6. The builder's package proxy is production-grade: Rhizo ships its own
   minimal proxy on the host, which enforces the index allowlist and
   checks pinned hashes (D5a).
7. The static chat agent becomes the default for `rhizo chat` once it
   exists (D6).
8. Key management for signing (generation, rotation, revocation, where
   keys live) is specified (D4a). Module events go to the audit log of
   H-D18.

Decided after the second review:

9. A module child gets an empty module policy and no `spawn` right by
   default. When it needs another capability, it asks its agent, which
   adds it as a sibling if it agrees (D3a).
10. Delegation is opt-in: an agent may give a module a subset of its own
    whitelist and a slice of its module budget, for orchestrator-style
    agents (D3a).

## D1. A module is a template, used as a child cell

A **module** is a frozen cell (a template) with a manifest. Adding it to an
agent means:

- the kernel copies the template as a child of the agent, with its limits
  carved from the agent's module budget (H-D4);
- the kernel creates a channel between the two (H-D10);
- the agent holds the channel at `/run/rhizo/chan/<label>` and a control
  handle on the child at `/run/rhizo/cell/<label>`;
- the child runs the program its manifest names, already warm (it was
  frozen after its warm-up), and serves requests on its end of the
  channel, `/run/rhizo/chan/parent`.

Nothing is merged into the agent: the agent's memory, files and egress are
unchanged. Removing the module is killing the child, which gives its
budget back. A module child has no module policy of its own unless its
whitelist entry gives it one (D3), so modules do not add modules by
default.

**Sub-templates.** Modules form a tree: `python` → `python+numpy` →
`python+numpy+pandas`. A sub-template is made by copying its base template,
installing into the copy, warming up and freezing. It therefore shares
with its base every page it did not write, copy-on-write
(`add-shared-snapshots`). An agent that asks for `pandas` gets one child,
from the deepest template that provides it. It does not get three
children.

**Module templates are public objects** (H-D21). A loaded module is
published: immutable, holding no secret (D5), and the only kind of
template whose frames copies in many domains share. Its mapped files'
pages are loaded at publication, so adding a module in one domain never
shows in another's page faults. Everything a module child writes is
private to its domain.

**A module child reseeds after the copy** (H-D22). Every add is a copy of
a template, so the module's server (`rhizo_module.py`, `rhizo-mod`)
reads `/run/rhizo/generation` before each request and, when it changed
since the warm-up, reseeds what the interpreter keeps in memory
(`random.seed()` for `repl:python`) before running anything. Two module
children of one template never draw the same numbers. The string hash
seed stays the template's (H-D22).

**File-only modules** (pure Python packages, datasets, prompts, models'
tokenizer files) are not cells. They are read-only snapshot views
(H-D11), mounted at `/opt/modules/<name>` in the agent, or in its
`python` child when the manifest says the files are for Python. Their
blocks come from a file set the host holds, so they are shared across
domains without creating a channel (H-D12, item 10).

**What a module offers** is named by its manifest's `interface`. Two
interfaces are defined. Both use JSON messages on the `SOCK_SEQPACKET`
channel, so any language can drive them:

- `exec`: run a command line in the module cell with given standard input
  and files; reply with its output, errors and exit status.
- `repl:<language>` (`repl:python`, `repl:node`): evaluate code in a
  session that persists between requests. This is the contract that
  `agent_chat.py`'s snippet tool implements today (output capped, time
  limit, the value of the last expression).

Each request carries an `id`, and its reply carries the same `id`. Files
travel in messages up to 64 KiB. Larger files go through a snapshot view
that the agent grants the child (`/in`, read-only), or one the child
grants back (`/out`). A child granting its parent a view is allowed: the
parent is an ancestor.

Alternatives:

- **Load the interpreter into the agent's own memory** (layered images,
  `dlopen` of a runtime). Rejected by the user: a module's code would run
  with the agent's memory, files, sockets and egress, and could not be
  removed, metered or killed alone.
- **One child per layer** (`python`, `numpy` and `pandas` as three
  cooperating cells). Rejected: numpy must live in the same address space
  as the interpreter that imports it.

## D2. How an agent asks for a capability

The agent asks the kernel through its kernel endpoint, which needs the
`modules` right (a new right of the endpoint, H-D9). A cell given a module
policy (D3) gets an endpoint with that right, even if it has no `spawn`
right. A small client is provided (`rhizo_module.py`, and `rhizo-mod`, a
static command for shells and other languages) so that agents send JSON
lines and do not have to encode control-protocol frames.

```text
agent ──add{module:"numpy", label:"np"}──► kernel (its own endpoint)
        kernel: whitelist? budget? egress? approval needed?
            ├─ allowed, nothing sensitive ──────────────► spawn
            └─ sensitive ──approval{cell, module, egress}──► approver
                                (parent's /run/rhizo/approvals, or the host)
                 approver ──approve | deny──► kernel ──► spawn | refuse
agent ◄──added{label:"np", limits, egress} + /run/rhizo/chan/np, cell/np
```

The kernel decides everything that can be decided mechanically:

- **Whitelist.** Is the module named in the agent's policy, with a
  matching version? If not, the request is refused with `EACCES`.
- **Catalogue.** Does the kernel's module table (D4) hold a module of that
  name and version that is signed, or explicitly allowed as unsigned by
  the host? If not, `ENOENT`.
- **Budget.** Do the module's minimum limits fit in what is left of the
  agent's module budget? If not, `EDQUOT`.
- **Egress.** The child's egress is the intersection of three things: the
  egress the manifest asks for, what the whitelist entry grants, and the
  agent's own egress. If the manifest asks for more than the agent has,
  the request is refused: no approval can widen a policy below the host
  (H-D7).

**Approval.** Approval is needed when the entry is marked `ask`, or when
the child would get any egress at all and the entry does not say
`egress = "granted"`. The kernel then sends an approval request to the
approver named by the policy, and the agent's request waits until the
answer comes. The approver is one of:

- `parent`: an `approvals` channel in the parent's `/run/rhizo/`, created
  by the kernel when the parent asked to approve for its children;
- `host`: a control-protocol event, handled with `rhizo approvals`,
  `rhizo approve ID` and `rhizo deny ID`.

**A root agent's sensitive modules always go to the operator.** A root
has no parent, and an operator decides what a top-level agent can grow
into. For a root:

- the approver is always `host`;
- `egress = "granted"` does not waive approval;
- no policy can make a sensitive add automatic.

For a non-root cell, the approver is whatever its policy says. A parent
can only name itself or `host`, and the parent's own policy may say
`approver = "host"` for its whole subtree, which children cannot undo.
Every approval and denial is an audit record (H-D18), with who answered.

Pending requests are bounded: one per cell. A pending request is
refused after a deadline, 10 minutes by default. The answer is delivered
to the agent as `added` or `refused{reason}`.

The agent's own requests go to the kernel, not to its parent, so the
fast, common path involves no other cell. The parent or the host is
involved only for approvals, as the user asked ("a channel to its parent
or the host").

Alternatives:

- **The parent as broker for every request**, holding template handles
  and spawning itself. Rejected for the agent's own requests: every
  capability would wake another cell, and an agent at the root has no
  parent to ask. It is, however, the default for a module child's
  requests (D3a): there the agent's judgement is the point.
- **A new system call.** Rejected for the reasons given in H-D10.

## D3. Whitelist and budget format

The module policy is part of the cell's state: it goes into snapshots and
forks, and a child of the cell holds at most a subset of it. The host
gives it as TOML (`rhizo run --modules agent.toml`), and a parent gives it
in a `spawn` request in the same structure, encoded:

```toml
# Budget for all modules together, carved from the agent's own limits
# when the agent is created (H-D4): reserved for modules, given back
# when they end.
[budget]
children = 4          # modules alive at once
memory   = "768M"
cpu      = "10m"      # CPU time over the agent's life
sockets  = 16

approver = "parent"   # or "host"; always "host" for a root (D2)

[[allow]]
module  = "python"
version = "3.13"      # a prefix of the module's version
[[allow]]
module  = "numpy"
version = "2"
memory  = "256M"      # cap for this module, within the budget
[[allow]]
module  = "requests"
ask     = true        # every add needs approval
[[allow]]
module  = "pip-tools"
egress  = "granted"   # may get egress within the agent's, no approval
```

A module added from these entries gets an empty policy unless the add
request delegates one (D3a).

**Rules.**

- `budget` is carved out of the agent's limits when the agent is created.
  An agent with 1 GiB and a 768 MiB module budget has 256 MiB left for its
  own pages and files.
- An entry's limits are caps inside the budget. The manifest's minimum
  limits must fit them, or the add is refused.
- The child's egress is always at most the agent's. `egress = "granted"`
  only waives the approval. It cannot widen anything.
- **Adding `requests` grants no internet access.** The `requests` module
  child gets egress only if the tree's egress policy allows it, and then
  only after approval. An agent with egress `model` that adds `requests`
  gets a child whose connections outside the model endpoint fail with
  `EACCES`.
- A child cell's policy (made by the agent with `spawn`, or by the host
  under the agent) has at most the parent's entries and a budget carved
  from the parent's module budget.
- In a snapshot or a fork the policy travels with the cell. A snapshot
  loaded from bytes gets its policy intersected with what its new parent
  allows (H-D13).

## D3a. What a module child gets: nothing by default

A module child (Data analysis, Coding tools, Browser, Documents, an MCP
tool cell) runs code a model wrote. It is the least trusted cell of the
tree, so by default it gets:

- an **empty module policy**: no entries and a budget of zero;
- a kernel endpoint with the `modules` right only, and **no `spawn`
  right**: every `add` it sends fails the whitelist check with `EACCES`,
  and the refusal is an audit record (H-D18).

**When a module needs another capability, it asks its agent**, over the
channel that already joins them, in whatever words its interface uses.
The agent decides. If it agrees, it adds the module itself, as **another
child of its own**: a sibling of the requester, carved from the agent's
own module budget. It may then pass the requester a handle to the new
sibling over their channel, attenuated with `derive` (H-D9): for
example a cell handle with the `message` right only, so the requester can
open a channel to its sibling but cannot observe, feed or control it.

```text
default: the module asks its agent      opt-in: the agent delegates
agent                                   agent
├── browser ─ "need documents" ─► agent └── coder   (delegated: python,
│      ◄─ cell/docs, message only                    numpy; 2 children)
└── docs     (added by the agent)           ├── python   (grandchild)
                                            └── numpy    (grandchild)
```

Why this default:

- the agent understands the whole task, and a module sees only its part;
- modules run model-generated code, so they should not decide on their
  own which further capabilities to create;
- budgets do not fragment: one module budget, held by the agent;
- the tree stays flat, and an operator reading `rhizo ps --tree` or the
  audit log sees every capability as a direct child of the agent that
  chose it.

**Opt-in delegation.** An `add` request may carry a `delegate` section, in
the policy structure of D3: entries and a budget. The policy of a root
agent is the operator's delegation to it; an agent delegates to a module
in the same way. The kernel checks, at every level:

- **entries**: each delegated entry matches one of the delegator's, with a
  version prefix at least as long, caps no higher, `ask` kept, and
  `egress = "granted"` only where the delegator's entry has it;
- **budget**: the slice is carved from what is left of the delegator's
  module budget, on top of the module's own limits, and given back when
  the module ends;
- **egress**: the module's grandchildren get at most the module's egress,
  which is at most the delegator's (D2);
- **approval**: sensitive adds of the module go to its approver, which is
  the delegator or the host; if the delegator's approver is the host, it
  stays the host, and a root's sensitive modules always go to the
  operator (D2);
- **depth**: the tree depth limit of 16 (H-D19) applies, with `EMLINK`.

A request past any of these is refused with its reason (`EACCES` for
entries and egress, `EDQUOT` for the budget) and nothing is made. A
delegation grants no `spawn` right: the module can add only modules,
which become the agent's grandchildren under the same rules.

Delegation is explicit and visible: the audit record of the add carries
the delegated entries and budget, and `rhizo ps --tree` marks a module
holding a delegated policy with its entries and budget. It is meant for
orchestrator-style agents that hand a sub-task, with its own budget, to a
module that plans on its own.

Alternatives:

- **Modules inherit the agent's whitelist by default.** Rejected: the
  least trusted cell would decide what to create, budgets would split
  across the tree without the agent knowing, and the tree would grow deep.
- **Modules ask the kernel, and the agent approves each add.** Rejected
  as the default: grandchildren would still deepen the tree and fragment
  the budget, and the agent would approve requests without having chosen
  the capability. With delegation the same thing is available when
  wanted.

## D4. The catalogue, the kernel's module table and signing

**Module files.** A module file is a snapshot file of `snapshots-on-disk`
(a template's exported snapshot with its CRC-32C) followed by a manifest,
with a detached signature next to it:

```text
numpy-2.1.3-py313-aarch64.rzmod       snapshot bytes || manifest || u32 manifest length
numpy-2.1.3-py313-aarch64.rzmod.minisig
```

The manifest (TOML, stored canonically: keys sorted, no comments) holds:

- `name`, `version` and `arch`;
- `base`, the name, version and BLAKE2b-512 digest of the base module's
  file;
- `interface` and `entry` (the program the child runs);
- `egress_request`, `min_limits` and `files_for` (for file-only modules);
- `lockfile_digest` and `tree_digest` (D5);
- `builder`, with the builder template's digest and the build's time.

**Signatures** use the minisign format: Ed25519 over the BLAKE2b-512 of the
whole file. Its trusted comment repeats `name@version` and the digest of
the base. Operators can therefore sign and check modules with the
`minisign` tool itself, and `rhizo` only has to verify.

**Keys stay on the host. Cells never hold keys, and neither does the
kernel.**

- The operator's secret key lives where they keep it: a minisign key file
  protected by a passphrase, or a hardware token through a signing
  command `rhizo` runs (D4a).
- `rhizo` holds only public keys (`trust/keys/*.pub`) and a signed
  revocation list (`trust/revoked.toml`), described in D4a.
- `rhizo module load FILE` verifies the signature against a trusted key,
  checks the revocation list, checks that the base module (if any) is
  already loaded with the digest the manifest names, and only then sends
  the bytes to the kernel (`load` as a template, protocol version 4), with
  the manifest's name, version and digest.
- The kernel registers the template in its **module table**. It still
  validates the bytes as untrusted (`Snapshot::from_bytes`): a signature
  says who approved a module, not that it is harmless.
- The kernel trusts the host's verification (the host is trusted, H-D0).
  Doing Ed25519 in the kernel would add a dependency and a surface for no
  gain against a trusted host.
- `--unsigned` (for development) loads a module marked unsigned. Only
  whitelist entries with `unsigned = true` match it, and `rhizo ps`
  shows it.

**Delta files.** A sub-template's file can refer to its base instead of
repeating it. A page record "same as base page N" (a new record kind,
snapshot format version 14) is honoured only when the base named by
digest is loaded. The loaded sub-template then maps the base's frames
copy-on-write: one copy of CPython for every Python module in memory and
on disk. A file with such records loaded without its base is refused,
and the records are honoured only against a published base (H-D21).

Module templates are held by the host's table and pinned. Copies in many
domains therefore never become sole owners of a shared frame (H-D12,
item 12), and the time to add a module does not depend on other domains.

Alternatives:

- **Signatures checked in the kernel.** Rejected for now: the host is
  trusted and already verifies.
- **Sigstore or GPG.** Rejected: sigstore needs network access and an
  identity provider, which offline laptops and air-gapped servers do not
  have. GPG is a large tool and a complex format. Minisign is small,
  Ed25519, and has an existing command-line tool.
- **Our own signature format.** Rejected: there is no reason to invent
  one when minisign fits.

## D4a. Key management

**Where things live.**

| What | Where | Who can write |
|---|---|---|
| Secret signing keys | Outside `rhizo` entirely. The default is a minisign secret key encrypted with a passphrase (scrypt, minisign's format) in `~/.config/rhizo/signing/<key-id>.key`, mode 0600, on the operator's machine, not on the servers that run VMs. Production can use a signing command instead (`signer = "cmd ..."` in `~/.config/rhizo/signing.toml`): an HSM, a cloud KMS or a hardware token, given the file's BLAKE2b-512 and returning the signature, so the key never exists as a file | The operator |
| Trusted public keys | `$RHIZO_TRUST/keys/<key-id>.pub` (`/etc/rhizo/trust` in a system install, `~/.rhizo/trust` otherwise), each with a sidecar `<key-id>.toml`: purpose (`modules` or `revocation`), `not_before`, `not_after`, owner | Root or the operator. `rhizo` refuses to use a trust directory or file that is group- or world-writable, or owned by another user, as ssh does |
| Revocation list | `$RHIZO_TRUST/revoked.toml`, with its signature `revoked.toml.minisig` | Written by `rhizo keys revoke`, signed by a key whose purpose is `revocation` |

No cell, no kernel and no builder ever sees a secret key or the signing
command.

**Generation.** `rhizo keys generate --purpose modules|revocation`
creates a minisign key pair. The secret key is written encrypted (it asks
for a passphrase twice) and the public key goes into the trust directory
with its sidecar. A production deployment keeps two kinds of key:

- one or more **module keys**, used often;
- one **revocation key**, kept offline and used only to sign revocation
  lists, so that a stolen module key can be revoked by a key the thief
  does not hold.

**Rotation.** `rhizo keys rotate OLD`:

1. generates a new module key and trusts it;
2. re-signs every catalogue file signed by `OLD` with the new key
   (`rhizo module resign`; the files themselves do not change);
3. sets `OLD`'s `not_after` to the end of a grace period (30 days by
   default).

A signature counts only if its key is trusted and the signature's
trusted comment carries a signing time within the key's validity. After
the grace period, files still signed only by `OLD` are refused, and
`rhizo module check` lists them in advance.

**Revocation.** `rhizo keys revoke` adds key identifiers (a stolen key)
or file digests (a bad module) to the list, with a reason and a date. It
increases the list's sequence number and signs the list with the
revocation key. Then:

- `rhizo` refuses a list whose signature does not verify, or whose
  sequence number is lower than the last one it saw (kept in
  `$RHIZO_STATE`), so an attacker cannot roll back to an older list;
- a revocation takes effect at once for new loads;
- `rhizo module check` reports modules already loaded in a running VM
  that are now revoked;
- `rhizo module unload NAME [--kill]` drops one from the kernel's table
  (no new adds) and optionally kills its running copies;
- every step is an audit record (H-D18).

**Several hosts.** The trust directory and the revocation list are plain
files. Configuration management distributes them. The signature and the
sequence number of the list make a stale or forged copy detectable.

**Loss of every module key.** Generate a new key, trust it, and rebuild or
re-sign the catalogue from the reviewed builds (the build records keep
the lockfile and the tree digest, D5). The revocation key revokes the
lost one.

**New host dependencies** (in `cli` only, never in the kernel):

- `ed25519-dalek` (BSD-3-Clause), to verify signatures;
- `blake2` (MIT or Apache-2.0), for the prehash.

Both are permissive and checked by `cargo deny`.

## D5. The builder cell, the only way into the catalogue

An agent may `pip install` inside its own cell if its egress allows the
package index. What it installs is private: copy-on-write in its own
files, gone when its tree ends, and never shared across domains or put in
the catalogue. To become a module for everyone, a package set goes through
a **builder cell**:

- **What it is.** A root cell in a domain of its own, a copy of a builder
  template (the base module plus pip and the build tools), made by the
  host with `rhizo module build`. It has no handles, no channels and no
  views from agents. Its only input is the build request (a lockfile and
  the base module), which the host writes into its files.
- **Egress: the package proxy only.** That is the host-defined rule set
  `index`, whose only entry is the address and port of Rhizo's package
  proxy (D5a) as the guest sees it, carved out of the deny floor (H-D7).
  The builder never reaches PyPI's real addresses, which are CDN addresses
  shared with unrelated sites. Its ceiling equals its egress.
- **Input: a lockfile with pinned versions and hashes**
  (`pip install --require-hashes --only-binary :all: --no-deps -r
  lock.txt`), as `xtask` already pins numpy's wheel by SHA-256
  (`add-pip-packages`). Source distributions are refused unless the
  lockfile allows them per package, and then they are built with the
  build dependencies the lockfile also pins.
- **Reproducibility.** The build fixes `SOURCE_DATE_EPOCH`, sets
  `PYTHONHASHSEED=0` and compiles with `compileall --invalidation-mode
  unchecked-hash`. The **installed tree's digest** (`tree_digest`: the
  BLAKE2b of the sorted paths, modes and contents) is recorded. `rhizo
  module build --twice` runs two independent builders and refuses to go
  on unless their tree digests are equal.
  - The frozen memory is not byte-reproducible (random seeds, `getrandom`
    and `AT_RANDOM` differ by design, spec `isolation`), so what is
    reproduced and attested is the file tree. The warm-up that produces
    the memory is a script named in the manifest and run from that tree.
- **No secret in the template.** The build token is the only secret a
  builder holds, and the proxy kills it when the install ends, before the
  warm-up and the freeze, so the template holds only a dead token. The
  builder connected through the network card, so its template can be
  published only with an attestation (H-D22): signing after the
  operator's review is that attestation, recorded in the audit log.
- **Freeze, review, sign.** Once it has installed and warmed up, the
  builder freezes itself as a template (`rhizo freeze`) and the host saves
  it (`rhizo save`, which needs no egress). `rhizo module review` then
  shows the operator:
  - the manifest;
  - the packages and their versions and hashes, from the lockfile;
  - the size;
  - the egress the module requests;
  - the difference from the base's file tree.

  After `rhizo module approve`, the host signs (D4) and copies the file to
  the catalogue directory. The builder's domain is killed. The builder
  never sees a key, and nothing in the catalogue is signed without an
  operator's approval.
- **An agent asks for a new module** by writing a lockfile. It cannot
  start a builder. Its request reaches the operator as data
  (`rhizo module request` shows it). Only the operator turns it into a
  build.

Alternative: let agents publish what they installed. Rejected: that is
exactly the path by which an agent's tree could put code in front of every
other agent.

## D5a. The package proxy (`rhizo proxy`)

The builder's only way out is a minimal proxy that Rhizo ships. It is a
subcommand of the `rhizo` binary, so there is nothing else to install.
Its whole job is to serve exactly the files a lockfile names, with
exactly the hashes it names, from exactly the configured index.

**Scope.**

- It speaks the read-only parts of the Python package index protocol:
  PEP 503 and PEP 691 "simple" pages (HTML and JSON), and file downloads.
  It serves nothing else: no uploads, no search, no other paths.
- **Allowlist from the lockfile.** For each build, the proxy loads that
  build's lockfile.
  - A simple page lists only the distributions the lockfile names, and
    only the files whose SHA-256 the lockfile pins.
  - Any other project, version or file gets `404`, and is logged.
- **Upstream.** Only the configured index and its file host (default
  `https://pypi.org/simple/` and `https://files.pythonhosted.org/`, or an
  internal mirror's URLs in `~/.config/rhizo/proxy.toml`).
  - TLS is verified against the system's roots, or a CA file the
    configuration names.
  - Redirects are followed only to those hosts.
  - The proxy never connects anywhere else, whatever a request says.
- **Hashes.** A file is streamed from upstream into a temporary file and
  its SHA-256 computed, then compared with the lockfile's pin before a
  single byte reaches the builder. A mismatch gives `502`, a log record,
  and the build's failure.
  - Verified files are kept in a content-addressed cache
    (`$RHIZO_STATE/proxy/cache/sha256/<hash>`), so later builds and
    air-gapped hosts can work from the cache alone (`--offline`).
- **Per-build access.**
  - `rhizo module build` starts the proxy, or uses one already running,
    and registers the build with a random token. The builder's index URL
    is `http://<proxy>/<token>/simple/`.
  - Requests without a live build's token get `403`.
  - A build's token dies when the install ends (the builder says so, or
    the host does at `freeze`), before the builder is frozen (D5).
- **Limits.**
  - Files: at most 1 GiB each (configurable).
  - Connections: at most 8 at once per build.
  - Builds: a total time limit per build.
  - Upstream fetches: retried at most twice.
- **Where it runs.** On the host that runs the VM, as the operator's user
  (or a dedicated `rhizo-proxy` user in a system install, with a systemd
  unit that `rhizo` installs).
  - It listens only on the address the guest reaches the host by: the tap
    device's host address on Firecracker and Cloud Hypervisor; on QEMU's
    user network, a host loopback port that the guest sees through the
    gateway. It never listens on all interfaces.
  - That address and port is the `index` exception in the deny floor,
    given only to builder cells. Agents can never reach it: their floor
    keeps it denied.
- **The link from the builder to the proxy is plain HTTP**, inside the
  host. Integrity comes from the pinned hashes, which the proxy checks
  and pip checks again (`--require-hashes`). Confidentiality is not
  needed for public packages.
- **Logging.** One JSON line per request, appended to
  `$RHIZO_STATE/proxy/proxy-<date>.jsonl`, with these fields:
  - time and build identifier;
  - client address;
  - method and path;
  - the decision (`served`, `denied`, `hash-mismatch`, `upstream-error`);
  - the upstream URL, SHA-256, bytes and duration.

  A summary of each build (files served, denials, mismatches) goes into
  the audit log of H-D18, so a module's review shows exactly what was
  fetched.

**Code.** About 600 lines in `cli/`, with `ureq` (MIT or Apache-2.0) and
`rustls` (Apache-2.0, ISC or MIT) as an HTTPS client, and `sha2` (MIT or
Apache-2.0). These are host dependencies only, justified in the change's
`deny.toml` entries. Host tests use a fake upstream and cover:

- a project outside the lockfile;
- a version outside it;
- a file with a wrong hash;
- a redirect to another host;
- a missing token;
- a file over the size limit.

Alternatives:

- **An existing mirror or proxy** (devpi, bandersnatch, a generic caching
  proxy). Rejected as the default: none restricts to one lockfile and
  checks pins before serving, and each is a large service to operate. An
  existing internal mirror can still be the proxy's upstream.
- **Downloading all wheels on the host before the build and giving the
  builder no network.** Kept as `--offline` from the cache. The online
  path stays, because resolving which files a lockfile's pins name
  needs the index's pages.

## D6. A small static chat agent

Today the chat template carries CPython: tens of MiB of interpreter for an
agent that mostly sends HTTP requests and prints text (`snapshot.file_bytes`
measures it). A new static chat agent, `cells/c/chat.c`, is linked
statically with glibc like the other C cells:

- it speaks the same OpenAI-compatible protocol;
- it reads a line from its input, sends the conversation, prints the
  answer and the timing line, exactly as `agent_chat.py` does in its
  transcript;
- it parses JSON with a small parser of its own (only the fields it
  needs);
- tools that need Python (the snippet tool) add the `python` module on
  first use, through `rhizo-mod`'s C client, and send the snippet over the
  `repl:python` interface;
- file tools (read, write, list) run in the agent itself.

`agent_chat.py` stays: it is the CPython workload and the existing
transcript. `rhizo chat` gains `--agent static|python`. **The static agent
becomes the default** in the same change that adds it, once it passes the
recorded transcript (the user's decision 7). `--agent python` keeps the
CPython chat agent. Both pass `--egress model` (H-D7).

Expected effect (to be measured, not promised):

- the chat template's memory and file size drop by roughly the size of
  CPython for conversations that never call a Python tool;
- the first snippet pays one `module.add_us`.

Alternative: a `no_std` Rust agent. Rejected for now: HTTP, JSON and
`getaddrinfo`-free sockets are simpler with glibc. The C cells are built
the same way already.

## D7. State, snapshots and fork

- The module policy, with its budget and what is in use, is cell state:
  snapshot format version 14 (with the delta page records of D4).
- A module child is an ordinary child cell. It follows `on_parent_end`
  (by default cascade: the agent's capabilities end with it).
- A fork of the agent by the host follows H-D13. The channels to its
  module children read as reset in the copy, and the copy has no
  children, so it adds its own modules again when it needs them. Each add
  is one warm template copy (`module.add_us`), and the copy's whitelist
  and budget are the agent's.
- A pending approval is not snapshotted. A suspended agent's pending
  request is refused, and the agent asks again after its resume.
- **Accelerators.** A module that needs an accelerator names the rights
  (`infer`, `compute`) and the accelerator budgets it needs in its
  manifest. Its child gets an accelerator handle only by derivation from
  one the agent holds, within the agent's budgets and its exposure
  ceiling (H-D24); an agent without one gets the module without it, or a
  refusal if the manifest requires it. The broker behind it is
  `add-accelerators`' design.

## D8. Tests and benchmarks

The hostile tests check every place where a capability could add
authority:

- **Add a module.** An agent adds `numpy`, sends a computation over the
  `repl:python` channel, and reads the answer.
- **Module children reseed.** Two agents in two domains each add
  `python` and evaluate `random.random()`; the answers differ. The
  builder's token is refused by the proxy once the builder is frozen.
- **Refusals.** A module not in the whitelist, one over the budget's
  children or memory, an unsigned module, and one whose request asks for
  egress beyond the agent's are each refused with their reason.
- **Module children ask their agent.** A module with the default policy
  sends `add` and is refused with `EACCES`; the agent then adds the
  module as a sibling and passes a message-only handle, with which the
  requester opens a channel but cannot observe or control its sibling.
- **Delegation.** A module given a delegated policy adds a grandchild
  within its slice. Delegations that name an entry the agent lacks, a
  budget above what is left, or egress above the agent's are refused,
  and so is an add past depth 16.
- **`requests` grants nothing.** An agent with egress `model` adds
  `requests` (approved). The child's connection to any host other than
  the model fails with `EACCES`.
- **Approvals.** A parent approver and a host approver both work: approve,
  deny, deadline, one pending request per cell. A root agent's `ask`
  module, and its module with `egress = "granted"`, both wait for the
  operator.
- **Signatures and keys.** `rhizo module load` refuses each of:
  - a changed byte;
  - an unknown key;
  - a key used outside its validity;
  - a revoked key or digest;
  - a revocation list with a lower sequence number or a bad signature;
  - a group-writable trust directory;
  - a delta file without its base.

  After `rhizo keys rotate`, the old key's files load until the end of
  the grace period, and not after.
- **Package proxy.** Each of these is refused and logged: a project
  outside the lockfile, a version outside it, a wrong hash, a redirect to
  another host, a missing or dead token, a file over the size limit. An
  agent cell cannot reach the proxy at all, because the floor denies it.
- **Builder.** It reaches only the package proxy (anything else gives
  `EACCES`). A hash mismatch in the lockfile fails the build. Two builds
  give equal tree digests. Its domain gets no channel from agents.
- **`pip install` stays private.** An agent installs into its own cell.
  Its sibling and another domain do not see it, and nothing of it remains
  after its tree ends.
- **Static chat.** The static chat agent passes the recorded chat
  transcript, adding `python` at the first snippet. `rhizo chat` with no
  `--agent` starts it.

Benchmarks:

- `module.add_us`: from the `add` request to the first reply from the
  module child, for `python` and for `numpy`, against
  `workload.cpython_warm_us` and `workload.numpy_warm_us`.
- `module.extra_bytes`: deterministic memory of each extra module child
  that has added nothing yet. It should be close to `cell.idle_bytes`
  plus the pages its warm-up left private, thanks to the sub-template
  copy-on-write.
- `module.file_bytes`: the size of `python+numpy` as a delta file against
  a full file.
- `chat.static_bytes`: the static chat template's memory, against the
  CPython chat template's.
- `chat.static_turn_us`, against `chat.turn_us`.
- None of the gated metrics of `add-cell-hierarchy` may regress: modules
  add no path to system calls, switches, forks or the network.
