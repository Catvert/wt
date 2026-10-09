//! `wt gc`: the cleanup `wt rm` would have run, for worktrees removed without it.
//!
//! The contract lives in `docs/gc.md`. In short: only a slug whose directory is gone
//! from disk ever gets removal hooks, run with the options recorded in its state; every
//! ambiguous case is reported and left alone; nothing happens without `--apply`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::git::{self, GitWorktree};
use crate::ops::{validate_slug, App};
use crate::state::{self, WtState};
use crate::tmpl::{self, Vars};

/// What `wt gc` concluded about one slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcKind {
    /// State present, directory gone: removal hooks, git metadata, state forgotten.
    Orphan,
    /// Git still registers a vanished directory `wt` has no state for: git metadata only.
    GitLeftover,
    /// The directory exists but git does not register it as a worktree.
    NotWorktree,
    /// Git marks the worktree locked.
    Locked,
    /// The state's branch is checked out in a live worktree elsewhere.
    Moved(PathBuf),
    /// The state file cannot be read.
    Unreadable(String),
    /// State written by `wt` ≤ 0.8.0 without options, while the project has removal
    /// hooks: their options cannot be known.
    OldState,
}

impl GcKind {
    /// Will `--apply` act on it?
    pub fn actionable(&self) -> bool {
        matches!(self, GcKind::Orphan | GcKind::GitLeftover)
    }
}

#[derive(Debug, Clone)]
pub struct GcEntry {
    pub slug: String,
    pub kind: GcKind,
    /// The recorded state, when it could be read.
    pub state: Option<WtState>,
    /// Where git records the worktree, when it still does.
    pub git_path: Option<PathBuf>,
}

/// The slug of a path directly under the worktree root, if it is one.
fn slug_under(path: &Path, roots: &[PathBuf]) -> Option<String> {
    roots.iter().find_map(|root| {
        let rest = path.strip_prefix(root).ok()?;
        let mut parts = rest.components();
        let first = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        Some(first.as_os_str().to_string_lossy().into_owned())
    })
}

impl App {
    fn has_rm_hooks(&self) -> bool {
        let h = &self.project.config.hooks;
        !h.pre_rm.is_empty() || !h.post_rm.is_empty()
    }

    /// Classifies every slug `wt` or git remembers under the root. Live worktrees are
    /// left out: there is nothing to say about them.
    pub fn gc_scan(&self) -> Result<Vec<GcEntry>> {
        let gits = git::worktrees(&self.project.main)?;
        let mut roots = vec![self.root.clone()];
        if let Ok(c) = fs::canonicalize(&self.root) {
            if c != self.root {
                roots.push(c);
            }
        }

        let mut by_slug: BTreeMap<String, &GitWorktree> = BTreeMap::new();
        for g in &gits {
            if let Some(slug) = slug_under(&g.path, &roots) {
                by_slug.insert(slug, g);
            }
        }

        let mut slugs: Vec<String> = state::recorded(&self.root);
        for slug in by_slug.keys() {
            if !slugs.contains(slug) && validate_slug(slug).is_ok() {
                slugs.push(slug.clone());
            }
        }
        slugs.sort();

        let mut out = Vec::new();
        for slug in slugs {
            let dir = self.dir(&slug);
            let git_entry = by_slug.get(&slug).copied();
            let has_state = state::state_path(&self.root, &slug).is_file();
            let git_path = git_entry.map(|g| g.path.clone());
            let entry = |kind, state| GcEntry {
                slug: slug.clone(),
                kind,
                state,
                git_path: git_path.clone(),
            };

            // Anything at that path — directory, file, even a dangling link — means the
            // worktree may still be there: no hook ever runs for it.
            if dir.symlink_metadata().is_ok() {
                let registered = git_entry.is_some_and(|g| !g.prunable);
                if !registered {
                    out.push(entry(GcKind::NotWorktree, None));
                }
                continue;
            }
            if git_entry.is_some_and(|g| g.locked) {
                out.push(entry(GcKind::Locked, None));
                continue;
            }
            if !has_state {
                out.push(entry(GcKind::GitLeftover, None));
                continue;
            }
            let st = match state::load_strict(&self.root, &slug) {
                Ok(st) => st,
                Err(e) => {
                    out.push(entry(GcKind::Unreadable(format!("{e:#}")), None));
                    continue;
                }
            };
            let elsewhere = (!st.branch.is_empty())
                .then(|| {
                    gits.iter().find(|g| {
                        !g.prunable && g.path != dir && g.branch.as_deref() == Some(&st.branch)
                    })
                })
                .flatten();
            let kind = if let Some(g) = elsewhere {
                GcKind::Moved(g.path.clone())
            } else if st.format == 0 && st.opts.is_empty() && self.has_rm_hooks() {
                GcKind::OldState
            } else {
                GcKind::Orphan
            };
            out.push(entry(kind, Some(st)));
        }
        Ok(out)
    }

    /// The variables of a removal hook run by `gc`: those of `wt rm`, plus `gc` = `1`.
    fn gc_vars(&self, slug: &str, st: &WtState) -> Vars {
        let mut vars = self.vars(slug, st);
        vars.insert("gc".into(), "1".into());
        vars
    }

    /// `wt gc [--apply] [--forget slug…]`.
    pub fn cmd_gc(&self, apply: bool, forget: &[String]) -> Result<()> {
        if !forget.is_empty() {
            return self.gc_forget(apply, forget);
        }
        let entries = self.gc_scan()?;
        let todo = entries.iter().filter(|e| e.kind.actionable()).count();
        let skipped = entries.len() - todo;
        if entries.is_empty() {
            self.ok(t!("gc.nothing").to_string());
            return Ok(());
        }

        let mut failed = 0;
        for e in &entries {
            if !e.kind.actionable() {
                self.warn(self.skip_reason(e));
                continue;
            }
            if !apply {
                for line in self.gc_plan(e) {
                    self.out(line);
                }
                continue;
            }
            if let Err(err) = self.gc_clean(e) {
                failed += 1;
                self.warn(t!("gc.failed", slug = e.slug, error = format!("{err:#}")).to_string());
            }
        }

        if !apply {
            self.out(t!("gc.dry_run_summary", todo = todo, skipped = skipped).to_string());
            if todo > 0 {
                self.out(format!("  {}", t!("gc.hint_apply")));
            }
            return Ok(());
        }
        self.out(
            t!(
                "gc.apply_summary",
                done = todo - failed,
                failed = failed,
                skipped = skipped
            )
            .to_string(),
        );
        if failed > 0 {
            bail!("{}", t!("gc.some_failed", failed = failed));
        }
        Ok(())
    }

    fn skip_reason(&self, e: &GcEntry) -> String {
        let dir = self.dir(&e.slug);
        let reason = match &e.kind {
            GcKind::NotWorktree => t!("gc.skip.not_worktree", path = dir.display()).to_string(),
            GcKind::Locked => t!("gc.skip.locked", path = dir.display()).to_string(),
            GcKind::Moved(at) => t!(
                "gc.skip.moved",
                branch = e.state.as_ref().map(|s| s.branch.as_str()).unwrap_or(""),
                path = at.display()
            )
            .to_string(),
            GcKind::Unreadable(err) => t!("gc.skip.unreadable", error = err).to_string(),
            GcKind::OldState => t!("gc.skip.old_state", slug = e.slug).to_string(),
            GcKind::Orphan | GcKind::GitLeftover => String::new(),
        };
        t!("gc.skipped", slug = e.slug, reason = reason).to_string()
    }

    /// What `--apply` would do for an actionable entry, one line per step.
    pub fn gc_plan(&self, e: &GcEntry) -> Vec<String> {
        let mut lines = Vec::new();
        match (&e.kind, &e.state) {
            (GcKind::Orphan, Some(st)) => {
                let opts = if st.opts.is_empty() {
                    t!("gc.no_options").to_string()
                } else {
                    st.opts
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                lines.push(
                    t!(
                        "gc.orphan",
                        slug = e.slug,
                        branch = st.branch,
                        options = opts
                    )
                    .to_string(),
                );
                let vars = self.gc_vars(&e.slug, st);
                let hooks = &self.project.config.hooks;
                for raw in &hooks.pre_rm.0 {
                    lines.push(format!("    pre_rm · {}", tmpl::render(raw, &vars)));
                }
                if let Some(p) = &e.git_path {
                    lines.push(format!("    git · git worktree remove {}", p.display()));
                }
                for raw in &hooks.post_rm.0 {
                    lines.push(format!("    post_rm · {}", tmpl::render(raw, &vars)));
                }
                lines.push(format!(
                    "    {}",
                    t!(
                        "gc.forget_state",
                        path = state::state_path(&self.root, &e.slug).display()
                    )
                ));
            }
            (GcKind::GitLeftover, _) => {
                lines.push(t!("gc.git_leftover", slug = e.slug).to_string());
                if let Some(p) = &e.git_path {
                    lines.push(format!("    git · git worktree remove {}", p.display()));
                }
            }
            _ => {}
        }
        lines
    }

    /// Runs the cleanup of one actionable entry. The state goes last, so that a failure
    /// leaves the entry in place for the next `wt gc`.
    fn gc_clean(&self, e: &GcEntry) -> Result<()> {
        // Checked again right before acting: the scan may be a few hooks old.
        if self.dir(&e.slug).symlink_metadata().is_ok() {
            bail!(
                "{}",
                t!("gc.skip.not_worktree", path = self.dir(&e.slug).display())
            );
        }
        match (&e.kind, &e.state) {
            (GcKind::Orphan, Some(st)) => {
                self.info(t!("gc.cleaning", slug = e.slug).to_string());
                let vars = self.gc_vars(&e.slug, st);
                let hooks = &self.project.config.hooks;
                if !hooks.pre_rm.is_empty() {
                    let scratch = scratch_dir(&e.slug)?;
                    let res = self.run_commands("pre_rm", &hooks.pre_rm, &vars, &scratch);
                    let _ = fs::remove_dir_all(&scratch);
                    res?;
                }
                if let Some(p) = &e.git_path {
                    git::worktree_forget(&self.project.main, p)?;
                }
                self.run_commands("post_rm", &hooks.post_rm, &vars, &self.project.main)?;
                state::remove(&self.root, &e.slug)?;
                self.ok(t!("gc.cleaned", slug = e.slug).to_string());
                if !st.branch.is_empty() && git::local_branch_exists(&self.project.main, &st.branch)
                {
                    self.out(format!("  {}", t!("hint.branch_kept", branch = st.branch)));
                }
            }
            (GcKind::GitLeftover, _) => {
                if let Some(p) = &e.git_path {
                    git::worktree_forget(&self.project.main, p)?;
                }
                self.ok(t!("gc.git_cleaned", slug = e.slug).to_string());
            }
            _ => {}
        }
        Ok(())
    }

    /// `--forget`: drops state files without running anything, for entries cleaned by
    /// hand or that `gc` refuses.
    fn gc_forget(&self, apply: bool, slugs: &[String]) -> Result<()> {
        let mut errors = Vec::new();
        for slug in slugs {
            if let Err(e) = validate_slug(slug) {
                errors.push(format!("{e:#}"));
                continue;
            }
            let dir = self.dir(slug);
            if dir.symlink_metadata().is_ok() {
                errors.push(t!("gc.forget_exists", slug = slug, path = dir.display()).to_string());
                continue;
            }
            let path = state::state_path(&self.root, slug);
            if !path.is_file() {
                errors.push(t!("gc.forget_unknown", slug = slug).to_string());
                continue;
            }
            if !apply {
                self.out(t!("gc.would_forget", slug = slug, path = path.display()).to_string());
                continue;
            }
            match state::remove(&self.root, slug) {
                Ok(()) => self.ok(t!("gc.forgotten", slug = slug).to_string()),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        if !apply && errors.len() < slugs.len() {
            self.out(format!("  {}", t!("gc.hint_apply")));
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("\n"));
        }
        Ok(())
    }
}

/// A fresh, empty directory to run `pre_rm` in when the worktree is gone: relative paths
/// in a hook then reach nothing, instead of the main checkout.
fn scratch_dir(slug: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("wt-gc-{}-{slug}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("{}: {e}", t!("err.mkdir_failed", path = dir.display())))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_under_takes_direct_children_only() {
        let roots = vec![PathBuf::from("/r/p-wt"), PathBuf::from("/real/p-wt")];
        assert_eq!(
            slug_under(Path::new("/r/p-wt/demo"), &roots).as_deref(),
            Some("demo")
        );
        assert_eq!(
            slug_under(Path::new("/real/p-wt/x"), &roots).as_deref(),
            Some("x")
        );
        assert_eq!(slug_under(Path::new("/r/p-wt/a/b"), &roots), None);
        assert_eq!(slug_under(Path::new("/r/p-wt"), &roots), None);
        assert_eq!(slug_under(Path::new("/r/other/demo"), &roots), None);
    }

    #[test]
    fn only_orphans_and_git_leftovers_are_actionable() {
        assert!(GcKind::Orphan.actionable());
        assert!(GcKind::GitLeftover.actionable());
        for k in [
            GcKind::NotWorktree,
            GcKind::Locked,
            GcKind::Moved(PathBuf::from("/x")),
            GcKind::Unreadable("e".into()),
            GcKind::OldState,
        ] {
            assert!(!k.actionable(), "{k:?}");
        }
    }
}
