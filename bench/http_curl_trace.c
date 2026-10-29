/* macOS benchmark-only libcurl request timing. Never loaded by normal runs.
 *
 * Build against Rasterio's bundled libcurl, not the macOS system libcurl:
 *   cc -std=c11 -Wall -Wextra -Werror -dynamiclib -headerpad_max_install_names \
 *      bench/http_curl_trace.c /ABS/rasterio/.dylibs/libcurl.4.dylib \
 *      -o /TMP/http_curl_trace.dylib
 *   install_name_tool -change /DLC/rasterio/.dylibs/libcurl.4.dylib \
 *      /ABS/rasterio/.dylibs/libcurl.4.dylib /TMP/http_curl_trace.dylib
 * Set DYLD_INSERT_LIBRARIES=/TMP/http_curl_trace.dylib and
 * TR_CURL_TRACE=/ABS/new-trace.jsonl for the single worker process only.
 * The output must not exist. No URL, query, header value, body, or libcurl
 * informational text is written. Only a restricted basename, method, numeric
 * Range, status, byte counts, identifiers, and clocks are retained.
 *
 * HEADER_OUT starts an HTTP request. For an unencoded, Content-Length-delimited
 * GET, its exact network-body endpoint is the final DATA_IN callback; for HEAD,
 * it is the final response-header terminator. API completion is observed
 * separately at easy_perform return, CURLMSG_DONE, or multi_remove_handle.
 * GDAL 3.12's VSICURLMultiPerform does not drain CURLMSG_DONE, so remove_handle
 * is necessary. We never drain, replace, or otherwise change its message queue.
 * An end with timing_exact=false MUST NOT support an actual-concurrency claim.
 * Redirects, authentication/proxy negotiations, chunked or encoded responses,
 * and incomplete/ambiguous transactions deliberately fail that qualification.
 * DATA_IN precedes the caller's write callback: this measures received HTTP
 * body completion, not the completion of the consumer's work.
 */
#ifndef __APPLE__
#error "This benchmark interposer requires macOS dyld."
#endif

#include <curl/curl.h>
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <time.h>
#include <unistd.h>

#define INTERPOSE(replacement, original)                                      \
    __attribute__((used)) static struct { const void *a; const void *b; }       \
        pair_##original __attribute__((section("__DATA,__interpose"))) =      \
            {(const void *)&replacement, (const void *)&original}

typedef struct { double mono, unix_s; } Stamp;
typedef struct Entry {
    CURL *easy;
    CURLM *multi;
    uint64_t handle_id, request_id, data_bytes, content_length;
    unsigned header_out_count;
    bool pending, active, easy_mode, have_length, header_done;
    bool bad_length, encoded, transfer_encoded, ambiguous;
    bool conditional, range_truncated;
    long status;
    Stamp last_body, body_complete;
    char method[12], source[256], range[4096];
    struct Entry *next;
} Entry;

static int trace_fd = -1;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static Entry *entries;
static uint64_t next_handle = 1, next_request = 1;
static uint64_t starts, ends, issues, inexact, pending_transfers;

static void fail(const char *message) {
    /* All call sites pass fixed strings, never source material. */
    (void)write(STDERR_FILENO, message, strlen(message));
    _exit(125);
}

static Stamp now(void) {
    struct timespec mono, real;
    if (clock_gettime(CLOCK_MONOTONIC, &mono) ||
        clock_gettime(CLOCK_REALTIME, &real)) fail("curl trace: clock failed\n");
    return (Stamp){mono.tv_sec + mono.tv_nsec * 1e-9,
                   real.tv_sec + real.tv_nsec * 1e-9};
}

static void checked(int result) {
    if (result < 0) fail("curl trace: output write failed\n");
}

__attribute__((constructor)) static void setup(void) {
    const char *path = getenv("TR_CURL_TRACE");
    if (!path || !*path) return;
    if (path[0] != '/') fail("curl trace: output must be absolute\n");
    trace_fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (trace_fd < 0) fail("curl trace: cannot create new output\n");
    Stamp t = now();
    checked(dprintf(trace_fd,
        "{\"event\":\"trace_init\",\"schema\":1,\"client\":\"gdal\","
        "\"pid\":%d,\"time_unix_s\":%.9f,\"monotonic_s\":%.9f}\n",
        getpid(), t.unix_s, t.mono));
}

/* Call only while holding lock. Entries live until their easy handle cleanup. */
static Entry *entry(CURL *easy, bool create) {
    for (Entry *p = entries; p; p = p->next)
        if (p->easy == easy) return p;
    if (!create) return NULL;
    Entry *p = calloc(1, sizeof(*p));
    if (!p) fail("curl trace: allocation failed\n");
    p->easy = easy;
    p->handle_id = next_handle++;
    p->next = entries;
    entries = p;
    return p;
}

static void issue(Entry *p, const char *reason, Stamp t) {
    ++issues;
    checked(dprintf(trace_fd,
        "{\"event\":\"issue\",\"client\":\"gdal\",\"pid\":%d,"
        "\"id\":%" PRIu64 ",\"handle\":%" PRIu64 ",\"reason\":\"%s\","
        "\"time_unix_s\":%.9f,\"monotonic_s\":%.9f}\n",
        getpid(), p->request_id, p->handle_id, reason, t.unix_s, t.mono));
}

static bool eq(const char *p, size_t n, const char *value) {
    return n == strlen(value) && !strncasecmp(p, value, n);
}

static bool safe_basename_char(unsigned char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
           (c >= '0' && c <= '9') || c == '.' || c == '_' || c == '-';
}

static void request_line(Entry *p, const char *line, size_t n) {
    const char *space = memchr(line, ' ', n);
    strcpy(p->method, "OTHER");
    strcpy(p->source, "redacted");
    if (!space) return;
    size_t method_n = (size_t)(space - line);
    static const char *methods[] = {"GET", "HEAD", "POST", "PUT", "DELETE",
                                    "PATCH", "OPTIONS", "CONNECT"};
    for (size_t i = 0; i < sizeof(methods) / sizeof(methods[0]); ++i)
        if (eq(line, method_n, methods[i])) strcpy(p->method, methods[i]);
    const char *target = space + 1, *end = line + n;
    for (const char *c = target; c < end; ++c)
        if (*c == ' ' || *c == '?' || *c == '#') { end = c; break; }
    const char *base = target;
    for (const char *c = target; c < end; ++c) if (*c == '/') base = c + 1;
    size_t base_n = (size_t)(end - base);
    if (!base_n || base_n >= sizeof(p->source)) return;
    for (const char *c = base; c < end; ++c)
        if (!safe_basename_char((unsigned char)*c)) return;
    memcpy(p->source, base, base_n);
    p->source[base_n] = '\0';
}

/* Iterate bounded callback buffers without assuming NUL termination. */
static void outgoing(Entry *p, const char *data, size_t size, Stamp t) {
    if (++p->header_out_count != 1) {
        p->ambiguous = true;
        issue(p, "multiple_header_out_before_api_completion", t);
        return;
    }
    p->active = true;
    p->request_id = next_request++;
    ++starts;
    bool first = true;
    for (size_t at = 0; at < size;) {
        const char *line = data + at;
        const char *lf = memchr(line, '\n', size - at);
        size_t n = lf ? (size_t)(lf - line) : size - at;
        at += n + (lf ? 1 : 0);
        if (n && line[n - 1] == '\r') --n;
        if (first) { request_line(p, line, n); first = false; continue; }
        const char *colon = memchr(line, ':', n);
        if (!colon) continue;
        size_t name_n = (size_t)(colon - line);
        const char *value = colon + 1, *end = line + n;
        while (value < end && (*value == ' ' || *value == '\t')) ++value;
        if (eq(line, name_n, "If-Match")) p->conditional = true;
        if (eq(line, name_n, "Range")) {
            size_t length = (size_t)(end - value);
            p->range_truncated = length >= sizeof(p->range);
            if (p->range_truncated) length = sizeof(p->range) - 1;
            bool safe = true;
            for (size_t i = 0; i < length; ++i)
                if (!strchr("bytes=0123456789,- ", value[i]) || !value[i]) safe = false;
            if (safe) { memcpy(p->range, value, length); p->range[length] = '\0'; }
            else { p->ambiguous = true; issue(p, "unsupported_range", t); }
        }
    }
    if (!strcmp(p->method, "OTHER") || !strcmp(p->method, "CONNECT")) {
        p->ambiguous = true;
        issue(p, "unsupported_request_method", t);
    }
    checked(dprintf(trace_fd,
        "{\"event\":\"start\",\"client\":\"gdal\",\"pid\":%d,"
        "\"id\":%" PRIu64 ",\"handle\":%" PRIu64 ",\"method\":\"%s\","
        "\"source\":\"%s\",\"range\":\"%s\",\"range_truncated\":%s,"
        "\"conditional\":%s,\"time_unix_s\":%.9f,\"monotonic_s\":%.9f}\n",
        getpid(), p->request_id, p->handle_id, p->method, p->source, p->range,
        p->range_truncated ? "true" : "false", p->conditional ? "true" : "false",
        t.unix_s, t.mono));
}

static void incoming_header(Entry *p, const char *line, size_t n, Stamp t) {
    while (n && (line[n - 1] == '\r' || line[n - 1] == '\n')) --n;
    if (n >= 5 && !strncasecmp(line, "HTTP/", 5)) {
        const char *space = memchr(line, ' ', n);
        p->status = 0;
        if (space && line + n - space >= 4 && isdigit((unsigned char)space[1]) &&
            isdigit((unsigned char)space[2]) && isdigit((unsigned char)space[3]))
            p->status = (space[1] - '0') * 100 + (space[2] - '0') * 10 + space[3] - '0';
        p->have_length = p->header_done = p->bad_length = false;
        p->encoded = p->transfer_encoded = false;
        p->content_length = 0;
        return;
    }
    if (!n) {
        if (p->status < 200) return; /* informational response */
        p->header_done = true;
        if (!strcmp(p->method, "HEAD") ||
            (!strcmp(p->method, "GET") && p->have_length && !p->content_length))
            p->body_complete = t;
        return;
    }
    const char *colon = memchr(line, ':', n);
    if (!colon) return;
    const char *value = colon + 1, *end = line + n;
    while (value < end && (*value == ' ' || *value == '\t')) ++value;
    while (end > value && (end[-1] == ' ' || end[-1] == '\t')) --end;
    size_t name_n = (size_t)(colon - line), value_n = (size_t)(end - value);
    if (eq(line, name_n, "Content-Length")) {
        uint64_t length = 0;
        bool valid = value_n > 0;
        for (const char *c = value; c < end && valid; ++c) {
            if (*c < '0' || *c > '9' || length > (UINT64_MAX - (*c - '0')) / 10)
                valid = false;
            else length = length * 10 + (*c - '0');
        }
        if (!valid || (p->have_length && p->content_length != length)) p->bad_length = true;
        else { p->content_length = length; p->have_length = true; }
    } else if (eq(line, name_n, "Content-Encoding")) {
        if (!eq(value, value_n, "identity")) p->encoded = true;
    } else if (eq(line, name_n, "Transfer-Encoding")) p->transfer_encoded = true;
}

static int debug_callback(CURL *easy, curl_infotype type, char *data,
                          size_t size, void *userdata) {
    if (type != CURLINFO_HEADER_OUT && type != CURLINFO_HEADER_IN &&
        type != CURLINFO_DATA_IN) return 0;
    Stamp t = now();
    pthread_mutex_lock(&lock);
    Entry *p = entry(easy, false);
    if (!p || p != userdata) {
        /* libcurl may use an internal handle; a duplicated handle may also
         * temporarily inherit DEBUGDATA. Never dereference an unregistered
         * userdata pointer, including during cleanup of such a duplicate. */
        Entry *owner = NULL;
        for (Entry *candidate = entries; candidate; candidate = candidate->next)
            if (candidate == userdata) { owner = candidate; break; }
        Entry unknown = {0};
        if (owner) owner->ambiguous = true;
        issue(owner ? owner : &unknown, "unregistered_or_internal_curl_handle", t);
    } else if (p->pending) {
        if (type == CURLINFO_HEADER_OUT) outgoing(p, data, size, t);
        else if (p->active && type == CURLINFO_HEADER_IN) {
            for (size_t at = 0; at < size;) {
                const char *line = data + at;
                const char *lf = memchr(line, '\n', size - at);
                size_t n = lf ? (size_t)(lf - line) + 1 : size - at;
                incoming_header(p, line, n, t);
                at += n;
            }
        } else if (p->active && type == CURLINFO_DATA_IN) {
            if (UINT64_MAX - p->data_bytes < size) p->bad_length = true;
            else p->data_bytes += size;
            p->last_body = t;
            if (p->header_done && p->have_length && p->data_bytes == p->content_length)
                p->body_complete = t;
            else p->body_complete = (Stamp){0, 0};
        }
    }
    pthread_mutex_unlock(&lock);
    /* Never chain the old callback: it could print signed URLs or credentials. */
    return 0;
}

static void prepare(CURL *easy, CURLM *multi, bool easy_mode) {
    pthread_mutex_lock(&lock);
    Entry *p = entry(easy, true);
    if (p->pending && p->easy_mode && !easy_mode) {
        /* Some libcurl builds interpose their easy API's internal multi calls. */
        p->multi = multi;
        pthread_mutex_unlock(&lock);
        return;
    }
    if (p->pending) fail("curl trace: handle reused before completion\n");
    CURL *saved_easy = p->easy;
    uint64_t saved_id = p->handle_id;
    Entry *saved_next = p->next;
    memset(p, 0, sizeof(*p));
    p->easy = saved_easy;
    p->handle_id = saved_id;
    p->next = saved_next;
    p->multi = multi;
    p->easy_mode = easy_mode;
    p->pending = true;
    ++pending_transfers;
    if (curl_easy_setopt(easy, CURLOPT_DEBUGFUNCTION, debug_callback) != CURLE_OK ||
        curl_easy_setopt(easy, CURLOPT_DEBUGDATA, p) != CURLE_OK ||
        curl_easy_setopt(easy, CURLOPT_VERBOSE, 1L) != CURLE_OK)
        fail("curl trace: callback installation failed\n");
    pthread_mutex_unlock(&lock);
}

static void finish(CURL *easy, bool code_known, CURLcode code, const char *reason) {
    Stamp observed = now();
    long status = 0, http_version = 0;
    curl_off_t downloaded = -1;
    CURLcode status_ok = curl_easy_getinfo(easy, CURLINFO_RESPONSE_CODE, &status);
    CURLcode bytes_ok = curl_easy_getinfo(easy, CURLINFO_SIZE_DOWNLOAD_T, &downloaded);
    (void)curl_easy_getinfo(easy, CURLINFO_HTTP_VERSION, &http_version);
    pthread_mutex_lock(&lock);
    Entry *p = entry(easy, false);
    if (!p || !p->pending) { pthread_mutex_unlock(&lock); return; }
    p->pending = false;
    --pending_transfers;
    if (!p->active) {
        issue(p, "transfer_completed_without_header_out", observed);
        pthread_mutex_unlock(&lock);
        return;
    }
    bool head = !strcmp(p->method, "HEAD"), get = !strcmp(p->method, "GET");
    bool exact = !p->ambiguous && p->header_out_count == 1 && p->header_done &&
        !p->bad_length && !p->encoded && !p->transfer_encoded &&
        status_ok == CURLE_OK && status >= 200 && status == p->status &&
        bytes_ok == CURLE_OK && downloaded >= 0 && p->body_complete.mono > 0 &&
        ((head && !p->data_bytes && !downloaded) ||
         (get && p->have_length && p->content_length == p->data_bytes &&
          (uint64_t)downloaded == p->data_bytes)) && (!code_known || code == CURLE_OK);
    if (!exact) ++inexact;
    Stamp endpoint = exact ? p->body_complete : observed;
    ++ends;
    char curl_code[32];
    if (code_known) snprintf(curl_code, sizeof(curl_code), "%d", (int)code);
    else strcpy(curl_code, "null");
    checked(dprintf(trace_fd,
        "{\"event\":\"end\",\"client\":\"gdal\",\"pid\":%d,"
        "\"id\":%" PRIu64 ",\"handle\":%" PRIu64 ",\"status\":%ld,"
        "\"bytes\":%lld,\"debug_data_bytes\":%" PRIu64 ","
        "\"content_length\":%" PRIu64 ",\"have_content_length\":%s,"
        "\"http_version_code\":%ld,\"header_out_count\":%u,"
        "\"content_encoded\":%s,\"transfer_encoded\":%s,"
        "\"curl_code\":%s,\"completion_observed_by\":\"%s\","
        "\"timing_exact\":%s,\"time_unix_s\":%.9f,\"monotonic_s\":%.9f,"
        "\"last_data_unix_s\":%.9f,\"last_data_monotonic_s\":%.9f,"
        "\"body_complete_unix_s\":%.9f,\"body_complete_monotonic_s\":%.9f,"
        "\"observed_done_unix_s\":%.9f,\"observed_done_monotonic_s\":%.9f}\n",
        getpid(), p->request_id, p->handle_id, status, (long long)downloaded,
        p->data_bytes, p->content_length, p->have_length ? "true" : "false",
        http_version, p->header_out_count, p->encoded ? "true" : "false",
        p->transfer_encoded ? "true" : "false",
        curl_code, reason, exact ? "true" : "false", endpoint.unix_s, endpoint.mono,
        p->last_body.unix_s, p->last_body.mono, p->body_complete.unix_s,
        p->body_complete.mono, observed.unix_s, observed.mono));
    p->active = false;
    pthread_mutex_unlock(&lock);
}

static CURLcode traced_easy_perform(CURL *easy) {
    if (trace_fd < 0) return curl_easy_perform(easy);
    prepare(easy, NULL, true);
    CURLcode code = curl_easy_perform(easy);
    finish(easy, true, code, "easy_perform");
    return code;
}
INTERPOSE(traced_easy_perform, curl_easy_perform);

static CURLMcode traced_multi_add_handle(CURLM *multi, CURL *easy) {
    if (trace_fd < 0) return curl_multi_add_handle(multi, easy);
    prepare(easy, multi, false);
    CURLMcode code = curl_multi_add_handle(multi, easy);
    if (code != CURLM_OK) finish(easy, false, CURLE_OK, "multi_add_failed");
    return code;
}
INTERPOSE(traced_multi_add_handle, curl_multi_add_handle);

static CURLMcode traced_multi_remove_handle(CURLM *multi, CURL *easy) {
    CURLMcode code = curl_multi_remove_handle(multi, easy);
    if (trace_fd >= 0 && code == CURLM_OK) finish(easy, false, CURLE_OK, "multi_remove_handle");
    return code;
}
INTERPOSE(traced_multi_remove_handle, curl_multi_remove_handle);

static CURLMsg *traced_multi_info_read(CURLM *multi, int *queued) {
    CURLMsg *message = curl_multi_info_read(multi, queued);
    if (trace_fd >= 0 && message && message->msg == CURLMSG_DONE)
        finish(message->easy_handle, true, message->data.result, "CURLMSG_DONE");
    return message;
}
INTERPOSE(traced_multi_info_read, curl_multi_info_read);

static void traced_easy_reset(CURL *easy) {
    if (trace_fd >= 0) finish(easy, false, CURLE_OK, "easy_reset");
    curl_easy_reset(easy);
}
INTERPOSE(traced_easy_reset, curl_easy_reset);

static void traced_easy_cleanup(CURL *easy) {
    if (trace_fd < 0 || !easy) { curl_easy_cleanup(easy); return; }
    finish(easy, false, CURLE_OK, "easy_cleanup");
    /* Detach before libcurl frees the handle: another thread can immediately
     * allocate a new CURL at that same address. Removing it after cleanup
     * could delete the new transfer's entry. Retain the old allocation until
     * cleanup returns, and disable its finished transfer's diagnostic callback
     * so cleanup never sees a detached DEBUGDATA pointer. */
    pthread_mutex_lock(&lock);
    Entry **p = &entries;
    while (*p && (*p)->easy != easy) p = &(*p)->next;
    Entry *old = *p;
    if (old) *p = old->next;
    pthread_mutex_unlock(&lock);
    (void)curl_easy_setopt(easy, CURLOPT_VERBOSE, 0L);
    (void)curl_easy_setopt(easy, CURLOPT_DEBUGFUNCTION, NULL);
    (void)curl_easy_setopt(easy, CURLOPT_DEBUGDATA, NULL);
    curl_easy_cleanup(easy);
    free(old);
}
INTERPOSE(traced_easy_cleanup, curl_easy_cleanup);

__attribute__((destructor)) static void teardown(void) {
    if (trace_fd < 0) return;
    pthread_mutex_lock(&lock);
    checked(dprintf(trace_fd,
        "{\"event\":\"trace_summary\",\"client\":\"gdal\",\"pid\":%d,"
        "\"starts\":%" PRIu64 ",\"ends\":%" PRIu64 ",\"issues\":%" PRIu64 ","
        "\"inexact\":%" PRIu64 ",\"pending_transfers\":%" PRIu64 "}\n",
        getpid(), starts, ends, issues, inexact, pending_transfers));
    close(trace_fd);
    trace_fd = -1;
    pthread_mutex_unlock(&lock);
}
