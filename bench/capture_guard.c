/* Reference-only sparse-file guard. Never linked into Tile-Ripper.
 * Build: cc -shared -fPIC -o guard.so capture_guard.c -ldl (Linux), or
 *        cc -dynamiclib -o guard.dylib capture_guard.c (macOS).
 * TR_CAPTURE_FILE selects an inode; TR_CAPTURE_EXTENTS contains half-open
 * decimal byte ranges, one pair per line. Every read is traced and a hole
 * returns EIO instead of filesystem-synthesized zero bytes.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>
#ifndef __APPLE__
#include <dlfcn.h>
#endif

static struct stat source;
static long long ranges[8192][2];
static size_t range_count;
static int ready, trace_fd = -1;

__attribute__((constructor)) static void setup(void) {
    const char *path = getenv("TR_CAPTURE_FILE");
    const char *extents = getenv("TR_CAPTURE_EXTENTS");
    const char *trace = getenv("TR_CAPTURE_TRACE");
    if (!path || !extents || !trace) _exit(120);
    if (stat(path, &source)) _exit(121);
    FILE *input = fopen(extents, "r");
    if (!input) _exit(122);
    long long a, b;
    while (fscanf(input, "%lld %lld", &a, &b) == 2) {
        if (range_count == 8192 || a < 0 || b <= a || b > source.st_size ||
            (range_count && a < ranges[range_count - 1][1])) _exit(123);
        ranges[range_count][0] = a;
        ranges[range_count++][1] = b;
    }
    if (!feof(input) || !range_count) _exit(124);
    fclose(input);
    trace_fd = open(trace, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (trace_fd < 0) _exit(125);
    ready = 1;
}

static int allowed(int fd, size_t length, off_t offset) {
    if (!ready || !length) return 1;
    struct stat info;
    if (fstat(fd, &info) || info.st_ino != source.st_ino || info.st_dev != source.st_dev)
        return 1;
    long long start = offset, end = start + (long long)length;
    if (start >= source.st_size) return 1; /* ordinary EOF */
    if (end > source.st_size) end = source.st_size;
    long long next = start;
    for (size_t i = 0; i < range_count && next < end; ++i) {
        if (ranges[i][1] <= next) continue;
        if (ranges[i][0] > next) break;
        next = ranges[i][1];
    }
    int hit = start >= 0 && next >= end;
    dprintf(trace_fd, "%s %lld %lld\n", hit ? "HIT" : "CAPTURE_MISS", start, end);
    if (!hit) errno = EIO;
    return hit;
}

#ifdef __APPLE__
static ssize_t guarded_pread(int fd, void *buf, size_t count, off_t offset) {
    return allowed(fd, count, offset) ? pread(fd, buf, count, offset) : -1;
}
static ssize_t guarded_read(int fd, void *buf, size_t count) {
    return allowed(fd, count, lseek(fd, 0, SEEK_CUR)) ? read(fd, buf, count) : -1;
}
#define INTERPOSE(replacement, original) \
    __attribute__((used)) static struct { const void *a; const void *b; } \
    pair_##original __attribute__((section("__DATA,__interpose"))) = \
    { (const void *)&replacement, (const void *)&original }
INTERPOSE(guarded_pread, pread);
INTERPOSE(guarded_read, read);
#else
ssize_t pread(int fd, void *buf, size_t count, off_t offset) {
    static ssize_t (*real_pread)(int, void *, size_t, off_t);
    if (!real_pread) real_pread = dlsym(RTLD_NEXT, "pread");
    return allowed(fd, count, offset) ? real_pread(fd, buf, count, offset) : -1;
}
ssize_t pread64(int fd, void *buf, size_t count, off64_t offset) {
    static ssize_t (*real_pread)(int, void *, size_t, off64_t);
    if (!real_pread) real_pread = dlsym(RTLD_NEXT, "pread64");
    return allowed(fd, count, offset) ? real_pread(fd, buf, count, offset) : -1;
}
ssize_t read(int fd, void *buf, size_t count) {
    static ssize_t (*real_read)(int, void *, size_t);
    if (!real_read) real_read = dlsym(RTLD_NEXT, "read");
    return allowed(fd, count, lseek(fd, 0, SEEK_CUR)) ? real_read(fd, buf, count) : -1;
}
#endif
