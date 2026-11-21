// Code written at run time (spec `workloads`, change `node-jit`): memory
// mapped writable and executable, as a JIT maps it, both ways V8 uses
// (`mmap`, and `mprotect` of `PROT_NONE` addresses); a function written
// there, run, rewritten and run again; `ready`, where the host may fork,
// suspend or freeze the cell; then each copy runs code of its own, reads a
// page it never touched, has a system call fill another, and two threads
// share a page of code: in turns (one writes, the other runs), then at
// once (one runs a function while the other writes the next one in the
// same page). Everything printed is the same on Linux and in every copy.
//
// With the argument `self`, it runs code that writes the page it runs
// from: on Linux that works, in a cell it ends the cell with a fault
// report (spec `linux-abi`), so only the kernel's tests run it. With
// `unmapped`, it unmaps part of its untouched code memory and touches it.
#define _GNU_SOURCE
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define PAGE 4096
#define RWX (PROT_READ | PROT_WRITE | PROT_EXEC)

typedef int (*fn_t)(void);

// Small thread stacks: copies of the cell run side by side.
#define STACK (256 * 1024) // at least PTHREAD_STACK_MIN on AArch64
static pthread_attr_t attr;

static void fail(const char *what) {
    printf("%s failed\n", what);
    exit(1);
}

// Writes at `at` a function returning `value`, then makes it visible to
// instruction fetches as a JIT must on AArch64 (GCC's `__clear_cache`).
static void emit(unsigned char *at, uint32_t value) {
#if defined(__x86_64__)
    unsigned char code[6] = {0xb8, 0, 0, 0, 0, 0xc3}; // mov eax, imm32; ret
    memcpy(code + 1, &value, 4);
#elif defined(__aarch64__)
    uint32_t code[3] = {
        0x52800000 | ((value & 0xffff) << 5),         // movz w0, #lo
        0x72a00000 | ((value >> 16) << 5),            // movk w0, #hi, lsl 16
        0xd65f03c0,                                   // ret
    };
#endif
    memcpy(at, code, sizeof code);
    __builtin___clear_cache((char *)at, (char *)at + sizeof code);
}

static int run(unsigned char *at) {
    return ((fn_t)at)();
}

// --- two threads, in turns ----------------------------------------------

#define TURNS 200
static unsigned char *turn_page;
static atomic_int written, ran;

static void *runner(void *arg) {
    (void)arg;
    for (int i = 1; i <= TURNS; i++) {
        while (atomic_load_explicit(&written, memory_order_acquire) != i)
            sched_yield();
#if defined(__aarch64__)
        __asm__ volatile("isb" ::: "memory"); // code another thread wrote
#endif
        if (run(turn_page) != i)
            fail("turn");
        atomic_store_explicit(&ran, i, memory_order_release);
    }
    return NULL;
}

static void turns(void) {
    pthread_t t;
    if (pthread_create(&t, &attr, runner, NULL) != 0)
        fail("pthread_create");
    for (int i = 1; i <= TURNS; i++) {
        emit(turn_page, (uint32_t)i);
        atomic_store_explicit(&written, i, memory_order_release);
        while (atomic_load_explicit(&ran, memory_order_acquire) != i)
            sched_yield();
    }
    pthread_join(t, NULL);
    printf("turns %d ok\n", TURNS);
}

// --- two threads at once in one page -------------------------------------

#define ROUNDS 2000
static unsigned char *bounce_page;
static atomic_int running;

static void *writer(void *arg) {
    (void)arg;
    atomic_fetch_add(&running, 1);
    while (atomic_load(&running) < 2)
        ;
    // The next function, next to the one the other thread runs.
    for (int i = 0; i < ROUNDS; i++)
        emit(bounce_page + 256, (uint32_t)i);
    return NULL;
}

static void bounce(void) {
    emit(bounce_page, 5);
    pthread_t t;
    if (pthread_create(&t, &attr, writer, NULL) != 0)
        fail("pthread_create");
    atomic_fetch_add(&running, 1);
    while (atomic_load(&running) < 2)
        ;
    long sum = 0;
    for (int i = 0; i < ROUNDS; i++)
        sum += run(bounce_page);
    pthread_join(t, NULL);
    if (sum != 5L * ROUNDS || run(bounce_page + 256) != ROUNDS - 1)
        fail("bounce");
    printf("bounce ok\n");
}

// Code that stores into its own page, then returns.
static void self_modifying(unsigned char *page) {
#if defined(__x86_64__)
    static const unsigned char code[] = {0x88, 0x05, 0x40, 0, 0, 0, 0xc3}; // mov [rip+64], al; ret
#elif defined(__aarch64__)
    static const uint32_t code[] = {0x10000001, 0xb9004020, 0xd65f03c0}; // adr x1, .; str w0, [x1, #64]; ret
#endif
    memcpy(page, code, sizeof code);
    __builtin___clear_cache((char *)page, (char *)page + sizeof code);
    printf("self-modifying\n");
    fflush(stdout);
    run(page);
    printf("survived\n");
}

int main(int argc, char **argv) {
    // Pages: code, the bounce, a page never touched before `ready`, one
    // a system call fills, one for the turns.
    unsigned char *jit = mmap(NULL, 5 * PAGE, RWX, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    unsigned char *reserved = mmap(NULL, 4 * PAGE, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (jit == MAP_FAILED || reserved == MAP_FAILED)
        fail("mmap");
    if (mprotect(reserved, 4 * PAGE, RWX) != 0)
        fail("mprotect");
    if (argc > 1 && strcmp(argv[1], "self") == 0) {
        self_modifying(jit);
        return 0;
    }
    if (argc > 1 && strcmp(argv[1], "unmapped") == 0) {
        // Part of the never-touched memory unmapped: the rest still comes
        // zeroed, the part unmapped is gone (a fault, as on Linux).
        if (munmap(jit, PAGE) != 0 || jit[PAGE] != 0)
            fail("unmapped");
        printf("rest zero\n");
        fflush(stdout);
        jit[0] = 1;
        printf("survived\n");
        return 0;
    }
    emit(jit, 7);
    printf("first %d\n", run(jit));
    emit(reserved + PAGE, 8);
    printf("reserved %d\n", run(reserved + PAGE));
    emit(jit, 11);
    printf("rewritten %d\n", run(jit));
    printf("ready\n");
    fflush(stdout);
    sched_yield(); // the host may fork, suspend or freeze the cell here

    // Each copy writes and runs its own code; copies interleave here.
    uint32_t me = (uint32_t)getpid();
    emit(jit, me);
    sched_yield();
    if ((uint32_t)run(jit) != me || run(reserved + PAGE) != 8)
        fail("own code");
    printf("own code ok\n");
    unsigned char *fresh = jit + 2 * PAGE;
    for (int i = 0; i < PAGE; i++)
        if (fresh[i] != 0)
            fail("zero page");
    printf("zero page ok\n");
    int fds[2];
    if (pipe(fds) != 0 || write(fds[1], "code", 4) != 4)
        fail("pipe");
    unsigned char *filled = jit + 3 * PAGE;
    if (read(fds[0], filled + 100, 4) != 4 || memcmp(filled + 100, "code", 4) != 0 ||
        filled[0] != 0)
        fail("read into code memory");
    printf("syscall into code ok\n");
    if (pthread_attr_init(&attr) != 0 || pthread_attr_setstacksize(&attr, STACK) != 0)
        fail("pthread_attr");
    turn_page = jit + 4 * PAGE;
    turns();
    bounce_page = jit + PAGE;
    bounce();
    if (munmap(jit, 5 * PAGE) != 0 || munmap(reserved, 4 * PAGE) != 0)
        fail("munmap");
    printf("jit ok\n");
    return 0;
}
