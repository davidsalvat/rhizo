// Exercises the allocator of a real libc: small and large blocks (the large
// ones come from mmap), realloc, and big memset/memcpy (vectorized code).
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
    unsigned long check = 0;
    char *blocks[64];
    for (int round = 0; round < 4; round++) {
        for (int i = 0; i < 64; i++) {
            size_t size = (size_t)(i * 131 + round * 977) % 4096 + 1;
            if (i % 16 == 0) size = 300000 + i * 1000; // above the mmap threshold
            blocks[i] = malloc(size);
            if (!blocks[i]) return 2;
            memset(blocks[i], i + round, size);
            check += (unsigned char)blocks[i][size - 1] * size;
        }
        for (int i = 0; i < 64; i += 2) {
            blocks[i] = realloc(blocks[i], 5000);
            if (!blocks[i]) return 3;
            check += (unsigned char)blocks[i][0];
        }
        for (int i = 0; i < 64; i++) free(blocks[i]);
    }
    size_t big = 8 << 20;
    char *a = malloc(big), *b = malloc(big);
    if (!a || !b) return 4;
    for (size_t i = 0; i < big; i++) a[i] = (char)(i * 31);
    memcpy(b, a, big);
    for (size_t i = 0; i < big; i += 4093) check += (unsigned char)b[i];
    free(a);
    free(b);
    printf("allocator checksum %lu\n", check);
    return 0;
}
