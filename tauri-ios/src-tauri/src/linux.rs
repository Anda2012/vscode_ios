//! Glue between the app and the Linux core (lr_embed.h): rootfs install, boot, terminal sessions.

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::server::{ReadResult, Terminal};

type OutputCb = extern "C" fn(c_int, *const u8, usize, *mut c_void);

extern "C" {
    fn lr_boot(fakefs_dir: *const c_char, binds: *const c_char, tmp_dir: *const c_char, cb: Option<OutputCb>, user: *mut c_void) -> c_int;
    fn lr_session_open(cols: u16, rows: u16, command: *const c_char) -> c_int;
    fn lr_session_write(session: c_int, data: *const u8, len: usize) -> c_int;
    fn lr_session_resize(session: c_int, cols: u16, rows: u16) -> c_int;
    fn lr_session_close(session: c_int);
    fn lr_session_alive(session: c_int) -> c_int;
}

#[derive(Default)]
struct Out {
    state: Mutex<(VecDeque<u8>, bool)>, // (pending bytes, closed)
    cv: Condvar,
}

static OUTPUTS: Mutex<Option<HashMap<i32, Arc<Out>>>> = Mutex::new(None);
static STATE: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
// VS Code server inside Ubuntu: (phase, message, url). Phases: "" (not started), starting, ready, fallback.
static VS: Mutex<(String, String, String)> = Mutex::new((String::new(), String::new(), String::new()));

static T0: OnceLock<Instant> = OnceLock::new();
fn since_start() -> u64 {
    T0.get_or_init(Instant::now).elapsed().as_secs()
}

// Human-readable trace of what the boot did, shown on the start page (and on failure).
static LOG: Mutex<String> = Mutex::new(String::new());
static VS_ID: Mutex<Option<i32>> = Mutex::new(None);

fn log(line: &str) {
    let mut l = LOG.lock().unwrap();
    l.push_str(line);
    l.push('\n');
    if l.len() > 6000 {
        let cut = l.len() - 6000;
        let at = l.char_indices().map(|(i, _)| i).find(|&i| i >= cut).unwrap_or(0);
        l.drain(..at);
    }
}

fn diagnostics() -> String {
    let mut s = LOG.lock().unwrap().clone();
    if let Some(id) = *VS_ID.lock().unwrap() {
        let t = tail_of(id);
        if !t.trim().is_empty() {
            s.push_str("--- server output ---\n");
            s.push_str(t.trim_end());
        }
    }
    s
}

/// Guest port the VS Code server listens on (guest sockets are real host sockets).
pub const VS_PORT: u16 = 3000;

fn set_vs(phase: &str, message: &str, url: &str) {
    *VS.lock().unwrap() = (phase.to_string(), message.to_string(), url.to_string());
}

fn http_up(port: u16) -> bool {
    use std::io::{Read, Write};
    let Ok(addr) = format!("127.0.0.1:{port}").parse() else { return false };
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    if s.write_all(b"GET / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = [0u8; 16];
    match s.read(&mut buf) {
        Ok(n) if n >= 12 => {
            let line = String::from_utf8_lossy(&buf[..n]);
            line.starts_with("HTTP/1.") && matches!(&line[9..10], "2" | "3")
        }
        _ => false,
    }
}

/// Last bytes of a session's buffered output (for error messages).
fn tail_of(id: i32) -> String {
    let out = out_for(id);
    let st = out.state.lock().unwrap();
    let bytes: Vec<u8> = st.0.iter().rev().take(600).rev().copied().collect();
    String::from_utf8_lossy(&bytes).replace('\r', "")
}

fn alive(id: i32) -> bool {
    unsafe { lr_session_alive(id) != 0 }
}

/// A session is over when the core said so, or when its process has exited (the core reports that only
/// through the pty state, not through a callback).
fn session_closed(id: i32) -> bool {
    let flagged = out_for(id).state.lock().unwrap().1;
    flagged || !alive(id)
}

/// Runs one command to completion in a throwaway session and returns its output.
fn run_probe(cmd: &str, timeout_secs: u64) -> Result<String, String> {
    let c = CString::new(cmd).unwrap();
    let id = unsafe { lr_session_open(100, 30, c.as_ptr()) };
    if id < 0 {
        return Err(format!("could not start `{cmd}` (error {id})"));
    }
    let started = Instant::now();
    while !session_closed(id) {
        if started.elapsed() > Duration::from_secs(timeout_secs) {
            unsafe { lr_session_close(id) };
            return Err(format!("`{cmd}` did not finish within {timeout_secs}s. Output so far:\n{}", tail_of(id).trim_end()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let out = out_for(id);
    let bytes: Vec<u8> = out.state.lock().unwrap().0.drain(..).collect();
    if let Some(map) = OUTPUTS.lock().unwrap().as_mut() {
        map.remove(&id);
    }
    Ok(String::from_utf8_lossy(&bytes).replace('\r', ""))
}

/// Starts OpenVSCode Server in a hidden Linux session and waits for it to answer on loopback.
fn start_vscode(documents_dir: &Path) {
    if documents_dir.join("Linux").join(".use-web-workbench").exists() {
        set_vs("fallback", "Basic editor selected (Linux/.use-web-workbench exists).", "");
        return;
    }
    let timeout = std::env::var("LR_VS_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(900u64);
    let probe_timeout = std::env::var("LR_PROBE_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok());
    for (cmd, secs, what) in [("/bin/uname -a", 90u64, "the Linux layer"), ("/opt/openvscode-server/node --version", 240, "Node.js")] {
        set_vs("starting", &format!("Checking {what}..."), "");
        log(&format!("$ {cmd}"));
        match run_probe(cmd, probe_timeout.unwrap_or(secs)) {
            Ok(out) => log(out.trim()),
            Err(e) => {
                log(&e);
                set_vs("fallback", &format!("Ubuntu check failed: {what} did not run."), "");
                return;
            }
        }
    }
    set_vs("starting", "Starting VS Code inside Ubuntu...", "");
    let cmd = CString::new(format!(
        "/opt/openvscode-server/bin/openvscode-server --host 127.0.0.1 --port {VS_PORT} --without-connection-token"
    ))
    .unwrap();
    let id = unsafe { lr_session_open(120, 40, cmd.as_ptr()) };
    if id < 0 {
        set_vs("fallback", &format!("Could not start VS Code inside Ubuntu (error {id})."), "");
        return;
    }
    *VS_ID.lock().unwrap() = Some(id);
    log("$ openvscode-server (started)");
    let started = Instant::now();
    loop {
        if http_up(VS_PORT) {
            set_vs("ready", "", &format!("http://127.0.0.1:{VS_PORT}/?folder=/mnt/vscode"));
            return;
        }
        if session_closed(id) {
            set_vs("fallback", "VS Code inside Ubuntu exited.", "");
            return;
        }
        if started.elapsed() > Duration::from_secs(timeout) {
            set_vs("fallback", &format!("VS Code inside Ubuntu did not start within {timeout} seconds."), "");
            return;
        }
        set_vs("starting", &format!("Starting VS Code inside Ubuntu... {}s\n(first start can take several minutes)", started.elapsed().as_secs()), "");
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn set_state(state: &str, message: &str) {
    *STATE.lock().unwrap() = (state.to_string(), message.to_string());
}

fn out_for(id: i32) -> Arc<Out> {
    let mut guard = OUTPUTS.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).entry(id).or_default().clone()
}

extern "C" fn on_output(session: c_int, data: *const u8, len: usize, _user: *mut c_void) {
    let out = out_for(session);
    let mut st = out.state.lock().unwrap();
    if data.is_null() {
        st.1 = true;
    } else {
        // SAFETY: the core guarantees `data` points to `len` readable bytes for this call.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        st.0.extend(bytes.iter().copied());
        if st.0.len() > 4 << 20 {
            let excess = st.0.len() - (4 << 20);
            st.0.drain(..excess); // never let a stalled reader grow memory without bound
        }
    }
    out.cv.notify_all();
}

pub(crate) fn unzip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    unzip_with_progress(zip_path, dest, &mut |_, _| {})
}

fn unzip_with_progress(zip_path: &Path, dest: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let total = archive.len();
    for i in 0..total {
        if i % 250 == 0 {
            progress(i, total);
        }
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue }; // skips ../ tricks
        let target = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = fs::File::create(&target).map_err(|e| e.to_string())?;
            io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub struct Paths {
    pub support_dir: PathBuf,   // private app storage (rootfs lives here)
    pub documents_dir: PathBuf, // visible in the Files app
    pub tmp_dir: PathBuf,       // short path for AF_UNIX sockets
    pub rootfs_zip: Option<PathBuf>,
}

/// Installs the rootfs on first launch, then boots the core. Runs on its own thread.
pub fn init(paths: Paths) {
    since_start();
    std::thread::spawn(move || {
        if cfg!(lr_stub) {
            let _ = unsafe { lr_boot(std::ptr::null(), std::ptr::null(), std::ptr::null(), Some(on_output), std::ptr::null_mut()) };
            set_state("ready", "stub (no Linux core in this build)");
            if std::env::var("LR_FORCE_VS").is_ok() { // tests only
                start_vscode(&paths.documents_dir);
            } else {
                set_vs("fallback", "No Linux core in this build.", "");
            }
            return;
        }
        let rootfs = paths.support_dir.join("ubuntu-fakefs");
        if !rootfs.join("data").is_dir() {
            let Some(zip_path) = paths.rootfs_zip.clone() else {
                set_state("no-rootfs", "This build does not include the Ubuntu root filesystem.");
                return;
            };
            set_state("installing", "Installing Ubuntu (first launch only)...");
            log("installing the bundled root filesystem");
            let staging = paths.support_dir.join("ubuntu-fakefs.partial");
            let _ = fs::remove_dir_all(&staging);
            if let Err(e) = fs::create_dir_all(&staging).map_err(|e| e.to_string()).and_then(|_| {
                unzip_with_progress(&zip_path, &staging, &mut |done, total| {
                    set_state("installing", &format!("Installing Ubuntu (first launch only)... {done} of {total} files"));
                })
            }) {
                set_state("error", &format!("Could not install Ubuntu: {e}"));
                return;
            }
            if let Err(e) = fs::rename(&staging, &rootfs) {
                set_state("error", &format!("Could not finish installing Ubuntu: {e}"));
                return;
            }
        }

        log("root filesystem ready; booting the Linux core");
        set_state("booting", "Starting Ubuntu...");
        let _ = fs::create_dir_all(paths.documents_dir.join("Linux"));
        let binds = format!("/root={}/Linux;/mnt/vscode={}", paths.documents_dir.display(), paths.documents_dir.display());
        let (Ok(root_c), Ok(binds_c), Ok(tmp_c)) = (
            CString::new(rootfs.to_string_lossy().as_bytes()),
            CString::new(binds),
            CString::new(format!("{}/", paths.tmp_dir.display())),
        ) else {
            set_state("error", "Invalid path");
            return;
        };
        let rc = unsafe { lr_boot(root_c.as_ptr(), binds_c.as_ptr(), tmp_c.as_ptr(), Some(on_output), std::ptr::null_mut()) };
        if rc < 0 {
            set_state("error", &format!("Linux core failed to start (error {rc})"));
        } else {
            log("Linux core booted");
            set_state("ready", "ok");
            start_vscode(&paths.documents_dir);
        }
    });
}

pub struct Linux;

impl Terminal for Linux {
    fn status(&self) -> (String, String) {
        let s = STATE.lock().unwrap().clone();
        if s.0.is_empty() { ("starting".into(), String::new()) } else { s }
    }

    fn boot_status(&self) -> (String, String, String) {
        let (phase, message, url) = {
            let vs = VS.lock().unwrap().clone();
            if !vs.0.is_empty() {
                vs
            } else {
                let st = self.status();
                match st.0.as_str() {
                    "error" | "no-rootfs" => ("fallback".into(), st.1, String::new()),
                    _ => ("starting".into(), if st.1.is_empty() { "Starting Linux...".into() } else { st.1 }, String::new()),
                }
            }
        };
        if phase == "ready" {
            return (phase, message, url);
        }
        let diag = diagnostics();
        let mut msg = format!("{message}\n[{}s since launch]", since_start());
        if !diag.trim().is_empty() {
            msg.push_str("\n\n");
            msg.push_str(diag.trim_end());
        }
        (phase, msg, url)
    }

    fn open(&self, cols: u16, rows: u16) -> Result<i32, String> {
        if self.status().0 != "ready" {
            return Err("Linux is not ready".into());
        }
        let cmd = CString::new("/bin/bash -l").unwrap();
        let id = unsafe { lr_session_open(cols, rows, cmd.as_ptr()) };
        if id < 0 { Err(format!("could not start a session (error {id})")) } else { Ok(id) }
    }

    fn read(&self, id: i32, wait: Duration) -> ReadResult {
        let out = out_for(id);
        let deadline = Instant::now() + wait;
        let forget = || {
            if let Some(map) = OUTPUTS.lock().unwrap().as_mut() {
                map.remove(&id); // session is over and fully read
            }
        };
        loop {
            {
                let mut st = out.state.lock().unwrap();
                if st.0.is_empty() && !st.1 {
                    let slice = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(500));
                    if !slice.is_zero() {
                        st = out.cv.wait_timeout(st, slice).unwrap().0;
                    }
                }
                if !st.0.is_empty() {
                    return ReadResult::Data(st.0.drain(..).collect());
                }
                if st.1 {
                    drop(st);
                    forget();
                    return ReadResult::Closed;
                }
            }
            // Checked without holding our lock: the core takes its own locks inside.
            if !alive(id) {
                let mut st = out.state.lock().unwrap();
                if !st.0.is_empty() {
                    return ReadResult::Data(st.0.drain(..).collect());
                }
                drop(st);
                forget();
                return ReadResult::Closed;
            }
            if Instant::now() >= deadline {
                return ReadResult::Timeout;
            }
        }
    }

    fn write(&self, id: i32, data: &[u8]) -> Result<(), String> {
        let rc = unsafe { lr_session_write(id, data.as_ptr(), data.len()) };
        if rc < 0 { Err("no such session".into()) } else { Ok(()) }
    }

    fn resize(&self, id: i32, cols: u16, rows: u16) -> Result<(), String> {
        let rc = unsafe { lr_session_resize(id, cols, rows) };
        if rc < 0 { Err("no such session".into()) } else { Ok(()) }
    }

    fn close(&self, id: i32) {
        unsafe { lr_session_close(id) };
    }
}
