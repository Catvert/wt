//! Persistent per-worktree state: branch, allocated ports, options of the last `up`.
//!
//! Stored in `<root>/.wt/<slug>.toml`, next to the worktrees rather than inside them:
//! the checkout stays clean (nothing to add to the project's .gitignore) and the state
//! survives a branch change.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Project;
use crate::tmpl::Vars;

pub const STATE_DIR: &str = ".wt";

/// The main repository, when the wt.toml has a `[main]`. Not a valid slug, so no
/// worktree can take it and `wt gc` never sees its state.
pub const MAIN_SLUG: &str = "@";

pub fn is_main(slug: &str) -> bool {
    slug == MAIN_SLUG
}

/// Written by `wt new` since `wt gc` exists. A state carrying it recorded its options from
/// the start, so an empty option set really means "no option". States written before
/// (no `format` key, read as 0) are trusted only when they hold options: see `wt gc`.
pub const STATE_FORMAT: u32 = 1;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WtState {
    /// 0 = written by `wt` ≤ 0.8.0. Kept as found by every command but `new`: a later
    /// `up` must not vouch for options it never saw being recorded.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub format: u32,
    #[serde(default)]
    pub branch: String,
    /// Ports frozen on first start, so they stay stable across `up`s.
    #[serde(default)]
    pub ports: BTreeMap<String, u16>,
    /// Last set of `--set k=v` options: a bare `wt up` repeats the previous start
    /// instead of falling back to an empty configuration.
    #[serde(default)]
    pub opts: BTreeMap<String, String>,
    /// Checkout taken over by `wt new --adopt`, created elsewhere by another tool:
    /// `<root>/<slug>` is then a link to it. Absent for a checkout `wt` created itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// When `wt down --expired` stops the worktree, in Unix seconds: set by `wt up` from
    /// `--stop-after` or `WT_STOP_AFTER`, moved by `wt keep`, cleared by `wt down`. A
    /// started worktree nobody comes back to does not run, and hold memory, for weeks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_at: Option<u64>,
}

/// Where the checkout of a worktree really is: the adopted path when `<root>/<slug>` is
/// still the link `wt` made to it, otherwise `<root>/<slug>` itself.
pub fn checkout(dir: &Path, state: &WtState) -> PathBuf {
    match &state.path {
        Some(p) if links_to(dir, p) => p.clone(),
        _ => dir.to_path_buf(),
    }
}

/// Is `dir` a link pointing at `target`? Read without following it: the target may
/// already be gone.
pub fn links_to(dir: &Path, target: &Path) -> bool {
    fs::read_link(dir).is_ok_and(|t| t == target)
}

/// Does anything of the worktree remain on disk? For an adopted checkout the link alone
/// does not count: the tool that created the checkout may have deleted it, leaving the
/// link dangling, and that is exactly the case `wt gc` cleans up after.
pub fn present(dir: &Path, state: &WtState) -> bool {
    match &state.path {
        Some(p) if links_to(dir, p) => p.symlink_metadata().is_ok(),
        _ => dir.symlink_metadata().is_ok(),
    }
}

pub fn state_path(root: &Path, slug: &str) -> PathBuf {
    root.join(STATE_DIR).join(format!("{slug}.toml"))
}

pub fn load(root: &Path, slug: &str) -> WtState {
    fs::read_to_string(state_path(root, slug))
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(root: &Path, slug: &str, state: &WtState) -> Result<()> {
    let path = state_path(root, slug);
    fs::create_dir_all(path.parent().unwrap())?;
    let body = toml::to_string_pretty(state)?;
    fs::write(&path, body)
        .with_context(|| t!("err.write_failed", path = path.display()).to_string())
}

pub fn forget(root: &Path, slug: &str) {
    let _ = fs::remove_file(state_path(root, slug));
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Like `load`, but a missing or unparsable file is an error instead of an empty state:
/// `wt gc` runs removal hooks from it and must never do so on a made-up configuration.
pub fn load_strict(root: &Path, slug: &str) -> Result<WtState> {
    let path = state_path(root, slug);
    let body = fs::read_to_string(&path)
        .with_context(|| t!("err.read_failed", path = path.display()).to_string())?;
    toml::from_str(&body)
        .with_context(|| t!("err.state_invalid", path = path.display()).to_string())
}

/// Removes a state file, failing loudly (unlike `forget`, which follows a removal that
/// already succeeded).
pub fn remove(root: &Path, slug: &str) -> Result<()> {
    let path = state_path(root, slug);
    fs::remove_file(&path)
        .with_context(|| t!("err.write_failed", path = path.display()).to_string())
}

/// Slugs that have a state file, whether or not their worktree still exists. Files that
/// `wt` cannot have written (not `<slug>.toml` with a valid slug) are left out.
pub fn recorded(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root.join(STATE_DIR)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let slug = name.strip_suffix(".toml")?.to_string();
            crate::ops::validate_slug(&slug).ok()?;
            Some(slug)
        })
        .collect();
    out.sort();
    out
}

pub struct Worktree {
    pub slug: String,
    pub path: PathBuf,
    pub state: WtState,
}

/// Worktrees present on disk. The directory is the source of truth: a checkout created
/// by hand under `<root>/` is listed even without a state file.
pub fn list(root: &Path) -> Vec<Worktree> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    let mut dirs: Vec<_> = entries.flatten().collect();
    dirs.sort_by_key(|e| e.file_name());
    for entry in dirs {
        let slug = entry.file_name().to_string_lossy().into_owned();
        if slug == STATE_DIR || !entry.path().is_dir() {
            continue;
        }
        out.push(Worktree {
            state: load(root, &slug),
            slug,
            path: entry.path(),
        });
    }
    out
}

/// Ports already reserved by other worktrees, stopped ones included, and by the main
/// repository's `@`, which has no directory under the root.
pub fn reserved_ports(root: &Path, except: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = list(root)
        .into_iter()
        .filter(|w| w.slug != except)
        .flat_map(|w| w.state.ports.into_values())
        .collect();
    if !is_main(except) {
        ports.extend(load(root, MAIN_SLUG).ports.into_values());
    }
    ports
}

/// Variables available to wt.toml templates.
///
/// Resolution order: computed values first (slug, paths, ports, options), then the
/// project's `[vars]`, which may build on them.
pub fn vars(project: &Project, root: &Path, slug: &str, state: &WtState) -> Vars {
    let mut v = Vars::new();
    v.insert("slug".into(), slug.into());
    v.insert("branch".into(), state.branch.clone());
    v.insert("main".into(), project.main.display().to_string());
    v.insert("root".into(), root.display().to_string());
    let path = if is_main(slug) {
        project.main.clone()
    } else {
        root.join(slug)
    };
    v.insert("path".into(), path.display().to_string());
    // `{{is_main}}` / `$WT_IS_MAIN`: a task or a probe shared with the worktrees can tell.
    v.insert(
        "is_main".into(),
        if is_main(slug) { "1" } else { "0" }.into(),
    );
    v.insert("project".into(), project.name());
    v.insert(
        "repo".into(),
        project
            .main
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    for (name, port) in &state.ports {
        v.insert(format!("port.{name}"), port.to_string());
    }
    for (name, value) in &state.opts {
        v.insert(format!("opt.{name}"), value.clone());
    }
    // An option never provided stays empty rather than showing as `{{opt.tenants}}`,
    // so a hook can reference it without the user having to define it.
    for name in declared_opts(project) {
        v.entry(format!("opt.{name}")).or_default();
    }
    match &project.config.main {
        Some(main) if is_main(slug) => {
            let mut project_vars = project.config.vars.clone();
            project_vars.extend(main.vars.clone());
            crate::tmpl::expand(&v, &project_vars)
        }
        _ => crate::tmpl::expand(&v, &project.config.vars),
    }
}

/// Options mentioned anywhere in the config, so missing ones can be blanked out.
fn declared_opts(project: &Project) -> Vec<String> {
    let mut found = Vec::new();
    let raw = fs::read_to_string(&project.config_path).unwrap_or_default();
    let mut rest = raw.as_str();
    while let Some(i) = rest.find("{{opt.") {
        rest = &rest[i + 6..];
        if let Some(end) = rest.find("}}") {
            let name = rest[..end].trim().to_string();
            if !name.is_empty() && !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found
}

/// Same information as the templates, but as environment variables: a longer hook reads
/// better with `$WT_SLUG` than with `{{slug}}` interpolated by wt.
pub fn env(vars: &Vars) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (k, v) in vars {
        let key = format!("WT_{}", k.to_uppercase().replace(['.', '-'], "_"));
        env.insert(key, v.clone());
    }
    env
}
