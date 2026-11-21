// What a program learns of the CPU, as Linux tells it (change
// `add-pip-packages`). On AArch64: `AT_HWCAP` has FP, Advanced SIMD and
// `HWCAP_CPUID`, so ID registers can be read at EL0 (the kernel answers):
// `MIDR_EL1` is the CPU's, `MPIDR_EL1` only bit 31 (no CPU number), and the
// ID registers of the space can all be read. OpenBLAS picks its kernels so
// (and crashes without `HWCAP_CPUID` when /proc and /sys are missing).
// On x86_64 programs ask `CPUID` themselves: nothing to check.
#include <stdio.h>
#include <sys/auxv.h>

int main(void) {
#if defined(__aarch64__)
    unsigned long hwcap = getauxval(AT_HWCAP);
    unsigned long want = 1 | 1 << 1 | 1 << 11; // FP, ASIMD, CPUID
    if ((hwcap & want) != want) {
        printf("hwcap %#lx lacks %#lx\n", hwcap, want & ~hwcap);
        return 1;
    }
    unsigned long midr, mpidr, isar0;
    __asm__ volatile("mrs %0, midr_el1" : "=r"(midr));
    __asm__ volatile("mrs %0, mpidr_el1" : "=r"(mpidr));
    __asm__ volatile("mrs %0, id_aa64isar0_el1" : "=r"(isar0));
    (void)isar0;
    if (midr == 0 || mpidr != 1ul << 31) {
        printf("midr %#lx mpidr %#lx\n", midr, mpidr);
        return 1;
    }
#endif
    puts("cpu features ok");
    return 0;
}
