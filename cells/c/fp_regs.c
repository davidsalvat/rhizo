// Holds its own values in every FP/SIMD register, and in the FP control
// and status registers, across system calls that let other threads and
// cells run, then checks that each register still holds them: another
// thread's or cell's values must never show, and none of its own may be
// lost (spec `cells`, change `lazy-fp-switch`).
//
// Usage: fp_regs [seed [rounds [threads [sleep_every]]]]
//   seed         the values held (default 1); each thread adds 1000 times
//                its number
//   rounds       how many times each thread holds values across a call
//                (default 100); 0 runs only the program around it
//   threads      threads holding values at once (default 1)
//   sleep_every  every that many rounds the call sleeps (`nanosleep`, the
//                thread blocks) instead of yielding (default 0: never)
//
// Prints `fp regs ok` and exits with 0, or names the first register that
// changed and exits with 1. The values are computed, not read from the
// registers, so the result is the same on Linux.
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#if defined(__x86_64__)
#define VREGS 16
#define NR_YIELD 24
#define NR_NANOSLEEP 35
#elif defined(__aarch64__)
#define VREGS 32
#define NR_YIELD 124
#define NR_NANOSLEEP 101
#else
#error "x86_64 or aarch64 only"
#endif

// What the registers hold: the vector registers, then the control and
// status registers (x86_64: MXCSR and the x87 control word; AArch64: FPCR
// and FPSR).
struct regs {
    uint64_t v[VREGS][2];
    uint64_t control;
    uint64_t status;
};

static unsigned rounds = 100;
static unsigned sleep_every;
static double seed = 1.0;

static uint64_t mix(uint64_t x) {
    x ^= x >> 33;
    x *= 0xff51afd7ed558ccdULL;
    x ^= x >> 33;
    x *= 0xc4ceb9fe1a85ec53ULL;
    return x ^ (x >> 33);
}

// The values of thread `id` in round `round`. The control values keep
// every FP exception masked, so nothing traps.
static void values(struct regs *r, uint64_t id, unsigned round) {
    uint64_t k = mix(id * 0x10001 + round);
    for (int i = 0; i < VREGS; i++) {
        r->v[i][0] = mix(k + 2 * i);
        r->v[i][1] = mix(k + 2 * i + 1);
    }
#if defined(__x86_64__)
    // MXCSR: masks set, sticky flags, rounding and flush-to-zero vary.
    r->control = 0x1f80 | (k & 0x3f) | ((k >> 6 & 3) << 13) | ((k >> 8 & 1) << 15);
    // x87 control word: masks set, precision and rounding vary.
    r->status = 0x7f | ((k >> 9 & 3) << 8) | ((k >> 11 & 3) << 10);
#else
    // FPCR: rounding mode, flush-to-zero and default NaN vary.
    r->control = (k >> 12 & 3) << 22 | (k >> 14 & 1) << 24 | (k >> 15 & 1) << 25;
    // FPSR: the cumulative exception flags and saturation vary.
    r->status = (k & 0x1f) | (k >> 5 & 1) << 7 | (k >> 6 & 1) << 27;
#endif
}

// Loads `in` into the registers, makes system call `nr` (sched_yield, or
// nanosleep of `ts`), and stores the registers into `out`, all in one
// block so the compiler cannot use them in between. Restores the default
// control registers before returning.
static void hold(const struct regs *in, struct regs *out, long nr, const struct timespec *ts) {
#if defined(__x86_64__)
    uint32_t mxcsr_in = in->control, mxcsr_out, mxcsr_default = 0x1f80;
    uint16_t fcw_in = in->status, fcw_out, fcw_default = 0x37f;
    __asm__ volatile(
        "movdqu 0(%[in]), %%xmm0\n\t"
        "movdqu 16(%[in]), %%xmm1\n\t"
        "movdqu 32(%[in]), %%xmm2\n\t"
        "movdqu 48(%[in]), %%xmm3\n\t"
        "movdqu 64(%[in]), %%xmm4\n\t"
        "movdqu 80(%[in]), %%xmm5\n\t"
        "movdqu 96(%[in]), %%xmm6\n\t"
        "movdqu 112(%[in]), %%xmm7\n\t"
        "movdqu 128(%[in]), %%xmm8\n\t"
        "movdqu 144(%[in]), %%xmm9\n\t"
        "movdqu 160(%[in]), %%xmm10\n\t"
        "movdqu 176(%[in]), %%xmm11\n\t"
        "movdqu 192(%[in]), %%xmm12\n\t"
        "movdqu 208(%[in]), %%xmm13\n\t"
        "movdqu 224(%[in]), %%xmm14\n\t"
        "movdqu 240(%[in]), %%xmm15\n\t"
        "ldmxcsr %[mi]\n\t"
        "fldcw %[fi]\n\t"
        "mov %[nr], %%rax\n\t"
        "mov %[ts], %%rdi\n\t"
        "xor %%esi, %%esi\n\t"
        "syscall\n\t"
        "stmxcsr %[mo]\n\t"
        "fnstcw %[fo]\n\t"
        "movdqu %%xmm0, 0(%[out])\n\t"
        "movdqu %%xmm1, 16(%[out])\n\t"
        "movdqu %%xmm2, 32(%[out])\n\t"
        "movdqu %%xmm3, 48(%[out])\n\t"
        "movdqu %%xmm4, 64(%[out])\n\t"
        "movdqu %%xmm5, 80(%[out])\n\t"
        "movdqu %%xmm6, 96(%[out])\n\t"
        "movdqu %%xmm7, 112(%[out])\n\t"
        "movdqu %%xmm8, 128(%[out])\n\t"
        "movdqu %%xmm9, 144(%[out])\n\t"
        "movdqu %%xmm10, 160(%[out])\n\t"
        "movdqu %%xmm11, 176(%[out])\n\t"
        "movdqu %%xmm12, 192(%[out])\n\t"
        "movdqu %%xmm13, 208(%[out])\n\t"
        "movdqu %%xmm14, 224(%[out])\n\t"
        "movdqu %%xmm15, 240(%[out])\n\t"
        "ldmxcsr %[md]\n\t"
        "fldcw %[fd]\n\t"
        : [mo] "=m"(mxcsr_out), [fo] "=m"(fcw_out)
        : [in] "r"(in->v), [out] "r"(out->v), [mi] "m"(mxcsr_in), [fi] "m"(fcw_in),
          [md] "m"(mxcsr_default), [fd] "m"(fcw_default), [nr] "r"(nr), [ts] "r"(ts)
        : "rax", "rcx", "rdi", "rsi", "r11", "memory", "xmm0", "xmm1", "xmm2", "xmm3",
          "xmm4", "xmm5", "xmm6", "xmm7", "xmm8", "xmm9", "xmm10", "xmm11", "xmm12",
          "xmm13", "xmm14", "xmm15");
    out->control = mxcsr_out;
    out->status = fcw_out;
#else
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = (long)ts;
    register long x1 __asm__("x1") = 0;
    uint64_t fpcr_out, fpsr_out;
    __asm__ volatile(
        "ldp q0, q1, [%[in], #0]\n\t"
        "ldp q2, q3, [%[in], #32]\n\t"
        "ldp q4, q5, [%[in], #64]\n\t"
        "ldp q6, q7, [%[in], #96]\n\t"
        "ldp q8, q9, [%[in], #128]\n\t"
        "ldp q10, q11, [%[in], #160]\n\t"
        "ldp q12, q13, [%[in], #192]\n\t"
        "ldp q14, q15, [%[in], #224]\n\t"
        "ldp q16, q17, [%[in], #256]\n\t"
        "ldp q18, q19, [%[in], #288]\n\t"
        "ldp q20, q21, [%[in], #320]\n\t"
        "ldp q22, q23, [%[in], #352]\n\t"
        "ldp q24, q25, [%[in], #384]\n\t"
        "ldp q26, q27, [%[in], #416]\n\t"
        "ldp q28, q29, [%[in], #448]\n\t"
        "ldp q30, q31, [%[in], #480]\n\t"
        "msr fpcr, %[ci]\n\t"
        "msr fpsr, %[si]\n\t"
        "svc #0\n\t"
        "mrs %[co], fpcr\n\t"
        "mrs %[so], fpsr\n\t"
        "stp q0, q1, [%[out], #0]\n\t"
        "stp q2, q3, [%[out], #32]\n\t"
        "stp q4, q5, [%[out], #64]\n\t"
        "stp q6, q7, [%[out], #96]\n\t"
        "stp q8, q9, [%[out], #128]\n\t"
        "stp q10, q11, [%[out], #160]\n\t"
        "stp q12, q13, [%[out], #192]\n\t"
        "stp q14, q15, [%[out], #224]\n\t"
        "stp q16, q17, [%[out], #256]\n\t"
        "stp q18, q19, [%[out], #288]\n\t"
        "stp q20, q21, [%[out], #320]\n\t"
        "stp q22, q23, [%[out], #352]\n\t"
        "stp q24, q25, [%[out], #384]\n\t"
        "stp q26, q27, [%[out], #416]\n\t"
        "stp q28, q29, [%[out], #448]\n\t"
        "stp q30, q31, [%[out], #480]\n\t"
        "msr fpcr, xzr\n\t"
        "msr fpsr, xzr\n\t"
        : [co] "=&r"(fpcr_out), [so] "=&r"(fpsr_out), "+r"(x0)
        : [in] "r"(in->v), [out] "r"(out->v), [ci] "r"(in->control), [si] "r"(in->status),
          "r"(x8), "r"(x1)
        : "memory", "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11",
          "v12", "v13", "v14", "v15", "v16", "v17", "v18", "v19", "v20", "v21", "v22", "v23",
          "v24", "v25", "v26", "v27", "v28", "v29", "v30", "v31");
    out->control = fpcr_out;
    out->status = fpsr_out;
#endif
}

// Thread `id`'s rounds; returns 0, or 1 after printing what changed.
static void *holder(void *arg) {
    uint64_t id = (uint64_t)(uintptr_t)arg;
    uint64_t base = (uint64_t)seed + 1000 * id;
    struct timespec ts = {0, 1000};
    for (unsigned round = 0; round < rounds; round++) {
        struct regs in, out;
        values(&in, base, round);
        int sleep = sleep_every && round % sleep_every == sleep_every - 1;
        hold(&in, &out, sleep ? NR_NANOSLEEP : NR_YIELD, &ts);
        for (int i = 0; i < VREGS + 1; i++) {
            const uint64_t *want = i < VREGS ? in.v[i] : &in.control;
            const uint64_t *got = i < VREGS ? out.v[i] : &out.control;
            if (want[0] != got[0] || want[1] != got[1]) {
                printf("fp regs MISMATCH thread %u round %u %s %d: %016llx %016llx, want "
                       "%016llx %016llx\n",
                       (unsigned)id, round, i < VREGS ? "vector" : "control/status", i,
                       (unsigned long long)got[1], (unsigned long long)got[0],
                       (unsigned long long)want[1], (unsigned long long)want[0]);
                return (void *)1;
            }
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1) seed = atof(argv[1]);
    if (argc > 2) rounds = (unsigned)atoi(argv[2]);
    unsigned threads = argc > 3 ? (unsigned)atoi(argv[3]) : 1;
    if (argc > 4) sleep_every = (unsigned)atoi(argv[4]);
    if (threads < 1 || threads > 8) threads = 1;
    pthread_t others[8];
    for (unsigned t = 1; t < threads; t++) {
        if (pthread_create(&others[t], 0, holder, (void *)(uintptr_t)t) != 0) {
            printf("fp regs: no thread\n");
            return 2;
        }
    }
    int failed = holder(0) != 0;
    for (unsigned t = 1; t < threads; t++) {
        void *result;
        pthread_join(others[t], &result);
        failed |= result != 0;
    }
    if (failed) return 1;
    printf("fp regs ok\n");
    return 0;
}
