// Real implementation of lr_embed.h on LinuxRunner's core. Modeled on the core's own boot code:
// xX_main_Xx.h / main.c (CLI) and app/AppDelegate.m + app/TerminalViewController.m (iOS app).
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "lr_embed.h"

#include "fs/dev.h"
#include "fs/devices.h"
#include "fs/path.h"
#include "fs/sock.h"
#include "fs/tty.h"
#include "kernel/calls.h"
#include "kernel/fs.h"
#include "kernel/init.h"
#include "kernel/task.h"

int fakefs_bind_mount(const char *linux_path, const char *host_path, bool read_only);
int fakefs_bind_mount_set_owner(const char *linux_path, uint32_t uid, uint32_t gid);

#define MAX_SESSIONS 64

struct session {
    int id;                 // 0 = the hidden init session (output discarded)
    struct tty *tty;        // pty master side; NULL once the guest side is gone
    volatile int in_use;
};

static struct session sessions[MAX_SESSIONS];
static int next_id = 1;
static pthread_mutex_t big_lock = PTHREAD_MUTEX_INITIALIZER;
static lr_output_cb out_cb;
static void *out_user;
static int booted;
static struct session *pending; // handed to the tty driver's init() while pty_open_fake() runs
static char sock_prefix[256];

// ---- tty driver: the "master" end of a guest pty is us -----------------------------------------

static int drv_init(struct tty *tty) {
    tty->data = pending;
    if (pending != NULL)
        pending->tty = tty;
    return 0;
}

static int drv_write(struct tty *tty, const void *buf, size_t len, bool blocking) {
    (void) blocking;
    struct session *s = tty->data;
    if (s != NULL && s->id > 0 && out_cb != NULL)
        out_cb(s->id, buf, len, out_user);
    return (int) len;
}

static void drv_cleanup(struct tty *tty) {
    // Called with ttys_lock held: do not take big_lock here.
    struct session *s = tty->data;
    tty->data = NULL;
    if (s != NULL) {
        s->tty = NULL;
        int id = s->id;
        s->in_use = 0;
        if (id > 0 && out_cb != NULL)
            out_cb(id, NULL, 0, out_user);
    }
}

static struct tty_driver_ops lr_tty_ops = {
    .init = drv_init,
    .write = drv_write,
    .cleanup = drv_cleanup,
};
static struct tty_driver lr_pty_driver = {.ops = &lr_tty_ops};

// ---- helpers ------------------------------------------------------------------------------------

// Splits "a b c" into NUL-separated "a\0b\0c\0\0". Returns argc or -1.
static int split_command(const char *command, char *out, size_t cap) {
    size_t p = 0;
    int argc = 0;
    const char *c = command;
    while (*c != '\0') {
        while (*c == ' ')
            c++;
        if (*c == '\0')
            break;
        size_t start = p;
        while (*c != '\0' && *c != ' ') {
            if (p + 2 >= cap)
                return -1;
            out[p++] = *c++;
        }
        if (p == start)
            break;
        out[p++] = '\0';
        argc++;
    }
    if (argc == 0 || p + 1 > cap)
        return -1;
    out[p] = '\0';
    return argc;
}

static void apply_binds(const char *binds) {
    if (binds == NULL || *binds == '\0')
        return;
    char *copy = strdup(binds);
    if (copy == NULL)
        return;
    char *save = NULL;
    for (char *entry = strtok_r(copy, ";", &save); entry != NULL; entry = strtok_r(NULL, ";", &save)) {
        char *eq = strchr(entry, '=');
        if (eq == NULL || entry[0] != '/')
            continue;
        *eq = '\0';
        const char *guest = entry;
        char *host = eq + 1;
        bool ro = false;
        size_t hl = strlen(host);
        if (hl > 3 && strcmp(host + hl - 3, ":ro") == 0) {
            ro = true;
            host[hl - 3] = '\0';
        }
        if (fakefs_bind_mount(guest, host, ro) == 0)
            fakefs_bind_mount_set_owner(guest, 0, 0); // show iOS files as root-owned inside Linux
    }
    free(copy);
}

static struct session *alloc_session(void) {
    for (int i = 0; i < MAX_SESSIONS; i++) {
        if (!sessions[i].in_use) {
            memset((void *) &sessions[i], 0, sizeof(sessions[i]));
            sessions[i].in_use = 1;
            return &sessions[i];
        }
    }
    return NULL;
}

// Starts `command` on a fresh guest pty. If `adopt_current`, the current task (the freshly created
// first process) becomes the new process; otherwise a new child of init is created.
static int start_process(struct session *s, int adopt_current, uint16_t cols, uint16_t rows, const char *command) {
    int err;
    if (!adopt_current) {
        err = become_new_init_child();
        if (err < 0)
            return err;
    }

    pending = s;
    struct tty *tty = pty_open_fake(&lr_pty_driver);
    pending = NULL;
    if (IS_ERR(tty)) {
        task_discard_unstarted(current);
        current = NULL;
        return (int) PTR_ERR(tty);
    }

    lock(&tty->lock);
    tty_set_winsize(tty, (struct winsize_) {.col = cols, .row = rows});
    unlock(&tty->lock);

    char slave[64];
    snprintf(slave, sizeof(slave), "/dev/pts/%d", tty->num);
    err = create_stdio(slave, TTY_PSEUDO_SLAVE_MAJOR, tty->num);
    tty_release(tty);
    if (err < 0)
        goto fail;

    struct fd *home = generic_open("/root", O_RDONLY_ | O_DIRECTORY_, 0);
    if (!IS_ERR(home))
        fs_chdir(current->fs, home);

    char argv[4096];
    int argc = split_command(command, argv, sizeof(argv));
    if (argc < 0) {
        err = _E2BIG;
        goto fail;
    }
    static const char envp[] =
        "TERM=xterm-256color\0"
        "HOME=/root\0"
        "LANG=C.UTF-8\0"
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\0"
        "PYTHONMALLOC=malloc\0";
    err = do_execve(argv, (size_t) argc, argv, envp);
    if (err < 0)
        goto fail;
    err = task_start(current);
    if (err < 0)
        goto fail;
    current = NULL; // the guest now owns the task
    return 0;

fail:
    task_discard_unstarted(current);
    current = NULL;
    return err;
}

// ---- public API ---------------------------------------------------------------------------------

int lr_boot(const char *fakefs_dir, const char *binds, const char *tmp_dir, lr_output_cb cb, void *user) {
    pthread_mutex_lock(&big_lock);
    if (booted) {
        pthread_mutex_unlock(&big_lock);
        return 0;
    }
    out_cb = cb;
    out_user = user;

    snprintf(sock_prefix, sizeof(sock_prefix), "%ss", tmp_dir != NULL ? tmp_dir : "/tmp/");
    sock_tmp_prefix = sock_prefix;

    char root[MAX_PATH + 1];
    if (realpath(fakefs_dir, root) == NULL) {
        pthread_mutex_unlock(&big_lock);
        return -ENOENT;
    }
    if (strlen(root) + strlen("/data") >= sizeof(root)) {
        pthread_mutex_unlock(&big_lock);
        return -ENAMETOOLONG;
    }
    strcat(root, "/data");

    int err = mount_root(&fakefs, root);
    if (err < 0) {
        pthread_mutex_unlock(&big_lock);
        return err;
    }
    err = become_first_process();
    if (err < 0) {
        pthread_mutex_unlock(&big_lock);
        return err;
    }

    generic_mknodat(AT_PWD, "/dev/null", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_NULL_MINOR));
    generic_mknodat(AT_PWD, "/dev/zero", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_ZERO_MINOR));
    generic_mknodat(AT_PWD, "/dev/full", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_FULL_MINOR));
    generic_mknodat(AT_PWD, "/dev/random", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_RANDOM_MINOR));
    generic_mknodat(AT_PWD, "/dev/urandom", S_IFCHR | 0666, dev_make(MEM_MAJOR, DEV_URANDOM_MINOR));
    generic_mknodat(AT_PWD, "/dev/tty", S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_TTY_MINOR));
    generic_mknodat(AT_PWD, "/dev/console", S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_CONSOLE_MINOR));
    generic_mknodat(AT_PWD, "/dev/ptmx", S_IFCHR | 0666, dev_make(TTY_ALTERNATE_MAJOR, DEV_PTMX_MINOR));

    do_mount(&procfs, "proc", "/proc", "", 0);
    do_mount(&devptsfs, "devpts", "/dev/pts", "", 0);
    apply_binds(binds);

    // A quiet long-lived init (pid 1) so later sessions have a parent; its pty output is discarded.
    struct session *init = alloc_session();
    if (init == NULL) {
        pthread_mutex_unlock(&big_lock);
        return -ENOMEM;
    }
    init->id = 0;
    err = start_process(init, 1, 80, 24, "/bin/sleep infinity");
    pthread_mutex_unlock(&big_lock);
    if (err < 0)
        return err;
    booted = 1;
    return 0;
}

int lr_session_open(uint16_t cols, uint16_t rows, const char *command) {
    pthread_mutex_lock(&big_lock);
    if (!booted) {
        pthread_mutex_unlock(&big_lock);
        return -EINVAL;
    }
    struct session *s = alloc_session();
    if (s == NULL) {
        pthread_mutex_unlock(&big_lock);
        return -ENOMEM;
    }
    s->id = next_id++;
    int id = s->id;
    int err = start_process(s, 0, cols, rows, command != NULL ? command : "/bin/bash -l");
    if (err < 0)
        s->in_use = 0;
    pthread_mutex_unlock(&big_lock);
    return err < 0 ? err : id;
}

static struct session *find_session(int id) {
    for (int i = 0; i < MAX_SESSIONS; i++)
        if (sessions[i].in_use && sessions[i].id == id)
            return &sessions[i];
    return NULL;
}

int lr_session_write(int session, const uint8_t *data, size_t len) {
    struct session *s = find_session(session);
    if (s == NULL || s->tty == NULL)
        return -ENOENT;
    tty_input(s->tty, (const char *) data, len, false);
    return 0;
}

int lr_session_resize(int session, uint16_t cols, uint16_t rows) {
    struct session *s = find_session(session);
    if (s == NULL || s->tty == NULL)
        return -ENOENT;
    lock(&s->tty->lock);
    tty_set_winsize(s->tty, (struct winsize_) {.col = cols, .row = rows});
    unlock(&s->tty->lock);
    return 0;
}

int lr_session_alive(int session) {
    // Same test the core uses for a "half-closed" pty master: the slave was opened and now only the
    // master's own reference is left (the process closed its stdio), or the slave was hung up.
    struct session *s = find_session(session);
    if (s == NULL || s->tty == NULL)
        return 0;
    struct tty *slave = s->tty->pty.other;
    if (slave == NULL)
        return 0;
    lock(&slave->lock);
    int gone = slave->ever_opened && (slave->refcount == 1 || slave->hung_up);
    unlock(&slave->lock);
    return !gone;
}

void lr_session_close(int session) {
    struct session *s = find_session(session);
    if (s == NULL || s->tty == NULL)
        return;
    lock(&s->tty->lock);
    tty_hangup(s->tty); // releases tty->lock
}
