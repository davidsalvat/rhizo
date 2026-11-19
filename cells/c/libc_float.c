// Floating point that keeps its state in registers across yields: two of
// these running together must not see each other's FP/SIMD registers. The
// result is exact (integers held in doubles), so it does not depend on the
// libm or the architecture.
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    double seed = argc > 1 ? atof(argv[1]) : 1.0;
    volatile double acc = 0.0;
    double step = seed;
    for (int i = 0; i < 200; i++) {
        double local = acc + step; // kept in a register across the yield
        sched_yield();
        acc = local;
        step += 1.0;
    }
    // sum of seed .. seed+199 = 200*seed + 19900
    double expected = 200.0 * seed + 19900.0;
    printf("fp %s: %.1f\n", acc == expected ? "ok" : "MISMATCH", acc);
    return acc == expected ? 0 : 1;
}
