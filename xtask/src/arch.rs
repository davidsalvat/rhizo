//! Target architectures (see openspec/specs/architectures).

use crate::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A kernel architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// Primary architecture.
    Aarch64,
    /// Second architecture behind the same `arch/` boundary.
    X86_64,
}

static CURRENT: OnceLock<Arch> = OnceLock::new();

impl Arch {
    /// Parses `aarch64`/`arm64` or `x86_64`/`x86`.
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "aarch64" | "arm64" => Ok(Arch::Aarch64),
            "x86_64" | "x86" | "amd64" => Ok(Arch::X86_64),
            other => Err(format!(
                "unknown architecture `{other}` (aarch64 or x86_64)"
            )),
        }
    }

    /// Stable name.
    pub fn id(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
        }
    }

    /// Rust target the kernel is built for.
    pub fn target(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64-unknown-none-softfloat",
            Arch::X86_64 => "x86_64-unknown-none",
        }
    }

    /// QEMU system emulator for this architecture.
    pub fn qemu(self) -> String {
        std::env::var("RHIZO_QEMU").unwrap_or_else(|_| {
            match self {
                Arch::Aarch64 => "qemu-system-aarch64",
                Arch::X86_64 => "qemu-system-x86_64",
            }
            .into()
        })
    }

    /// Whether this host's CPU runs this architecture (so KVM can help).
    pub fn is_native(self) -> bool {
        std::env::consts::ARCH == self.id()
    }

    /// Turns the linked ELF into what the VMMs load: the ELF itself on x86_64
    /// (PVH), a flat arm64 Image on AArch64.
    pub fn bootable_image(self, elf: &Path) -> Result<PathBuf> {
        match self {
            Arch::X86_64 => Ok(elf.to_path_buf()),
            Arch::Aarch64 => {
                let data =
                    fs::read(elf).map_err(|e| format!("cannot read {}: {e}", elf.display()))?;
                let image = flatten(&data)?;
                let out = elf.with_extension("img");
                fs::write(&out, image)
                    .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
                Ok(out)
            }
        }
    }
}

/// The architecture selected with `--arch` (default: AArch64).
pub fn current() -> Arch {
    *CURRENT.get_or_init(|| Arch::Aarch64)
}

/// Selects the architecture for this run. Call once, before `current`.
pub fn select(arch: Arch) {
    let _ = CURRENT.set(arch);
}

/// Lays out the loadable segments of a 64-bit little-endian ELF as one flat
/// image starting at the lowest segment address (`.bss` is left out: the
/// image header declares its size and the kernel zeroes it).
pub fn flatten(elf: &[u8]) -> Result<Vec<u8>> {
    let u16_at = |o: usize| elf.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u64_at = |o: usize| {
        elf.get(o..o + 8)
            .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
    };
    if elf.get(..5) != Some(b"\x7fELF\x02") {
        return Err("not a 64-bit ELF file".into());
    }
    let bad = || "truncated ELF file".to_string();
    let phoff = u64_at(0x20).ok_or_else(bad)? as usize;
    let phentsize = u16_at(0x36).ok_or_else(bad)? as usize;
    let phnum = u16_at(0x38).ok_or_else(bad)? as usize;
    let mut segments = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let kind = elf.get(ph..ph + 4).ok_or_else(bad)?;
        if kind == [1, 0, 0, 0] {
            let offset = u64_at(ph + 8).ok_or_else(bad)? as usize;
            let vaddr = u64_at(ph + 16).ok_or_else(bad)?;
            let filesz = u64_at(ph + 32).ok_or_else(bad)? as usize;
            segments.push((vaddr, offset, filesz));
        }
    }
    let base = segments
        .iter()
        .map(|s| s.0)
        .min()
        .ok_or("ELF has no loadable segment")?;
    let end = segments
        .iter()
        .map(|s| s.0 + s.2 as u64)
        .max()
        .unwrap_or(base);
    let mut image = vec![0u8; (end - base) as usize];
    for (vaddr, offset, filesz) in segments {
        let at = (vaddr - base) as usize;
        let bytes = elf.get(offset..offset + filesz).ok_or_else(bad)?;
        image[at..at + filesz].copy_from_slice(bytes);
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64-bit ELF with two PT_LOAD segments at 0x1000 and 0x3000.
    fn elf() -> Vec<u8> {
        let mut e = vec![0u8; 0x200];
        e[..5].copy_from_slice(b"\x7fELF\x02");
        e[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        e[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        e[0x38..0x3a].copy_from_slice(&2u16.to_le_bytes());
        for (i, (vaddr, offset, data)) in [(0x1000u64, 0x100u64, b"AB"), (0x3000, 0x180, b"CD")]
            .into_iter()
            .enumerate()
        {
            let ph = 0x40 + i * 56;
            e[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
            e[ph + 8..ph + 16].copy_from_slice(&offset.to_le_bytes());
            e[ph + 16..ph + 24].copy_from_slice(&vaddr.to_le_bytes());
            e[ph + 32..ph + 40].copy_from_slice(&2u64.to_le_bytes());
            e[offset as usize..offset as usize + 2].copy_from_slice(data);
        }
        e
    }

    #[test]
    fn flatten_places_segments_by_address() {
        let image = flatten(&elf()).unwrap();
        assert_eq!(image.len(), 0x2002);
        assert_eq!(&image[..2], b"AB");
        assert_eq!(&image[0x2000..], b"CD");
        assert!(image[2..0x2000].iter().all(|&b| b == 0));
    }

    #[test]
    fn flatten_rejects_non_elf() {
        assert!(flatten(b"MZ...").is_err());
    }

    #[test]
    fn names() {
        assert_eq!(Arch::parse("arm64"), Ok(Arch::Aarch64));
        assert_eq!(Arch::parse("x86_64"), Ok(Arch::X86_64));
        assert!(Arch::parse("riscv64").is_err());
    }
}
