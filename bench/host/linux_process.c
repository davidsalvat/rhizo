// Linux process baselines, measured on the benchmark host itself, for the
// comparison with cells (`cargo xtask bench`, metrics under guest
// `linux-process`). Processes are not sandboxes: these numbers bound from
// below what an isolation layer built on Linux processes can do.
//
//   linux_process <static executable>
//
// prints `linux-process fork_us=<median> spawn_us=<median>`:
// - fork_us: a process with 50 MiB written forks; the child exits at once;
//   the parent waits for it (fork + exit + wait).
// - spawn_us: posix_spawn of the executable, run until it exits.
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
    if (argc < 2) {
        fprintf(stderr, "usage: %s <static executable>\n", argv[0]);
        return 2;
    }
    size_t size = 50 << 20;
    char *memory = malloc(size);
    if (!memory) return 3;
    memset(memory, 1, size);

    double fork_us[RUNS], spawn_us[RUNS];
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
        char *args[] = {argv[1], NULL};
        double t0 = now_us();
        pid_t pid;
        if (posix_spawn(&pid, argv[1], NULL, NULL, args, environ) != 0) return 5;
        int status;
        waitpid(pid, &status, 0);
        spawn_us[i] = now_us() - t0;
    }
    printf("linux-process fork_us=%.3f spawn_us=%.3f\n", median(fork_us), median(spawn_us));
    return 0;
}
