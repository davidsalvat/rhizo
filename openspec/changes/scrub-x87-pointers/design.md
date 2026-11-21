# Design

## D1. Which CPUs leak, decided once at boot

`fxsave_leaks()` is true when:

- the vendor is AMD (`AuthenticAMD`) or Hygon (`HygonGenuine`) and the CPU
  does not report `XSAVEERPTR` (`CPUID.8000_0008:EBX[2]`, or leaf
  `0x8000_0008` absent). These CPUs' `FXSAVE`/`FXRSTOR` handle `FOP`,
  `FIP` and `FDP` only while an x87 exception is pending (`FSW.ES`); Zen 2
  and later report `XSAVEERPTR` and always load them, as Intel CPUs do.
  This is the rule Linux uses for `X86_BUG_FXSAVE_LEAK`.
- or the CPU is QEMU's emulation: hypervisor bit (`CPUID.1:ECX[31]`) and
  signature `TCGTCGTCGTCG` at leaf `0x4000_0000`. Measured on QEMU 8.2
  with `-cpu max` (TCG), as `xtask` runs it: vendor `AuthenticAMD`,
  highest extended leaf `0x8000_000a`, `0x8000_0008` EBX `0x204`
  (`XSAVEERPTR` reported), and after `fild` from a kernel address,
  `FXRSTOR` of an image whose pointers are zero, then `FNSTENV`, the
  environment still holds the `fild`'s instruction and data addresses.
  (`FXSAVE` writes zero pointers on TCG, so `FNSTENV` is what shows it.)
  The legacy `qemu64` model also says `AuthenticAMD`; it is not used by
  `xtask`, and the TCG signature covers every TCG model alike.

The vendor check moves out of `kpti_needed` into `amd_like()`, shared by
both; KPTI's decision does not change.

`init_tags` stores the answer in `FXSAVE_LEAK` on the boot CPU, before the
other CPUs start; secondary CPUs read it, as they read `PCID`. All CPUs of
one virtual machine share a model.

Alternatives:

- Scrub on every CPU: costs a few instructions per FP/SIMD load on CPUs
  that load the pointers themselves; the spec asks for the scrub only
  where needed.
- Read CPUID on every load: CPUID traps to the hypervisor (hundreds of
  cycles under KVM, a VM exit); the flag is one relaxed load.
- Trust `XSAVEERPTR` alone: leaves QEMU TCG open, as measured above.

## D2. The scrub, in `fp_load`

Where the flag is set, `fp_load` runs, after clearing `CR0.TS` and before
`FXRSTOR`:

```
fnclex                  ; no pending x87 exception: the load cannot raise one
emms                    ; every x87 tag empty: the push cannot overflow
fild dword [FP_SCRUB]   ; FOP, FIP, FDP now name this instruction and FP_SCRUB
```

`FP_SCRUB` is a kernel `static u32`. `FXRSTOR` then loads everything else
from the thread's image (and the pointers too when the image has an
exception pending, which keeps the thread's own). What a cell can read
afterwards is the address of one kernel instruction and one kernel
variable, the same for every cell; the kernel is at a fixed address
(no KASLR, `docs/security.md`), so this discloses nothing.

`fp_load` is the only way FP/SIMD state enters a CPU for a thread: the
first FP/SIMD use after a switch (`Trap::FpUnavailable` in `run_slice`)
and the same trap for threads on several CPUs (gangs). One place covers
threads of one cell, other cells, forks, resumed snapshots and a thread's
first use (initial image).

The sequence is written from the instruction set's description (AMD's
`FXRSTOR` notes); no Linux code is copied.

## D3. Cost

On CPUs that do not leak: one relaxed load and a branch per FP/SIMD load.
Where it leaks (including CI's TCG `-icount` runs): four instructions more
per FP/SIMD load, against `FXRSTOR` itself and the `#NM` trap that leads to
it. Loads happen only when a thread that uses FP/SIMD runs after another
one did on that CPU.

Measured with `cargo xtask --arch x86_64 bench --icount-only --env ci` on
QEMU 8.2 (TCG, so with the scrub on), against main on the same QEMU: no
`cell.*` or `workload.*` figure grows by more than 0.3 % (`syscall_insns`
336 → 337, `switch_insns` 896 → 897, `pipe_rtt_insns` 2035 → 2037; the
rest unchanged or within 0.1 %), well inside the +2 % gate, and `--check`
reports no regression against `bench/baseline.json`.

## D4. What the tests can show

- `the_x87_scrub_follows_the_cpu`: the flag equals the rule of D1,
  computed again from CPUID in the test, and prints `(x87 scrub on)` or
  `off`. Under QEMU TCG it is on.
- `x87_pointers_never_survive_a_load`: through `fp_load` and `FNSTENV`,
  an x87 load's instruction and data pointers are gone after loading
  another thread's image (the initial one). It checks the property every
  CPU must hold, so it passes without the scrub on Intel and Zen 2 or
  later, and needs the scrub on pre-Zen 2 AMD and under TCG: without it,
  it fails under `cargo xtask --arch x86_64 test --only ktest`. A pointer
  the CPU does not record (FDP on Intel CPUs with `FDP_EXCPTN_ONLY`) is
  not checked; if neither is recorded the test says so.
- The leak on real pre-Zen 2 AMD hardware cannot be run in CI (no such
  runners); the same test covers it on such a host under KVM (Firecracker,
  Cloud Hypervisor, QEMU with `-accel kvm`).

## D5. AArch64 is not affected

AArch64's FP/SIMD state is `V0`–`V31`, `FPCR` and `FPSR`, and `fp_load`
loads all of them from the thread's image; the architecture keeps no
last-instruction or operand addresses for floating point. Nothing changes
there.
