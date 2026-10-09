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
///
/// `from` is the start point of a branch that has yet to be created — `dev`,
/// `origin/main`, a tag, a commit. Without it git starts at the main repository's HEAD,
/// which is only the right guess when that is where the work belongs.
pub fn worktree_add(main: &Path, path: &Path, branch: &str, from: Option<&str>) -> Result<()> {
    let path_s = path.to_string_lossy().into_owned();
    // A branch that already exists carries its own history: a start point would be
    // silently dropped, so say it rather than create something else than what was asked.
    let reject_start_point = |from: Option<&str>| -> Result<()> {
        match from {
            Some(f) => bail!(
                "{}",
                t!("err.start_point_on_existing", branch = branch, from = f)
            ),
            None => Ok(()),
        }
    };

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
        reject_start_point(from)?;
        git(main, &["worktree", "add", &path_s, branch])?;
        ensure_upstream(main, branch);
    } else if remote_branch_exists(main, branch) {
        // origin/foo -> local branch foo tracking it.
        let local = branch.split_once('/').map(|(_, b)| b).unwrap_or(branch);
        if local_branch_exists(main, local) {
            bail!("{}", t!("err.local_branch_exists", branch = local));
        }
        reject_start_point(from)?;
        git(
            main,
            &["worktree", "add", &path_s, "-b", local, "--track", branch],
        )?;
    } else {
        let mut args = vec!["worktree", "add", &path_s, "-b", branch];
        // `git worktree add -b x <path> origin/dev` also sets up the tracking, which is
        // what someone branching off a remote expects.
        args.extend(from);
        git(main, &args)?;
        ensure_upstream(main, branch);
    }
    Ok(())
}

/// Wires a branch that tracks nothing to `origin/<branch>`, so the first `git push`
/// from the worktree needs no `-u origin …`. The remote ref usually does not exist
/// yet — that is exactly what the push will create. Branches born with a start point
/// on `origin/…` already track it (git's autoSetupMerge) and are left alone.
fn ensure_upstream(main: &Path, branch: &str) {
    let merge_key = format!("branch.{branch}.merge");
    if git_opt(main, &["config", "--get", &merge_key]).is_some() {
        return;
    }
    if git_opt(main, &["remote", "get-url", "origin"]).is_none() {
        return;
    }
    let _ = git(
        main,
        &["config", &format!("branch.{branch}.remote"), "origin"],
    );
    let _ = git(
        main,
        &["config", &merge_key, &format!("refs/heads/{branch}")],
    );
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct GitWorktree {
    pub path: PathBuf,
    /// Short branch name; `None` for a detached HEAD or a bare repository.
    pub branch: Option<String>,
    pub locked: bool,
    /// Git says the directory is gone (or no longer points back at the repository).
    pub prunable: bool,
}

/// Every worktree git knows of, the main repository included. Unlike the lookups above,
/// a failure is an error: `wt gc` decides what to clean from this list.
pub fn worktrees(main: &Path) -> Result<Vec<GitWorktree>> {
    let out = git(main, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktrees(&out))
}

fn parse_worktrees(out: &str) -> Vec<GitWorktree> {
    let mut list = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            list.extend(current.take());
            current = Some(GitWorktree {
                path: PathBuf::from(p),
                branch: None,
                locked: false,
                prunable: false,
            });
            continue;
        }
        let Some(wt) = current.as_mut() else {
            continue;
        };
        if let Some(b) = line.strip_prefix("branch ") {
            wt.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
        } else if line == "locked" || line.starts_with("locked ") {
            wt.locked = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            wt.prunable = true;
        }
    }
    list.extend(current);
    list
}

/// Drops git's record of a worktree whose directory is gone. Without `--force`: should
/// the directory have come back with changes, git refuses instead of deleting them.
pub fn worktree_forget(main: &Path, path: &Path) -> Result<()> {
    let path_s = path.to_string_lossy().into_owned();
    git(main, &["worktree", "remove", &path_s])?;
    Ok(())
}

pub fn worktree_remove(main: &Path, path: &Path) -> Result<()> {
    let path_s = path.to_string_lossy().into_owned();
    git(main, &["worktree", "remove", "--force", &path_s])?;
    git(main, &["worktree", "prune"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_flags_and_branches() {
        let out = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
                   worktree /repo-wt/a\nHEAD abc\nbranch refs/heads/wt/a\nprunable gitdir file points to non-existent location\n\n\
                   worktree /repo-wt/b\nHEAD abc\ndetached\nlocked on a usb disk\n";
        let list = parse_worktrees(out);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert!(!list[0].prunable && !list[0].locked);
        assert_eq!(list[1].path, PathBuf::from("/repo-wt/a"));
        assert_eq!(list[1].branch.as_deref(), Some("wt/a"));
        assert!(list[1].prunable);
        assert_eq!(list[2].branch, None);
        assert!(list[2].locked && !list[2].prunable);
    }
}
