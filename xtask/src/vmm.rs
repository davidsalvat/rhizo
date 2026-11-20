//! Virtual machine monitors: launching a kernel, reading its console,
//! timing it from the host, and deciding how the run ended.

use crate::Result;
use crate::arch::{self, Arch};
use crate::tools;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// QEMU exit status produced by the kernel writing 0x10 to `isa-debug-exit`.
const QEMU_SUCCESS: i32 = (0x10 << 1) | 1;
/// QEMU exit status produced by the kernel writing 0x11 to `isa-debug-exit`.
const QEMU_FAILURE: i32 = (0x11 << 1) | 1;

/// Memory of a VM unless a run asks for more.
pub const DEFAULT_MEMORY_MIB: u32 = 128;

/// Line printed by the kernel (and the reference guest) when it is ready.
pub const READY_MARKER: &str = "rhizo-ready";

/// A supported virtual machine monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vmm {
    /// QEMU `virt` machine (AArch64 default).
    QemuVirt,
    /// QEMU `microvm` machine (x86_64 default).
    QemuMicrovm,
    /// QEMU `q35` machine (x86_64).
    QemuQ35,
    /// Firecracker.
    Firecracker,
    /// Cloud Hypervisor.
    CloudHypervisor,
}

impl Vmm {
    /// The VMMs of an architecture, in the order benchmarks report them.
    pub fn for_arch(arch: Arch) -> &'static [Vmm] {
        match arch {
            Arch::Aarch64 => &[Vmm::QemuVirt, Vmm::Firecracker, Vmm::CloudHypervisor],
            Arch::X86_64 => &[
                Vmm::QemuMicrovm,
                Vmm::QemuQ35,
                Vmm::Firecracker,
                Vmm::CloudHypervisor,
            ],
        }
    }

    /// The default VMM of an architecture.
    pub fn default_for(arch: Arch) -> Vmm {
        Vmm::for_arch(arch)[0]
    }

    /// Stable identifier used on the command line and in results.
    pub fn id(self) -> &'static str {
        match self {
            Vmm::QemuVirt => "qemu-virt",
            Vmm::QemuMicrovm => "qemu-microvm",
            Vmm::QemuQ35 => "qemu-q35",
            Vmm::Firecracker => "firecracker",
            Vmm::CloudHypervisor => "cloud-hypervisor",
        }
    }

    /// Parses an identifier (also accepts `virt`, `microvm` and `q35`).
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "qemu-virt" | "virt" => Ok(Vmm::QemuVirt),
            "qemu-microvm" | "microvm" => Ok(Vmm::QemuMicrovm),
            "qemu-q35" | "q35" => Ok(Vmm::QemuQ35),
            "qemu" => Ok(Vmm::default_for(arch::current())),
            "firecracker" | "fc" => Ok(Vmm::Firecracker),
            "cloud-hypervisor" | "ch" => Ok(Vmm::CloudHypervisor),
            other => Err(format!(
                "unknown VMM `{other}` (expected qemu-virt, qemu-microvm, qemu-q35, firecracker \
                 or cloud-hypervisor)"
            )),
        }
    }

    /// Whether the VMM needs KVM (QEMU can emulate instead).
    pub fn requires_kvm(self) -> bool {
        matches!(self, Vmm::Firecracker | Vmm::CloudHypervisor)
    }

    /// Whether the run's exit status carries the kernel's success/failure:
    /// QEMU's exit device on x86_64, semihosting under emulation on AArch64.
    fn has_exit_device(self, kvm: bool) -> bool {
        match self {
            Vmm::QemuMicrovm | Vmm::QemuQ35 => true,
            Vmm::QemuVirt => !kvm,
            _ => false,
        }
    }

    /// Command-line options telling the Rhizo kernel how to talk to this VMM.
    fn rhizo_cmdline(self, kvm: bool) -> &'static str {
        match self {
            Vmm::QemuMicrovm | Vmm::QemuQ35 => "console=e9 exit=debug-exit",
            Vmm::QemuVirt if kvm => "console=serial exit=psci",
            Vmm::QemuVirt => "console=serial exit=semihosting",
            Vmm::Firecracker if arch::current() == Arch::Aarch64 => "console=serial exit=psci",
            Vmm::Firecracker => "console=serial exit=i8042",
            Vmm::CloudHypervisor if arch::current() == Arch::Aarch64 => "console=serial exit=psci",
            Vmm::CloudHypervisor => "console=e9 exit=ch-acpi",
        }
    }

    /// Why this VMM cannot run here, if it cannot.
    pub fn unavailable_reason(self, accel: Accel) -> Option<String> {
        let arch = arch::current();
        if !Vmm::for_arch(arch).contains(&self) {
            return Some(format!("not a VMM for {}", arch.id()));
        }
        if self.requires_kvm() && !kvm_available() {
            return Some(format!(
                "needs KVM for {} guests (/dev/kvm unusable, or the host is not {})",
                arch.id(),
                arch.id()
            ));
        }
        if !self.requires_kvm() && accel == Accel::Kvm && !kvm_available() {
            return Some(
                "--accel kvm requested but KVM is not usable for this architecture".into(),
            );
        }
        None
    }
}

/// CPU acceleration for QEMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accel {
    /// KVM when usable, otherwise emulation.
    Auto,
    /// Require KVM.
    Kvm,
    /// Always emulate (TCG).
    Tcg,
    /// Emulate, counting instructions (TCG with `-icount shift=0`): the
    /// guest's clock advances one nanosecond per instruction, so a kernel
    /// timing itself measures retired instructions, the same on every run
    /// (spec `profiling`).
    Icount,
}

impl Accel {
    /// Parses `auto`, `kvm`, `tcg` or `icount`.
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "auto" => Ok(Accel::Auto),
            "kvm" => Ok(Accel::Kvm),
            "tcg" => Ok(Accel::Tcg),
            "icount" => Ok(Accel::Icount),
            other => Err(format!(
                "unknown accelerator `{other}` (expected auto, kvm, tcg or icount)"
            )),
        }
    }

    fn uses_kvm(self) -> bool {
        match self {
            Accel::Auto => kvm_available(),
            Accel::Kvm => true,
            Accel::Tcg | Accel::Icount => false,
        }
    }
}

/// Whether KVM can run guests of the selected architecture: `/dev/kvm` opens
/// for reading and writing, and the host CPU has that architecture.
pub fn kvm_available() -> bool {
    arch::current().is_native()
        && OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok()
}

/// What to boot.
#[derive(Debug, Clone)]
pub enum Guest {
    /// The Rhizo kernel image.
    Rhizo(PathBuf),
    /// The reference Linux guest: kernel image and initramfs.
    ReferenceLinux { kernel: PathBuf, initramfs: PathBuf },
}

/// How a VM run ended, as seen by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmResult {
    /// The kernel reported success.
    Passed,
    /// The kernel reported failure (exit status, panic or exception report).
    Failed,
    /// The VMM ended without a success or failure report.
    Abnormal(Option<i32>),
    /// The VM did not finish before the time limit and was killed.
    TimedOut,
    /// The run was stopped by the host right after the ready marker.
    StoppedAtReady,
}

/// Maps QEMU's process exit code to a [`VmResult`] (QEMU exit device).
pub fn interpret_exit(code: Option<i32>) -> VmResult {
    match code {
        Some(QEMU_SUCCESS) => VmResult::Passed,
        Some(QEMU_FAILURE) => VmResult::Failed,
        other => VmResult::Abnormal(other),
    }
}

/// Decides the result from the console for VMMs without an exit device.
pub fn interpret_console(output: &str, code: Option<i32>) -> VmResult {
    if output.contains("\nPANIC at") || output.contains("\nEXCEPTION:") {
        VmResult::Failed
    } else if output.contains("boot complete") || output.contains("test result: ok.") {
        VmResult::Passed
    } else {
        VmResult::Abnormal(code)
    }
}

/// Everything learned from one VM run.
#[derive(Debug)]
pub struct VmRun {
    /// How the run ended.
    pub result: VmResult,
    /// Console output.
    pub output: String,
    /// Host time from launching the VMM to receiving the ready marker.
    pub time_to_ready: Option<Duration>,
    /// Cycles reported in the ready marker (Rhizo only).
    pub ready_cycles: Option<u64>,
    /// The `rhizo-mark NAME` lines the guest printed, in order.
    pub marks: Vec<Mark>,
}

/// A `rhizo-mark NAME` line (design D6 of `add-reference-workloads`): when
/// the host read it, and the CPU time the VMM process had used by then.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub name: String,
    pub at: Duration,
    /// Nanoseconds every thread of the VMM process ran (`None` when
    /// `/proc` does not say).
    pub cpu_ns: Option<u64>,
}

/// Nanoseconds every thread of process `pid` has run, from the first field
/// of each `/proc/PID/task/TID/schedstat`.
pub fn process_cpu_ns(pid: u32) -> Option<u64> {
    let tasks = fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    let mut total = 0;
    for task in tasks.flatten() {
        let text = fs::read_to_string(task.path().join("schedstat")).ok()?;
        total += text.split_whitespace().next()?.parse::<u64>().ok()?;
    }
    Some(total)
}

/// A reference workload a `bench-cell` kernel runs (spec `benchmarks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bench {
    /// `bench=laptop secs=IDLE llm=HOST:PORT`: the model answers after
    /// `llm_delay_ms`.
    Laptop { idle_secs: u16, llm_delay_ms: u32 },
    /// `bench=server cells=N secs=S`.
    Server { cells: u16, secs: u16 },
}

impl Bench {
    /// The kernel options for this workload on `vmm`.
    fn cmdline(self, vmm: Vmm) -> String {
        match self {
            Bench::Laptop {
                idle_secs,
                llm_delay_ms,
            } => {
                let llm = crate::hostnet::llm_option(vmm, llm_delay_ms)
                    .map(|o| format!(" {o}"))
                    .unwrap_or_default();
                format!(" bench=laptop secs={idle_secs}{llm}")
            }
            Bench::Server { cells, secs } => format!(" bench=server cells={cells} secs={secs}"),
        }
    }
}

/// Options for one run.
#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    /// Kill the VM once the ready marker arrives (benchmarks).
    pub stop_at_ready: bool,
    /// Kill the VM after this long without console output: a hang, not a
    /// long run that keeps making progress.
    pub timeout: Duration,
    /// Echo console output to the terminal.
    pub echo: bool,
    /// QEMU acceleration.
    pub accel: Accel,
    /// Wait for gdb (QEMU only).
    pub gdb: bool,
    /// Give a Rhizo guest a network card and a host echo server where the
    /// VMM allows it (`hostnet`).
    pub network: bool,
    /// Attach QEMU `virt`'s card through virtio-pci instead of virtio-mmio
    /// (the other VMMs have a single transport: virtio-mmio on `microvm`
    /// and Firecracker, virtio-pci on `q35` and Cloud Hypervisor).
    pub pci_card: bool,
    /// Virtual CPUs of the VM.
    pub cpus: u32,
    /// Memory of the VM, in MiB.
    pub memory_mib: u32,
    /// A reference workload for a `bench-cell` Rhizo kernel.
    pub bench: Option<Bench>,
    /// The share of the in-kernel tests this VM runs (`shard=I/N`).
    pub shard: Option<(u16, u16)>,
}

/// The `shard=I/N` option for a Rhizo command line, if any.
fn shard_arg(opts: &RunOptions) -> String {
    opts.shard
        .map(|(i, n)| format!(" shard={i}/{n}"))
        .unwrap_or_default()
}

/// Parses `rhizo-ready cycles=<n>` from console output.
pub fn parse_ready_cycles(output: &str) -> Option<u64> {
    let line = output.lines().find(|l| l.starts_with(READY_MARKER))?;
    let value = line.split("cycles=").nth(1)?;
    value.split_whitespace().next()?.parse().ok()
}

/// Builds the command that launches `guest` on `vmm`.
fn command(vmm: Vmm, guest: &Guest, opts: &RunOptions, scratch: &Path) -> Result<Command> {
    match vmm {
        Vmm::QemuVirt | Vmm::QemuMicrovm | Vmm::QemuQ35 => Ok(qemu_command(vmm, guest, opts)),
        Vmm::Firecracker => firecracker_command(guest, opts, scratch),
        Vmm::CloudHypervisor => cloud_hypervisor_command(guest, opts),
    }
}

/// Reference guest command line for `vmm`. `earlyprintk` is needed on QEMU
/// `microvm`, where Linux otherwise registers no serial console and `/init`
/// output is lost. On AArch64 the console is the PL011 (`ttyAMA0`) except on
/// Firecracker, which emulates a 16550 (`ttyS0`).
pub(crate) fn linux_cmdline(vmm: Vmm) -> &'static str {
    match (arch::current(), vmm) {
        (Arch::X86_64, _) => {
            "console=ttyS0 earlyprintk=ttyS0 quiet loglevel=0 panic=-1 reboot=k pci=off \
             nomodule rdinit=/init"
        }
        (Arch::Aarch64, Vmm::Firecracker) => {
            "console=ttyS0 quiet loglevel=0 panic=-1 nomodule rdinit=/init"
        }
        (Arch::Aarch64, _) => "console=ttyAMA0 quiet loglevel=0 panic=-1 nomodule rdinit=/init",
    }
}

fn qemu_command(vmm: Vmm, guest: &Guest, opts: &RunOptions) -> Command {
    let kvm = opts.accel.uses_kvm();
    let mut cmd = Command::new(arch::current().qemu());
    // The microvm option ROM must stay enabled: it implements PVH boot for
    // -kernel. Each guest gets the smallest machine it needs: Rhizo needs no
    // legacy timers or interrupt controller; Linux needs them to calibrate its
    // clock when KVM's paravirtual clock is not available.
    match (vmm, guest) {
        // Without ACPI, `microvm` lists its virtio-mmio devices on the
        // kernel command line (`virtio_mmio.device=`), where Rhizo finds its
        // network card.
        (Vmm::QemuMicrovm, Guest::Rhizo(_)) if opts.network => cmd.args([
            "-M",
            "microvm,rtc=off,pit=off,pic=off,isa-serial=on,acpi=off",
        ]),
        (Vmm::QemuMicrovm, Guest::Rhizo(_)) => {
            cmd.args(["-M", "microvm,rtc=off,pit=off,pic=off,isa-serial=on"])
        }
        (Vmm::QemuMicrovm, Guest::ReferenceLinux { .. }) => {
            cmd.args(["-M", "microvm,isa-serial=on"])
        }
        // GICv3: the interrupt controller Firecracker and Cloud Hypervisor
        // expose, and the only one Rhizo drives.
        (Vmm::QemuVirt, Guest::Rhizo(_)) => cmd.args(["-M", "virt,gic-version=3"]),
        (Vmm::QemuVirt, _) => cmd.args(["-M", "virt"]),
        _ => cmd.args(["-M", "q35"]),
    };
    let cpu = if vmm == Vmm::QemuVirt && kvm {
        "host"
    } else {
        "max"
    };
    cmd.args(["-cpu", cpu, "-m", &format!("{}M", opts.memory_mib)])
        .args(["-nodefaults", "-no-user-config"])
        .args(["-smp", &opts.cpus.to_string()])
        .args(["-display", "none", "-no-reboot"])
        .args(["-accel", if kvm { "kvm" } else { "tcg" }]);
    if opts.accel == Accel::Icount {
        // Time jumps over idle periods instead of following the host's
        // clock, so nothing depends on the host.
        cmd.args(["-icount", "shift=0,sleep=off"]);
    }
    if vmm != Vmm::QemuVirt {
        cmd.args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
    } else if !kvm {
        // Semihosting gives the run an exit status under emulation.
        cmd.arg("-semihosting");
    }
    match guest {
        Guest::Rhizo(kernel) => {
            if vmm == Vmm::QemuVirt {
                cmd.args(["-serial", "stdio"]);
            } else {
                cmd.args(["-debugcon", "stdio", "-serial", "none"]);
            }
            let mut args = vmm.rhizo_cmdline(kvm).to_string();
            if opts.network
                && let crate::hostnet::Network::Qemu { cmdline } = crate::hostnet::for_vmm(vmm)
            {
                cmd.args(["-netdev", "user,id=n0"]);
                if vmm == Vmm::QemuQ35 || (vmm == Vmm::QemuVirt && opts.pci_card) {
                    // A modern-only virtio-pci card, without the boot ROM.
                    cmd.args([
                        "-device",
                        "virtio-net-pci,netdev=n0,disable-legacy=on,romfile=",
                    ]);
                } else {
                    // A modern virtio-mmio network card.
                    cmd.args(["-device", "virtio-net-device,netdev=n0"])
                        .args(["-global", "virtio-mmio.force-legacy=false"]);
                }
                args += &cmdline;
            }
            if let Some(bench) = opts.bench {
                args += &bench.cmdline(vmm);
            }
            args += &shard_arg(opts);
            cmd.arg("-kernel").arg(kernel).args(["-append", &args]);
        }
        Guest::ReferenceLinux { kernel, initramfs } => {
            cmd.args(["-serial", "stdio"])
                .arg("-kernel")
                .arg(kernel)
                .arg("-initrd")
                .arg(initramfs)
                .args(["-append", linux_cmdline(vmm)]);
        }
    }
    if opts.gdb {
        cmd.args(["-s", "-S"]);
    }
    cmd
}

fn firecracker_command(guest: &Guest, opts: &RunOptions, scratch: &Path) -> Result<Command> {
    let binary = tools::firecracker()?;
    let (kernel, initrd, mut args) = match guest {
        Guest::Rhizo(kernel) => (
            kernel.clone(),
            None,
            Vmm::Firecracker.rhizo_cmdline(true).to_string(),
        ),
        Guest::ReferenceLinux { kernel, initramfs } => (
            kernel.clone(),
            Some(initramfs.clone()),
            linux_cmdline(Vmm::Firecracker).to_string(),
        ),
    };
    let mut interfaces = String::new();
    if opts.network
        && matches!(guest, Guest::Rhizo(_))
        && let crate::hostnet::Network::Tap { tap, cmdline } =
            crate::hostnet::for_vmm(Vmm::Firecracker)
    {
        interfaces = format!(
            "{{\"iface_id\": \"net0\", \"host_dev_name\": {tap:?}, \"guest_mac\": \"06:00:ac:10:00:02\"}}"
        );
        args += &cmdline;
    }
    if let (Some(bench), Guest::Rhizo(_)) = (opts.bench, guest) {
        args += &bench.cmdline(Vmm::Firecracker);
    }
    if matches!(guest, Guest::Rhizo(_)) {
        args += &shard_arg(opts);
    }
    let initrd_json = match &initrd {
        Some(path) => format!(", \"initrd_path\": {:?}", path.display().to_string()),
        None => String::new(),
    };
    let config = format!(
        "{{\"boot-source\": {{\"kernel_image_path\": {:?}, \"boot_args\": {:?}{initrd_json}}}, \
         \"drives\": [], \"network-interfaces\": [{interfaces}], \
         \"machine-config\": {{\"vcpu_count\": {}, \"mem_size_mib\": {}}}}}",
        kernel.display().to_string(),
        args,
        opts.cpus,
        opts.memory_mib
    );
    let path = scratch.join(format!("firecracker-{}.json", std::process::id()));
    fs::write(&path, config).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut cmd = Command::new(binary);
    cmd.arg("--no-api")
        .arg("--config-file")
        .arg(&path)
        .args(["--level", "Error"]);
    Ok(cmd)
}

fn cloud_hypervisor_command(guest: &Guest, opts: &RunOptions) -> Result<Command> {
    let binary = tools::cloud_hypervisor()?;
    let mut cmd = Command::new(binary);
    cmd.args(["--cpus", &format!("boot={}", opts.cpus)])
        .args(["--memory", &format!("size={}M", opts.memory_mib)])
        .args(["--console", "off"]);
    match guest {
        Guest::Rhizo(kernel) => {
            let mut args = Vmm::CloudHypervisor.rhizo_cmdline(true).to_string();
            if opts.network
                && let crate::hostnet::Network::Tap { tap, cmdline } =
                    crate::hostnet::for_vmm(Vmm::CloudHypervisor)
            {
                // A virtio-pci card on the tap device.
                cmd.args(["--net", &format!("tap={tap},mac=06:00:ac:10:01:02")]);
                args += &cmdline;
            }
            if let Some(bench) = opts.bench {
                args += &bench.cmdline(Vmm::CloudHypervisor);
            }
            args += &shard_arg(opts);
            cmd.arg("--kernel").arg(kernel).args(["--cmdline", &args]);
            if arch::current() == Arch::X86_64 {
                cmd.args(["--serial", "off", "--debug-console", "tty"]);
            } else {
                cmd.args(["--serial", "tty"]);
            }
        }
        Guest::ReferenceLinux { kernel, initramfs } => {
            cmd.arg("--kernel")
                .arg(kernel)
                .arg("--initramfs")
                .arg(initramfs)
                .args(["--cmdline", linux_cmdline(Vmm::CloudHypervisor)])
                .args(["--serial", "tty"]);
        }
    }
    Ok(cmd)
}

/// Boots `guest` on `vmm`, streams and captures its console, and measures
/// the time from launching the VMM to the ready marker.
pub fn run(vmm: Vmm, guest: &Guest, opts: RunOptions) -> Result<VmRun> {
    if let Some(reason) = vmm.unavailable_reason(opts.accel) {
        return Err(format!("{} is not available: {reason}", vmm.id()));
    }
    let scratch = tools::dir()?;
    let mut cmd = command(vmm, guest, &opts, &scratch)?;
    cmd.stdin(if opts.gdb {
        Stdio::inherit()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    if std::env::var_os("RHIZO_PRINT_VMM_COMMAND").is_some() {
        eprintln!("[xtask] {cmd:?}");
    }
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {} (is it installed?): {e}", vmm.id()))?;

    let pid = child.id();
    let marks = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::new(Mutex::new(Vec::new()));
    // Milliseconds since `started` of the last console output.
    let last_output = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ready_at: Arc<Mutex<Option<Duration>>> = Arc::new(Mutex::new(None));
    let ready_flag = Arc::new(AtomicBool::new(false));

    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = {
        let captured = Arc::clone(&captured);
        let ready_at = Arc::clone(&ready_at);
        let ready_flag = Arc::clone(&ready_flag);
        let last_output = Arc::clone(&last_output);
        let marks = Arc::clone(&marks);
        let echo = opts.echo;
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // Where the first line not yet looked at for a mark starts.
            let mut line_start = 0;
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let now = started.elapsed();
                last_output.store(now.as_millis() as u64, Ordering::Relaxed);
                let chunk = &buf[..n];
                if echo {
                    crate::report::raw(chunk);
                }
                let mut all = captured.lock().expect("capture lock");
                let search_from = all.len().saturating_sub(READY_MARKER.len());
                all.extend_from_slice(chunk);
                while let Some(end) = all[line_start..].iter().position(|&b| b == b'\n') {
                    let line = &all[line_start..line_start + end];
                    if let Some(name) = line.strip_prefix(b"rhizo-mark ") {
                        marks.lock().expect("marks lock").push(Mark {
                            name: String::from_utf8_lossy(name).trim().to_string(),
                            at: now,
                            cpu_ns: process_cpu_ns(pid),
                        });
                    }
                    line_start += end + 1;
                }
                if !ready_flag.load(Ordering::Relaxed)
                    && contains(&all[search_from..], READY_MARKER.as_bytes())
                {
                    *ready_at.lock().expect("ready lock") = Some(now);
                    ready_flag.store(true, Ordering::Release);
                }
            }
        })
    };
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let result = loop {
        if opts.stop_at_ready && ready_flag.load(Ordering::Acquire) {
            // Give the line a moment to complete, then stop the VM.
            thread::sleep(Duration::from_millis(2));
            let _ = child.kill();
            let _ = child.wait();
            break VmResult::StoppedAtReady;
        }
        match child.try_wait() {
            Ok(Some(status)) => break finish(vmm, opts.accel.uses_kvm(), status.code(), &captured),
            Ok(None)
                if started.elapsed()
                    >= Duration::from_millis(last_output.load(Ordering::Relaxed))
                        + opts.timeout =>
            {
                let _ = child.kill();
                let _ = child.wait();
                break VmResult::TimedOut;
            }
            Ok(None) => thread::sleep(Duration::from_millis(1)),
            Err(e) => return Err(format!("failed to wait for {}: {e}", vmm.id())),
        }
    };
    let _ = reader.join();
    let stderr_text = stderr_reader.join().unwrap_or_default();

    let output = String::from_utf8_lossy(&captured.lock().expect("capture lock")).into_owned();
    if result == VmResult::TimedOut && opts.echo {
        crate::report::text(&format!(
            "\n[xtask] VM killed after {}s without output",
            opts.timeout.as_secs()
        ));
    }
    if matches!(result, VmResult::Abnormal(_) | VmResult::TimedOut)
        && !stderr_text.trim().is_empty()
    {
        crate::report::text(&format!(
            "[xtask] {} stderr:\n{}",
            vmm.id(),
            stderr_text.trim_end()
        ));
    }
    let time_to_ready = *ready_at.lock().expect("ready lock");
    Ok(VmRun {
        result: match result {
            // A VMM without an exit device that we stopped is judged by output.
            VmResult::StoppedAtReady => VmResult::StoppedAtReady,
            other => other,
        },
        ready_cycles: parse_ready_cycles(&output),
        time_to_ready,
        output,
        marks: std::mem::take(&mut *marks.lock().expect("marks lock")),
    })
}

fn finish(vmm: Vmm, kvm: bool, code: Option<i32>, captured: &Mutex<Vec<u8>>) -> VmResult {
    if vmm.has_exit_device(kvm) {
        interpret_exit(code)
    } else {
        let output = String::from_utf8_lossy(&captured.lock().expect("capture lock")).into_owned();
        interpret_console(&output, code)
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_map_to_results() {
        assert_eq!(interpret_exit(Some(33)), VmResult::Passed);
        assert_eq!(interpret_exit(Some(35)), VmResult::Failed);
        assert_eq!(interpret_exit(Some(0)), VmResult::Abnormal(Some(0)));
        assert_eq!(interpret_exit(None), VmResult::Abnormal(None));
    }

    #[test]
    fn console_decides_for_vmms_without_exit_device() {
        assert_eq!(
            interpret_console("x\nboot complete\n", Some(0)),
            VmResult::Passed
        );
        assert_eq!(
            interpret_console("test result: ok. 3 passed", Some(0)),
            VmResult::Passed
        );
        assert_eq!(
            interpret_console("boot complete\nPANIC at a.rs:1:1: x", Some(0)),
            VmResult::Failed
        );
        assert_eq!(
            interpret_console("\nEXCEPTION: page fault", None),
            VmResult::Failed
        );
        assert_eq!(
            interpret_console("Rhizo 0.1.0\n", Some(1)),
            VmResult::Abnormal(Some(1))
        );
    }

    #[test]
    fn ready_cycles_are_parsed() {
        assert_eq!(
            parse_ready_cycles("Rhizo\nrhizo-ready cycles=1234\nboot"),
            Some(1234)
        );
        assert_eq!(parse_ready_cycles("rhizo-ready linux\n"), None);
        assert_eq!(parse_ready_cycles("nothing"), None);
    }

    #[test]
    fn vmm_names_round_trip() {
        for arch in [Arch::Aarch64, Arch::X86_64] {
            for &vmm in Vmm::for_arch(arch) {
                assert_eq!(Vmm::parse(vmm.id()), Ok(vmm));
            }
        }
        assert_eq!(Vmm::parse("q35"), Ok(Vmm::QemuQ35));
        assert_eq!(Vmm::parse("virt"), Ok(Vmm::QemuVirt));
        assert!(Vmm::parse("xen").is_err());
        assert_eq!(Vmm::default_for(Arch::Aarch64), Vmm::QemuVirt);
        assert_eq!(Vmm::default_for(Arch::X86_64), Vmm::QemuMicrovm);
    }

    #[test]
    fn rhizo_cmdlines_pick_console_and_exit() {
        assert_eq!(
            Vmm::QemuMicrovm.rhizo_cmdline(false),
            "console=e9 exit=debug-exit"
        );
        assert_eq!(
            Vmm::QemuVirt.rhizo_cmdline(false),
            "console=serial exit=semihosting"
        );
        assert_eq!(
            Vmm::QemuVirt.rhizo_cmdline(true),
            "console=serial exit=psci"
        );
        assert!(
            Vmm::Firecracker
                .rhizo_cmdline(true)
                .contains("console=serial")
        );
    }

    #[test]
    fn exit_status_carriers() {
        assert!(Vmm::QemuQ35.has_exit_device(true));
        assert!(
            Vmm::QemuVirt.has_exit_device(false),
            "semihosting under emulation"
        );
        assert!(
            !Vmm::QemuVirt.has_exit_device(true),
            "no semihosting under KVM"
        );
        assert!(!Vmm::Firecracker.has_exit_device(true));
    }
}
