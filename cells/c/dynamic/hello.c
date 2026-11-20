// A dynamically linked C program (change `add-dynamic-linking`): its
// interpreter, the distribution's `ld.so`, starts it and loads libc. With
// the argument `wait` it prints `ready` and sleeps 50 ms first, so the host
// can suspend, fork or freeze it once it runs.
#include <stdio.h>
#include <string.h>
#include <sys/auxv.h>
#include <time.h>

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "wait") == 0) {
        puts("ready");
        fflush(stdout);
        struct timespec pause = {0, 50 * 1000 * 1000};
        nanosleep(&pause, NULL);
    }
    printf("hello from a dynamically linked cell: %d argument(s), interpreter %s\n",
           argc, getauxval(AT_BASE) != 0 ? "found" : "missing");
    return 0;
}
