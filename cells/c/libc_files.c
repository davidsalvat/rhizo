// Files through a real libc (spec `files`): stdio and POSIX calls that
// create, write, read, list, rename and remove files. Run in an empty
// directory, it prints the same on Linux and in a cell.
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("FAILED %s: errno %d\n", what, errno);
    return 1;
}

static int by_name(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

// Names in `path`, sorted (directory order differs between file systems).
static void list(const char *path) {
    char *names[32];
    int n = 0;
    DIR *dir = opendir(path);
    if (!dir) {
        printf("opendir %s: errno %d\n", path, errno);
        return;
    }
    struct dirent *e;
    while ((e = readdir(dir)) && n < 32)
        if (strcmp(e->d_name, ".") && strcmp(e->d_name, ".."))
            names[n++] = strdup(e->d_name);
    closedir(dir);
    qsort(names, n, sizeof *names, by_name);
    printf("%s:", path);
    for (int i = 0; i < n; i++) {
        printf(" %s", names[i]);
        free(names[i]);
    }
    printf("\n");
}

int main(void) {
    if (mkdir("work", 0755) || chdir("work")) return fail("mkdir/chdir");
    char cwd[4096];
    if (!getcwd(cwd, sizeof cwd)) return fail("getcwd");
    printf("cwd ends in %s\n", strrchr(cwd, '/'));

    // stdio: write lines, read them back.
    FILE *f = fopen("notes.txt", "w");
    if (!f) return fail("fopen w");
    for (int i = 0; i < 100; i++) fprintf(f, "line %03d of the notes\n", i);
    fclose(f);
    f = fopen("notes.txt", "r");
    if (!f) return fail("fopen r");
    char line[64];
    unsigned long sum = 0;
    int lines = 0;
    while (fgets(line, sizeof line, f)) {
        lines++;
        for (char *p = line; *p; p++) sum = sum * 31 + (unsigned char)*p;
    }
    fclose(f);
    struct stat st;
    if (stat("notes.txt", &st)) return fail("stat");
    printf("notes: %d lines, %ld bytes, digest %lx, regular %d\n", lines, (long)st.st_size,
           sum, S_ISREG(st.st_mode));

    // POSIX: a bigger file crossing blocks, pread/pwrite, lseek, a hole.
    int fd = open("data.bin", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) return fail("open");
    static unsigned char buf[20000];
    for (int i = 0; i < 20000; i++) buf[i] = (unsigned char)(i * 7);
    if (write(fd, buf, sizeof buf) != (ssize_t)sizeof buf) return fail("write");
    if (pwrite(fd, "XYZ", 3, 4095) != 3) return fail("pwrite");
    if (lseek(fd, 30000, SEEK_SET) != 30000 || write(fd, "!", 1) != 1) return fail("hole");
    unsigned char back[8];
    if (pread(fd, back, 5, 4094) != 5) return fail("pread");
    printf("pread: %02x %c%c%c %02x\n", back[0], back[1], back[2], back[3], back[4]);
    if (pread(fd, back, 4, 29998) != 3) return fail("pread end");
    printf("hole: %d %d %c, size %ld\n", back[0], back[1], back[2], (long)lseek(fd, 0, SEEK_END));

    // dup shares the offset; O_APPEND writes at the end.
    lseek(fd, 0, SEEK_SET);
    int copy = dup(fd);
    if (read(fd, back, 2) != 2) return fail("read");
    printf("dup offset: %ld\n", (long)lseek(copy, 0, SEEK_CUR));
    close(copy);
    if (ftruncate(fd, 10) || fstat(fd, &st)) return fail("ftruncate");
    printf("truncated to %ld\n", (long)st.st_size);
    int app = open("data.bin", O_WRONLY | O_APPEND);
    if (write(app, "tail", 4) != 4) return fail("append");
    close(app);
    fstat(fd, &st);
    printf("appended to %ld\n", (long)st.st_size);

    // Errors with Linux numbers (errno read after each call).
    int r = open("data.bin", O_CREAT | O_EXCL | O_WRONLY, 0644);
    printf("exclusive: %d errno %d\n", r, errno);
    r = open("nothing", O_RDONLY);
    printf("missing: %d errno %d\n", r, errno);
    r = open("data.bin/x", O_RDONLY);
    printf("not a dir: %d errno %d\n", r, errno);
    r = access("data.bin", R_OK | W_OK);
    int x = access("data.bin", X_OK);
    printf("access: %d, x %d errno %d\n", r, x, errno);

    // Directories, rename, unlink while open.
    if (mkdir("sub", 0755) || rename("notes.txt", "sub/moved.txt")) return fail("rename");
    list(".");
    list("sub");
    r = rmdir("sub");
    printf("rmdir non-empty: %d errno %d\n", r, errno);
    int keep = open("sub/moved.txt", O_RDONLY);
    if (keep < 0 || unlink("sub/moved.txt")) return fail("unlink");
    char first[9] = {0};
    if (read(keep, first, 8) != 8) return fail("read unlinked");
    r = stat("sub/moved.txt", &st);
    printf("after unlink: \"%s\", stat %d errno %d\n", first, r, errno);
    close(keep);
    if (rmdir("sub") || unlink("data.bin")) return fail("cleanup");
    close(fd);
    list(".");
    if (chdir("..") || rmdir("work")) return fail("rmdir work");
    printf("files ok\n");
    return 0;
}
