// A workload with an expensive start: it builds a prime sieve and a lookup
// table (the "warm-up", like an interpreter importing its modules), prints
// `warm` and yields, then answers a query and exits. The host can freeze it
// right after the warm-up and fork every request from there (spec `fork`,
// templates): `workload.warm_us` against `workload.cold_us`.
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define LIMIT 2000000

int main(void) {
    unsigned char *composite = calloc(LIMIT + 1, 1);
    unsigned int *prefix = malloc((LIMIT + 1) * sizeof *prefix);
    if (!composite || !prefix) return 2;
    for (unsigned long i = 2; i * i <= LIMIT; i++)
        if (!composite[i])
            for (unsigned long j = i * i; j <= LIMIT; j += i) composite[j] = 1;
    unsigned int count = 0;
    for (unsigned long i = 0; i <= LIMIT; i++) {
        if (i >= 2 && !composite[i]) count++;
        prefix[i] = count;
    }
    puts("warm");
    fflush(stdout);
    sched_yield(); // ready: a template is frozen here

    printf("primes below %d: %u, below 1000: %u\n", LIMIT, prefix[LIMIT], prefix[1000]);
    return 0;
}
