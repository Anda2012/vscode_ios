// Stand-in for lr_embed.c in builds that don't include LinuxRunner's core (desktop checks, tests).
// Offers a tiny "shell" so the whole terminal path (extension -> server -> Rust -> C) can be exercised.
#include <pthread.h>
#include <stdlib.h>
#include <string.h>

#include "lr_embed.h"

static lr_output_cb cb;
static void *user;
static int next = 1;
static int open_ids[64];
static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;

static void say(int id, const char *s) { if (cb) cb(id, (const uint8_t *) s, strlen(s), user); }

int lr_boot(const char *d, const char *b, const char *t, lr_output_cb c, void *u) {
    (void) d; (void) b; (void) t;
    cb = c; user = u;
    return 0;
}

int lr_session_open(uint16_t cols, uint16_t rows, const char *command) {
    (void) cols; (void) rows; (void) command;
    pthread_mutex_lock(&m);
    int id = next++;
    open_ids[id % 64] = id;
    pthread_mutex_unlock(&m);
    if (command && strstr(command, "uname")) {
        if (!getenv("LR_STUB_HANG")) { say(id, "Linux stub 6.0.0 aarch64\r\n"); if (cb) cb(id, NULL, 0, user); }
        return id;
    }
    if (command && strstr(command, "--version")) {
        say(id, "v22.0.0-stub\r\n"); if (cb) cb(id, NULL, 0, user);
        return id;
    }
    say(id, "VScode stub shell (the Linux core is not part of this build)\r\n$ ");
    return id;
}

int lr_session_write(int id, const uint8_t *data, size_t len) {
    for (size_t i = 0; i < len; i++) {
        if (data[i] == '\r') say(id, "\r\nno Linux here\r\n$ ");
        else if (cb) cb(id, &data[i], 1, user); // echo
    }
    return 0;
}

int lr_session_resize(int id, uint16_t cols, uint16_t rows) { (void) id; (void) cols; (void) rows; return 0; }

void lr_session_close(int id) { if (cb) cb(id, NULL, 0, user); }
