//! `wt up @`: the main repository's command, run in the foreground.
//!
//! wt stays its parent for as long as it runs. The command gets a process group of its
//! own — `wt down @` stops it and everything it started, nothing else — and, when wt
//! runs in a terminal, that terminal: Ctrl-C reaches the command, as if typed in a
//! shell. Signals sent to wt itself (the terminal closing, `kill`) are passed on.
//!
//! "Started" is a lock, not a pid: `<root>/.wt/@.lock`, held by `wt up @` and released
//! by the kernel however it ends. A pid written in a file outlives a crash and, after a
//! reboot, names some other process — which `wt down @` would then kill.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::state::{self, MAIN_SLUG};

pub fn lock_path(root: &Path) -> PathBuf {
    root.join(state::STATE_DIR)
        .join(format!("{MAIN_SLUG}.lock"))
}

/// The lock of a running `wt up @`; dropping it is the stop.
pub struct Running {
    file: File,
}

impl Running {
    /// Takes the lock, or `None` when another `wt up @` holds it. A few tries: `running`
    /// takes it for an instant to look, and a start must not fail on that.
    pub fn acquire(path: &Path) -> Result<Option<Running>> {
        fs::create_dir_all(path.parent().unwrap())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| t!("err.write_failed", path = path.display()).to_string())?;
        for attempt in 0..5 {
            match file.try_lock() {
                Ok(()) => return Ok(Some(Running { file })),
                Err(TryLockError::WouldBlock) if attempt < 4 => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(e)) => return Err(e.into()),
            }
        }
        Ok(None)
    }

    /// The process group `wt down @` must stop: the one of the command running now.
    fn record(&mut self, group: i32) -> Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        write!(self.file, "{group}")?;
        Ok(())
    }
}

/// Is a `wt up @` running? Its command's process group if so — 0 while it has none yet.
pub fn running(path: &Path) -> Option<i32> {
    let mut file = File::open(path).ok()?;
    match file.try_lock_shared() {
        Ok(()) => None,
        Err(TryLockError::WouldBlock) => {
            let mut body = String::new();
            let _ = file.read_to_string(&mut body);
            Some(body.trim().parse().unwrap_or(0))
        }
        Err(TryLockError::Error(_)) => None,
    }
}

/// `wt down @`: SIGTERM to the command's group, SIGKILL if it is still there after ten
/// seconds. `false` when nothing was running.
pub fn stop(path: &Path) -> Result<bool> {
    let Some(mut group) = running(path) else {
        return Ok(false);
    };
    // Between two commands of `[main] up`, or just started: the group is on its way.
    let deadline = Instant::now() + Duration::from_secs(2);
    while group <= 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        match running(path) {
            Some(g) => group = g,
            None => return Ok(true),
        }
    }
    if group <= 0 {
        bail!("{}", t!("err.main_no_group"));
    }
    for (signal, wait) in [(libc::SIGTERM, 10), (libc::SIGKILL, 5)] {
        // The group changes when `[main] up` moves to its next command: always the
        // recorded one, which is the one running.
        if let Some(g) = running(path).filter(|g| *g > 0) {
            group = g;
        }
        unsafe { libc::killpg(group, signal) };
        let deadline = Instant::now() + Duration::from_secs(wait);
        while Instant::now() < deadline {
            if running(path).is_none() {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    bail!("{}", t!("err.main_still_running", group = group))
}

/// Did the command end because it was stopped (Ctrl-C, `wt down @`, the terminal
/// closing) rather than on its own? A shell reports a child killed by a signal as
/// 128 + the signal.
pub fn stopped(status: &ExitStatus) -> bool {
    status.signal().is_some() || status.code().is_some_and(|c| (129..=159).contains(&c))
}

/// The command's process group, for the signal handler.
static GROUP: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward(signal: libc::c_int) {
    let group = GROUP.load(Ordering::SeqCst);
    if group > 0 {
        unsafe { libc::killpg(group, signal) };
    }
}

const FORWARDED: [libc::c_int; 4] = [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM];

/// Runs `cmd` with `sh -c` in `dir`, in the foreground, and waits for it.
pub fn run(
    cmd: &str,
    dir: &Path,
    env: &BTreeMap<String, String>,
    running: &mut Running,
) -> Result<ExitStatus> {
    let tty = foreground_terminal();
    let previous = install(forward as extern "C" fn(libc::c_int) as libc::sighandler_t);

    let mut command = Command::new(crate::util::shell());
    command.arg("-c").arg(cmd).current_dir(dir).envs(env);
    // In the child, between fork and exec: only async-signal-safe calls.
    unsafe {
        command.pre_exec(move || {
            libc::setpgid(0, 0);
            if let Some(fd) = tty {
                give_terminal(fd, libc::getpid());
            }
            Ok(())
        });
    }
    let spawned = command.spawn();
    let result = spawned
        .with_context(|| t!("err.spawn_failed", command = cmd).to_string())
        .and_then(|mut child| {
            let group = child.id() as i32;
            // The same from this side: whichever runs first, the other finds it done, and
            // the command never reads the terminal before owning it.
            unsafe { libc::setpgid(group, group) };
            if let Some(fd) = tty {
                give_terminal(fd, group);
            }
            GROUP.store(group, Ordering::SeqCst);
            let _ = running.record(group);
            let status = child.wait();
            GROUP.store(0, Ordering::SeqCst);
            // What the command started and left behind (a server it did not wait for)
            // would go on holding its port.
            unsafe { libc::killpg(group, libc::SIGTERM) };
            let _ = running.record(0);
            status.map_err(Into::into)
        });

    if let Some(fd) = tty {
        give_terminal(fd, unsafe { libc::getpgrp() });
    }
    restore(&previous);
    result
}

/// Standard input, when it is a terminal whose foreground is wt: the terminal to hand
/// over. Started in the background (`wt up @ &`), wt leaves it to whoever has it.
fn foreground_terminal() -> Option<libc::c_int> {
    if !std::io::stdin().is_terminal() {
        return None;
    }
    let fd = libc::STDIN_FILENO;
    (unsafe { libc::tcgetpgrp(fd) == libc::getpgrp() }).then_some(fd)
}

/// Makes `group` the terminal's foreground. From a background group this raises
/// SIGTTOU, which stops the caller unless blocked: blocked for the call.
fn give_terminal(fd: libc::c_int, group: libc::pid_t) {
    unsafe {
        let mut block: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGTTOU);
        libc::sigprocmask(libc::SIG_BLOCK, &block, &mut old);
        libc::tcsetpgrp(fd, group);
        libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
    }
}

/// Installs `handler` for the forwarded signals; returns what was there. A handler, not
/// SIG_IGN: an ignored signal stays ignored across exec, and the command would inherit
/// it.
fn install(handler: libc::sighandler_t) -> Vec<(libc::c_int, libc::sigaction)> {
    FORWARDED
        .iter()
        .map(|&signal| unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, &action, &mut old);
            (signal, old)
        })
        .collect()
}

fn restore(previous: &[(libc::c_int, libc::sigaction)]) {
    for (signal, old) in previous {
        unsafe { libc::sigaction(*signal, old, std::ptr::null_mut()) };
    }
}
