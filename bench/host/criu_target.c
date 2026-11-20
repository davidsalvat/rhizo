// The process CRIU checkpoints and restores in the comparison with cell
// suspend and resume (design D2 of `add-fair-comparisons`): it writes as
// much memory as the cell snapshot holds, says so, and then sleeps.
//
//   criu_target <MiB>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    size_t size = (size_t)(argc > 1 ? atoi(argv[1]) : 8) << 20;
    char *memory = malloc(size);
    if (!memory) return 3;
    memset(memory, 1, size);
    printf("criu-target ready\n");
    fflush(stdout);
    for (;;) pause();
}
