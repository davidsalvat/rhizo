//! Boot options from the kernel command line.
//!
//! The VMM passes a command line through the PVH start info. Recognized
//! options (unknown ones are ignored):
//!
//! - `console=e9` (debug port 0xE9) or `console=serial` (default)
//! - `exit=debug-exit` (QEMU `isa-debug-exit`, default), `exit=i8042`
//!   (keyboard-controller reset, Firecracker) or `exit=ch-acpi`
//!   (Cloud Hypervisor ACPI shutdown device), `exit=psci` (AArch64 PSCI) or
//!   `exit=semihosting` (AArch64 under QEMU emulation)
//! - `ip=dhcp` or `ip=ADDR/PREFIX[,GATEWAY]`: the network card's address
//! - `echo=ADDR:PORT`: a TCP echo server the in-kernel tests may reach
//! - `virtio_mmio.device=SIZE@BASE:IRQ` (added by QEMU `microvm` and
//!   Firecracker on x86_64): read by [`virtio_mmio_devices`]
//! - `smp=N`: use at most `N` CPUs (`N` from 1 to 65535; default: every CPU
//!   the firmware describes)
//! - `bench=laptop` or `bench=server`: the reference workload a benchmark
//!   kernel runs instead of its cell figures (spec `benchmarks`), with
//!   `cells=N` (target cells, 1 to 65535), `secs=N` (seconds of load, 1 to
//!   65535) and `llm=ADDR:PORT` (the host's model server, fake or real)
//! - `shard=I/N`: an in-kernel test build runs only the tests whose position
//!   is `I` modulo `N` (`0 <= I < N`), so several VMs share the tests

use rhizo_hal::PhysicalMemory;

/// Where boot output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleKind {
    /// First 16550 serial port.
    Serial,
    /// Debug port 0xE9 (QEMU `-debugcon`, Cloud Hypervisor `--debug-console`).
    DebugPort,
}

/// How the kernel ends the virtual machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// QEMU `isa-debug-exit`: carries a success/failure status.
    DebugExit,
    /// Keyboard-controller reset; the VMM process ends.
    I8042Reset,
    /// Cloud Hypervisor ACPI shutdown device; the VMM process ends.
    CloudHypervisorAcpi,
    /// PSCI `SYSTEM_OFF` (AArch64); the VMM process ends.
    Psci,
    /// Semihosting `SYS_EXIT_EXTENDED` (AArch64 under QEMU emulation):
    /// carries a success/failure status.
    Semihosting,
}

/// How the network card gets its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpOption {
    Dhcp,
    Static {
        addr: [u8; 4],
        prefix: u8,
        gateway: Option<[u8; 4]>,
    },
}

/// A reference workload (spec `benchmarks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// Local agents on a laptop: few, mostly suspended, talking to a model.
    Laptop,
    /// Many active cells on a server: density and tail latencies.
    Server,
}

/// Parsed boot options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootOptions {
    /// Boot console.
    pub console: ConsoleKind,
    /// Exit mechanism.
    pub exit: ExitKind,
    /// The network card's address, if it is to have one.
    pub ip: Option<IpOption>,
    /// A TCP echo server for the in-kernel tests.
    pub echo: Option<([u8; 4], u16)>,
    /// Most CPUs to use (`smp=N`); `None`: all the firmware describes.
    pub smp: Option<u16>,
    /// The reference workload to run (`bench=`).
    pub bench: Option<Workload>,
    /// Target number of cells of a workload (`cells=N`).
    pub cells: Option<u16>,
    /// Seconds of load of a workload (`secs=N`).
    pub secs: Option<u16>,
    /// The host's fake model server (`llm=ADDR:PORT`).
    pub llm: Option<([u8; 4], u16)>,
    /// The share of the in-kernel tests to run (`shard=I/N`): `(I, N)`.
    pub shard: Option<(u16, u16)>,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            console: ConsoleKind::Serial,
            exit: ExitKind::DebugExit,
            ip: None,
            echo: None,
            smp: None,
            bench: None,
            cells: None,
            secs: None,
            llm: None,
            shard: None,
        }
    }
}

/// A dotted IPv4 address.
fn ipv4(s: &[u8]) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split(|&b| b == b'.');
    for o in &mut out {
        *o = number(parts.next()?)?.try_into().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// A decimal or `0x` hexadecimal number.
fn number(s: &[u8]) -> Option<u64> {
    let (digits, radix) = match s.strip_prefix(b"0x") {
        Some(hex) => (hex, 16),
        None => (s, 10),
    };
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |n, &d| {
        let d = (d as char).to_digit(radix)?;
        n.checked_mul(u64::from(radix))?.checked_add(u64::from(d))
    })
}

fn ip_option(value: &[u8]) -> Option<IpOption> {
    if value == b"dhcp" {
        return Some(IpOption::Dhcp);
    }
    let mut parts = value.splitn(2, |&b| b == b',');
    let cidr = parts.next()?;
    let gateway = match parts.next() {
        Some(g) => Some(ipv4(g)?),
        None => None,
    };
    let slash = cidr.iter().position(|&b| b == b'/')?;
    let prefix = number(&cidr[slash + 1..]).filter(|&p| p <= 32)? as u8;
    Some(IpOption::Static {
        addr: ipv4(&cidr[..slash])?,
        prefix,
        gateway,
    })
}

fn endpoint(value: &[u8]) -> Option<([u8; 4], u16)> {
    let colon = value.iter().position(|&b| b == b':')?;
    let port = number(&value[colon + 1..])?.try_into().ok()?;
    Some((ipv4(&value[..colon])?, port))
}

/// Calls `f(base, size)` for every `virtio_mmio.device=SIZE@BASE:IRQ` of
/// `cmdline` (`SIZE` in bytes, or with a `K` or `M` suffix).
pub fn virtio_mmio_devices(cmdline: &[u8], mut f: impl FnMut(u64, u64)) {
    for word in cmdline.split(|b| b.is_ascii_whitespace()) {
        let Some(value) = word.strip_prefix(b"virtio_mmio.device=") else {
            continue;
        };
        let Some(at) = value.iter().position(|&b| b == b'@') else {
            continue;
        };
        let (size, rest) = (&value[..at], &value[at + 1..]);
        let base = &rest[..rest.iter().position(|&b| b == b':').unwrap_or(rest.len())];
        let (size, unit) = match size.last() {
            Some(b'K' | b'k') => (&size[..size.len() - 1], 1 << 10),
            Some(b'M' | b'm') => (&size[..size.len() - 1], 1 << 20),
            _ => (size, 1),
        };
        if let (Some(size), Some(base)) = (number(size), number(base)) {
            f(base, size * unit);
        }
    }
}

impl BootOptions {
    /// Parses a command line; unknown or malformed options keep defaults.
    pub fn parse(cmdline: &[u8]) -> Self {
        let mut options = Self::default();
        for word in cmdline.split(|b| b.is_ascii_whitespace()) {
            match word {
                b"console=e9" => options.console = ConsoleKind::DebugPort,
                b"console=serial" => options.console = ConsoleKind::Serial,
                b"exit=debug-exit" => options.exit = ExitKind::DebugExit,
                b"exit=i8042" => options.exit = ExitKind::I8042Reset,
                b"exit=ch-acpi" => options.exit = ExitKind::CloudHypervisorAcpi,
                b"exit=psci" => options.exit = ExitKind::Psci,
                b"exit=semihosting" => options.exit = ExitKind::Semihosting,
                b"bench=laptop" => options.bench = Some(Workload::Laptop),
                b"bench=server" => options.bench = Some(Workload::Server),
                _ => {
                    if let Some(v) = word.strip_prefix(b"ip=") {
                        options.ip = ip_option(v).or(options.ip);
                    } else if let Some(v) = word.strip_prefix(b"echo=") {
                        options.echo = endpoint(v).or(options.echo);
                    } else if let Some(v) = word.strip_prefix(b"llm=") {
                        options.llm = endpoint(v).or(options.llm);
                    } else if let Some(n) = word.strip_prefix(b"smp=").and_then(parse_count) {
                        options.smp = Some(n);
                    } else if let Some(n) = word.strip_prefix(b"cells=").and_then(parse_count) {
                        options.cells = Some(n);
                    } else if let Some(n) = word.strip_prefix(b"secs=").and_then(parse_count) {
                        options.secs = Some(n);
                    } else if let Some(v) = word.strip_prefix(b"shard=") {
                        options.shard = shard(v).or(options.shard);
                    }
                }
            }
        }
        options
    }
}

/// `I/N` with `0 <= I < N <= 65535`.
fn shard(value: &[u8]) -> Option<(u16, u16)> {
    let slash = value.iter().position(|&b| b == b'/')?;
    let n = parse_count(&value[slash + 1..])?;
    let i = u16::try_from(number(&value[..slash])?).ok()?;
    (i < n).then_some((i, n))
}

/// A count from 1 to 65535 (with [`number`]'s syntax, which the other
/// numeric options share and keeps the boot image small).
fn parse_count(digits: &[u8]) -> Option<u16> {
    u16::try_from(number(digits)?).ok().filter(|&n| n > 0)
}

/// Offset of `cmdline_paddr` in `hvm_start_info`.
const CMDLINE_OFFSET: u64 = 24;

/// Copies the NUL-terminated command line referenced by `hvm_start_info` at
/// `start_info` into `buf` and returns the copied bytes (without the NUL).
/// Returns an empty slice when there is no command line.
pub fn read_cmdline<'b>(mem: &impl PhysicalMemory, start_info: u64, buf: &'b mut [u8]) -> &'b [u8] {
    let addr = mem.read_u64(start_info + CMDLINE_OFFSET);
    if addr == 0 {
        return &[];
    }
    let mut len = 0;
    while len < buf.len() {
        let at = addr + len as u64;
        let byte = (mem.read_u64(at & !7) >> ((at & 7) * 8)) as u8;
        if byte == 0 {
            break;
        }
        buf[len] = byte;
        len += 1;
    }
    &buf[..len]
}

/// The value of the last non-empty `selfcheck=NAME` of `cmdline` (which may
/// end in NUL padding): the harness self-check an in-kernel test build runs
/// instead of its tests (spec `kernel-testing`). Read apart from
/// [`BootOptions`], so builds without tests never parse it.
pub fn self_check(cmdline: &[u8]) -> Option<&[u8]> {
    cmdline
        .split(|&b| b.is_ascii_whitespace() || b == 0)
        .filter_map(|word| word.strip_prefix(b"selfcheck="))
        .rfind(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::FakePhysicalMemory;

    #[test]
    fn defaults_when_empty_or_unknown() {
        assert_eq!(BootOptions::parse(b""), BootOptions::default());
        assert_eq!(
            BootOptions::parse(b"quiet console=ttyS0 exit=bogus"),
            BootOptions::default()
        );
    }

    #[test]
    fn cpu_limit() {
        assert_eq!(BootOptions::parse(b"smp=4").smp, Some(4));
        assert_eq!(BootOptions::parse(b"shard=0/3").shard, Some((0, 3)));
        assert_eq!(
            BootOptions::parse(b"shard=2/3 console=e9").shard,
            Some((2, 3))
        );
        for bad in [
            &b"shard=3/3"[..],
            b"shard=1/0",
            b"shard=1",
            b"shard=/2",
            b"shard=a/2",
        ] {
            assert_eq!(BootOptions::parse(bad).shard, None, "{bad:?}");
        }
        assert_eq!(BootOptions::parse(b"console=e9 smp=1").smp, Some(1));
        assert_eq!(BootOptions::parse(b"smp=2 smp=3").smp, Some(3));
        for bad in [
            &b"smp=0"[..],
            b"smp=",
            b"smp=-1",
            b"smp=+2",
            b"smp=x",
            b"smp=65536",
        ] {
            assert_eq!(BootOptions::parse(bad).smp, None, "{bad:?}");
        }
    }

    #[test]
    fn self_check_names_one_check() {
        assert_eq!(self_check(b""), None);
        assert_eq!(self_check(b"console=e9 shard=0/2"), None);
        assert_eq!(self_check(b"selfcheck="), None);
        assert_eq!(self_check(b"xselfcheck=hang"), None);
        assert_eq!(
            self_check(b"console=e9 selfcheck=page-fault exit=psci"),
            Some(&b"page-fault"[..])
        );
        assert_eq!(
            self_check(b"selfcheck=hang selfcheck=oom selfcheck="),
            Some(&b"oom"[..])
        );
        assert_eq!(
            self_check(b"exit=psci selfcheck=hang\0\0\0"),
            Some(&b"hang"[..])
        );
        assert_eq!(BootOptions::parse(b"selfcheck=oom"), BootOptions::default());
    }

    #[test]
    fn recognizes_every_option() {
        let o = BootOptions::parse(b"console=e9  exit=i8042");
        assert_eq!(o.console, ConsoleKind::DebugPort);
        assert_eq!(o.exit, ExitKind::I8042Reset);
        assert_eq!(
            BootOptions::parse(b"exit=ch-acpi").exit,
            ExitKind::CloudHypervisorAcpi
        );
        assert_eq!(BootOptions::parse(b"exit=psci").exit, ExitKind::Psci);
        assert_eq!(
            BootOptions::parse(b"exit=semihosting").exit,
            ExitKind::Semihosting
        );
        assert_eq!(
            BootOptions::parse(b"console=e9 console=serial").console,
            ConsoleKind::Serial
        );
        assert_eq!(
            BootOptions::parse(b"exit=i8042 exit=debug-exit").exit,
            ExitKind::DebugExit
        );
    }

    #[test]
    fn network_options() {
        assert_eq!(BootOptions::parse(b"ip=dhcp").ip, Some(IpOption::Dhcp));
        assert_eq!(
            BootOptions::parse(b"ip=172.16.0.2/24,172.16.0.1").ip,
            Some(IpOption::Static {
                addr: [172, 16, 0, 2],
                prefix: 24,
                gateway: Some([172, 16, 0, 1])
            })
        );
        assert_eq!(
            BootOptions::parse(b"ip=10.0.0.2/8").ip,
            Some(IpOption::Static {
                addr: [10, 0, 0, 2],
                prefix: 8,
                gateway: None
            })
        );
        for bad in [
            &b"ip=10.0.0/8"[..],
            b"ip=10.0.0.2/33",
            b"ip=10.0.0.256/8",
            b"ip=x",
        ] {
            assert_eq!(BootOptions::parse(bad).ip, None, "{bad:?}");
        }
        assert_eq!(
            BootOptions::parse(b"echo=10.0.2.2:7777").echo,
            Some(([10, 0, 2, 2], 7777))
        );
        assert_eq!(BootOptions::parse(b"echo=10.0.2.2:70000").echo, None);
        let both = BootOptions::parse(b"echo=10.0.2.2:7 llm=172.16.0.1:8080");
        assert_eq!(both.echo, Some(([10, 0, 2, 2], 7)));
        assert_eq!(both.llm, Some(([172, 16, 0, 1], 8080)));
        assert_eq!(BootOptions::parse(b"llm=172.16.0.1").llm, None);
    }

    #[test]
    fn workload_options() {
        let o = BootOptions::parse(b"bench=server cells=10000 secs=3 llm=10.0.2.2:8080");
        assert_eq!(o.bench, Some(Workload::Server));
        assert_eq!((o.cells, o.secs), (Some(10000), Some(3)));
        assert_eq!(o.llm, Some(([10, 0, 2, 2], 8080)));
        assert_eq!(
            BootOptions::parse(b"bench=laptop").bench,
            Some(Workload::Laptop)
        );
        for bad in [
            &b"bench=other cells=0 secs=x llm=1.2.3:4"[..],
            b"cells=70000 secs=",
        ] {
            let o = BootOptions::parse(bad);
            assert_eq!(
                (o.bench, o.cells, o.secs, o.llm),
                (None, None, None, None),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn virtio_mmio_devices_from_the_command_line() {
        let mut found = Vec::new();
        virtio_mmio_devices(
            b"console=e9 virtio_mmio.device=512@0xfeb00e00:12 virtio_mmio.device=4K@0xd0000000:5 \
              virtio_mmio.device=bad",
            |base, size| found.push((base, size)),
        );
        assert_eq!(found, [(0xfeb0_0e00, 512), (0xd000_0000, 4096)]);
    }

    fn write_str(mem: &mut FakePhysicalMemory, addr: u64, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(1).enumerate() {
            let at = addr + i as u64;
            let word = mem.read_u64(at & !7);
            let shift = (at & 7) * 8;
            mem.write_u64(
                at & !7,
                (word & !(0xff << shift)) | ((chunk[0] as u64) << shift),
            );
        }
    }

    #[test]
    fn reads_unaligned_command_line() {
        let mut mem = FakePhysicalMemory::new();
        mem.write_u64(0x6000 + CMDLINE_OFFSET, 0x7003);
        write_str(&mut mem, 0x7003, b"console=e9 exit=i8042\0junk");
        let mut buf = [0u8; 64];
        assert_eq!(
            read_cmdline(&mem, 0x6000, &mut buf),
            b"console=e9 exit=i8042"
        );
    }

    #[test]
    fn missing_command_line_is_empty_and_long_ones_are_truncated() {
        let mut mem = FakePhysicalMemory::new();
        let mut buf = [0u8; 4];
        assert_eq!(read_cmdline(&mem, 0x6000, &mut buf), b"");
        mem.write_u64(0x6000 + CMDLINE_OFFSET, 0x7000);
        write_str(&mut mem, 0x7000, b"abcdefgh\0");
        assert_eq!(read_cmdline(&mem, 0x6000, &mut buf), b"abcd");
    }
}
