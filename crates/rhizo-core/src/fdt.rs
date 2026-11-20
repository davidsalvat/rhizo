//! Minimal flattened device tree (FDT / DTB) reader.
//!
//! Reads only what the kernel needs at boot: memory regions, the command line
//! (`/chosen/bootargs`), the PSCI conduit (`/psci/method`), the CPUs
//! (`/cpus`) and the first device with a given `compatible` string. No allocation; every query is a
//! walk over the structure block. Format: Devicetree Specification v0.4, §5.

const MAGIC: u32 = 0xd00d_feed;
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROP: u32 = 3;
const NOP: u32 = 4;
const END: u32 = 9;
/// Deepest node nesting the reader supports.
const MAX_DEPTH: usize = 16;

/// Why a device tree could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    /// The blob does not start with the FDT magic.
    BadMagic(u32),
    /// The header points outside the blob, or a token runs past its end.
    Truncated,
    /// Format version older than 16.
    UnsupportedVersion(u32),
    /// Nodes nested deeper than the reader supports.
    TooDeep,
}

/// A memory or device region: physical base and size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: u64,
    pub size: u64,
}

/// A CPU node of `/cpus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuNode {
    /// Its `reg`: the affinity fields of `MPIDR_EL1` on AArch64.
    pub id: u64,
    /// Whether PSCI starts it (`enable-method = "psci"`).
    pub psci: bool,
}

/// A parsed device tree.
#[derive(Debug, Clone, Copy)]
pub struct Fdt<'a> {
    structs: &'a [u8],
    strings: &'a [u8],
}

fn be32(data: &[u8], at: usize) -> Result<u32, FdtError> {
    let bytes = data.get(at..at + 4).ok_or(FdtError::Truncated)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Total size declared by the header at the start of `head` (at least 8
/// bytes), so a caller holding only a pointer can build the full slice.
pub fn total_size(head: &[u8]) -> Result<usize, FdtError> {
    let magic = be32(head, 0)?;
    if magic != MAGIC {
        return Err(FdtError::BadMagic(magic));
    }
    Ok(be32(head, 4)? as usize)
}

/// One step of a structure-block walk.
enum Event<'a> {
    Begin {
        depth: usize,
        name: &'a [u8],
    },
    Prop {
        depth: usize,
        name: &'a [u8],
        value: &'a [u8],
    },
    End {
        depth: usize,
    },
}

/// Reads `cells` big-endian 32-bit cells as one number.
fn read_cells(value: &[u8], cells: u32) -> Option<u64> {
    let bytes = value.get(..cells as usize * 4)?;
    Some(bytes.chunks(4).fold(0u64, |acc, c| {
        (acc << 32) | u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as u64
    }))
}

/// The first `(base, size)` pair of a `reg` property.
fn first_reg(value: &[u8], address_cells: u32, size_cells: u32) -> Option<Region> {
    nth_reg(value, address_cells, size_cells, 0)
}

fn nth_reg(value: &[u8], address_cells: u32, size_cells: u32, n: usize) -> Option<Region> {
    let value = value.get(n * (address_cells + size_cells) as usize * 4..)?;
    let base = read_cells(value, address_cells)?;
    let size = read_cells(value.get(address_cells as usize * 4..)?, size_cells)?;
    Some(Region { base, size })
}

/// Whether a string-list property (NUL-separated) contains `wanted`.
fn list_contains(value: &[u8], wanted: &[u8]) -> bool {
    value.split(|&b| b == 0).any(|s| s == wanted)
}

/// Strips the trailing NUL of a string property.
fn string(value: &[u8]) -> &[u8] {
    value.strip_suffix(&[0]).unwrap_or(value)
}

/// Per-depth cell sizes: (#address-cells, #size-cells) that apply to children.
#[derive(Clone, Copy)]
struct Cells {
    address: u32,
    size: u32,
}

const DEFAULT_CELLS: Cells = Cells {
    address: 2,
    size: 1,
};

impl<'a> Fdt<'a> {
    /// Validates the header of `data` and locates the structure and strings
    /// blocks.
    pub fn new(data: &'a [u8]) -> Result<Self, FdtError> {
        let total = total_size(data)?;
        let data = data.get(..total).ok_or(FdtError::Truncated)?;
        let version = be32(data, 20)?;
        if version < 16 {
            return Err(FdtError::UnsupportedVersion(version));
        }
        let off_struct = be32(data, 8)? as usize;
        let off_strings = be32(data, 12)? as usize;
        let size_strings = be32(data, 32)? as usize;
        let size_struct = be32(data, 36)? as usize;
        Ok(Self {
            structs: data
                .get(off_struct..off_struct + size_struct)
                .ok_or(FdtError::Truncated)?,
            strings: data
                .get(off_strings..off_strings + size_strings)
                .ok_or(FdtError::Truncated)?,
        })
    }

    fn walk(&self, mut f: impl FnMut(Event<'a>) -> bool) -> Result<(), FdtError> {
        let s = self.structs;
        let mut at = 0;
        let mut depth = 0;
        loop {
            let token = be32(s, at)?;
            at += 4;
            match token {
                BEGIN_NODE => {
                    let rest = s.get(at..).ok_or(FdtError::Truncated)?;
                    let len = rest
                        .iter()
                        .position(|&b| b == 0)
                        .ok_or(FdtError::Truncated)?;
                    let name = &rest[..len];
                    at += (len + 1).next_multiple_of(4);
                    if depth == MAX_DEPTH {
                        return Err(FdtError::TooDeep);
                    }
                    if !f(Event::Begin { depth, name }) {
                        return Ok(());
                    }
                    depth += 1;
                }
                END_NODE => {
                    depth = depth.checked_sub(1).ok_or(FdtError::Truncated)?;
                    if !f(Event::End { depth }) {
                        return Ok(());
                    }
                }
                PROP => {
                    let len = be32(s, at)? as usize;
                    let name_off = be32(s, at + 4)? as usize;
                    at += 8;
                    let value = s.get(at..at + len).ok_or(FdtError::Truncated)?;
                    at += len.next_multiple_of(4);
                    let names = self.strings.get(name_off..).ok_or(FdtError::Truncated)?;
                    let name_len = names
                        .iter()
                        .position(|&b| b == 0)
                        .ok_or(FdtError::Truncated)?;
                    let name = &names[..name_len];
                    if !f(Event::Prop { depth, name, value }) {
                        return Ok(());
                    }
                }
                NOP => {}
                END => return Ok(()),
                _ => return Err(FdtError::Truncated),
            }
        }
    }

    /// Value of property `prop` of the top-level node `node` (name without a
    /// unit address, e.g. `chosen`).
    fn top_level_prop(&self, node: &[u8], prop: &[u8]) -> Result<Option<&'a [u8]>, FdtError> {
        let mut inside = false;
        let mut found = None;
        self.walk(|event| {
            match event {
                Event::Begin { depth: 1, name } => {
                    inside = name.split(|&b| b == b'@').next() == Some(node);
                }
                Event::Prop {
                    depth: 2,
                    name,
                    value,
                } if inside && name == prop => {
                    found = Some(value);
                    return false;
                }
                Event::End { depth: 1 } => inside = false,
                _ => {}
            }
            true
        })?;
        Ok(found)
    }

    /// The kernel command line (`/chosen/bootargs`), without its NUL.
    pub fn bootargs(&self) -> Result<Option<&'a [u8]>, FdtError> {
        Ok(self.top_level_prop(b"chosen", b"bootargs")?.map(string))
    }

    /// The PSCI conduit (`/psci/method`): `hvc` or `smc`.
    pub fn psci_method(&self) -> Result<Option<&'a [u8]>, FdtError> {
        Ok(self.top_level_prop(b"psci", b"method")?.map(string))
    }

    /// Calls `f` with every available CPU of `/cpus`, in tree order (nodes
    /// with `device_type = "cpu"` and a `reg`, skipping `status` other than
    /// `okay`).
    pub fn cpus(&self, mut f: impl FnMut(CpuNode)) -> Result<(), FdtError> {
        let mut in_cpus = false;
        let mut address = DEFAULT_CELLS.address;
        // The CPU node being read: reg, psci, is a CPU, available.
        let mut node = (None, false, false, true);
        self.walk(|event| {
            match event {
                Event::Begin { depth: 1, name } => in_cpus = name == b"cpus",
                Event::End { depth: 1 } => in_cpus = false,
                Event::Prop {
                    depth: 2,
                    name: b"#address-cells",
                    value,
                } if in_cpus => address = read_cells(value, 1).unwrap_or(2) as u32,
                Event::Begin { depth: 2, .. } if in_cpus => node = (None, false, false, true),
                Event::Prop {
                    depth: 3,
                    name,
                    value,
                } if in_cpus => match name {
                    b"reg" => node.0 = read_cells(value, address),
                    b"enable-method" => node.1 = list_contains(value, b"psci"),
                    b"device_type" => node.2 = string(value) == b"cpu",
                    b"status" => node.3 = matches!(string(value), b"okay" | b"ok"),
                    _ => {}
                },
                Event::End { depth: 2 } if in_cpus => {
                    if let (Some(id), psci, true, true) = node {
                        f(CpuNode { id, psci });
                    }
                }
                _ => {}
            }
            true
        })
    }

    /// Calls `f` with every RAM region (`reg` of `memory` nodes).
    pub fn memory_regions(&self, mut f: impl FnMut(Region)) -> Result<(), FdtError> {
        let mut root = DEFAULT_CELLS;
        let mut in_memory = false;
        self.walk(|event| {
            match event {
                Event::Prop {
                    depth: 1,
                    name: b"#address-cells",
                    value,
                } => {
                    root.address = read_cells(value, 1).unwrap_or(2) as u32;
                }
                Event::Prop {
                    depth: 1,
                    name: b"#size-cells",
                    value,
                } => {
                    root.size = read_cells(value, 1).unwrap_or(1) as u32;
                }
                Event::Begin { depth: 1, name } => {
                    in_memory = name.split(|&b| b == b'@').next() == Some(b"memory");
                }
                Event::Prop {
                    depth: 2,
                    name: b"reg",
                    value,
                } if in_memory => {
                    let stride = (root.address + root.size) as usize * 4;
                    for entry in value.chunks_exact(stride.max(4)) {
                        if let Some(region) = first_reg(entry, root.address, root.size) {
                            f(region);
                        }
                    }
                }
                Event::End { depth: 1 } => in_memory = false,
                _ => {}
            }
            true
        })
    }

    /// The first `reg` region of the first node whose `compatible` list
    /// contains `compatible`, using its parent's cell sizes.
    pub fn find_compatible(&self, compatible: &[u8]) -> Result<Option<Region>, FdtError> {
        self.find_compatible_reg(compatible, 0)
    }

    /// Region `index` of the `reg` property of the first node whose
    /// `compatible` list contains `compatible` (for example the GICv3
    /// distributor, 0, and redistributors, 1).
    pub fn find_compatible_reg(
        &self,
        compatible: &[u8],
        index: usize,
    ) -> Result<Option<Region>, FdtError> {
        let mut cells = [DEFAULT_CELLS; MAX_DEPTH + 1];
        let mut matches = [false; MAX_DEPTH + 1];
        let mut regs: [Option<&'a [u8]>; MAX_DEPTH + 1] = [None; MAX_DEPTH + 1];
        let mut found = None;
        self.walk(|event| {
            match event {
                Event::Begin { depth, .. } => {
                    // A node's children use the cells it declares (defaults
                    // until then); its own `reg` uses its parent's.
                    cells[depth + 1] = DEFAULT_CELLS;
                    matches[depth + 1] = false;
                    regs[depth + 1] = None;
                }
                Event::Prop { depth, name, value } => match name {
                    b"#address-cells" => {
                        cells[depth].address = read_cells(value, 1).unwrap_or(2) as u32
                    }
                    b"#size-cells" => cells[depth].size = read_cells(value, 1).unwrap_or(1) as u32,
                    b"compatible" => matches[depth] = list_contains(value, compatible),
                    b"reg" => regs[depth] = Some(value),
                    _ => {}
                },
                Event::End { depth } => {
                    let node = depth + 1;
                    if matches[node]
                        && let Some(reg) = regs[node]
                    {
                        let parent = cells[depth];
                        found = nth_reg(reg, parent.address, parent.size, index);
                        return false;
                    }
                }
            }
            true
        })?;
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Device tree dumped from QEMU 8.2 `-M virt -m 128M` with
    /// `-append "console=serial exit=psci"` (compacted with dtc).
    const QEMU_VIRT: &[u8] = include_bytes!("../fixtures/qemu-virt.dtb");

    /// Same, with `-M virt,gic-version=3` (the GIC of Firecracker and Cloud
    /// Hypervisor too).
    const QEMU_VIRT_GICV3: &[u8] = include_bytes!("../fixtures/qemu-virt-gicv3.dtb");

    #[test]
    fn finds_both_gicv3_regions() {
        let fdt = Fdt::new(QEMU_VIRT_GICV3).unwrap();
        let reg = |i| fdt.find_compatible_reg(b"arm,gic-v3", i).unwrap();
        assert_eq!(
            reg(0),
            Some(Region {
                base: 0x800_0000,
                size: 0x1_0000
            })
        );
        assert_eq!(
            reg(1),
            Some(Region {
                base: 0x80a_0000,
                size: 0xf6_0000
            })
        );
        assert_eq!(reg(2), None);
    }

    /// `-M virt,gic-version=3 -smp 4`.
    const QEMU_VIRT_SMP4: &[u8] = include_bytes!("../fixtures/qemu-virt-smp4.dtb");

    #[test]
    fn lists_the_cpus() {
        let cpus = |blob| {
            let mut v = Vec::new();
            Fdt::new(blob).unwrap().cpus(|c| v.push(c)).unwrap();
            v
        };
        let psci = |id| CpuNode { id, psci: true };
        assert_eq!(cpus(QEMU_VIRT_SMP4), [psci(0), psci(1), psci(2), psci(3)]);
        assert_eq!(
            cpus(QEMU_VIRT_GICV3),
            [CpuNode { id: 0, psci: false }],
            "a single CPU has no enable method"
        );
        let (plain, with_cpus) = (handmade(false), handmade(true));
        assert_eq!(cpus(&plain), [], "no /cpus node");
        assert_eq!(
            cpus(&with_cpus),
            [
                CpuNode {
                    id: 0x100,
                    psci: false
                },
                psci(0x1_0000_0002)
            ]
        );
    }

    #[test]
    fn reads_qemu_virt() {
        let fdt = Fdt::new(QEMU_VIRT).unwrap();
        let mut memory = Vec::new();
        fdt.memory_regions(|r| memory.push(r)).unwrap();
        assert_eq!(
            memory,
            [Region {
                base: 0x4000_0000,
                size: 128 << 20
            }]
        );
        assert_eq!(
            fdt.bootargs().unwrap(),
            Some(&b"console=serial exit=psci"[..])
        );
        assert_eq!(fdt.psci_method().unwrap(), Some(&b"hvc"[..]));
        assert_eq!(
            fdt.find_compatible(b"arm,pl011").unwrap(),
            Some(Region {
                base: 0x0900_0000,
                size: 0x1000
            })
        );
        assert_eq!(fdt.find_compatible(b"ns16550a").unwrap(), None);
    }

    #[test]
    fn total_size_comes_from_the_header() {
        assert_eq!(total_size(QEMU_VIRT).unwrap(), QEMU_VIRT.len());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Fdt::new(&[0; 64]).unwrap_err(), FdtError::BadMagic(0));
        assert_eq!(
            Fdt::new(&QEMU_VIRT[..100]).unwrap_err(),
            FdtError::Truncated
        );
        let mut old = QEMU_VIRT.to_vec();
        old[20..24].copy_from_slice(&15u32.to_be_bytes());
        assert_eq!(
            Fdt::new(&old).unwrap_err(),
            FdtError::UnsupportedVersion(15)
        );
    }

    /// Builds a tiny device tree by hand: root with 1-cell addresses and
    /// sizes, two memory banks in one `reg`, a 16550 UART under `/soc`
    /// (2-cell addresses), no `/chosen`, and `/cpus` when `with_cpus`.
    fn handmade(with_cpus: bool) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut name = |s: &str| {
            let off = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            off
        };
        let (ac, sc, reg, compat, method) = (
            name("#address-cells"),
            name("#size-cells"),
            name("reg"),
            name("compatible"),
            name("method"),
        );
        let (device_type, enable, status) =
            (name("device_type"), name("enable-method"), name("status"));
        let mut s = Vec::new();
        let begin = |s: &mut Vec<u8>, n: &str| {
            s.extend(BEGIN_NODE.to_be_bytes());
            s.extend(n.as_bytes());
            s.push(0);
            while !s.len().is_multiple_of(4) {
                s.push(0);
            }
        };
        let prop = |s: &mut Vec<u8>, off: u32, v: &[u8]| {
            s.extend(PROP.to_be_bytes());
            s.extend((v.len() as u32).to_be_bytes());
            s.extend(off.to_be_bytes());
            s.extend(v);
            while !s.len().is_multiple_of(4) {
                s.push(0);
            }
        };
        let cells = |v: &[u32]| v.iter().flat_map(|c| c.to_be_bytes()).collect::<Vec<u8>>();
        begin(&mut s, "");
        prop(&mut s, ac, &cells(&[1]));
        prop(&mut s, sc, &cells(&[1]));
        begin(&mut s, "memory@80000000");
        prop(
            &mut s,
            reg,
            &cells(&[0x8000_0000, 0x1000_0000, 0xa000_0000, 0x0010_0000]),
        );
        s.extend(END_NODE.to_be_bytes());
        begin(&mut s, "soc");
        prop(&mut s, ac, &cells(&[2]));
        prop(&mut s, sc, &cells(&[2]));
        begin(&mut s, "uart@1000");
        prop(&mut s, compat, b"ns16550a\0");
        prop(&mut s, reg, &cells(&[0, 0x1000, 0, 0x100]));
        s.extend(END_NODE.to_be_bytes());
        s.extend(END_NODE.to_be_bytes());
        begin(&mut s, "psci");
        prop(&mut s, method, b"smc\0");
        s.extend(END_NODE.to_be_bytes());
        if with_cpus {
            // Two-cell CPU identifiers; one CPU without PSCI, one disabled,
            // and a node that is not a CPU.
            begin(&mut s, "cpus");
            prop(&mut s, ac, &cells(&[2]));
            prop(&mut s, sc, &cells(&[0]));
            for (n, id, psci, state) in [
                ("cpu@100", [0, 0x100], false, None),
                ("cpu@101", [0, 0x101], true, Some(&b"disabled\0"[..])),
                ("cpu@100000002", [1, 2], true, Some(b"okay\0")),
            ] {
                begin(&mut s, n);
                prop(&mut s, device_type, b"cpu\0");
                prop(&mut s, reg, &cells(&id));
                if psci {
                    prop(&mut s, enable, b"psci\0");
                }
                if let Some(state) = state {
                    prop(&mut s, status, state);
                }
                s.extend(END_NODE.to_be_bytes());
            }
            begin(&mut s, "cpu-map");
            s.extend(END_NODE.to_be_bytes());
            s.extend(END_NODE.to_be_bytes());
        }
        s.extend(END_NODE.to_be_bytes());
        s.extend(END.to_be_bytes());

        let header = 40;
        let mut blob = Vec::new();
        let total = header + 16 + s.len() + strings.len();
        for v in [
            MAGIC,
            total as u32,
            (header + 16) as u32,           // structure block
            (header + 16 + s.len()) as u32, // strings block
            header as u32,                  // memory reservation map
            17,
            16,
            0,
            strings.len() as u32,
            s.len() as u32,
        ] {
            blob.extend(v.to_be_bytes());
        }
        blob.extend([0u8; 16]); // empty reservation map
        blob.extend(&s);
        blob.extend(&strings);
        blob
    }

    #[test]
    fn honors_cell_sizes_and_multiple_banks() {
        let blob = handmade(true);
        let fdt = Fdt::new(&blob).unwrap();
        let mut memory = Vec::new();
        fdt.memory_regions(|r| memory.push(r)).unwrap();
        assert_eq!(
            memory,
            [
                Region {
                    base: 0x8000_0000,
                    size: 0x1000_0000
                },
                Region {
                    base: 0xa000_0000,
                    size: 0x10_0000
                },
            ]
        );
        assert_eq!(
            fdt.find_compatible(b"ns16550a").unwrap(),
            Some(Region {
                base: 0x1000,
                size: 0x100
            })
        );
        assert_eq!(fdt.psci_method().unwrap(), Some(&b"smc"[..]));
        assert_eq!(fdt.bootargs().unwrap(), None);
    }
}
