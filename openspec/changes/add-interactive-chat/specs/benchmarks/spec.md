# Spec Delta

## ADDED Requirements

### Requirement: Interactive turn metric
`cargo xtask bench` SHALL report, on every VMM where the control kernel
runs and CPython is built, `chat.turn_us`: the Rhizo side of one
interactive turn of the chat agent against the host's fake model, from the
host's `input` request until the agent's answer is read back, less the
time the agent waited for the model.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm` with CPython built
- **THEN** the results contain `chat.turn_us` for guest `rhizo` on `qemu-microvm`
