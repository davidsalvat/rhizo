## ADDED Requirements

### Requirement: In-kernel tests grouped by area
The in-kernel tests SHALL be grouped by area, one source file per area
under `kernel/src/tests/`, each listing its own tests in execution order.
The suite SHALL run the areas in one fixed order, each area's tests in its
order. Adding a test to an existing area SHALL change only that area's
file; only adding an area SHALL change the list of areas.

#### Scenario: A new test
- **WHEN** a change adds an in-kernel test to an existing area
- **THEN** the only test source file it changes is that area's file, and the test runs in `ktest` and `ktest-smp` at its place in the area's list

#### Scenario: Same tests after a regrouping
- **WHEN** tests are moved between files without changing any area's list or the order of areas
- **THEN** `cargo xtask test` runs the same tests, by the same names, in the same order
