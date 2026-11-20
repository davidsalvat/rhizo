// A program that needs more stack than a cell starts with (design D5 of
// `add-cpython`): 4 MiB of recursion, then a system call writing into a
// 256 KiB stack buffer, as an interpreter's deep calls do. Linux grows a
// process's stack on touch up to its 8 MiB limit; so must a cell.
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define FRAME 4096
#define DEPTH 1024

static unsigned long down(int depth) {
    volatile unsigned char frame[FRAME];
    memset((void *)frame, depth & 0xff, sizeof frame);
    unsigned long below = depth > 0 ? down(depth - 1) : 0;
    return below + frame[depth % FRAME];
}

static int buffer(void) {
    char big[256 << 10];
    return getcwd(big, sizeof big) != NULL && big[0] == '/';
}

int main(void) {
    unsigned long sum = down(DEPTH - 1);
    printf("deep %lu\n", sum);
    printf("buffer %s\n", buffer() ? "ok" : "failed");
    return 0;
}
