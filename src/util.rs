//! Stateless building blocks: shell execution, ports, copies, URL opening.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use anyhow::{bail, Context, Result};

use crate::config::CopyMode;

/// Resolves `..` and `.` without touching the disk (the path may not exist yet).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn shell() -> String {
    // The user's shell may be fish or nu, whose syntax is not what hooks expect.
    // `sh` is the contract.
    std::env::var("WT_SHELL").unwrap_or_else(|_| "sh".to_string())
}

/// Runs a shell command in the foreground (inherited stdio), failing on a non-zero code.
pub fn run(cmd: &str, cwd: &Path, env: &BTreeMap<String, String>) -> Result<()> {
    let status = Command::new(shell())
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env)
        .status()
        .with_context(|| t!("err.spawn_failed", command = cmd).to_string())?;
    if !status.success() {
        bail!(
            "{}",
            t!(
                "err.command_failed",
                code = status.code().unwrap_or(-1),
                command = cmd
            )
        );
    }
    Ok(())
}

/// A progress event, meant for display.
///
/// Colours are not decided here: the same action must be able to write to a terminal
/// (ANSI sequences) or into a ratatui panel (styles) without being duplicated.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Current step (command started, file copied…).
    Info(String),
    Ok(String),
    Warn(String),
    /// Line produced by the command itself.
    Out(String),
    /// End of the action: `Some(error)` when it failed.
    Done(Option<String>),
}

/// Runs a shell command, relaying its output line by line as it comes.
///
/// stdin is closed: a hook waiting for input would hang with nobody able to answer,
/// since the terminal is busy showing the interface.
pub fn run_streamed(
    cmd: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    tx: &Sender<Msg>,
) -> Result<()> {
    let mut child = Command::new(shell())
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| t!("err.spawn_failed", command = cmd).to_string())?;

    // Two concurrent readers: with a single one, a command chatty on stderr while stdout
    // is full would fill the pipe and block.
    let pumps: Vec<_> = [
        child.stdout.take().map(PipeRead::Out),
        child.stderr.take().map(PipeRead::Err),
    ]
    .into_iter()
    .flatten()
    .map(|pipe| {
        let tx = tx.clone();
        std::thread::spawn(move || pipe.pump(tx))
    })
    .collect();

    let status = child.wait()?;
    for p in pumps {
        let _ = p.join();
    }
    if !status.success() {
        bail!(
            "{}",
            t!(
                "err.command_failed",
                code = status.code().unwrap_or(-1),
                command = cmd
            )
        );
    }
    Ok(())
}

enum PipeRead {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

impl PipeRead {
    fn pump(self, tx: Sender<Msg>) {
        fn drain<R: std::io::Read>(r: R, tx: Sender<Msg>) {
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                if tx.send(Msg::Out(line)).is_err() {
                    return; // nobody is listening any more
                }
            }
        }
        match self {
            PipeRead::Out(s) => drain(s, tx),
            PipeRead::Err(s) => drain(s, tx),
        }
    }
}

/// Standard output of a shell command, never failing: TUI previews must not break just
/// because `docker` is missing.
pub fn capture(cmd: &str, cwd: &Path, env: &BTreeMap<String, String>) -> String {
    Command::new(shell())
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Exit status of a silent shell command (`[status] up`, a prompt's `when`).
pub fn succeeds(cmd: &str, cwd: &Path, env: &BTreeMap<String, String>) -> bool {
    Command::new(shell())
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// First free port from `base`, skipping those reserved by other worktrees — even
/// stopped ones, so their ports stay stable across restarts.
pub fn alloc_port(base: u16, reserved: &[u16]) -> Result<u16> {
    let mut p = base;
    loop {
        if !reserved.contains(&p) && TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return Ok(p);
        }
        p = p
            .checked_add(1)
            .with_context(|| t!("err.no_free_port", base = base).to_string())?;
    }
}

/// Reproduces `from` into `to`. Hardlinks share inodes: copying a 300 MB `vendor/`
/// takes a few seconds and almost no disk space.
pub fn copy_tree(from: &Path, to: &Path, mode: CopyMode) -> Result<()> {
    if mode == CopyMode::Symlink {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(from, to).with_context(|| {
            t!(
                "err.symlink_failed",
                from = from.display(),
                to = to.display()
            )
            .to_string()
        })?;
        return Ok(());
    }

    let meta = fs::symlink_metadata(from)
        .with_context(|| t!("err.read_failed", path = from.display()).to_string())?;

    if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, to)?;
        return Ok(());
    }

    if meta.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()), mode)?;
        }
        return Ok(());
    }

    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    match mode {
        // A hardlink fails across filesystems: fall back to a copy rather than giving
        // up on the whole tree.
        CopyMode::Hardlink => {
            if fs::hard_link(from, to).is_err() {
                fs::copy(from, to)?;
            }
        }
        _ => {
            fs::copy(from, to)?;
        }
    }
    Ok(())
}

pub fn is_wsl() -> bool {
    if std::env::var_os("WSL_DISTRO_NAME").is_some() {
        return true;
    }
    fs::read_to_string("/proc/version")
        .map(|v| v.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

/// Opens a URL in the user's browser.
///
/// Under WSL the browser lives on the Windows side: `xdg-open` is usually missing, and
/// when present it looks for a Linux browser with no display server.
pub fn open_url(url: &str) -> Result<()> {
    let mut candidates: Vec<&str> = Vec::new();
    if is_wsl() {
        candidates.push("wslview");
        candidates.push("explorer.exe");
    }
    candidates.extend(["xdg-open", "open"]);

    for bin in candidates {
        if which(bin).is_none() {
            continue;
        }
        // explorer.exe returns a non-zero code even when it did open the URL.
        let _ = Command::new(bin)
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        return Ok(());
    }
    bail!("{}", t!("err.no_browser", url = url));
}

/// Equivalent of `command -v`, including for an absolute path (`WT_IDE=/mnt/c/…exe`).
pub fn which(bin: &str) -> Option<PathBuf> {
    let p = Path::new(bin);
    if p.is_absolute() || bin.contains('/') {
        return p.is_file().then(|| p.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let cand = dir.join(bin);
        cand.is_file().then_some(cand)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_parent_segments() {
        assert_eq!(
            normalize(Path::new("/home/x/project/../project-wt/demo")),
            PathBuf::from("/home/x/project-wt/demo")
        );
    }

    #[test]
    fn skips_reserved_ports() {
        let p = alloc_port(45123, &[45123, 45124]).unwrap();
        assert!(p >= 45125);
    }
}
