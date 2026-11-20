//! The ACPI tables the kernel reads on x86_64 to find its CPUs: the RSDP,
//! the RSDT or XSDT, and the processor entries of the MADT (ACPI 6.5, §5.2).
//! Everything is read through [`PhysicalMemory`] and checked (signatures,
//! checksums, lengths); nothing is allocated.

use rhizo_hal::PhysicalMemory;

/// Why the processors could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// A table's checksum or length is wrong; its signature.
    Invalid([u8; 4]),
    /// The RSDT or XSDT lists no MADT.
    NoMadt,
}

/// Offset of `rsdp_paddr` in the PVH `hvm_start_info`.
const PVH_RSDP_OFFSET: u64 = 32;
/// Size of the standard table header.
const HEADER: u64 = 36;
/// Longest table read (a MADT for hundreds of CPUs is a few KiB).
const MAX_TABLE: u32 = 64 * 1024;

/// The RSDP address the PVH boot protocol passed in `hvm_start_info`
/// (0 when the VMM left it out).
pub fn pvh_rsdp(mem: &impl PhysicalMemory, start_info: u64) -> u64 {
    mem.read_u64(start_info + PVH_RSDP_OFFSET)
}

fn bytes<const N: usize>(mem: &impl PhysicalMemory, at: u64) -> [u8; N] {
    let mut out = [0; N];
    mem.read_bytes(at, &mut out);
    out
}

fn u32_at(mem: &impl PhysicalMemory, at: u64) -> u32 {
    u32::from_le_bytes(bytes(mem, at))
}

/// Whether the `len` bytes at `at` add up to zero (mod 256).
fn sums_to_zero(mem: &impl PhysicalMemory, at: u64, len: u64) -> bool {
    let mut sum = 0u8;
    let mut chunk = [0u8; 64];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(chunk.len() as u64) as usize;
        mem.read_bytes(at + done, &mut chunk[..n]);
        sum = chunk[..n].iter().fold(sum, |s, &b| s.wrapping_add(b));
        done += n as u64;
    }
    sum == 0
}

fn is_rsdp(mem: &impl PhysicalMemory, at: u64) -> bool {
    if bytes::<8>(mem, at) != *b"RSD PTR " || !sums_to_zero(mem, at, 20) {
        return false;
    }
    let revision = bytes::<1>(mem, at + 15)[0];
    revision < 2 || {
        let len = u32_at(mem, at + 20);
        (36..=MAX_TABLE).contains(&len) && sums_to_zero(mem, at, len as u64)
    }
}

/// Finds the RSDP: at `hint` (from the boot protocol) when it holds one,
/// else in the first KiB of the EBDA, else in `0xe0000..0x100000`, on 16-byte
/// boundaries.
pub fn find_rsdp(mem: &impl PhysicalMemory, hint: u64) -> Option<u64> {
    if hint != 0 && is_rsdp(mem, hint) {
        return Some(hint);
    }
    let ebda = (u16::from_le_bytes(bytes(mem, 0x40e)) as u64) << 4;
    let ebda = (0x8_0000..0xa_0000)
        .contains(&ebda)
        .then_some(ebda..ebda + 1024);
    ebda.into_iter()
        .flatten()
        .chain(0xe_0000..0x10_0000)
        .step_by(16)
        .find(|&at| is_rsdp(mem, at))
}

/// Checks the header of the table at `at`: signature `sig` (when given),
/// sane length and checksum. Returns its length.
fn table(mem: &impl PhysicalMemory, at: u64, sig: Option<[u8; 4]>) -> Result<u64, AcpiError> {
    let found: [u8; 4] = bytes(mem, at);
    if sig.is_some_and(|s| s != found) {
        return Err(AcpiError::Invalid(found));
    }
    let len = u32_at(mem, at + 4);
    if !(HEADER as u32..=MAX_TABLE).contains(&len) || !sums_to_zero(mem, at, len as u64) {
        return Err(AcpiError::Invalid(found));
    }
    Ok(len as u64)
}

/// The MADT listed by the RSDT or XSDT of the RSDP at `rsdp`.
fn madt(mem: &impl PhysicalMemory, rsdp: u64) -> Result<u64, AcpiError> {
    let revision = bytes::<1>(mem, rsdp + 15)[0];
    let (root, entry) = if revision >= 2 {
        (u64::from_le_bytes(bytes(mem, rsdp + 24)), 8)
    } else {
        (u32_at(mem, rsdp + 16) as u64, 4)
    };
    let sig = if entry == 8 { *b"XSDT" } else { *b"RSDT" };
    let len = table(mem, root, Some(sig))?;
    (root + HEADER..root + len)
        .step_by(entry)
        .take_while(|&at| at + entry as u64 <= root + len)
        .map(|at| match entry {
            8 => u64::from_le_bytes(bytes(mem, at)),
            _ => u32_at(mem, at) as u64,
        })
        .find(|&t| bytes::<4>(mem, t) == *b"APIC")
        .ok_or(AcpiError::NoMadt)
}

/// Calls `f` with the APIC identifier of every processor the MADT marks
/// enabled or online capable (local APIC and x2APIC entries), in table
/// order.
pub fn processors(
    mem: &impl PhysicalMemory,
    rsdp: u64,
    mut f: impl FnMut(u32),
) -> Result<(), AcpiError> {
    let madt = madt(mem, rsdp)?;
    let len = table(mem, madt, Some(*b"APIC"))?;
    // Header, local interrupt controller address and flags, then entries.
    let mut at = madt + HEADER + 8;
    while at + 2 <= madt + len {
        let [kind, size] = bytes::<2>(mem, at);
        if size < 2 || at + size as u64 > madt + len {
            return Err(AcpiError::Invalid(*b"APIC"));
        }
        let usable = |flags: u32| flags & 0b11 != 0;
        match (kind, size) {
            (0, 8..) if usable(u32_at(mem, at + 4)) => f(bytes::<1>(mem, at + 3)[0] as u32),
            (9, 16..) if usable(u32_at(mem, at + 8)) => f(u32_at(mem, at + 4)),
            _ => {}
        }
        at += size as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::FakePhysicalMemory;

    /// Writes `body` behind a table header with `sig` at `at`, checksummed.
    fn put_table(mem: &mut FakePhysicalMemory, at: u64, sig: &[u8; 4], body: &[u8]) {
        let mut t = Vec::new();
        t.extend(sig);
        t.extend(((HEADER as usize + body.len()) as u32).to_le_bytes());
        t.extend([1, 0]); // revision, checksum
        t.extend(*b"RHIZO RHIZOTBL"); // OEM and table identifiers
        t.extend([0; 12]); // revisions, creator
        t.extend(body);
        fix_checksum(&mut t, 9);
        mem.write_bytes(at, &t);
    }

    fn fix_checksum(t: &mut [u8], at: usize) {
        t[at] = 0;
        let sum = t.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        t[at] = 0u8.wrapping_sub(sum);
    }

    fn put_rsdp(mem: &mut FakePhysicalMemory, at: u64, revision: u8, root: u64) {
        let mut r = Vec::new();
        r.extend(*b"RSD PTR ");
        r.push(0); // checksum
        r.extend(*b"RHIZO ");
        r.push(revision);
        r.extend((root as u32).to_le_bytes());
        if revision >= 2 {
            r.extend(36u32.to_le_bytes());
            r.extend(root.to_le_bytes());
            r.extend([0; 4]); // extended checksum, reserved
        }
        fix_checksum(&mut r[..20], 8);
        if revision >= 2 {
            fix_checksum(&mut r, 32);
        }
        mem.write_bytes(at, &r);
    }

    /// A MADT with a disabled CPU 7, CPUs 0 and 2, an I/O APIC, an online
    /// capable x2APIC CPU 300 and a disabled x2APIC CPU 301.
    fn madt_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(0xfee0_0000u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        for (uid, id, flags) in [(0u8, 7u8, 0u32), (1, 0, 1), (2, 2, 1)] {
            b.extend([0, 8, uid, id]);
            b.extend(flags.to_le_bytes());
        }
        b.extend([1, 12, 0, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0]);
        for (id, flags) in [(300u32, 2u32), (301, 0)] {
            b.extend([9, 16, 0, 0]);
            b.extend(id.to_le_bytes());
            b.extend(flags.to_le_bytes());
            b.extend(id.to_le_bytes());
        }
        b
    }

    fn cpus(mem: &FakePhysicalMemory, rsdp: u64) -> Result<Vec<u32>, AcpiError> {
        let mut v = Vec::new();
        processors(mem, rsdp, |id| v.push(id)).map(|()| v)
    }

    #[test]
    fn reads_processors_through_the_xsdt() {
        let mut mem = FakePhysicalMemory::new();
        put_table(&mut mem, 0x7000, b"FACP", &[0; 8]);
        put_table(&mut mem, 0x8000, b"APIC", &madt_body());
        let mut xsdt = Vec::new();
        xsdt.extend(0x7000u64.to_le_bytes());
        xsdt.extend(0x8000u64.to_le_bytes());
        put_table(&mut mem, 0x6000, b"XSDT", &xsdt);
        put_rsdp(&mut mem, 0xf_0010, 2, 0x6000);
        assert_eq!(find_rsdp(&mem, 0), Some(0xf_0010), "found by scanning");
        assert_eq!(cpus(&mem, 0xf_0010), Ok(vec![0, 2, 300]));
    }

    #[test]
    fn reads_processors_through_the_rsdt_from_the_hint() {
        let mut mem = FakePhysicalMemory::new();
        put_table(&mut mem, 0x8000, b"APIC", &madt_body());
        put_table(&mut mem, 0x6000, b"RSDT", &0x8000u32.to_le_bytes());
        put_rsdp(&mut mem, 0x5000, 0, 0x6000);
        mem.write_u64(0x9000 + PVH_RSDP_OFFSET, 0x5000);
        let hint = pvh_rsdp(&mem, 0x9000);
        assert_eq!(find_rsdp(&mem, hint), Some(0x5000));
        assert_eq!(cpus(&mem, 0x5000), Ok(vec![0, 2, 300]));
    }

    #[test]
    fn finds_the_rsdp_in_the_ebda() {
        let mut mem = FakePhysicalMemory::new();
        mem.write_bytes(0x40e, &0x9fc0u16.to_le_bytes());
        put_rsdp(&mut mem, 0x9_fc20, 0, 0x6000);
        assert_eq!(find_rsdp(&mem, 0x1234), Some(0x9_fc20), "bad hint ignored");
    }

    #[test]
    fn rejects_bad_tables() {
        let mut mem = FakePhysicalMemory::new();
        assert_eq!(find_rsdp(&mem, 0), None);
        put_rsdp(&mut mem, 0xe_0000, 0, 0x6000);
        put_table(&mut mem, 0x6000, b"RSDT", &0x8000u32.to_le_bytes());
        put_table(&mut mem, 0x8000, b"FACP", &[0; 4]);
        assert_eq!(cpus(&mem, 0xe_0000), Err(AcpiError::NoMadt));
        put_table(&mut mem, 0x8000, b"APIC", &madt_body());
        mem.write_bytes(0x8000 + 50, &[0x55]); // breaks the checksum
        assert_eq!(cpus(&mem, 0xe_0000), Err(AcpiError::Invalid(*b"APIC")));
        // An entry that runs past the table.
        let mut body = madt_body();
        body.extend([0, 9, 0, 0]);
        put_table(&mut mem, 0x8000, b"APIC", &body);
        assert_eq!(cpus(&mem, 0xe_0000), Err(AcpiError::Invalid(*b"APIC")));
        // A corrupted RSDP is not found.
        mem.write_bytes(0xe_0000 + 9, &[0x77]);
        assert_eq!(find_rsdp(&mem, 0), None);
    }
}
