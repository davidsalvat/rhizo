# Spec Delta

## ADDED Requirements

### Requirement: Module policy in snapshots
A snapshot (format version 14) SHALL hold the cell's module policy and
what of its module budget is in use; a snapshot loaded from bytes SHALL
get its policy intersected with what its new parent allows. A pending
module request SHALL NOT be kept.

#### Scenario: Policy across suspend
- **WHEN** an agent with a module policy and one module child is suspended and resumed
- **THEN** it has the same policy, the child counts in its budget, and a request pending at the suspend was refused

### Requirement: Pages shared with a base module
A snapshot file SHALL be able to record a page as the same as a page of
its base module, named by digest; it SHALL load only when that base is
loaded and published with that digest, and its pages SHALL then map the
base's frames copy-on-write.

#### Scenario: Missing base
- **WHEN** a file with base page records is decoded and no module with its base digest is loaded
- **THEN** decoding fails naming the missing base
