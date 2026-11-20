---
name: openspec-change
description: Propose, implement and archive a Rhizo change with OpenSpec. Use for any new behavior, before writing code.
---

# Making a change with OpenSpec

1. Read the relevant current specs: `openspec list --specs`, then
   `openspec show <spec> --type spec`.
2. Create the change: `openspec new change <verb-noun-id>`.
3. Write, in this order, under `openspec/changes/<id>/`:
   - `proposal.md`: why, what changes, out of scope, capabilities, impact.
   - `specs/<capability>/spec.md`: `## ADDED|MODIFIED Requirements`, each
     requirement with `#### Scenario:` blocks using WHEN/THEN. MODIFIED blocks
     copy the whole existing requirement.
   - `design.md`: decisions with the alternatives that lost.
   - `tasks.md`: `- [ ] N.M task; verify <how>`.
4. `openspec validate <id> --strict` must pass.
5. Implement task by task. Tick a task only after its verification passed.
   Run `cargo xtask check` often and `cargo xtask ci` before pushing.
6. After merge: `openspec archive <id> --yes`, then validate all specs. If the
   change completes a milestone or adds a headline metric, the same PR updates
   the README from one benchmark run: `python3 docs/img/gen.py <results.json>`
   and `cargo xtask bench --from <results.json> --readme`, then check the
   changed images in light and dark themes.

Rules that apply to every change are in `AGENTS.md`.
