# Spec Delta

## ADDED Requirements

### Requirement: Files are forked copy-on-write
A forked cell SHALL start with its parent's files, descriptors and working
directory; afterwards each SHALL see only its own changes, and a fork SHALL
NOT copy file contents.

#### Scenario: Diverging files
- **WHEN** a parent and its child each write different contents to the same file after a fork
- **THEN** each reads its own contents

#### Scenario: Template files
- **WHEN** cells are spawned from a template that has files
- **THEN** each starts with those files
