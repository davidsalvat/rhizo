// numpy on Linux in the benchmark VM (design D6 of `add-pip-packages`):
// the `/init` of a Linux guest in Firecracker whose initramfs holds the
// same files as the packages cell (the dynamically linked
// `/usr/bin/python3`, its standard library and `site-packages` under
// `/usr/lib/python3.13`, `ld.so` and the libraries, `/app/numpy_demo.py`).
//
// Prints `linux-numpy cold_us=<median> warm_us=<median>`:
// - cold_us: fork and exec `python3 /app/numpy_demo.py` with an empty
//   environment, its output read from a pipe, until it exits (pairs with
//   workload.numpy_cold_us).
// - warm_us: what `/app/numpy_forkserver.py` prints: a CPython process
//   that imported numpy forks one child per request; the child computes
//   and exits like the cell does (pairs with workload.numpy_warm_us).
// Then it pauses: the host reads the line and stops the VM.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define RUNS 7

static double now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static int by_value(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

// Runs python3 with `args`, its standard output and error in `out`
// (NUL-terminated, truncated to `size`); returns the exit status, or -1.
static int python(char *const args[], char *out, size_t size) {
    char *const env[] = {NULL};
    int fds[2];
    if (pipe(fds) != 0) return -1;
    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        dup2(fds[1], 1);
        dup2(fds[1], 2);
        close(fds[0]);
        close(fds[1]);
        execve("/usr/bin/python3", args, env);
        _exit(127);
    }
    close(fds[1]);
    size_t len = 0;
    for (;;) {
        char buf[4096];
        ssize_t n = read(fds[0], buf, sizeof buf);
        if (n <= 0) break;
        size_t keep = (size_t)n < size - 1 - len ? (size_t)n : size - 1 - len;
        memcpy(out + len, buf, keep);
        len += keep;
    }
    out[len] = 0;
    close(fds[0]);
    int status;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) return -1;
    return WEXITSTATUS(status);
}

int main(void) {
    static char out[1 << 16];
    double cold[RUNS];
    char *const run[] = {"/usr/bin/python3", "-s", "/app/numpy_demo.py", NULL};
    for (int i = 0; i < RUNS; i++) {
        double t0 = now_us();
        if (python(run, out, sizeof out) != 0 || !strstr(out, "matmul ")) {
            for (char *c = out; *c; c++)
                if (*c == '\n') *c = '|';
            printf("linux-numpy failed: %.300s\n", out);
            fflush(stdout);
            for (;;) pause();
        }
        cold[i] = now_us() - t0;
    }
    qsort(cold, RUNS, sizeof *cold, by_value);
    char *const serve[] = {"/usr/bin/python3", "-s", "/app/numpy_forkserver.py", "7", NULL};
    char warm[64] = "warm_us=failed";
    if (python(serve, out, sizeof out) == 0 && strstr(out, "warm_us="))
        snprintf(warm, sizeof warm, "%.*s", (int)strcspn(strstr(out, "warm_us="), "\n"),
                 strstr(out, "warm_us="));
    printf("linux-numpy cold_us=%.1f %s\n", cold[RUNS / 2], warm);
    fflush(stdout);
    // Process 1 must not exit (the kernel would panic): wait for the host.
    for (;;) pause();
}
