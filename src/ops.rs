//! The operations: new / up / down / rm / ls / run / ide / open.
//!
//! Everything goes through here, the TUI included: the interactive interface is just
//! another caller, never a second implementation.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};

use crate::config::{Allocate, Ask, Commands, Cwd, Project, Prompt, PromptKind, PromptOption};
use crate::state::{self, Worktree, WtState, MAIN_SLUG};
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
    /// Third column of `[open] source`: a tool of the worktree (database, mail catcher…)
    /// rather than an address of the application. A Lucide icon name, for the Paseo plugin.
    pub icon: Option<String>,
}

pub struct App {
    pub project: Project,
    pub root: PathBuf,
    /// Where messages go: the terminal (command line) or, while the interface runs an
    /// action, the panel showing it as it happens.
    sink: Mutex<Option<Sender<Msg>>>,
    /// The emulator opening a terminal window, looked up on first use.
    window_term: OnceLock<Option<String>>,
}

impl App {
    pub fn new(project: Project) -> Result<Self> {
        let root = project.root()?;
        Ok(App {
            project,
            root,
            sink: Mutex::new(None),
            window_term: OnceLock::new(),
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

    pub(crate) fn info(&self, msg: impl Into<String>) {
        self.emit(Msg::Info(msg.into()));
    }
    pub(crate) fn ok(&self, msg: impl Into<String>) {
        self.emit(Msg::Ok(msg.into()));
    }
    pub(crate) fn warn(&self, msg: impl Into<String>) {
        self.emit(Msg::Warn(msg.into()));
    }
    pub(crate) fn out(&self, msg: impl Into<String>) {
        self.emit(Msg::Out(msg.into()));
    }

    pub fn dir(&self, slug: &str) -> PathBuf {
        if state::is_main(slug) {
            return self.project.main.clone();
        }
        self.root.join(slug)
    }

    /// The worktrees, preceded by the main repository's `@` when the wt.toml has a
    /// `[main]`.
    pub fn list(&self) -> Vec<Worktree> {
        let mut out = Vec::new();
        if self.has_main() {
            out.push(Worktree {
                slug: MAIN_SLUG.into(),
                path: self.project.main.clone(),
                state: state::load(&self.root, MAIN_SLUG),
            });
        }
        out.extend(state::list(&self.root));
        out
    }

    /// The worktree `dir` is in: the slug of the checkout that contains it, `@` for the
    /// main repository when the wt.toml has a `[main]`. The deepest checkout wins — a
    /// `root` inside the main repository puts the worktrees within it.
    pub fn slug_at(&self, dir: &Path) -> Option<String> {
        let dir = fs::canonicalize(dir).ok()?;
        self.list()
            .into_iter()
            .filter_map(|w| {
                let checkout = fs::canonicalize(state::checkout(&w.path, &w.state)).ok()?;
                dir.starts_with(&checkout)
                    .then(|| (checkout.components().count(), w.slug))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, slug)| slug)
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
    /// Is the main repository one of the worktrees, `@`?
    pub fn has_main(&self) -> bool {
        self.project.config.main.is_some()
    }
    /// Does `wt up <slug>` start anything? `@` has its own command, the worktrees the
    /// `up` hooks.
    pub fn has_up_for(&self, slug: &str) -> bool {
        match &self.project.config.main {
            Some(main) if state::is_main(slug) => !main.up.is_empty(),
            _ => self.has_up(),
        }
    }
    /// Does `wt down <slug>` stop anything? `@` is stopped by wt itself.
    pub fn has_down_for(&self, slug: &str) -> bool {
        if state::is_main(slug) {
            self.has_up_for(slug)
        } else {
            self.has_down()
        }
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
        // `@` runs as long as its `wt up @` does, whatever the project probes.
        if state::is_main(&wt.slug) {
            return Some(
                crate::foreground::running(&crate::foreground::lock_path(&self.root)).is_some(),
            );
        }
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

    /// Questions a task declares (`prompt = ["…"]`), in the task's order. Always asked:
    /// they are per-run inputs, never remembered options. Unknown names are ignored.
    pub fn task_prompts(&self, task: &str) -> Vec<Prompt> {
        let Some(t) = self.project.config.tasks.get(task) else {
            return Vec::new();
        };
        t.prompt
            .iter()
            .filter_map(|name| {
                self.project
                    .config
                    .prompts
                    .iter()
                    .find(|p| &p.name == name)
                    .cloned()
            })
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
        if state::is_main(slug) && !self.has_main() {
            bail!(
                "{}",
                t!("err.no_main", path = self.project.config_path.display())
            );
        }
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
        self.run_commands(label, cmds, &vars, &dir)
    }

    /// The commands of a hook, rendered with `vars` and run in `dir`, stopping at the
    /// first failure.
    pub(crate) fn run_commands(
        &self,
        label: &str,
        cmds: &Commands,
        vars: &Vars,
        dir: &Path,
    ) -> Result<()> {
        let env = state::env(vars);
        for raw in &cmds.0 {
            let cmd = tmpl::render(raw, vars);
            self.info(format!("{label} · {cmd}"));
            self.exec_shell(&cmd, dir, &env)?;
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
        if dir.symlink_metadata().is_ok() {
            bail!("{}", t!("err.dir_exists", path = dir.display()));
        }

        let mut st = WtState {
            format: state::STATE_FORMAT,
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

        let st = self.provision(slug, st, false)?;
        self.ok(t!("ok.created", slug = slug, path = dir.display()).to_string());
        self.print_endpoints(slug, &st);
        Ok(())
    }

    /// `wt new <slug> --adopt <path>`: takes over a checkout another tool created (an
    /// agent orchestrator, an IDE) and gives it what `wt new` would have. The checkout
    /// stays where it is; `<root>/<slug>` becomes a link to it, so that every other
    /// command finds it like any worktree.
    pub fn cmd_adopt(&self, slug: &str, path: &Path, sets: &[String]) -> Result<()> {
        validate_slug(slug)?;
        let dir = self.dir(slug);
        if dir.symlink_metadata().is_ok() {
            bail!("{}", t!("err.dir_exists", path = dir.display()));
        }
        let target = fs::canonicalize(path)
            .with_context(|| t!("err.read_failed", path = path.display()).to_string())?;

        let main = fs::canonicalize(&self.project.main).unwrap_or(self.project.main.clone());
        let registered = crate::git::worktrees(&self.project.main)?
            .iter()
            .any(|g| !g.prunable && same_path(&g.path, &target));
        if target == main || !registered {
            bail!("{}", t!("err.adopt_not_worktree", path = target.display()));
        }
        if let Some(other) = self.list().into_iter().find(|w| {
            w.state
                .path
                .as_deref()
                .is_some_and(|p| same_path(p, &target))
        }) {
            bail!(
                "{}",
                t!(
                    "err.adopt_taken",
                    path = target.display(),
                    slug = other.slug
                )
            );
        }

        let st = WtState {
            format: state::STATE_FORMAT,
            opts: parse_sets(sets)?,
            branch: crate::git::current_branch(&target),
            path: Some(target.clone()),
            ..Default::default()
        };

        fs::create_dir_all(&self.root)?;
        self.info(
            t!(
                "info.adopting",
                path = target.display(),
                link = dir.display()
            )
            .to_string(),
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &dir).with_context(|| {
            t!(
                "err.symlink_failed",
                from = target.display(),
                to = dir.display()
            )
            .to_string()
        })?;

        // Undone on failure, so that the other tool's setup can simply run `--adopt`
        // again: the checkout itself was never ours to remove.
        let st = match self.provision(slug, st, true) {
            Ok(st) => st,
            Err(e) => {
                let _ = fs::remove_file(&dir);
                state::forget(&self.root, slug);
                return Err(e);
            }
        };
        self.ok(t!("ok.adopted", slug = slug, path = target.display()).to_string());
        self.print_endpoints(slug, &st);
        Ok(())
    }

    /// What follows the checkout in `new` and `adopt`: directories, copies, ports, state,
    /// `post_new` hooks. An adopted checkout may already hold some of the copies (the
    /// other tool ran its own setup): those are kept rather than overwritten.
    fn provision(&self, slug: &str, mut st: WtState, keep_existing: bool) -> Result<WtState> {
        let dir = self.dir(slug);
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
            if keep_existing && dir.join(&to_rel).symlink_metadata().is_ok() {
                self.info(t!("info.copy_kept", path = to_rel).to_string());
                continue;
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
        Ok(st)
    }

    // --------------------------------------------------------------------------------
    // up / down
    // --------------------------------------------------------------------------------

    pub fn cmd_up(&self, slug: &str, sets: &[String]) -> Result<()> {
        self.cmd_up_for(slug, sets, None)
    }

    /// `stop_after`: how long the worktree may run (`--stop-after`), otherwise
    /// `WT_STOP_AFTER`, otherwise for as long as nobody stops it. Every start sets the
    /// deadline again.
    pub fn cmd_up_for(&self, slug: &str, sets: &[String], stop_after: Option<&str>) -> Result<()> {
        if state::is_main(slug) {
            return self.cmd_up_main(sets, stop_after);
        }
        let mut st = self.require_existing(slug)?;
        // Given options add to the previous ones: a bare `wt up demo` repeats the last
        // start, and `--set queue=0` only resets that one key.
        st.opts.extend(parse_sets(sets)?);
        st.stop_at = stop_deadline(stop_after)?;
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
        if let Some(at) = st.stop_at {
            let left = util::format_left(at.saturating_sub(util::now()));
            self.out(format!("  {}", t!("info.stops_in", left = left)));
        }
        Ok(())
    }

    /// `wt up @`: the `[main] up` commands in the main repository, one after the other,
    /// in the foreground. Returns when the last one ends or the run is stopped (Ctrl-C,
    /// `wt down @`). Nothing of the worktrees' setup applies: no hook, no prompt.
    fn cmd_up_main(&self, sets: &[String], stop_after: Option<&str>) -> Result<()> {
        let mut st = self.require_existing(MAIN_SLUG)?;
        let main = self
            .project
            .config
            .main
            .as_ref()
            .expect("checked by require_existing");
        if main.up.is_empty() {
            bail!(
                "{}",
                t!("err.no_main_up", path = self.project.config_path.display())
            );
        }
        let lock = crate::foreground::lock_path(&self.root);
        let Some(mut running) = crate::foreground::Running::acquire(&lock)? else {
            bail!("{}", t!("err.main_running"));
        };

        st.opts.extend(parse_sets(sets)?);
        st.stop_at = stop_deadline(stop_after)?;
        // `@` never goes through `new`: every port is allocated now.
        self.allocate_ports(MAIN_SLUG, &mut st, Allocate::New)?;
        self.allocate_ports(MAIN_SLUG, &mut st, Allocate::Up)?;
        state::save(&self.root, MAIN_SLUG, &st)?;

        // Before the command, which takes the terminal until it ends.
        self.print_endpoints(MAIN_SLUG, &st);
        if let Some(at) = st.stop_at {
            let left = util::format_left(at.saturating_sub(util::now()));
            self.out(format!("  {}", t!("info.stops_in", left = left)));
        }

        let vars = self.vars(MAIN_SLUG, &st);
        let mut env = state::env(&vars);
        for (name, value) in &main.env {
            env.insert(name.clone(), tmpl::render(value, &vars));
        }
        let result = (|| {
            for raw in &main.up.0 {
                let cmd = tmpl::render(raw, &vars);
                self.info(format!("up · {cmd}"));
                let status = crate::foreground::run(&cmd, &self.project.main, &env, &mut running)?;
                if crate::foreground::stopped(&status) {
                    self.ok(t!("ok.main_stopped").to_string());
                    return Ok(());
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
            }
            self.ok(t!("ok.main_ended").to_string());
            Ok(())
        })();
        // The deadline was this run's.
        let cleared = self.clear_stop_at(MAIN_SLUG);
        drop(running);
        result.and(cleared)
    }

    /// `wt keep <slug> <duration>`: the worktree may run that long from now; `never` lifts
    /// the automatic stop.
    pub fn cmd_keep(&self, slug: &str, duration: &str) -> Result<()> {
        let mut st = self.require_existing(slug)?;
        st.stop_at = util::parse_duration(duration)?.map(|s| util::now() + s);
        state::save(&self.root, slug, &st)?;
        match st.stop_at {
            Some(at) => {
                let left = util::format_left(at.saturating_sub(util::now()));
                self.ok(t!("ok.keep", slug = slug, left = left).to_string());
            }
            None => self.ok(t!("ok.keep_forever", slug = slug).to_string()),
        }
        Ok(())
    }

    /// `wt down --expired`: stops every worktree whose deadline has passed. Meant for a
    /// timer; one failure does not stop the others, and is reported at the end.
    pub fn cmd_down_expired(&self) -> Result<()> {
        let now = util::now();
        let mut failed = 0;
        for wt in self.list() {
            if !wt.state.stop_at.is_some_and(|at| at <= now) {
                continue;
            }
            self.out(t!("info.expired", slug = wt.slug).to_string());
            // Already stopped by other means: only the deadline goes.
            if self.is_up(&wt) == Some(false) {
                self.clear_stop_at(&wt.slug)?;
                continue;
            }
            if let Err(e) = self.cmd_down(&wt.slug) {
                self.warn(format!("{}: {e:#}", wt.slug));
                failed += 1;
            }
        }
        if failed > 0 {
            bail!("{}", t!("err.expired_failed", failed = failed));
        }
        Ok(())
    }

    fn clear_stop_at(&self, slug: &str) -> Result<()> {
        let mut st = state::load(&self.root, slug);
        if st.stop_at.take().is_some() {
            state::save(&self.root, slug, &st)?;
        }
        Ok(())
    }

    pub fn cmd_down(&self, slug: &str) -> Result<()> {
        let st = self.require_existing(slug)?;
        if state::is_main(slug) {
            let stopped = crate::foreground::stop(&crate::foreground::lock_path(&self.root))?;
            self.clear_stop_at(slug)?;
            if stopped {
                self.ok(t!("ok.stopped", slug = slug, kept = t!("label.kept_main")).to_string());
            } else {
                self.info(t!("info.main_not_running").to_string());
            }
            return Ok(());
        }
        if !self.has_down() {
            self.warn(
                t!(
                    "warn.no_down_hook",
                    path = self.project.config_path.display()
                )
                .to_string(),
            );
            return self.clear_stop_at(slug);
        }
        self.run_hook(
            "down",
            &self.project.config.hooks.down,
            slug,
            &st,
            Cwd::Worktree,
        )?;
        self.clear_stop_at(slug)?;
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
        if state::is_main(slug) {
            bail!("{}", t!("err.main_not_removable"));
        }
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

        let dir = self.dir(slug);
        let checkout = state::checkout(&dir, &st);
        let branch = crate::git::current_branch(&checkout);
        crate::git::worktree_remove(&self.project.main, &checkout)?;
        // `git worktree remove` leaves a directory behind when it holds ignored files
        // (vendor/, node_modules/…).
        let _ = fs::remove_dir_all(&checkout);
        if checkout != dir {
            let _ = fs::remove_file(&dir);
        }
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
        for e in [
            "phpstorm",
            "pstorm",
            "idea",
            "code",
            "code-insiders",
            "cursor",
            "zed",
            "zed-dev",
            "nvim",
            "vim",
        ] {
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

    // --------------------------------------------------------------------------------
    // shell / cd — getting into the worktree
    // --------------------------------------------------------------------------------

    /// Opens an interactive shell at the worktree root.
    ///
    /// The natural way to run something *in* a worktree — `claude`, a build, a git
    /// rebase — without teaching wt.toml about it, and the editor's companion too: an
    /// IDE window does not give you a terminal already sitting in the right place.
    ///
    /// The session inherits the worktree's variables (`$WT_SLUG`, `$WT_PATH`,
    /// `$WT_PORT_*`…), the same ones the hooks get: what runs by hand in there sees
    /// exactly what a hook would.
    pub fn cmd_shell(&self, slug: &str) -> Result<()> {
        let st = self.require_existing(slug)?;
        let dir = self.dir(slug);
        let shell = self.shell_command();
        self.info(format!("{shell} — {}", dir.display()));
        // Interactive: inherited stdio, and we wait for the session to end.
        let status = Command::new(&shell)
            .current_dir(&dir)
            .envs(state::env(&self.vars(slug, &st)))
            .status()
            .with_context(|| t!("err.spawn_failed", command = shell).to_string())?;
        if !status.success() {
            // Leaving a shell with a non-zero code is routine, not an error.
            self.info(t!("info.session_ended").to_string());
        }
        Ok(())
    }

    /// The same shell, in a window of its own — the interface it was asked from stays
    /// where it is, and the worktree keeps a terminal after the session ends.
    ///
    /// `false` when no emulator could be found or started: the caller falls back to
    /// [`cmd_shell`](Self::cmd_shell), which always works. Nothing is printed either
    /// way — the interface is drawing, and a stray line would land on top of it.
    pub fn cmd_shell_window(&self, slug: &str) -> Result<bool> {
        let st = self.require_existing(slug)?;
        let Some(bin) = self.terminal_window() else {
            return Ok(false);
        };
        let dir = self.dir(slug);
        let env = state::env(&self.vars(slug, &st));
        let argv = window_argv(bin, &dir, &self.shell_command(), &env);

        let mut cmd = Command::new(bin);
        cmd.args(&argv)
            .current_dir(&dir)
            .envs(&env)
            // The interface goes on reading the keyboard and drawing: the window must
            // take neither our input nor our screen.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        // An emulator lives as long as its window; osascript only carries the order to
        // Terminal.app and leaves, which is what makes its failure worth waiting for.
        if bin.ends_with("osascript") {
            return Ok(cmd.status().map(|s| s.success()).unwrap_or(false));
        }
        Ok(cmd.spawn().is_ok())
    }

    /// Shell opened in the worktree: `WT_TERMINAL`, then `[editor] terminal`, then the
    /// one this session runs.
    fn shell_command(&self) -> String {
        std::env::var("WT_TERMINAL")
            .ok()
            .or_else(|| self.project.config.editor.terminal.clone())
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "sh".to_string())
    }

    /// The emulator that opens a terminal window, if this machine has one.
    ///
    /// Looked up once: the answer cannot change while the interface runs, and it is
    /// asked for on every frame — the shell action words itself differently depending
    /// on whether it opens a window or takes this terminal.
    pub fn terminal_window(&self) -> Option<&str> {
        self.window_term
            .get_or_init(|| {
                if let Some(named) = std::env::var("WT_TERMINAL_WINDOW")
                    .ok()
                    .or_else(|| self.project.config.editor.terminal_window.clone())
                {
                    // Named outright, it is taken as it stands — an empty value is how
                    // one asks for the old behaviour back.
                    return (!named.trim().is_empty()).then_some(named);
                }
                let mut candidates: Vec<&str> = Vec::new();
                if util::is_wsl() {
                    // Windows Terminal is the one WSL actually has: a Linux emulator
                    // there needs a display server the machine often has not got.
                    candidates.push("wt.exe");
                }
                candidates.extend([
                    "ghostty",
                    "wezterm",
                    "kitty",
                    "alacritty",
                    "foot",
                    "gnome-terminal",
                    "konsole",
                    "xfce4-terminal",
                ]);
                if cfg!(target_os = "macos") {
                    // Terminal.app, driven by AppleScript — after the emulators one
                    // installs on purpose, before the fallbacks nobody chooses.
                    candidates.push("osascript");
                }
                candidates.extend(["x-terminal-emulator", "xterm"]);
                candidates
                    .into_iter()
                    .find(|bin| util::which(bin).is_some())
                    .map(|bin| bin.to_string())
            })
            .as_deref()
    }

    /// Prints a worktree's path, for the shell function `wt shell-init` installs.
    ///
    /// A process cannot change its parent's directory: the binary can only say where to
    /// go, and the function does the `cd`. Which is why the path — and nothing else —
    /// goes to stdout.
    ///
    /// When stdout *is* a terminal, nobody is capturing what we print: the integration
    /// is missing, and saying so beats leaving a path to copy by hand.
    pub fn cmd_cd(&self, pattern: Option<&str>) -> Result<()> {
        let slug = self.choose_slug(pattern)?;
        println!("{}", self.dir(&slug).display());
        if io::stdout().is_terminal() {
            warn(&t!(
                "hint.no_shell_integration",
                shell = current_shell_name(),
                slug = slug
            ));
        }
        Ok(())
    }

    /// Turns what was typed on the command line into an existing worktree.
    ///
    /// `wt cd auth` reaches `fix-auth`: a slug is what one types to get somewhere, not
    /// an identifier to be transcribed in full. An exact name always wins over a fuzzy
    /// match — a worktree literally called `fix` must never be shadowed by a longer
    /// neighbour.
    ///
    /// Nothing typed at all, or a pattern still matching several worktrees, asks. wt
    /// never guesses between candidates: the whole point of `cd` is to land somewhere
    /// known, and a wrong directory is discovered three commands later.
    pub fn choose_slug(&self, pattern: Option<&str>) -> Result<String> {
        let all = self.list();
        if all.is_empty() {
            bail!("{}", t!("info.no_worktrees", path = self.root.display()));
        }
        let mut candidates: Vec<Worktree> = match pattern {
            None => all,
            Some(p) if all.iter().any(|w| w.slug == p) => {
                all.into_iter().filter(|w| w.slug == p).collect()
            }
            Some(p) => {
                let mut scored: Vec<(i32, Worktree)> = all
                    .into_iter()
                    .filter_map(|w| crate::fuzzy::matches(p, &w.slug).map(|m| (m.score, w)))
                    .collect();
                // Best match first — it is the one an ENTER accepts. Ties keep the
                // alphabetical order `list` gives, so the menu is stable.
                scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
                scored.into_iter().map(|(_, w)| w).collect()
            }
        };
        if candidates.is_empty() {
            bail!(
                "{}",
                t!("err.unknown_worktree", slug = pattern.unwrap_or_default())
            );
        }
        let chosen = if candidates.len() == 1 {
            0
        } else {
            ask_which(&candidates)?
        };
        Ok(candidates.swap_remove(chosen).slug)
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
                icon: None,
            });
        }
        if let Some(source) = &self.project.config.open.source {
            let vars = self.vars(slug, st);
            let cwd = self.prompt_cwd(slug);
            let raw = util::capture(&tmpl::render(source, &vars), &cwd, &state::env(&vars));
            for line in raw.lines().filter(|l| !l.trim().is_empty()) {
                let mut cols = line.splitn(3, '\t').map(str::trim);
                let url = cols.next().unwrap_or_default();
                let label = cols.next().unwrap_or_default();
                let icon = cols.next().filter(|i| !i.is_empty());
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
                    icon: icon.map(str::to_string),
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
        if let Some(at) = wt.state.stop_at {
            let left = util::format_left(at.saturating_sub(util::now()));
            out.push(format!(
                "{:<9}: {}",
                t!("label.stop_at"),
                t!("info.in", left = left)
            ));
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

/// Git prints the paths it was given, which may differ from the canonical one (a link in
/// the way, a trailing component resolved differently): compare both spellings.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (fs::canonicalize(a), fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

pub fn validate_slug(slug: &str) -> Result<()> {
    if state::is_main(slug) {
        bail!("{}", t!("err.main_reserved"));
    }
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

/// When a start stops by itself: `--stop-after`, otherwise `WT_STOP_AFTER`, otherwise
/// never.
fn stop_deadline(stop_after: Option<&str>) -> Result<Option<u64>> {
    let stop_after = match stop_after {
        Some(d) => Some(d.to_string()),
        None => std::env::var("WT_STOP_AFTER").ok(),
    };
    Ok(match stop_after {
        Some(d) => util::parse_duration(&d)?.map(|s| util::now() + s),
        None => None,
    })
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

/// Asks which worktree, when what was typed leaves more than one. Returns its index.
///
/// Question and list go to **stderr**: `wt cd`'s stdout is the path, and the shell
/// function captures it — a menu mixed into it would be what we tried to `cd` to.
fn ask_which(candidates: &[Worktree]) -> Result<usize> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        // A script has nobody to answer: a failure naming the candidates beats a
        // command waiting forever on a stdin that will never come.
        let slugs: Vec<&str> = candidates.iter().map(|w| w.slug.as_str()).collect();
        bail!("{}", t!("err.ambiguous", slugs = slugs.join(", ")));
    }
    for (i, wt) in candidates.iter().enumerate() {
        eprintln!(
            "  {}) {:<20} {}",
            i + 1,
            wt.slug,
            crate::git::current_branch(&wt.path)
        );
    }
    eprint!("{} ", t!("ui.which_worktree", max = candidates.len()));
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    // A bare ENTER takes the first line — the best match, which is what one aimed at.
    if answer.is_empty() {
        return Ok(0);
    }
    answer
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=candidates.len()).contains(n))
        .map(|n| n - 1)
        .with_context(|| t!("err.cancelled").to_string())
}

/// The user's shell, by name — used to point at the right `wt shell-init` line.
fn current_shell_name() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|s| {
            Path::new(&s)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .filter(|n| matches!(n.as_str(), "bash" | "zsh" | "fish"))
        .unwrap_or_else(|| "bash".to_string())
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

/// What a given emulator wants to be told to open `shell` in `dir`.
///
/// The worktree's variables are handed to the shell explicitly rather than left to be
/// inherited: gnome-terminal hands the job to a server process that never saw our
/// environment, and under WSL it crosses to the Windows side and back. `env` is on
/// every machine that has any of these emulators.
fn window_argv(bin: &str, dir: &Path, shell: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    let d = dir.display().to_string();
    let name = Path::new(bin)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| bin.to_string());

    let mut cmd: Vec<String> = vec!["env".to_string()];
    cmd.extend(env.iter().map(|(k, v)| format!("{k}={v}")));
    cmd.push(shell.to_string());

    let head: Vec<String> = match name.as_str() {
        // A Windows Terminal tab holding a WSL session: the Linux path is `wsl.exe`'s
        // business, not the emulator's.
        "wt.exe" => {
            let mut a = vec!["new-tab".to_string(), "wsl.exe".to_string()];
            a.extend(["--cd".to_string(), d, "-e".to_string()]);
            a
        }
        // Terminal.app takes a shell line, not an argv: it is written out in full.
        "osascript" => return terminal_app_argv(dir, shell, env),
        "wezterm" => vec![
            "start".to_string(),
            "--cwd".to_string(),
            d,
            "--".to_string(),
        ],
        "kitty" => vec!["--directory".to_string(), d],
        "foot" => vec![format!("--working-directory={d}")],
        "ghostty" => vec![format!("--working-directory={d}"), "-e".to_string()],
        "alacritty" => vec!["--working-directory".to_string(), d, "-e".to_string()],
        "konsole" => vec!["--workdir".to_string(), d, "-e".to_string()],
        "gnome-terminal" | "mate-terminal" => {
            vec![format!("--working-directory={d}"), "--".to_string()]
        }
        "xfce4-terminal" => vec![format!("--working-directory={d}"), "-x".to_string()],
        // xterm and anything named in the wt.toml: `-e` is the convention they share,
        // and the directory comes from the cwd the emulator is started in.
        _ => vec!["-e".to_string()],
    };
    [head, cmd].concat()
}

/// The AppleScript that makes Terminal.app open a window in the worktree.
fn terminal_app_argv(dir: &Path, shell: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    let mut line = format!("cd {} && exec env", shell_quote(dir));
    for (k, v) in env {
        line.push_str(&format!(" {k}={}", quote(v)));
    }
    line.push(' ');
    line.push_str(shell);
    let script = line.replace('\\', r"\\").replace('"', "\\\"");
    vec![
        "-e".to_string(),
        format!("tell application \"Terminal\" to do script \"{script}\""),
        "-e".to_string(),
        "tell application \"Terminal\" to activate".to_string(),
    ]
}

fn shell_quote(path: &Path) -> String {
    quote(&path.display().to_string())
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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

    fn demo_env() -> BTreeMap<String, String> {
        BTreeMap::from([("WT_SLUG".to_string(), "demo".to_string())])
    }

    #[test]
    fn a_terminal_window_opens_on_the_worktree() {
        let dir = Path::new("/w/demo");
        let env = demo_env();

        // Under WSL the emulator is a Windows process: the Linux path and the variables
        // are wsl.exe's business.
        assert_eq!(
            window_argv("wt.exe", dir, "fish", &env),
            [
                "new-tab",
                "wsl.exe",
                "--cd",
                "/w/demo",
                "-e",
                "env",
                "WT_SLUG=demo",
                "fish"
            ]
        );
        assert_eq!(
            window_argv("/usr/bin/kitty", dir, "fish", &env),
            ["--directory", "/w/demo", "env", "WT_SLUG=demo", "fish"]
        );
        assert_eq!(
            window_argv("gnome-terminal", dir, "bash", &env),
            [
                "--working-directory=/w/demo",
                "--",
                "env",
                "WT_SLUG=demo",
                "bash"
            ]
        );
        // Unknown — xterm, or a command named in the wt.toml: the `-e` convention, the
        // directory coming from the cwd the emulator is started in.
        assert_eq!(
            window_argv("xterm", dir, "sh", &env),
            ["-e", "env", "WT_SLUG=demo", "sh"]
        );
    }

    #[test]
    fn terminal_app_is_told_in_its_own_language() {
        let env = BTreeMap::from([("WT_PATH".to_string(), "/w/a b".to_string())]);
        let argv = terminal_app_argv(Path::new("/w/a b"), "zsh", &env);
        assert_eq!(argv[0], "-e");
        assert!(argv[1].contains(r"cd '/w/a b' && exec env WT_PATH='/w/a b' zsh"));
        assert!(argv[1].starts_with("tell application \"Terminal\""));
    }
}
