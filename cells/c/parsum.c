// A sum computed in parallel by POSIX threads (spec `workloads`, parallel
// threaded programs): `parsum [threads [millions [time]]]` splits
// `millions` million rounds of a xorshift generator into chunks that the
// threads take under a mutex; each adds its chunk's sum under the mutex and
// signals a condition variable the first thread waits on. The sum does not
// depend on the number of threads or on who took which chunk. With a third
// argument it also prints the nanoseconds the computation took (in-kernel
// tests and benchmarks compare 1 thread with 4).
#define _GNU_SOURCE
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define CHUNKS 64
#define MAX_THREADS 16
#define STACK (256 * 1024) // at least PTHREAD_STACK_MIN on AArch64

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t chunk_done = PTHREAD_COND_INITIALIZER;
static int next_chunk, chunks_done;
static uint64_t total;
static long rounds_per_chunk;

static uint64_t sum_chunk(long chunk) {
    uint64_t x = (uint64_t)(chunk + 1) * 0x9e3779b97f4a7c15ull, sum = 0;
    for (long i = 0; i < rounds_per_chunk; i++) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        sum += x >> 11;
    }
    return sum;
}

static void *worker(void *arg) {
    (void)arg;
    for (;;) {
        pthread_mutex_lock(&lock);
        int chunk = next_chunk < CHUNKS ? next_chunk++ : -1;
        pthread_mutex_unlock(&lock);
        if (chunk < 0)
            return NULL;
        uint64_t sum = sum_chunk(chunk);
        pthread_mutex_lock(&lock);
        total += sum;
        chunks_done++;
        pthread_cond_signal(&chunk_done);
        pthread_mutex_unlock(&lock);
    }
}

static int64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000 + ts.tv_nsec;
}

int main(int argc, char **argv) {
    int threads = argc > 1 ? atoi(argv[1]) : 4;
    long millions = argc > 2 ? atol(argv[2]) : 1;
    if (threads < 1 || threads > MAX_THREADS || millions < 1) {
        printf("usage: parsum [threads (1-16) [millions [time]]]\n");
        return 2;
    }
    rounds_per_chunk = millions * 1000000 / CHUNKS;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, STACK);
    int64_t t0 = now_ns();
    pthread_t t[MAX_THREADS];
    for (int i = 0; i < threads; i++) {
        int err = pthread_create(&t[i], &attr, worker, NULL);
        if (err != 0) {
            printf("pthread_create: %d\n", err);
            return 1;
        }
    }
    pthread_mutex_lock(&lock);
    while (chunks_done < CHUNKS)
        pthread_cond_wait(&chunk_done, &lock);
    pthread_mutex_unlock(&lock);
    for (int i = 0; i < threads; i++)
        pthread_join(t[i], NULL);
    int64_t ns = now_ns() - t0;
    printf("sum %016llx\n", (unsigned long long)total);
    if (argc > 3)
        printf("ns %lld\n", (long long)ns);
    return 0;
}
