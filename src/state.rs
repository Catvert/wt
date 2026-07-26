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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WtState {
    #[serde(default)]
    pub branch: String,
    /// Ports frozen on first start, so they stay stable across `up`s.
    #[serde(default)]
    pub ports: BTreeMap<String, u16>,
    /// Last set of `--set k=v` options: a bare `wt up` repeats the previous start
    /// instead of falling back to an empty configuration.
    #[serde(default)]
    pub opts: BTreeMap<String, String>,
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

/// Ports already reserved by other worktrees, stopped ones included.
pub fn reserved_ports(root: &Path, except: &str) -> Vec<u16> {
    list(root)
        .into_iter()
        .filter(|w| w.slug != except)
        .flat_map(|w| w.state.ports.into_values())
        .collect()
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
    v.insert("path".into(), root.join(slug).display().to_string());
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
    crate::tmpl::expand(&v, &project.config.vars)
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
