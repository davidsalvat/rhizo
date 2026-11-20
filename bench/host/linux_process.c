// Linux process baselines, measured on the benchmark host itself, for the
// comparison with cells (`cargo xtask bench`, metrics under guest
// `linux-process`). Processes are not sandboxes: these numbers bound from
// below what an isolation layer built on Linux processes can do.
//
//   linux_process <static executable>
//
// prints `linux-process fork_us=<median> fork_call_us=<median>
// fork_touch_us=<median> spawn_us=<median>`:
// - fork_us: a process with 50 MiB written forks; the child exits at once;
//   the parent waits for it (fork + exit + wait).
// - fork_call_us: the parent's fork() call alone (pairs with cell.fork_us).
// - fork_touch_us: fork, the child writes every page, exits, the parent
//   waits (pairs with cell.fork_touch_us).
// - spawn_us: posix_spawn of the executable, run until it exits.
//
// Run as process 1 (the `/init` of a guest, design D1 of
// `add-fair-comparisons`), it runs `/hello`, prints the same line on the
// console and then pauses: the host reads the line and stops the VM.
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define RUNS 51

extern char **environ;

static double now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static int by_value(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

static double median(double *v) {
    qsort(v, RUNS, sizeof *v, by_value);
    return v[RUNS / 2];
}

int main(int argc, char **argv) {
    int init = getpid() == 1;
    char *program = init ? "/hello" : argc > 1 ? argv[1] : NULL;
    if (!program) {
        fprintf(stderr, "usage: %s <static executable>\n", argv[0]);
        return 2;
    }
    size_t size = 50 << 20;
    long page = sysconf(_SC_PAGESIZE);
    char *memory = malloc(size);
    if (!memory) return 3;
    memset(memory, 1, size);

    double fork_us[RUNS], call_us[RUNS], touch_us[RUNS], spawn_us[RUNS];
    for (int i = 0; i < RUNS; i++) {
        double t0 = now_us();
        pid_t pid = fork();
        if (pid == 0) _exit(memory[i] == 1 ? 0 : 1);
        int status;
        waitpid(pid, &status, 0);
        fork_us[i] = now_us() - t0;
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) return 4;
    }
    for (int i = 0; i < RUNS; i++) {
        double t0 = now_us();
        pid_t pid = fork();
        if (pid == 0) _exit(0);
        call_us[i] = now_us() - t0;
        waitpid(pid, NULL, 0);
    }
    for (int i = 0; i < RUNS; i++) {
        double t0 = now_us();
        pid_t pid = fork();
        if (pid == 0) {
            for (size_t at = 0; at < size; at += page) memory[at] = (char)i;
            _exit(0);
        }
        int status;
        waitpid(pid, &status, 0);
        touch_us[i] = now_us() - t0;
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) return 4;
    }
    for (int i = 0; i < RUNS; i++) {
        char *args[] = {program, NULL};
        double t0 = now_us();
        pid_t pid;
        if (posix_spawn(&pid, program, NULL, NULL, args, environ) != 0) return 5;
        int status;
        waitpid(pid, &status, 0);
        spawn_us[i] = now_us() - t0;
    }
    printf("linux-process fork_us=%.3f fork_call_us=%.3f fork_touch_us=%.3f spawn_us=%.3f\n",
           median(fork_us), median(call_us), median(touch_us), median(spawn_us));
    fflush(stdout);
    // Process 1 must not exit (the kernel would panic): wait for the host.
    while (init) pause();
    return 0;
}
