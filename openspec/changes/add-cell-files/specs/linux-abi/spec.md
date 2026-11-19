# Spec Delta

## ADDED Requirements

### Requirement: Opening and closing files
The kernel SHALL implement `openat` (and `open` on x86_64) with `O_CREAT`,
`O_EXCL`, `O_TRUNC`, `O_APPEND` and `O_DIRECTORY`, and `close`, with Linux
semantics and error numbers.

#### Scenario: Missing file
- **WHEN** a cell opens a path that does not exist without `O_CREAT`
- **THEN** it receives `-ENOENT`

#### Scenario: Exclusive create
- **WHEN** a cell opens an existing path with `O_CREAT | O_EXCL`
- **THEN** it receives `-EEXIST`

### Requirement: File input and output
The kernel SHALL implement `read`, `write` and `writev` on files, `pread64`,
`pwrite64`, `lseek`, `ftruncate`, `fsync`, `dup`, `dup2`, `dup3` and
`fcntl` (`F_DUPFD`, `F_DUPFD_CLOEXEC`, `F_GETFD`, `F_SETFD`, `F_GETFL`,
`F_SETFL`) with Linux semantics.

#### Scenario: Shared offset
- **WHEN** a cell duplicates a descriptor, reads through one and then through the other
- **THEN** the second read continues where the first stopped

### Requirement: Names and metadata
The kernel SHALL implement `fstat` and `newfstatat` on files and directories
(and `stat`, `lstat` on x86_64), `faccessat`, `getdents64`, `mkdirat` (and
`mkdir`), `unlinkat` (and `unlink`, `rmdir`), `renameat`, `renameat2`
without flags (and `rename`), `getcwd` and `chdir`.

#### Scenario: Removing a non-empty directory
- **WHEN** a cell removes a directory that still has entries
- **THEN** it receives `-ENOTEMPTY`
