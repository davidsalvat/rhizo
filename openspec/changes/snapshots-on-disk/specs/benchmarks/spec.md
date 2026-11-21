# Spec Delta

## ADDED Requirements

### Requirement: Snapshot file metrics
`cargo xtask bench` SHALL report on QEMU, with CPython built, for the chat
agent frozen into a template after one turn: the time to save it
(`snapshot.save_us`), its size (`snapshot.file_bytes`), the time to load it
as a template into a fresh VM (`snapshot.load_us`), and there the time
from a `fork` of it to its first answer (`snapshot.loaded_answer_us`) next
to that from a `run` of a new agent (`snapshot.cold_answer_us`).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm` with CPython built
- **THEN** the results contain the five snapshot file metrics for guest `rhizo` on `qemu-microvm`
