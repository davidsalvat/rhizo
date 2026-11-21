# Spec Delta

## ADDED Requirements

### Requirement: No storage channel through accelerators
Nothing a cell observes through an accelerator SHALL depend on another
domain's use of it, apart from elapsed time on a shared hardware
partition: no shared device memory, no shared KV frames, no shared
engine prefix cache and no shared provider cache key across domains.

#### Scenario: Prefix probe across domains
- **WHEN** a cell of domain A sends a secret prompt, then a cell of domain B sends the same prompt and compares the engine's cache hit report and its own KV frame count with those of a fresh prompt
- **THEN** domain B's frame count and cache hit report are the same for both prompts
