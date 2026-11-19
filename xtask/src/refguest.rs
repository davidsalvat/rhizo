//! The reference guest: a minimal Linux userspace that prints the ready
//! marker as soon as the kernel starts `/init` (design decision D8).
//!
//! The init program is a hand-assembled static Linux ELF (no libc, no
//! toolchain needed) for the selected architecture: `write(1, "rhizo-ready
//! linux\n")`, then wait forever.
//! It is packed with a `/dev/console` node into an uncompressed `newc` cpio
//! initramfs.

use crate::Result;
use crate::arch::Arch;
use std::fs;
use std::path::PathBuf;

const MESSAGE: &[u8] = b"rhizo-ready linux\n";
const LOAD_ADDR: u64 = 0x40_0000;
const HEADERS: usize = 64 + 56; // ELF header + one program header

/// AArch64 machine code: write the message, then spin (the host stops the VM).
fn code_aarch64() -> Vec<u8> {
    let insns: [u32; 6] = [
        0xd280_0808,                                 //  0: mov x8, #64 (write)
        0xd280_0020,                                 //  4: mov x0, #1 (stdout)
        0x1000_0081,                                 //  8: adr x1, #16 -> 24
        0xd280_0002 | ((MESSAGE.len() as u32) << 5), // 12: mov x2, #len
        0xd400_0001,                                 // 16: svc #0
        0x1400_0000,                                 // 20: b . (wait)
    ];
    let mut c: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();
    c.extend(MESSAGE); // 24: message
    c
}

/// x86_64 machine code; offsets are documented next to each instruction.
fn code_x86_64() -> Vec<u8> {
    let mut c = Vec::new();
    c.extend([0xb8, 1, 0, 0, 0]); //  0: mov eax, 1 (write)
    c.extend([0xbf, 1, 0, 0, 0]); //  5: mov edi, 1 (stdout)
    c.extend([0x48, 0x8d, 0x35, 16, 0, 0, 0]); // 10: lea rsi, [rip + 16] -> 33
    c.extend([0xba, MESSAGE.len() as u8, 0, 0, 0]); // 17: mov edx, len
    c.extend([0x0f, 0x05]); // 22: syscall
    c.extend([0xb8, 34, 0, 0, 0]); // 24: mov eax, 34 (pause)
    c.extend([0x0f, 0x05]); // 29: syscall
    c.extend([0xeb, 0xf7]); // 31: jmp 24
    c.extend(MESSAGE); // 33: message
    c
}

/// A static ELF executable for `arch` running the code above.
pub fn init_elf(arch: Arch) -> Vec<u8> {
    let (code, machine) = match arch {
        Arch::X86_64 => (code_x86_64(), 0x3eu16),
        Arch::Aarch64 => (code_aarch64(), 183u16),
    };
    let size = (HEADERS + code.len()) as u64;
    let entry = LOAD_ADDR + HEADERS as u64;
    let mut e = Vec::with_capacity(size as usize);
    // ELF header.
    e.extend(b"\x7fELF");
    e.extend([2, 1, 1, 0]); // 64-bit, little endian, version 1, System V
    e.extend([0; 8]);
    e.extend(2u16.to_le_bytes()); // ET_EXEC
    e.extend(machine.to_le_bytes()); // e_machine
    e.extend(1u32.to_le_bytes());
    e.extend(entry.to_le_bytes());
    e.extend(64u64.to_le_bytes()); // program headers offset
    e.extend(0u64.to_le_bytes()); // section headers offset
    e.extend(0u32.to_le_bytes()); // flags
    e.extend(64u16.to_le_bytes()); // ELF header size
    e.extend(56u16.to_le_bytes()); // program header size
    e.extend(1u16.to_le_bytes()); // program header count
    e.extend([0; 6]); // section header size, count, string index
    // PT_LOAD, R+X, the whole file at LOAD_ADDR.
    e.extend(1u32.to_le_bytes());
    e.extend(5u32.to_le_bytes());
    e.extend(0u64.to_le_bytes());
    e.extend(LOAD_ADDR.to_le_bytes());
    e.extend(LOAD_ADDR.to_le_bytes());
    e.extend(size.to_le_bytes());
    e.extend(size.to_le_bytes());
    e.extend(0x1000u64.to_le_bytes());
    e.extend(code);
    e
}

/// One `newc` cpio entry.
fn cpio_entry(out: &mut Vec<u8>, ino: u32, name: &str, mode: u32, rdev: (u32, u32), data: &[u8]) {
    let fields = [
        ino,
        mode,
        0,
        0,
        1,
        0,
        data.len() as u32,
        0,
        0,
        rdev.0,
        rdev.1,
        name.len() as u32 + 1,
        0,
    ];
    out.extend(b"070701");
    for field in fields {
        out.extend(format!("{field:08x}").as_bytes());
    }
    out.extend(name.as_bytes());
    out.push(0);
    pad4(out);
    out.extend(data);
    pad4(out);
}

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// An initramfs with `/dev/console` and `/init`.
pub fn initramfs() -> Vec<u8> {
    let mut out = Vec::new();
    cpio_entry(&mut out, 1, "dev", 0o040755, (0, 0), &[]);
    cpio_entry(&mut out, 2, "dev/console", 0o020600, (5, 1), &[]);
    cpio_entry(
        &mut out,
        3,
        "init",
        0o100755,
        (0, 0),
        &init_elf(crate::arch::current()),
    );
    cpio_entry(&mut out, 4, "TRAILER!!!", 0, (0, 0), &[]);
    out
}

/// Writes the initramfs into the tools directory and returns its path.
pub fn write_initramfs() -> Result<PathBuf> {
    let name = format!("reference-initramfs-{}.cpio", crate::arch::current().id());
    let path = crate::tools::dir()?.join(name);
    fs::write(&path, initramfs()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aarch64_init_addresses_its_message() {
        let elf = init_elf(Arch::Aarch64);
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 183, "EM_AARCH64");
        let code = &elf[HEADERS..];
        let adr = u32::from_le_bytes(code[8..12].try_into().unwrap());
        // ADR: imm = immhi:immlo; the target is the message at offset 24.
        let imm = (((adr >> 5) & 0x7ffff) << 2) | ((adr >> 29) & 0b11);
        assert_eq!(&code[8 + imm as usize..], MESSAGE);
        let mov_len = u32::from_le_bytes(code[12..16].try_into().unwrap());
        assert_eq!((mov_len >> 5) & 0xffff, MESSAGE.len() as u32);
    }

    #[test]
    fn init_is_a_well_formed_static_elf() {
        let elf = init_elf(Arch::X86_64);
        assert_eq!(&elf[..4], b"\x7fELF");
        let entry = u64::from_le_bytes(elf[24..32].try_into().unwrap());
        assert_eq!(entry, LOAD_ADDR + HEADERS as u64);
        let filesz = u64::from_le_bytes(elf[64 + 32..64 + 40].try_into().unwrap());
        assert_eq!(filesz as usize, elf.len());
        // The `lea` displacement must land exactly on the message.
        let code = &elf[HEADERS..];
        let lea_end = 17;
        let disp = i32::from_le_bytes(code[13..17].try_into().unwrap()) as usize;
        assert_eq!(&code[lea_end + disp..], MESSAGE);
        // The jump goes back to `mov eax, 34`.
        assert_eq!(code[31], 0xeb);
        assert_eq!((33i32 + code[32] as i8 as i32) as usize, 24);
    }

    #[test]
    fn initramfs_is_newc_and_aligned() {
        let cpio = initramfs();
        assert_eq!(&cpio[..6], b"070701");
        assert_eq!(cpio.len() % 4, 0);
        let text = String::from_utf8_lossy(&cpio);
        for name in ["dev\0", "dev/console\0", "init\0", "TRAILER!!!\0"] {
            assert!(text.contains(name), "missing {name}");
        }
    }
}
