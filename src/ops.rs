//! The operations: new / up / down / rm / ls / run / ide / open.
//!
//! Everything goes through here, the TUI included: the interactive interface is just
//! another caller, never a second implementation.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};

use crate::config::{Allocate, Ask, Commands, Cwd, Project, Prompt, PromptKind, PromptOption};
use crate::state::{self, Worktree, WtState};
use crate::tmpl::{self, Vars};
use crate::util::{self, Msg};

pub fn info(msg: &str) {
    println!("\x1b[36m→\x1b[0m {msg}");
}
pub fn ok(msg: &str) {
    println!("\x1b[32m✓\x1b[0m {msg}");
}
pub fn warn(msg: &str) {
    eprintln!("\x1b[33m!\x1b[0m {msg}");
}

/// Une adresse ouvrable dans le navigateur.
pub struct Link {
    pub url: String,
    pub label: String,
}

pub struct App {
    pub project: Project,
    pub root: PathBuf,
    /// Where messages go: the terminal (command line) or, while the interface runs an
    /// action, the panel showing it as it happens.
    sink: Mutex<Option<Sender<Msg>>>,
}

impl App {
    pub fn new(project: Project) -> Result<Self> {
        let root = project.root()?;
        Ok(App {
            project,
            root,
            sink: Mutex::new(None),
        })
    }

    /// Routes the next action's output to a channel (the interface), or gives it back
    /// to the terminal with `None`.
    pub fn set_sink(&self, tx: Option<Sender<Msg>>) {
        *self.sink.lock().unwrap() = tx;
    }

    fn emit(&self, msg: Msg) {
        let sink = self.sink.lock().unwrap();
        let Some(tx) = sink.as_ref() else {
            match msg {
                Msg::Info(m) => info(&m),
                Msg::Ok(m) => ok(&m),
                Msg::Warn(m) => warn(&m),
                Msg::Out(m) => println!("{m}"),
                Msg::Done(_) => {}
            }
            return;
        };
        let _ = tx.send(msg);
    }

    fn info(&self, msg: impl Into<String>) {
        self.emit(Msg::Info(msg.into()));
    }
    fn ok(&self, msg: impl Into<String>) {
        self.emit(Msg::Ok(msg.into()));
    }
    fn warn(&self, msg: impl Into<String>) {
        self.emit(Msg::Warn(msg.into()));
    }
    fn out(&self, msg: impl Into<String>) {
        self.emit(Msg::Out(msg.into()));
    }

    pub fn dir(&self, slug: &str) -> PathBuf {
        self.root.join(slug)
    }

    pub fn list(&self) -> Vec<Worktree> {
        state::list(&self.root)
    }

    // Not every project has services: a library, a CLI or a game only need the
    // checkout. These capabilities report what the wt.toml actually declares, so that
    // ports, state and browser actions stay hidden when the project has none.
    pub fn has_up(&self) -> bool {
        !self.project.config.hooks.up.is_empty()
    }
    pub fn has_down(&self) -> bool {
        !self.project.config.hooks.down.is_empty()
    }
    pub fn has_status(&self) -> bool {
        self.project.config.status.up.is_some()
    }
    pub fn has_ports(&self) -> bool {
        !self.project.config.ports.is_empty()
    }
    pub fn has_open(&self) -> bool {
        self.project.config.open.url.is_some()
    }
    pub fn has_tasks(&self) -> bool {
        !self.project.config.tasks.is_empty()
    }

    /// Does the task claim the terminal?
    pub fn task_is_interactive(&self, name: &str) -> bool {
        self.project
            .config
            .tasks
            .get(name)
            .is_some_and(|t| t.interactive)
    }

    /// Is a worktree started? `None` when the project has no `[status] up`: the notion
    /// of "state" does not exist then, which is not the same as stopped.
    pub fn is_up(&self, wt: &Worktree) -> Option<bool> {
        let cmd = self.project.config.status.up.as_ref()?;
        let vars = self.vars(&wt.slug, &wt.state);
        let cwd = if wt.path.is_dir() {
            wt.path.clone()
        } else {
            self.project.main.clone()
        };
        Some(util::succeeds(
            &tmpl::render(cmd, &vars),
            &cwd,
            &state::env(&vars),
        ))
    }

    pub fn vars(&self, slug: &str, st: &WtState) -> Vars {
        state::vars(&self.project, &self.root, slug, st)
    }

    // --------------------------------------------------------------------------------
    // Prompts — the dialogue the project declares in its wt.toml
    // --------------------------------------------------------------------------------

    /// Questions to ask for a given phase, in file order.
    ///
    /// An option already known — answered during a previous start, or given with
    /// `--set` — is not asked again: that is what makes a repeated `wt up` silently
    /// reproduce the same setup.
    pub fn prompts_for(&self, phase: Ask, known: &BTreeMap<String, String>) -> Vec<Prompt> {
        self.project
            .config
            .prompts
            .iter()
            .filter(|p| p.ask.covers(phase))
            .filter(|p| p.always || !known.contains_key(&p.name))
            .cloned()
            .collect()
    }

    /// Working directory for a prompt's commands: the worktree if it already exists
    /// (the `up` case), otherwise the main repository (the `new` case, nothing checked
    /// out yet).
    fn prompt_cwd(&self, slug: &str) -> PathBuf {
        let dir = self.dir(slug);
        if dir.is_dir() {
            dir
        } else {
            self.project.main.clone()
        }
    }

    fn prompt_context(
        &self,
        slug: &str,
        opts: &BTreeMap<String, String>,
        phase: Ask,
    ) -> (Vars, PathBuf) {
        let st = WtState {
            opts: opts.clone(),
            ..state::load(&self.root, slug)
        };
        let mut vars = self.vars(slug, &st);
        // `{{phase}}` / `$WT_PHASE`: the same question does not necessarily mean the
        // same thing at creation and at start-up, and `when` must be able to tell.
        vars.insert(
            "phase".into(),
            match phase {
                Ask::New => "new".into(),
                _ => "up".into(),
            },
        );
        (vars, self.prompt_cwd(slug))
    }

    /// Does the question apply, given the answers already provided?
    pub fn prompt_applies(
        &self,
        p: &Prompt,
        slug: &str,
        opts: &BTreeMap<String, String>,
        phase: Ask,
    ) -> bool {
        let Some(cond) = &p.when else {
            return true;
        };
        let (vars, cwd) = self.prompt_context(slug, opts, phase);
        util::succeeds(&tmpl::render(cond, &vars), &cwd, &state::env(&vars))
    }

    /// Offered choices: those from the file, or those a shell command enumerates
    /// (`value<TAB>label<TAB>detail`).
    pub fn prompt_choices(
        &self,
        p: &Prompt,
        slug: &str,
        opts: &BTreeMap<String, String>,
        phase: Ask,
    ) -> Vec<PromptOption> {
        if p.kind == PromptKind::Confirm && p.options.is_empty() {
            return vec![
                PromptOption {
                    value: "1".into(),
                    label: "oui".into(),
                    detail: String::new(),
                },
                PromptOption {
                    value: "0".into(),
                    label: "non".into(),
                    detail: String::new(),
                },
            ];
        }
        let Some(source) = &p.source else {
            return p.options.clone();
        };
        let (vars, cwd) = self.prompt_context(slug, opts, phase);
        let out = util::capture(&tmpl::render(source, &vars), &cwd, &state::env(&vars));
        out.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                let mut cols = line.splitn(3, '\t');
                let value = cols.next().unwrap_or_default().trim().to_string();
                let label = cols.next().unwrap_or_default().trim().to_string();
                let detail = cols.next().unwrap_or_default().trim().to_string();
                PromptOption {
                    value,
                    label,
                    detail,
                }
            })
            .filter(|o| !o.value.is_empty())
            .collect()
    }

    fn require_existing(&self, slug: &str) -> Result<WtState> {
        if !self.dir(slug).is_dir() {
            bail!("worktree '{slug}' inconnu (voir: wt ls)");
        }
        let mut st = state::load(&self.root, slug);
        if st.branch.is_empty() {
            st.branch = crate::git::current_branch(&self.dir(slug));
        }
        Ok(st)
    }

    fn run_hook(
        &self,
        label: &str,
        cmds: &Commands,
        slug: &str,
        st: &WtState,
        cwd: Cwd,
    ) -> Result<()> {
        if cmds.is_empty() {
            return Ok(());
        }
        let vars = self.vars(slug, st);
        let env = state::env(&vars);
        let dir = match cwd {
            Cwd::Main => self.project.main.clone(),
            Cwd::Worktree => {
                let d = self.dir(slug);
                if d.is_dir() {
                    d
                } else {
                    self.project.main.clone()
                }
            }
        };
        for raw in &cmds.0 {
            let cmd = tmpl::render(raw, &vars);
            self.info(format!("{label} · {cmd}"));
            self.exec_shell(&cmd, &dir, &env)?;
        }
        Ok(())
    }

    /// Runs a hook or task command, relaying its output to the panel when the interface
    /// expects it, and letting it flow to the terminal otherwise.
    fn exec_shell(&self, cmd: &str, dir: &Path, env: &BTreeMap<String, String>) -> Result<()> {
        let tx = self.sink.lock().unwrap().clone();
        match tx {
            Some(tx) => util::run_streamed(cmd, dir, env, &tx),
            None => util::run(cmd, dir, env),
        }
    }

    /// Allocates the requested phase's ports that are not already frozen in the state.
    fn allocate_ports(&self, slug: &str, st: &mut WtState, phase: Allocate) -> Result<bool> {
        let mut changed = false;
        let reserved = state::reserved_ports(&self.root, slug);
        for (name, spec) in &self.project.config.ports {
            if spec.allocate != phase || st.ports.contains_key(name) {
                continue;
            }
            let mut taken = reserved.clone();
            taken.extend(st.ports.values().copied());
            let port = util::alloc_port(spec.base, &taken)?;
            st.ports.insert(name.clone(), port);
            changed = true;
        }
        Ok(changed)
    }

    // --------------------------------------------------------------------------------
    // new
    // --------------------------------------------------------------------------------

    /// `from` is the branch (or tag, or commit) a *new* branch starts from; `None`
    /// means the main repository's HEAD.
    pub fn cmd_new(
        &self,
        slug: &str,
        branch: Option<&str>,
        from: Option<&str>,
        sets: &[String],
    ) -> Result<()> {
        validate_slug(slug)?;
        let dir = self.dir(slug);
        if dir.exists() {
            bail!("{}", t!("err.dir_exists", path = dir.display()));
        }

        let mut st = WtState {
            opts: parse_sets(sets)?,
            ..Default::default()
        };
        let branch = match branch {
            Some(b) => b.to_string(),
            None => {
                let tpl = self
                    .project
                    .config
                    .branch
                    .clone()
                    .unwrap_or_else(|| "wt/{{slug}}".into());
                tmpl::render(&tpl, &self.vars(slug, &st))
            }
        };
        st.branch = branch.clone();

        fs::create_dir_all(&self.root)?;
        self.info(match from {
            Some(f) => t!("info.creating_from", branch = branch, from = f).to_string(),
            None => t!("info.creating", branch = branch).to_string(),
        });
        crate::git::worktree_add(&self.project.main, &dir, &branch, from)?;
        // `git worktree add origin/x` creates a local branch under the short name: we
        // record the one actually checked out.
        st.branch = crate::git::current_branch(&dir);

        for d in &self.project.config.dirs {
            let rendered = tmpl::render(d, &self.vars(slug, &st));
            fs::create_dir_all(dir.join(&rendered))
                .with_context(|| t!("err.mkdir_failed", path = rendered).to_string())?;
        }

        for spec in &self.project.config.copies {
            let vars = self.vars(slug, &st);
            let from_rel = tmpl::render(&spec.from, &vars);
            let to_rel = tmpl::render(spec.to.as_ref().unwrap_or(&spec.from), &vars);
            let from = self.project.main.join(&from_rel);
            if !from.exists() {
                if spec.optional {
                    continue;
                }
                bail!("{}", t!("err.copy_source_missing", path = from.display()));
            }
            self.info(
                t!(
                    "info.copying",
                    from = from_rel,
                    to = to_rel,
                    mode = format!("{:?}", spec.mode)
                )
                .to_string(),
            );
            util::copy_tree(&from, &dir.join(&to_rel), spec.mode)
                .with_context(|| t!("err.copy_failed", path = from_rel).to_string())?;
        }

        self.allocate_ports(slug, &mut st, Allocate::New)?;
        state::save(&self.root, slug, &st)?;

        self.run_hook(
            "post_new",
            &self.project.config.hooks.post_new,
            slug,
            &st,
            Cwd::Worktree,
        )?;

        self.ok(t!("ok.created", slug = slug, path = dir.display()).to_string());
        self.print_endpoints(slug, &st);
        Ok(())
    }

    // --------------------------------------------------------------------------------
    // up / down
    // --------------------------------------------------------------------------------

    pub fn cmd_up(&self, slug: &str, sets: &[String]) -> Result<()> {
        let mut st = self.require_existing(slug)?;
        // Given options add to the previous ones: a bare `wt up demo` repeats the last
        // start, and `--set queue=0` only resets that one key.
        st.opts.extend(parse_sets(sets)?);
        self.allocate_ports(slug, &mut st, Allocate::Up)?;
        state::save(&self.root, slug, &st)?;

        // Many projects have nothing to start (library, CLI, game). We say so without
        // making it an anomaly — and above all without announcing a start that never
        // happened.
        if !self.has_up() {
            self.warn(t!("warn.no_up_hook", path = self.project.config_path.display()).to_string());
            self.print_endpoints(slug, &st);
            return Ok(());
        }

        self.run_hook(
            "up",
            &self.project.config.hooks.up,
            slug,
            &st,
            Cwd::Worktree,
        )?;
        self.ok(t!("ok.started", slug = slug).to_string());
        self.print_endpoints(slug, &st);
        Ok(())
    }

    pub fn cmd_down(&self, slug: &str) -> Result<()> {
        let st = self.require_existing(slug)?;
        if !self.has_down() {
            self.warn(
                t!(
                    "warn.no_down_hook",
                    path = self.project.config_path.display()
                )
                .to_string(),
            );
            return Ok(());
        }
        self.run_hook(
            "down",
            &self.project.config.hooks.down,
            slug,
            &st,
            Cwd::Worktree,
        )?;
        let kept = if self.has_ports() {
            t!("label.kept_with_ports")
        } else {
            t!("label.kept")
        };
        self.ok(t!("ok.stopped", slug = slug, kept = kept).to_string());
        Ok(())
    }

    // --------------------------------------------------------------------------------
    // rm
    // --------------------------------------------------------------------------------

    pub fn cmd_rm(&self, slug: &str, yes: bool) -> Result<()> {
        let st = self.require_existing(slug)?;
        if !yes && !confirm(&t!("confirm.remove", slug = slug))? {
            bail!("{}", t!("err.cancelled"));
        }

        self.run_hook(
            "pre_rm",
            &self.project.config.hooks.pre_rm,
            slug,
            &st,
            Cwd::Worktree,
        )?;

        let branch = crate::git::current_branch(&self.dir(slug));
        crate::git::worktree_remove(&self.project.main, &self.dir(slug))?;
        // `git worktree remove` leaves a directory behind when it holds ignored files
        // (vendor/, node_modules/…).
        let _ = fs::remove_dir_all(self.dir(slug));
        state::forget(&self.root, slug);

        self.run_hook(
            "post_rm",
            &self.project.config.hooks.post_rm,
            slug,
            &st,
            Cwd::Main,
        )?;

        self.ok(t!("ok.removed", slug = slug).to_string());
        // The branch deliberately survives: removing a worktree must never make
        // unmerged work disappear.
        if crate::git::local_branch_exists(&self.project.main, &branch) {
            self.out(format!("  {}", t!("hint.branch_kept", branch = branch)));
        }
        Ok(())
    }

    // --------------------------------------------------------------------------------
    // ls / run / ide / open
    // --------------------------------------------------------------------------------

    pub fn cmd_ls(&self) -> Result<()> {
        let worktrees = self.list();
        if worktrees.is_empty() {
            println!("{}", t!("info.no_worktrees", path = self.root.display()));
            return Ok(());
        }
        // Variable columns: a project without services or ports has no reason to read
        // two empty ones.
        let mut header = format!("{:<20} {:<28}", "SLUG", t!("col.branch").to_uppercase());
        if self.has_status() {
            header.push_str(&format!(" {:<10}", t!("col.state").to_uppercase()));
        }
        if self.has_ports() {
            header.push_str(&format!(" {}", t!("col.ports").to_uppercase()));
        }
        println!("{}", header.trim_end());

        for wt in &worktrees {
            let mut line = format!(
                "{:<20} {:<28}",
                wt.slug,
                crate::git::current_branch(&wt.path)
            );
            if let Some(up) = self.is_up(wt) {
                let state = if up {
                    t!("state.started")
                } else {
                    t!("state.stopped")
                };
                line.push_str(&format!(" {state:<10}"));
            }
            if self.has_ports() {
                let ports = wt
                    .state
                    .ports
                    .iter()
                    .map(|(k, v)| format!("{k}:{v}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                line.push(' ');
                line.push_str(&ports);
            }
            println!("{}", line.trim_end());
        }
        Ok(())
    }

    pub fn cmd_run(&self, task: &str, slug: &str, args: &[String]) -> Result<()> {
        let st = self.require_existing(slug)?;
        let t = self
            .project
            .config
            .tasks
            .get(task)
            .with_context(|| t!("err.unknown_task", task = task).to_string())?;
        let mut vars = self.vars(slug, &st);
        vars.insert("args".into(), args.join(" "));
        let env = state::env(&vars);
        let dir = match t.cwd {
            Cwd::Main => self.project.main.clone(),
            Cwd::Worktree => self.dir(slug),
        };
        for raw in &t.run.0 {
            let cmd = tmpl::render(raw, &vars);
            self.info(format!("{task} · {cmd}"));
            self.exec_shell(&cmd, &dir, &env)?;
        }
        Ok(())
    }

    pub fn cmd_tasks(&self) -> Result<()> {
        if self.project.config.tasks.is_empty() {
            println!(
                "{}",
                t!("info.no_tasks", path = self.project.config_path.display())
            );
            return Ok(());
        }
        for (name, t) in &self.project.config.tasks {
            println!("{:<16} {}", name, t.description);
        }
        Ok(())
    }

    /// Preferred editor: `WT_IDE` from the environment, then `[editor] command` from
    /// wt.toml, then `EDITOR`, then whatever is found in PATH.
    pub fn editors(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |c: String| {
            if !c.is_empty() && !out.contains(&c) && util::which(&c).is_some() {
                out.push(c);
            }
        };
        if let Ok(e) = std::env::var("WT_IDE") {
            push(e);
        }
        if let Some(e) = &self.project.config.editor.command {
            push(e.clone());
        }
        if let Ok(e) = std::env::var("EDITOR") {
            push(e);
        }
        for e in ["phpstorm", "pstorm", "idea", "code", "zed", "nvim", "vim"] {
            push(e.to_string());
        }
        out
    }

    pub fn cmd_ide(&self, slug: &str, editor: Option<&str>) -> Result<()> {
        self.require_existing(slug)?;
        let dir = self.dir(slug);
        let editor = match editor {
            Some(e) => e.to_string(),
            None => self
                .editors()
                .into_iter()
                .next()
                .with_context(|| t!("err.no_editor").to_string())?,
        };
        let target = ide_path(&dir, &editor);
        self.info(format!("{editor} {}", target.display()));

        if is_terminal_editor(&editor) {
            // An editor living in the current terminal must stay in the foreground:
            // detached like a GUI IDE, it would be killed immediately.
            let status = Command::new(&editor).arg(".").current_dir(&dir).status()?;
            std::process::exit(status.code().unwrap_or(0));
        }
        Command::new(&editor)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| t!("err.spawn_failed", command = editor).to_string())?;
        Ok(())
    }

    /// Opens an interactive shell at the worktree root.
    ///
    /// This is the editor's natural companion: an IDE window does not give you a
    /// terminal already sitting in the right place.
    pub fn cmd_shell(&self, slug: &str) -> Result<()> {
        self.require_existing(slug)?;
        let dir = self.dir(slug);
        let shell = std::env::var("WT_TERMINAL")
            .ok()
            .or_else(|| self.project.config.editor.terminal.clone())
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "sh".to_string());
        self.info(format!("{shell} — {}", dir.display()));
        // Interactive: inherited stdio, and we wait for the session to end.
        let status = Command::new(&shell)
            .current_dir(&dir)
            .status()
            .with_context(|| t!("err.spawn_failed", command = shell).to_string())?;
        if !status.success() {
            // Leaving a shell with a non-zero code is routine, not an error.
            self.info(t!("info.session_ended").to_string());
        }
        Ok(())
    }

    /// Does the chosen editor live in the current terminal?
    pub fn editor_is_terminal(&self, editor: &str) -> bool {
        is_terminal_editor(editor)
    }

    pub fn url(&self, slug: &str, st: &WtState) -> Option<String> {
        self.project
            .config
            .open
            .url
            .as_ref()
            .map(|u| tmpl::render(u, &self.vars(slug, st)))
    }

    /// Addresses this worktree can open: the main URL, then those enumerated by
    /// `[open] source` (one per tenant, per service…).
    ///
    /// The command only runs when opening: it may query a database or a container, which
    /// has no business running while rendering a list.
    pub fn links(&self, slug: &str, st: &WtState) -> Vec<Link> {
        let mut out = Vec::new();
        if let Some(url) = self.url(slug, st).filter(|u| !u.contains("{{")) {
            out.push(Link {
                url,
                label: self
                    .project
                    .config
                    .open
                    .label
                    .clone()
                    .unwrap_or_else(|| "application".into()),
            });
        }
        if let Some(source) = &self.project.config.open.source {
            let vars = self.vars(slug, st);
            let cwd = self.prompt_cwd(slug);
            let raw = util::capture(&tmpl::render(source, &vars), &cwd, &state::env(&vars));
            for line in raw.lines().filter(|l| !l.trim().is_empty()) {
                let (url, label) = match line.split_once('\t') {
                    Some((u, l)) => (u.trim(), l.trim()),
                    None => (line.trim(), ""),
                };
                if url.is_empty() {
                    continue;
                }
                out.push(Link {
                    url: url.to_string(),
                    label: if label.is_empty() {
                        url.to_string()
                    } else {
                        label.to_string()
                    },
                });
            }
        }
        out
    }

    /// `target`: a full URL, or a fragment of a label/address. Without a target the
    /// first address is opened — the project's, not some random tenant's.
    pub fn cmd_open(&self, slug: &str, target: Option<&str>, list: bool) -> Result<()> {
        let st = self.require_existing(slug)?;
        let links = self.links(slug, &st);
        if links.is_empty() {
            // A non-web application has nothing to open: this is not a configuration
            // error, just a command with no purpose here.
            bail!(
                "{}",
                t!("err.no_url", path = self.project.config_path.display())
            );
        }

        if list {
            for l in &links {
                self.out(format!("{:<40} {}", l.url, l.label));
            }
            return Ok(());
        }

        let chosen = match target {
            None => &links[0],
            Some(t) if t.starts_with("http://") || t.starts_with("https://") => {
                return self.launch(t);
            }
            Some(t) => links
                .iter()
                .find(|l| l.label.contains(t) || l.url.contains(t))
                .with_context(|| t!("err.no_matching_url", target = t, slug = slug).to_string())?,
        };
        self.launch(&chosen.url)
    }

    fn launch(&self, url: &str) -> Result<()> {
        self.info(url.to_string());
        util::open_url(url)
    }

    fn print_endpoints(&self, slug: &str, st: &WtState) {
        // After `wt new`, ports allocated at start-up do not exist yet: a URL full of
        // holes teaches nothing, so we show nothing.
        if let Some(url) = self.url(slug, st).filter(|u| !u.contains("{{")) {
            self.out(format!("  {:<6}: {url}", t!("label.url")));
        }
        for (name, port) in &st.ports {
            self.out(format!("  {name:<6}: {port}"));
        }
    }

    /// Preview lines (TUI and `wt show`): git state, ports, `[status.info]` probes.
    pub fn preview(&self, wt: &Worktree) -> Vec<String> {
        let mut out = vec![
            format!("{:<9}: {}", t!("label.path"), wt.path.display()),
            format!(
                "{:<9}: {}",
                t!("label.branch"),
                crate::git::current_branch(&wt.path)
            ),
            format!(
                "{:<9}: {}",
                t!("label.commit"),
                crate::git::head_commit(&wt.path)
            ),
        ];
        if let Some(up) = self.is_up(wt) {
            let state = if up {
                t!("state.started")
            } else {
                t!("state.stopped")
            };
            out.push(format!("{:<9}: {state}", t!("label.state")));
        }
        for (name, port) in &wt.state.ports {
            out.push(format!("{name:<9}: {port}"));
        }
        for (name, value) in &wt.state.opts {
            out.push(format!("{name:<9}: {value}"));
        }
        if let Some(url) = self.url(&wt.slug, &wt.state).filter(|u| !u.contains("{{")) {
            out.push(format!("{:<9}: {url}", t!("label.url")));
        }

        let vars = self.vars(&wt.slug, &wt.state);
        let probe_env = state::env(&vars);
        for (name, cmd) in &self.project.config.status.info {
            let value = util::capture(&tmpl::render(cmd, &vars), &wt.path, &probe_env);
            out.push(format!("{name:<9}: {value}"));
        }

        let changes = crate::git::status_short(&wt.path);
        if !changes.is_empty() {
            out.push(String::new());
            out.push(format!("── {} ──", t!("label.git_changes")));
            out.extend(changes.lines().take(20).map(|l| l.to_string()));
        }
        out
    }
}

pub fn validate_slug(slug: &str) -> Result<()> {
    let valid = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if !valid {
        bail!("{}", t!("err.invalid_slug", slug = slug));
    }
    Ok(())
}

/// A branch name is not a DNS label: `feature/Refonte_Devis` -> `feature-refonte-devis`.
pub fn slugify(branch: &str) -> String {
    let mut out = String::new();
    for c in branch.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

fn parse_sets(sets: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for s in sets {
        let (k, v) = s
            .split_once('=')
            .with_context(|| t!("err.bad_set", value = s).to_string())?;
        out.insert(k.trim().to_string(), v.to_string());
    }
    Ok(out)
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [{}] ", t!("hint.yes_no"));
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "o" | "O"))
}

fn is_terminal_editor(editor: &str) -> bool {
    let name = Path::new(editor)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| editor.to_string());
    matches!(
        name.as_str(),
        "nvim" | "vim" | "vi" | "nano" | "hx" | "helix" | "kak" | "emacs"
    )
}

/// A Windows executable launched from WSL does not understand `/home/…`: it needs the
/// UNC path that `wslpath -w` produces.
fn ide_path(dir: &Path, editor: &str) -> PathBuf {
    let windows_exe = editor.to_lowercase().ends_with(".exe");
    if windows_exe && util::is_wsl() && util::which("wslpath").is_some() {
        let converted = util::capture(
            &format!("wslpath -w {}", shell_quote(dir)),
            dir,
            &BTreeMap::new(),
        );
        if !converted.is_empty() {
            return PathBuf::from(converted);
        }
    }
    dir.to_path_buf()
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_slugs() {
        assert!(validate_slug("refonte-devis").is_ok());
        assert!(validate_slug("Refonte").is_err());
        assert!(validate_slug("-x").is_err());
        assert!(validate_slug("").is_err());
    }

    #[test]
    fn slugifies_branches() {
        assert_eq!(slugify("feature/Refonte_Devis"), "feature-refonte-devis");
        assert_eq!(slugify("origin/fix--pdf"), "origin-fix-pdf");
    }

    #[test]
    fn parses_set_options() {
        let m = parse_sets(&["tenants=a,b".into(), "queue=1".into()]).unwrap();
        assert_eq!(m["tenants"], "a,b");
        assert_eq!(m["queue"], "1");
        assert!(parse_sets(&["oups".into()]).is_err());
    }
}
