//! `wt` — a git worktree manager driven by a `wt.toml` file.
//!
//! The binary knows nothing about the project's language: it creates worktrees,
//! allocates ports, copies what it is told to, and runs the commands declared in the
//! project's `wt.toml`. A Laravel app, a Rust CLI and a static site use the same binary
//! with three different configurations.
//!
//! Le binaire n'est qu'une façade : tout ce qu'il appelle vit dans la bibliothèque du
//! même nom (`src/lib.rs`), que Claudhub consomme sans la caractéristique `cli`.

#[macro_use]
extern crate rust_i18n;

// Les catalogues sont compilés par crate : `t!` se résout en `crate::_rust_i18n_translate`,
// et le binaire a le sien. La locale, elle, est un état global de `rust_i18n` — c'est le
// `i18n::init()` de la bibliothèque qui la choisit, pour les deux.
rust_i18n::i18n!("locales", fallback = "en");

use wt::{complete, config, git, i18n, json, ops, skim_ui, ui};

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
// `Shell` on its own would read as the `wt shell` subcommand two screens below.
use clap_complete::Shell as CompletionShell;

use config::Project;
use ops::App;

#[derive(Parser)]
#[command(
    name = "wt",
    version,
    about = "Git worktree manager, configured by a per-project wt.toml",
    after_help = "Without a subcommand: opens the persistent Ratatui dashboard. Use `wt pick` for the one-shot Skim fuzzy interface."
)]
struct Cli {
    /// Starting directory used to find the project (default: current directory).
    #[arg(
        short = 'C',
        long,
        global = true,
        value_name = "DIR",
        value_hint = clap::ValueHint::DirPath
    )]
    dir: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Writes an example wt.toml at the project root.
    Init {
        /// Starting template: `plain` (no services, default) or `web` (port + URL).
        #[arg(long, default_value = "plain")]
        preset: Preset,
        /// Overwrite an existing wt.toml.
        #[arg(long)]
        force: bool,
    },
    /// Creates a worktree (checkout, directories, copies, post_new hooks).
    New {
        slug: String,
        /// Branch to check out. Defaults to the wt.toml `branch` template.
        #[arg(add = complete::branches())]
        branch: Option<String>,
        /// Where a branch that does not exist yet starts: dev, origin/main, a tag, a
        /// commit. Defaults to the main repository's HEAD.
        #[arg(long, value_name = "REF", add = complete::start_points())]
        from: Option<String>,
        /// Take over a git worktree another tool already created at PATH instead of
        /// checking one out: <root>/<slug> becomes a link to it, then the directories,
        /// copies, ports and post_new hooks run as for a new one.
        #[arg(long, value_name = "PATH", conflicts_with_all = ["branch", "from"])]
        adopt: Option<std::path::PathBuf>,
        /// Option passed to hooks, available as {{opt.key}}. Repeatable.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Starts a worktree (up hooks).
    Up {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Stop it automatically after this long (90m, 2h, 1h30m, 1d; `never`), through
        /// `wt down --expired`. Default: WT_STOP_AFTER, otherwise never.
        #[arg(long, value_name = "DURATION")]
        stop_after: Option<String>,
    },
    /// Stops a worktree (down hooks). The checkout and state are kept.
    Down {
        #[arg(add = complete::slugs(), conflicts_with = "expired")]
        slug: Option<String>,
        /// Stop every worktree whose automatic stop is due (`wt up --stop-after`, `wt
        /// keep`), for a timer.
        #[arg(long)]
        expired: bool,
    },
    /// Moves the automatic stop of a started worktree: DURATION from now, or `never`.
    Keep {
        #[arg(add = complete::slugs())]
        slug: String,
        #[arg(value_name = "DURATION")]
        duration: String,
    },
    /// Lists the worktrees and their state.
    Ls {
        /// Machine-readable output (stable fields, independent of WT_LANG).
        #[arg(long)]
        json: bool,
    },
    /// Opens the persistent Ratatui dashboard — what `wt` alone does.
    Tui,
    /// Opens the one-shot Skim interface: a worktree, then an action.
    Pick,
    /// Details of one worktree.
    Show {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
        /// Machine-readable output (stable fields, independent of WT_LANG).
        #[arg(long)]
        json: bool,
    },
    /// Removes a worktree (pre_rm/post_rm hooks). The branch is kept.
    Rm {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
        #[arg(long, short)]
        yes: bool,
    },
    /// Cleans up after worktrees removed without `wt rm` (dry run unless --apply).
    ///
    /// For each worktree wt has a state for but whose directory is gone: pre_rm hooks
    /// (in an empty temporary directory), git metadata, post_rm hooks, then the state.
    /// Ambiguous cases are reported and left alone. See docs/gc.md.
    Gc {
        /// Do it. Without this flag, only print what would be done.
        #[arg(long)]
        apply: bool,
        /// Forget this entry's state without running any hook (repeatable). Only the
        /// named entries are then handled.
        #[arg(long, value_name = "SLUG")]
        forget: Vec<String>,
        /// Only these entries. Default: every entry found.
        #[arg(value_name = "SLUG")]
        only: Vec<String>,
    },
    /// Opens a shell at the worktree root (to run claude, a build, a rebase…).
    ///
    /// Without a slug: the only worktree, or a menu. `exit` comes back here.
    Shell {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
    },
    /// Changes directory to a worktree — needs `wt shell-init` (see below).
    ///
    /// A slug, a fragment of one (`wt cd auth` finds `fix-auth`), or nothing for a menu.
    Cd {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
    },
    /// Writes the shell function that makes `wt cd` work.
    ///
    /// `eval "$(wt shell-init bash)"` in ~/.bashrc / ~/.zshrc, or
    /// `wt shell-init fish > ~/.config/fish/functions/wt.fish`.
    ShellInit { shell: InitShell },
    /// Opens a worktree in an editor.
    Ide {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
        /// Editor command. Defaults to WT_IDE, [editor] command, EDITOR, then PATH.
        editor: Option<String>,
    },
    /// Opens one of the worktree's addresses in the browser ([open]).
    Open {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
        /// Address to open: a full URL, or a fragment of a label ("tenant acme").
        /// Without a target, the first address — the application's.
        target: Option<String>,
        /// List the available addresses instead of opening one.
        #[arg(long, short)]
        list: bool,
        /// With --list: machine-readable output.
        #[arg(long, requires = "list")]
        json: bool,
    },
    /// Runs a wt.toml task on a worktree.
    Run {
        #[arg(add = complete::tasks())]
        task: Option<String>,
        #[arg(add = complete::slugs())]
        slug: Option<String>,
        /// Extra arguments, available as {{args}}.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Lists the tasks declared in wt.toml.
    Tasks {
        /// Machine-readable output (stable fields, independent of WT_LANG).
        #[arg(long)]
        json: bool,
    },
    /// Prints a worktree's path (`cd "$(wt path demo)"`).
    Path {
        #[arg(add = complete::slugs())]
        slug: Option<String>,
    },
    /// Prints the worktree root and the wt.toml in use.
    Root {
        /// Machine-readable output, with what the wt.toml declares and its tasks (stable fields, independent of WT_LANG).
        #[arg(long)]
        json: bool,
    },
    /// Writes the shell completion script to stdout.
    ///
    /// `wt completions zsh > ~/.zfunc/_wt`. It completes slugs, tasks and branches by
    /// asking the binary, so it stays right as worktrees come and go.
    Completions { shell: CompletionShell },
}

/// Shells `wt shell-init` knows how to write a function for. Fewer than the completions
/// support: this one is hand-written shell, and only what we can actually test ships.
#[derive(Clone, Copy, ValueEnum)]
enum InitShell {
    Bash,
    Zsh,
    Fish,
}

fn main() {
    if let Err(e) = try_main() {
        eprintln!("\x1b[31m{}:\x1b[0m {e:#}", t!("label.error"));
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    i18n::init();
    // `COMPLETE=<shell> wt …`: answer the shell and exit. Before anything can write to
    // stdout, which is where the candidates go.
    complete::serve(<Cli as clap::CommandFactory>::command);
    let cli = Cli::parse();
    let start = match cli.dir {
        Some(d) => d,
        None => std::env::current_dir().with_context(|| t!("err.no_cwd").to_string())?,
    };

    // These must work outside a project: `init` creates the config, completions are
    // generated at package build time far from any repository, and a shell's rc file
    // sources `shell-init` once for every project it will ever visit.
    match &cli.cmd {
        Some(Cmd::Init { preset, force }) => return cmd_init(&start, *preset, *force),
        Some(Cmd::Completions { shell }) => {
            return complete::registration(&shell.to_string(), &mut std::io::stdout());
        }
        Some(Cmd::ShellInit { shell }) => {
            print!(
                "{}",
                match shell {
                    InitShell::Bash | InitShell::Zsh => include_str!("../templates/shell-init.sh"),
                    InitShell::Fish => include_str!("../templates/shell-init.fish"),
                }
            );
            return Ok(());
        }
        _ => {}
    }

    let app = Arc::new(App::new(Project::load(&start)?)?);

    match cli.cmd {
        None => ui::run(Arc::clone(&app)),
        Some(Cmd::Init { .. }) | Some(Cmd::Completions { .. }) | Some(Cmd::ShellInit { .. }) => {
            unreachable!()
        }
        Some(Cmd::New {
            slug,
            adopt: Some(path),
            set,
            ..
        }) => app.cmd_adopt(&slug, &path, &set),
        Some(Cmd::New {
            slug,
            branch,
            from,
            set,
            ..
        }) => app.cmd_new(&slug, branch.as_deref(), from.as_deref(), &set),
        Some(Cmd::Up {
            slug,
            set,
            stop_after,
        }) => {
            let slug = selected_slug(&app, slug)?;
            app.cmd_up_for(&slug, &set, stop_after.as_deref())
        }
        Some(Cmd::Down { expired: true, .. }) => app.cmd_down_expired(),
        Some(Cmd::Down { slug, .. }) => {
            let slug = selected_slug(&app, slug)?;
            app.cmd_down(&slug)
        }
        Some(Cmd::Keep { slug, duration }) => app.cmd_keep(&slug, &duration),
        Some(Cmd::Ls { json: true }) => json::print(
            &app.list()
                .iter()
                .map(|w| json::worktree(&app, w))
                .collect::<Vec<_>>(),
        ),
        Some(Cmd::Ls { json: false }) => app.cmd_ls(),
        Some(Cmd::Tui) => ui::run(Arc::clone(&app)),
        Some(Cmd::Pick) => skim_ui::run(&app),
        Some(Cmd::Show { slug, json }) => {
            let slug = selected_slug(&app, slug)?;
            let wt = app
                .list()
                .into_iter()
                .find(|w| w.slug == slug)
                .with_context(|| t!("err.unknown_worktree", slug = slug).to_string())?;
            if json {
                return json::print(&json::detail(&app, &wt));
            }
            for line in app.preview(&wt) {
                println!("{line}");
            }
            Ok(())
        }
        Some(Cmd::Rm { slug, yes }) => {
            let slug = selected_slug(&app, slug)?;
            app.cmd_rm(&slug, yes)
        }
        Some(Cmd::Gc {
            apply,
            forget,
            only,
        }) => app.cmd_gc(apply, &forget, &only),
        Some(Cmd::Shell { slug }) => {
            let slug = match slug {
                Some(pattern) => app.choose_slug(Some(&pattern))?,
                None => selected_slug(&app, None)?,
            };
            app.cmd_shell(&slug)
        }
        Some(Cmd::Cd { slug }) => match slug {
            Some(pattern) => app.cmd_cd(Some(&pattern)),
            None => {
                let slug = selected_slug(&app, None)?;
                app.cmd_cd(Some(&slug))
            }
        },
        Some(Cmd::Ide { slug, editor }) => {
            let slug = selected_slug(&app, slug)?;
            app.cmd_ide(&slug, editor.as_deref())
        }
        Some(Cmd::Open {
            slug, json: true, ..
        }) => {
            let slug = selected_slug(&app, slug)?;
            let wt = app
                .list()
                .into_iter()
                .find(|w| w.slug == slug)
                .with_context(|| t!("err.unknown_worktree", slug = slug).to_string())?;
            // No address is an empty list here, not an error: the consumer decides.
            json::print(&json::links(&app, &wt))
        }
        Some(Cmd::Open {
            slug, target, list, ..
        }) => {
            let slug = selected_slug(&app, slug)?;
            app.cmd_open(&slug, target.as_deref(), list)
        }
        Some(Cmd::Run { task, slug, args }) => {
            let task = selected_task(&app, task)?;
            let slug = selected_slug(&app, slug)?;
            app.cmd_run(&task, &slug, &args)
        }
        Some(Cmd::Tasks { json: true }) => json::print(&json::tasks(&app)),
        Some(Cmd::Tasks { json: false }) => app.cmd_tasks(),
        Some(Cmd::Path { slug }) => {
            let slug = selected_slug(&app, slug)?;
            println!("{}", app.dir(&slug).display());
            Ok(())
        }
        Some(Cmd::Root { json: true }) => json::print(&json::project(&app)),
        Some(Cmd::Root { json: false }) => {
            println!(
                "{:<10}: {}",
                t!("label.config"),
                app.project.config_path.display()
            );
            println!("{:<10}: {}", t!("label.main"), app.project.main.display());
            println!("{:<10}: {}", t!("label.root"), app.root.display());
            Ok(())
        }
    }
}

fn selected_slug(app: &App, slug: Option<String>) -> Result<String> {
    match slug {
        Some(slug) => Ok(slug),
        None => skim_ui::choose_worktree(app)?.with_context(|| t!("err.cancelled").to_string()),
    }
}

fn selected_task(app: &App, task: Option<String>) -> Result<String> {
    match task {
        Some(task) => Ok(task),
        None => skim_ui::choose_task(app)?.with_context(|| t!("err.cancelled").to_string()),
    }
}

/// `wt.toml` template written by `wt init`. The default assumes no services: many
/// projects have neither a port nor a URL, and a config full of inapplicable sections
/// gets copied around more than it gets read.
#[derive(Clone, Copy, ValueEnum)]
enum Preset {
    /// No services: library, CLI, desktop app, game, data pipeline…
    Plain,
    /// Application served on a port: one port per worktree and a URL to open.
    Web,
}

fn cmd_init(start: &std::path::Path, preset: Preset, force: bool) -> Result<()> {
    let main = git::main_repo(start)?;
    let path = main.join(config::CONFIG_NAME);
    if path.exists() && !force {
        anyhow::bail!("{}", t!("err.config_exists", path = path.display()));
    }
    let body = match preset {
        Preset::Plain => include_str!("../templates/plain.toml"),
        Preset::Web => include_str!("../templates/web.toml"),
    };
    std::fs::write(&path, body)?;
    ops::ok(&t!("ok.config_created", path = path.display()));
    println!("  {}", t!("hint.edit_then_new"));
    Ok(())
}
