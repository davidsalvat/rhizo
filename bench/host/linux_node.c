// Node.js on Linux in the benchmark VM (change `add-node-workload`): the
// `/init` of a Linux guest in Firecracker whose initramfs holds the same
// files as the Node.js cell (`/usr/bin/node`, its interpreter and libraries,
// `/app/demo.js`).
//
// Prints `linux-node cold_us=<median>`: fork and exec of
// `node --jitless /app/demo.js` with an empty environment, its output read
// from a pipe, until it exits (pairs with workload.node_cold_us). Node.js
// cannot fork itself, so there is no warm figure to pair with
// workload.node_warm_us. Then it pauses: the host reads the line and stops
// the VM.
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

// Runs Node.js on the script, its standard output and error in `out`
// (NUL-terminated, truncated to `size`); returns the exit status, or -1.
static int node(char *out, size_t size) {
    char *const args[] = {"/usr/bin/node", "--jitless", "/app/demo.js", NULL};
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
        execve(args[0], args, env);
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
    // A first run reads the files into the page cache, as the cell's first
    // spawn fills its page cache.
    for (int i = -1; i < RUNS; i++) {
        double t0 = now_us();
        if (node(out, sizeof out) != 0 || !strstr(out, "\"folded\"")) {
            for (char *c = out; *c; c++)
                if (*c == '\n') *c = '|';
            printf("linux-node failed: %.300s\n", out);
            fflush(stdout);
            for (;;) pause();
        }
        if (i >= 0) cold[i] = now_us() - t0;
    }
    qsort(cold, RUNS, sizeof *cold, by_value);
    printf("linux-node cold_us=%.1f\n", cold[RUNS / 2]);
    fflush(stdout);
    // Process 1 must not exit (the kernel would panic): wait for the host.
    for (;;) pause();
}
