# Spec Delta

## ADDED Requirements

### Requirement: Fork and template metrics
`cargo xtask bench` SHALL report the time to fork a cell with 50 MiB of
memory, the page-table frames that fork allocates, the time to fork and
then write every page, and the time to spawn the `hello` program from a
template.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.fork_us`, `cell.fork_pt_frames`, `cell.fork_touch_us` and `cell.spawn_from_template_us`
