//! `rhizo up --pin` (design D17 of `add-cell-hierarchy`): each virtual CPU
//! of the VM on a host core of its own, no two on hyperthreads of one core,
//! so that the hardware does not share again between domains what their
//! exclusive CPU sets separate in the kernel.

use std::fs;

/// The vCPU a VMM thread named `name` runs: QEMU names its threads
/// `CPU <n>/KVM` (or `/TCG`), Cloud Hypervisor `vcpu<n>`.
fn vcpu_of(name: &str) -> Option<usize> {
    let name = name.trim_end();
    let n = match name.strip_prefix("CPU ") {
        Some(rest) => rest.split_once('/')?.0,
        None => name.strip_prefix("vcpu")?,
    };
    n.parse().ok()
}

/// The host CPU for each of `vcpus` vCPUs: of the CPUs `allowed`, in order,
/// the first of each physical core (`core_of` names a CPU's core: CPUs it
/// names alike are hyperthreads of one core).
fn plan(
    vcpus: usize,
    allowed: &[usize],
    core_of: impl Fn(usize) -> Option<String>,
) -> Result<Vec<usize>, String> {
    let mut cores = Vec::new();
    let mut cpus = Vec::new();
    for &cpu in allowed {
        let core = core_of(cpu).unwrap_or_else(|| cpu.to_string());
        if !cores.contains(&core) {
            cores.push(core);
            cpus.push(cpu);
        }
    }
    if cpus.len() < vcpus {
        return Err(format!(
            "--pin: {vcpus} vCPUs need as many host cores, and this process may use {}",
            cpus.len()
        ));
    }
    cpus.truncate(vcpus);
    Ok(cpus)
}

/// The CPUs this process may run on.
fn allowed() -> Result<Vec<usize>, String> {
    // SAFETY: a zeroed `cpu_set_t` is an empty set.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: `set` is a valid `cpu_set_t` of the size passed.
    let r = unsafe { libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut set) };
    if r != 0 {
        return Err(format!("--pin: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: `CPU_ISSET` reads the set for CPUs below its size.
    let on = |cpu: usize| unsafe { libc::CPU_ISSET(cpu, &set) };
    Ok((0..libc::CPU_SETSIZE as usize).filter(|&c| on(c)).collect())
}

/// Runs thread `tid` on host CPU `cpu` only.
fn pin_thread(tid: i32, cpu: usize) -> Result<(), String> {
    // SAFETY: a zeroed `cpu_set_t` is an empty set.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: `cpu` is one `allowed` gave, below `CPU_SETSIZE`.
    unsafe { libc::CPU_SET(cpu, &mut set) };
    // SAFETY: `set` is a valid `cpu_set_t` of the size passed.
    let r = unsafe { libc::sched_setaffinity(tid, size_of::<libc::cpu_set_t>(), &set) };
    if r != 0 {
        return Err(format!(
            "--pin: thread {tid} to CPU {cpu}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Pins the `vcpus` vCPU threads of the VMM process `pid` (QEMU or Cloud
/// Hypervisor) each to a host core of its own; returns the host CPUs, by
/// vCPU.
pub fn pin(pid: u32, vcpus: usize) -> Result<Vec<usize>, String> {
    let cpus = plan(vcpus, &allowed()?, |cpu| {
        let topology = format!("/sys/devices/system/cpu/cpu{cpu}/topology");
        fs::read_to_string(format!("{topology}/core_cpus_list"))
            .or_else(|_| fs::read_to_string(format!("{topology}/thread_siblings_list")))
            .ok()
    })?;
    let tasks = fs::read_dir(format!("/proc/{pid}/task")).map_err(|e| format!("--pin: {e}"))?;
    let mut found = vec![None; vcpus];
    for task in tasks.flatten() {
        let name = fs::read_to_string(task.path().join("comm")).unwrap_or_default();
        let tid = task.file_name().to_string_lossy().parse::<i32>();
        if let (Some(v), Ok(tid)) = (vcpu_of(&name), tid)
            && v < vcpus
        {
            found[v] = Some(tid);
        }
    }
    for (v, tid) in found.iter().enumerate() {
        let tid = tid.ok_or_else(|| {
            format!(
                "--pin: no thread of the VMM runs vCPU {v} (QEMU and Cloud Hypervisor name theirs)"
            )
        })?;
        pin_thread(tid, cpus[v])?;
    }
    Ok(cpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_finds_the_vcpu_threads_of_qemu_and_cloud_hypervisor() {
        assert_eq!(vcpu_of("CPU 0/KVM\n"), Some(0));
        assert_eq!(vcpu_of("CPU 3/TCG"), Some(3));
        assert_eq!(vcpu_of("vcpu2\n"), Some(2));
        assert_eq!(vcpu_of("qemu-system-x86"), None);
        assert_eq!(vcpu_of("CPU x/KVM"), None);
        assert_eq!(vcpu_of("vcpu"), None);
    }

    #[test]
    fn pin_takes_one_cpu_per_core() {
        // Hyperthread pairs (0,4), (1,5), (2,6), (3,7); CPU 1 not allowed.
        let core = |cpu: usize| Some(format!("{},{}", cpu % 4, cpu % 4 + 4));
        assert_eq!(plan(3, &[0, 2, 3, 4, 5, 6, 7], core), Ok(vec![0, 2, 3]));
        assert_eq!(plan(4, &[0, 2, 3, 4, 5, 6, 7], core), Ok(vec![0, 2, 3, 5]));
        assert!(plan(5, &[0, 2, 3, 4, 5, 6, 7], core).is_err(), "4 cores");
        assert_eq!(plan(2, &[6, 7], |_| None), Ok(vec![6, 7]), "no topology");
    }
}
