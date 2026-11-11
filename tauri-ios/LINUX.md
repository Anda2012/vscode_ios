# Ubuntu (arm64) as the integrated terminal

## What is implemented (all untested on a device)
* `linux/lr_embed.c` - embedding shim on LinuxRunner's core (boot, ptys, resize, bind mounts). It compiles
  against the core's real headers (checked on x86 with the flags meson generates), but it has never run.
* `ios-shell/start.html` - start page: boots Ubuntu + VS Code server, then opens it (falls back to the bundled editor).
* `linux/lr_stub.c` - tiny stand-in used when the core is not built in, so the rest can be tested.
* `src-tauri/src/linux.rs` - first-launch rootfs install (unzip), boot, session output queues.
* `src-tauri/src/server.rs` - `/__fs/term-*` endpoints (open, long-poll read, write, resize, close).
* `ios-fs-ext` - an `Ubuntu` terminal profile and the command "VScode: New Ubuntu Terminal".
* Workflows: `linux-core-probe.yml` (does Ubuntu run, how fast), `ios-tauri-linux-ipa.yml` (full IPA).

## How it fits together
workbench terminal -> extension Pseudoterminal -> `/__fs/term-*` (loopback) -> Rust -> `lr_*` C API ->
LinuxRunner core (arm64 interpreter + Linux syscalls in-process) -> `/bin/bash -l` in Ubuntu.

## Layout inside Linux
* `/` Ubuntu rootfs in app-private storage (fakefs: `data/` + `meta.db`), unpacked from a bundled zip on first launch.
* `/root` -> `<Documents>/Linux` (home folder, visible in Files and in the Explorer).
* `/mnt/vscode` -> `<Documents>` (everything the Explorer shows).

## Known problems and risks
* LinuxRunner's `fs/sock.c` uses Darwin-only socket fields, so the core only builds on Apple hosts
  (macOS / iOS), not on Linux. Hence the probe runs on macOS and the rootfs is built on Linux.
* The iOS static-library build (`scripts/build-linux-core.sh`) is modeled on LinuxRunner's Xcode build
  script but has not run; the first CI run will probably need fixes (extra frameworks, flags).
* Interpreter-only speed (no JIT), iOS memory limits, LiveContainer restrictions.
* The combined app is GPL (LinuxRunner): keep source available if you distribute it.
* Closing a terminal races with output delivery in the shim (the original app uses locks the shim lacks).
