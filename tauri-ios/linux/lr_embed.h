// Embeddable terminal API on top of LinuxRunner's core (an arm64 Linux user-space runner).
// Implemented by lr_embed.c (real core) or lr_stub.c (stand-in for builds without the core).
#ifndef LR_EMBED_H
#define LR_EMBED_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Called with terminal output. `data == NULL && len == 0` means the session ended. May be called from
// any thread and must not block.
typedef void (*lr_output_cb)(int session, const uint8_t *data, size_t len, void *user);

// Boots the guest kernel once: mounts the fakefs root at `fakefs_dir`, applies `binds`
// ("/guest/path=/host/path;/other=/host2:ro"), starts a background init. `tmp_dir` is a short host
// directory for AF_UNIX sockets. Returns 0 or a negative Linux errno.
int lr_boot(const char *fakefs_dir, const char *binds, const char *tmp_dir, lr_output_cb cb, void *user);

// Starts a shell session on a new guest pty. `command` is split on spaces (e.g. "/bin/bash -l").
// Returns a session id (> 0) or a negative errno.
int lr_session_open(uint16_t cols, uint16_t rows, const char *command);
int lr_session_write(int session, const uint8_t *data, size_t len);
int lr_session_resize(int session, uint16_t cols, uint16_t rows);
void lr_session_close(int session);
// 1 while the session's process is running, 0 once it has exited (or the session is gone).
int lr_session_alive(int session);

#ifdef __cplusplus
}
#endif
#endif
