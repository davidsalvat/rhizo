# Spec Delta

## ADDED Requirements

### Requirement: Snapshot checksum computed by the CPU
The kernel SHALL compute the CRC-32C of exported snapshots, and check it
when decoding, with the CPU's CRC instructions where the CPU has them
(AArch64 `CRC32C*` when `ID_AA64ISAR0_EL1` lists CRC32, x86_64 SSE4.2
`crc32` when `CPUID` lists SSE4.2), and with the portable table of
`rhizo-snapshot` otherwise. Both SHALL give the same checksum for any
bytes, so a file saved by one kernel loads in the other.

#### Scenario: Same checksum for any bytes
- **WHEN** the kernel's checksum and the portable table are computed over random bytes of every length from 0 to 63 and of about one and two pages, at every offset modulo 8
- **THEN** they are equal, and equal to the CRC-32C check value for `123456789`

#### Scenario: The function passed in is the one used
- **WHEN** a snapshot is exported with one checksum function and decoded with another that gives other values
- **THEN** decoding fails with a checksum error, and decoding with the first succeeds
