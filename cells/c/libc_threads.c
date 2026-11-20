// POSIX threads through glibc (spec `workloads`, threaded libc programs):
// a pool of workers waiting on a condition variable until the first thread
// says go (the host may fork or freeze the cell there, at `ready`), a
// bounded queue with producers and consumers, a counter under a mutex, and
// floating point kept in registers by two threads taking turns. Every
// thread has a glibc-made stack with a guard page. Everything printed is
// computed after the joins, so the output does not depend on scheduling.
#define _GNU_SOURCE
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>

#define STACK (256 * 1024) // at least PTHREAD_STACK_MIN on AArch64

static pthread_attr_t attr;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pid_t tids[16];
static int ntids;

static void record_tid(void) {
    pid_t tid = (pid_t)syscall(SYS_gettid);
    pthread_mutex_lock(&lock);
    tids[ntids++] = tid;
    pthread_mutex_unlock(&lock);
}

static void start(pthread_t *t, void *(*fn)(void *), void *arg) {
    int err = pthread_create(t, &attr, fn, arg);
    if (err != 0) {
        printf("pthread_create: %d\n", err);
        exit(1);
    }
}

static void *join(pthread_t t) {
    void *result;
    if (pthread_join(t, &result) != 0) {
        printf("pthread_join failed\n");
        exit(1);
    }
    return result;
}

// --- a pool waiting for the go -------------------------------------------

#define WORKERS 3
static pthread_cond_t pool_changed = PTHREAD_COND_INITIALIZER;
static int waiting, go;

static void *worker(void *arg) {
    long id = (long)arg;
    record_tid();
    pthread_mutex_lock(&lock);
    waiting++;
    pthread_cond_broadcast(&pool_changed);
    while (!go)
        pthread_cond_wait(&pool_changed, &lock);
    pthread_mutex_unlock(&lock);
    long sum = 0;
    for (long i = 1; i <= 1000 * id; i++)
        sum += i;
    return (void *)sum;
}

static void pool(void) {
    pthread_t t[WORKERS];
    for (long i = 0; i < WORKERS; i++)
        start(&t[i], worker, (void *)(i + 1));
    pthread_mutex_lock(&lock);
    while (waiting < WORKERS)
        pthread_cond_wait(&pool_changed, &lock);
    pthread_mutex_unlock(&lock);
    printf("ready\n");
    fflush(stdout);
    sched_yield(); // a scheduling point with every worker waiting
    pthread_mutex_lock(&lock);
    go = 1;
    pthread_cond_broadcast(&pool_changed);
    pthread_mutex_unlock(&lock);
    printf("pool:");
    for (int i = 0; i < WORKERS; i++)
        printf(" %ld", (long)join(t[i]));
    printf("\n");
}

// --- producers and consumers ---------------------------------------------

#define ITEMS 200
#define SLOTS 8
static int queue[SLOTS], head, count, producers_left = 2;
static pthread_cond_t not_full = PTHREAD_COND_INITIALIZER;
static pthread_cond_t not_empty = PTHREAD_COND_INITIALIZER;

static void *producer(void *arg) {
    int base = (int)(long)arg;
    record_tid();
    for (int i = 1; i <= ITEMS; i++) {
        pthread_mutex_lock(&lock);
        while (count == SLOTS)
            pthread_cond_wait(&not_full, &lock);
        queue[(head + count++) % SLOTS] = base + i;
        pthread_cond_signal(&not_empty);
        pthread_mutex_unlock(&lock);
        if (i % 25 == 0)
            sched_yield();
    }
    pthread_mutex_lock(&lock);
    producers_left--;
    pthread_cond_broadcast(&not_empty);
    pthread_mutex_unlock(&lock);
    return NULL;
}

static void *consumer(void *arg) {
    (void)arg;
    long sum = 0, items = 0;
    record_tid();
    for (;;) {
        pthread_mutex_lock(&lock);
        while (count == 0 && producers_left > 0)
            pthread_cond_wait(&not_empty, &lock);
        if (count == 0) {
            pthread_mutex_unlock(&lock);
            break;
        }
        sum += queue[head];
        head = (head + 1) % SLOTS;
        count--;
        items++;
        pthread_cond_signal(&not_full);
        pthread_mutex_unlock(&lock);
    }
    long *result = malloc(2 * sizeof(long)); // per-thread malloc arenas
    result[0] = sum;
    result[1] = items;
    return result;
}

static void producers_and_consumers(void) {
    pthread_t p[2], c[2];
    start(&c[0], consumer, NULL);
    start(&c[1], consumer, NULL);
    start(&p[0], producer, (void *)0L);
    start(&p[1], producer, (void *)1000L);
    join(p[0]);
    join(p[1]);
    long sum = 0, items = 0;
    for (int i = 0; i < 2; i++) {
        long *r = join(c[i]);
        sum += r[0];
        items += r[1];
        free(r);
    }
    printf("queue: %ld items, sum %ld\n", items, sum);
}

// --- a counter under a mutex -----------------------------------------------

#define COUNTERS 4
#define INCREMENTS 100
static long counter;

static void *count_up(void *arg) {
    (void)arg;
    record_tid();
    for (int i = 0; i < INCREMENTS; i++) {
        pthread_mutex_lock(&lock);
        long v = counter;
        if (i % 25 == 0)
            sched_yield(); // let the others find the lock taken
        counter = v + 1;
        pthread_mutex_unlock(&lock);
    }
    return NULL;
}

static void mutex_counter(void) {
    pthread_t t[COUNTERS];
    for (int i = 0; i < COUNTERS; i++)
        start(&t[i], count_up, NULL);
    for (int i = 0; i < COUNTERS; i++)
        join(t[i]);
    printf("mutex: %ld increments\n", counter);
}

// --- floating point in two threads -------------------------------------------

static void *fp_sum(void *arg) {
    double seed = *(double *)arg;
    volatile double acc = 0.0;
    double step = seed;
    record_tid();
    for (int i = 0; i < 20; i++) {
        double local = acc + step; // kept in a register across the yield
        sched_yield();
        acc = local;
        step += 1.0;
    }
    *(double *)arg = acc;
    return NULL;
}

static void fp_threads(void) {
    double a = 1.0, b = 1000.0;
    pthread_t t[2];
    start(&t[0], fp_sum, &a);
    start(&t[1], fp_sum, &b);
    join(t[0]);
    join(t[1]);
    // sum of seed .. seed+19 = 20*seed + 190
    printf("fp: %.1f %.1f\n", a, b);
}

int main(void) {
    // Not compared with getpid(): a resumed or forked cell gets a new
    // process id but keeps its thread ids (spec `threads`, identifiers).
    pid_t me = (pid_t)syscall(SYS_gettid);
    pthread_attr_init(&attr);
    if (pthread_attr_setstacksize(&attr, STACK) != 0) {
        printf("pthread_attr_setstacksize failed\n");
        return 1;
    }
    pool();
    producers_and_consumers();
    mutex_counter();
    fp_threads();
    int distinct = 1;
    for (int i = 0; i < ntids; i++) {
        distinct &= tids[i] != me && tids[i] > 0;
        for (int j = 0; j < i; j++)
            distinct &= tids[i] != tids[j];
    }
    printf("threads: %d, distinct ids %s\n", ntids, distinct ? "yes" : "no");
    return 0;
}
