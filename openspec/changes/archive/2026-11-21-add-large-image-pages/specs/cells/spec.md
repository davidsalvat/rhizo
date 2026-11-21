# Spec Delta

## ADDED Requirements

### Requirement: Large pages for executables
When the kernel loads an executable, every 2 MiB-aligned 2 MiB piece that
lies wholly inside one segment SHALL be mapped with a 2 MiB page when a
2 MiB frame is free, with the segment's permissions and contents; the rest
of each segment SHALL be mapped with 4 KiB pages, and nothing outside the
segments SHALL be mapped.

#### Scenario: A segment with whole 2 MiB pieces
- **WHEN** an executable whose code segment covers two whole 2 MiB-aligned pieces is loaded
- **THEN** those pieces are backed by 2 MiB pages, the rest of the segment by 4 KiB pages, and every byte reads as in the file

#### Scenario: No 2 MiB frame free
- **WHEN** no 2 MiB frame is free while such an executable is loaded
- **THEN** it is loaded with 4 KiB pages only and runs the same
