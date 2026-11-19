// A real C program on a real libc (static glibc): formatted output with
// integers, floats and strings, the heap, sorting and string conversion.
// Its output is the same on Linux and on Rhizo (differential check).
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int by_value(const void *a, const void *b) {
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

int main(int argc, char **argv) {
    printf("hello from libc, %d argument%s, first %s\n", argc, argc == 1 ? "" : "s", argv[0]);
    int n = 1000;
    int *v = malloc(n * sizeof *v);
    if (!v) return 2;
    for (int i = 0; i < n; i++) v[i] = (i * 7919) % 1009;
    qsort(v, n, sizeof *v, by_value);
    long sum = 0;
    for (int i = 0; i < n; i++) sum += (long)v[i] * (i + 1);
    printf("sorted: min %d max %d weighted sum %ld\n", v[0], v[n - 1], sum);
    free(v);
    double x = strtod("3.14159265358979", NULL);
    printf("strtod %.6f, scaled %.3e, %g\n", x, x * 1e10, x / 7);
    char buf[64];
    snprintf(buf, sizeof buf, "%08.3f|%-6s|%x", x * 100, "rhizo", 48879);
    printf("%s (%zu chars)\n", buf, strlen(buf));
    return 0;
}
