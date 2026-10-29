//! Machine-readable output: `--json` on `ls`, `show`, `open --list`, `root` and `tasks`.
//!
//! Same reasoning as the library itself (`src/lib.rs`): a tool driving `wt` from outside
//! — a Paseo plugin, an editor, a script — should not parse aligned, colored, translated
//! output. These shapes are a contract: fields are added, never renamed or removed, and
//! nothing in them depends on `WT_LANG`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::config::{Ask, PromptKind, PromptOption};
use crate::ops::App;
use crate::state::{self, Worktree};

#[derive(Serialize)]
pub struct ProjectJson {
    pub name: String,
    pub config: PathBuf,
    pub main: PathBuf,
    pub root: PathBuf,
    /// What the wt.toml declares, so a consumer can hide what does not apply.
    pub has_up: bool,
    pub has_down: bool,
    pub has_status: bool,
    pub has_ports: bool,
    pub has_open: bool,
    pub tasks: Vec<TaskJson>,
    /// The `[[prompt]]` questions, so a consumer can ask them in its own UI and pass the
    /// answers with `--set`. Computed choices (`source`) are not run here: `has_source` says a
    /// prompt has some, to be left to wt's own dialogue.
    pub prompts: Vec<PromptJson>,
}

#[derive(Serialize)]
pub struct PromptJson {
    pub name: String,
    /// The label to display: `question`, or the name.
    pub question: String,
    #[serde(rename = "type")]
    pub kind: PromptKind,
    pub ask: Ask,
    pub options: Vec<PromptOption>,
    pub has_source: bool,
    pub default: Option<String>,
    pub separator: String,
    /// The `when` shell condition, unevaluated.
    pub when: Option<String>,
    pub always: bool,
}

#[derive(Serialize)]
pub struct TaskJson {
    pub name: String,
    pub description: String,
    pub interactive: bool,
}

#[derive(Serialize)]
pub struct WorktreeJson {
    pub slug: String,
    /// `<root>/<slug>`: a directory, or a link for an adopted checkout.
    pub dir: PathBuf,
    /// Where the checkout really is: the adopted path, otherwise `dir`.
    pub checkout: PathBuf,
    pub adopted: bool,
    pub branch: String,
    /// `null` when the project has no `[status] up`: no notion of state, not stopped.
    pub up: Option<bool>,
    pub ports: BTreeMap<String, u16>,
    pub opts: BTreeMap<String, String>,
    pub url: Option<String>,
}

#[derive(Serialize)]
pub struct WorktreeDetailJson {
    #[serde(flatten)]
    pub worktree: WorktreeJson,
    pub commit: String,
    /// `[status.info]` probes, by name.
    pub info: BTreeMap<String, String>,
    /// `git status --short` lines.
    pub changes: Vec<String>,
    pub links: Vec<LinkJson>,
}

#[derive(Serialize)]
pub struct LinkJson {
    pub url: String,
    pub label: String,
}

pub fn project(app: &App) -> ProjectJson {
    ProjectJson {
        name: app.project.name(),
        config: app.project.config_path.clone(),
        main: app.project.main.clone(),
        root: app.root.clone(),
        has_up: app.has_up(),
        has_down: app.has_down(),
        has_status: app.has_status(),
        has_ports: app.has_ports(),
        has_open: app.has_open(),
        tasks: tasks(app),
        prompts: app
            .project
            .config
            .prompts
            .iter()
            .map(|p| PromptJson {
                name: p.name.clone(),
                question: p.title().to_string(),
                kind: p.kind,
                ask: p.ask,
                options: p.options.clone(),
                has_source: p.source.is_some(),
                default: p.default.clone(),
                separator: p.separator.clone(),
                when: p.when.clone(),
                always: p.always,
            })
            .collect(),
    }
}

pub fn tasks(app: &App) -> Vec<TaskJson> {
    app.project
        .config
        .tasks
        .iter()
        .map(|(name, t)| TaskJson {
            name: name.clone(),
            description: t.description.clone(),
            interactive: t.interactive,
        })
        .collect()
}

pub fn worktree(app: &App, wt: &Worktree) -> WorktreeJson {
    let checkout = state::checkout(&wt.path, &wt.state);
    WorktreeJson {
        slug: wt.slug.clone(),
        dir: wt.path.clone(),
        adopted: checkout != wt.path,
        branch: crate::git::current_branch(&wt.path),
        checkout,
        up: app.is_up(wt),
        ports: wt.state.ports.clone(),
        opts: wt.state.opts.clone(),
        url: app.url(&wt.slug, &wt.state).filter(|u| !u.contains("{{")),
    }
}

/// Like `wt show`: runs the `[status.info]` probes and the `[open] source` command.
pub fn detail(app: &App, wt: &Worktree) -> WorktreeDetailJson {
    let vars = app.vars(&wt.slug, &wt.state);
    let env = state::env(&vars);
    let info = app
        .project
        .config
        .status
        .info
        .iter()
        .map(|(name, cmd)| {
            let value = crate::util::capture(&crate::tmpl::render(cmd, &vars), &wt.path, &env);
            (name.clone(), value)
        })
        .collect();
    WorktreeDetailJson {
        worktree: worktree(app, wt),
        commit: crate::git::head_commit(&wt.path),
        info,
        changes: crate::git::status_short(&wt.path)
            .lines()
            .map(str::to_string)
            .collect(),
        links: links(app, wt),
    }
}

pub fn links(app: &App, wt: &Worktree) -> Vec<LinkJson> {
    app.links(&wt.slug, &wt.state)
        .into_iter()
        .map(|l| LinkJson {
            url: l.url,
            label: l.label,
        })
        .collect()
}

/// Pretty-printed JSON on stdout, followed by a newline.
pub fn print<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
