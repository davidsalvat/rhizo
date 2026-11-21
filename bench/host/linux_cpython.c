// CPython on Linux in the benchmark VM (design D4 of `add-cpython`): the
// `/init` of a Linux guest in Firecracker whose initramfs holds the same
// static CPython, standard library and scripts as the CPython cell
// (`/usr/bin/python3`, `/usr/lib/python3.13`, `/app/agent_demo.py`,
// `/app/agent_loop.py`).
//
// Prints `linux-cpython cold_us=<median> warm_us=<median> agent=<...>`:
// - cold_us: fork and exec `python3 /app/agent_demo.py` with an empty
//   environment, its output read from a pipe, until it exits (pairs with
//   workload.cpython_cold_us).
// - warm_us: what `/app/cpython_forkserver.py` prints: a CPython process
//   that imported and warmed up the script forks one child per request; the
//   child answers and exits like the cell does (pairs with
//   workload.cpython_warm_us).
// - agent: the agent of `add-agent-workload` (design D5) as a Linux
//   process talking to the host's fake model through the VM's network card
//   (`rhizo_ip` and `rhizo_llm=ADDR:PORT` in the environment, from the
//   kernel command line): `agent_turn_us=<median>` (each turn but the first,
//   as the agent times it), `agent_model_us=<median>` (the part of those
//   turns spent on the model's answer) and `agent_fork_us=<median>` (its
//   `os.fork` into branches until they run), or `agent=skipped` /
//   `agent=failed`.
// Then it pauses: the host reads the line and stops the VM.
#include <arpa/inet.h>
#include <net/if.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define RUNS 7
#define AGENT_RUNS 3

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

// Gives interface `name` address `addr`/24 (none for loopback) and brings
// it up; returns 0 on success.
static int interface_up(const char *name, const char *addr) {
    int s = socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0) return -1;
    struct ifreq r;
    memset(&r, 0, sizeof r);
    strncpy(r.ifr_name, name, IFNAMSIZ - 1);
    int failed = 0;
    if (addr) {
        struct sockaddr_in *a = (struct sockaddr_in *)&r.ifr_addr;
        a->sin_family = AF_INET;
        failed |= inet_pton(AF_INET, addr, &a->sin_addr) != 1;
        failed |= ioctl(s, SIOCSIFADDR, &r) != 0;
        failed |= inet_pton(AF_INET, "255.255.255.0", &a->sin_addr) != 1;
        failed |= ioctl(s, SIOCSIFNETMASK, &r) != 0;
    }
    failed |= ioctl(s, SIOCGIFFLAGS, &r) != 0;
    r.ifr_flags |= IFF_UP | IFF_RUNNING;
    failed |= ioctl(s, SIOCSIFFLAGS, &r) != 0;
    close(s);
    return failed ? -1 : 0;
}

// Appends to `all` (in microseconds) the comma-separated nanoseconds after
// `key` in `text`, the first one left out when `skip_first`; < 0 when
// absent.
static double median_of(const char *text, const char *key, int skip_first, double *all, int *n) {
    const char *at = strstr(text, key);
    if (!at) return -1;
    at += strlen(key);
    for (int i = 0;; i++) {
        char *end;
        double v = strtod(at, &end);
        if (end == at) break;
        if (i > 0 || !skip_first) all[(*n)++] = v / 1e3;
        if (*end != ',') break;
        at = end + 1;
    }
    return 0;
}

// The agent figures into `line` (see the top of this file).
static void agent(char *line, size_t size, char *out, size_t out_size) {
    const char *ip = getenv("rhizo_ip"), *llm = getenv("rhizo_llm");
    if (!ip || !llm || !strchr(llm, ':')) {
        snprintf(line, size, "agent=skipped");
        return;
    }
    if (interface_up("lo", NULL) != 0 || interface_up("eth0", ip) != 0) {
        snprintf(line, size, "agent=failed:no-network");
        return;
    }
    char url[96];
    snprintf(url, sizeof url, "http://%s", llm);
    char *const run[] = {"/usr/bin/python3", "/app/agent_loop.py", url, "fake",
                         "linux", "8", "timed", NULL};
    double turns[AGENT_RUNS * 8], model[AGENT_RUNS * 8], forks[AGENT_RUNS];
    int n_turns = 0, n_model = 0, n_forks = 0;
    for (int i = 0; i < AGENT_RUNS; i++) {
        if (python(run, out, out_size) != 0 ||
            median_of(out, "agent turn_ns=", 1, turns, &n_turns) < 0 ||
            median_of(out, "agent model_ns=", 1, model, &n_model) < 0 ||
            median_of(out, "agent fork_ns=", 0, forks, &n_forks) < 0) {
            for (char *c = out; *c; c++)
                if (*c == '\n' || *c == ' ') *c = '|';
            snprintf(line, size, "agent=failed:%.200s", out);
            return;
        }
    }
    qsort(turns, n_turns, sizeof *turns, by_value);
    qsort(model, n_model, sizeof *model, by_value);
    qsort(forks, n_forks, sizeof *forks, by_value);
    snprintf(line, size, "agent_turn_us=%.1f agent_model_us=%.1f agent_fork_us=%.1f",
             turns[n_turns / 2], model[n_model / 2], forks[n_forks / 2]);
}

int main(void) {
    static char out[1 << 16];
    double cold[RUNS];
    char *const run[] = {"/usr/bin/python3", "/app/agent_demo.py", NULL};
    for (int i = 0; i < RUNS; i++) {
        double t0 = now_us();
        if (python(run, out, sizeof out) != 0 || !strstr(out, "cpython ok")) {
            printf("linux-cpython failed: %s\n", out);
            fflush(stdout);
            for (;;) pause();
        }
        cold[i] = now_us() - t0;
    }
    qsort(cold, RUNS, sizeof *cold, by_value);
    char *const serve[] = {"/usr/bin/python3", "/app/cpython_forkserver.py", "7", NULL};
    char warm[64] = "warm_us=failed";
    if (python(serve, out, sizeof out) == 0 && strstr(out, "warm_us="))
        snprintf(warm, sizeof warm, "%.*s", (int)strcspn(strstr(out, "warm_us="), "\n"),
                 strstr(out, "warm_us="));
    char agent_line[512];
    agent(agent_line, sizeof agent_line, out, sizeof out);
    printf("linux-cpython cold_us=%.1f %s %s\n", cold[RUNS / 2], warm, agent_line);
    fflush(stdout);
    // Process 1 must not exit (the kernel would panic): wait for the host.
    for (;;) pause();
}
