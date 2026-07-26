//! Thin wrapper around the `git` command.
//!
//! No libgit2: worktrees are exactly what `git worktree` does, and another C dependency
//! to run six commands would not pay for itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| t!("err.git_missing").to_string())?;
    if !out.status.success() {
        bail!(
            "{}",
            t!(
                "err.git_failed",
                args = args.join(" "),
                message = String::from_utf8_lossy(&out.stderr).trim()
            )
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Same, but a failure yields None instead of an error (optional lookups).
fn git_opt(dir: &Path, args: &[&str]) -> Option<String> {
    git(dir, args).ok()
}

fn git_status(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Root of the main repository, even when called from a worktree: `--git-common-dir`
/// always points at the original repository's `.git`.
pub fn main_repo(start: &Path) -> Result<PathBuf> {
    let common = git(start, &["rev-parse", "--git-common-dir"])
        .with_context(|| t!("err.not_a_repo", path = start.display()).to_string())?;
    let common = PathBuf::from(&common);
    let common = if common.is_absolute() {
        common
    } else {
        start.join(common)
    };
    let common = common.canonicalize().unwrap_or(common);
    common
        .parent()
        .map(|p| p.to_path_buf())
        .with_context(|| t!("err.no_main_repo").to_string())
}

pub fn current_branch(dir: &Path) -> String {
    git_opt(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "?".into())
}

pub fn head_commit(dir: &Path) -> String {
    git_opt(dir, &["log", "-1", "--format=%h %s"]).unwrap_or_default()
}

pub fn status_short(dir: &Path) -> String {
    git_opt(dir, &["status", "--short"]).unwrap_or_default()
}

pub fn local_branch_exists(main: &Path, branch: &str) -> bool {
    git_status(
        main,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

pub fn remote_branch_exists(main: &Path, branch: &str) -> bool {
    git_status(
        main,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{branch}"),
        ],
    )
}

/// Path of the worktree (or main repository) that already has this branch checked out.
/// Git refuses two checkouts of the same branch, so we say it before trying.
pub fn branch_worktree(main: &Path, branch: &str) -> Option<PathBuf> {
    let out = git_opt(main, &["worktree", "list", "--porcelain"])?;
    let mut current: Option<&str> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(p);
        } else if line.strip_prefix("branch ") == Some(&format!("refs/heads/{branch}")[..]) {
            return current.map(PathBuf::from);
        }
    }
    None
}

pub struct BranchInfo {
    pub name: String,
    pub date: String,
    pub subject: String,
    pub used_by: Option<PathBuf>,
}

/// Branches offered at creation time: local ones first, then remotes, most recent
/// first. A remote already present locally is hidden as a pointless duplicate.
pub fn branches(main: &Path) -> Vec<BranchInfo> {
    let locals: Vec<String> = git_opt(
        main,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .unwrap_or_default()
    .lines()
    .map(|s| s.to_string())
    .collect();

    let raw = git_opt(
        main,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)|%(committerdate:relative)|%(contents:subject)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )
    .unwrap_or_default();

    raw.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '|');
            let name = parts.next()?.to_string();
            let date = parts.next().unwrap_or_default().to_string();
            let subject = parts.next().unwrap_or_default().to_string();
            // refs/remotes/origin/HEAD shows up as "origin": that is not a branch.
            if name == "origin" || name == "origin/HEAD" {
                return None;
            }
            if let Some(short) = name.strip_prefix("origin/") {
                if locals.iter().any(|l| l == short) {
                    return None;
                }
            }
            let used_by = branch_worktree(main, &name);
            Some(BranchInfo {
                name,
                date,
                subject,
                used_by,
            })
        })
        .collect()
}

/// Creates the worktree, picking the right mode depending on whether the branch exists.
pub fn worktree_add(main: &Path, path: &Path, branch: &str) -> Result<()> {
    let path_s = path.to_string_lossy().into_owned();
    if local_branch_exists(main, branch) {
        if let Some(holder) = branch_worktree(main, branch) {
            bail!(
                "{}",
                t!(
                    "err.branch_in_use",
                    branch = branch,
                    path = holder.display()
                )
            );
        }
        git(main, &["worktree", "add", &path_s, branch])?;
    } else if remote_branch_exists(main, branch) {
        // origin/foo -> local branch foo tracking it.
        let local = branch.split_once('/').map(|(_, b)| b).unwrap_or(branch);
        if local_branch_exists(main, local) {
            bail!("{}", t!("err.local_branch_exists", branch = local));
        }
        git(
            main,
            &["worktree", "add", &path_s, "-b", local, "--track", branch],
        )?;
    } else {
        git(main, &["worktree", "add", &path_s, "-b", branch])?;
    }
    Ok(())
}

pub fn worktree_remove(main: &Path, path: &Path) -> Result<()> {
    let path_s = path.to_string_lossy().into_owned();
    git(main, &["worktree", "remove", "--force", &path_s])?;
    git(main, &["worktree", "prune"])?;
    Ok(())
}
