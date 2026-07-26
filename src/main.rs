//! `wt` — a git worktree manager driven by a `wt.toml` file.
//!
//! The binary knows nothing about the project's language: it creates worktrees,
//! allocates ports, copies what it is told to, and runs the commands declared in the
//! project's `wt.toml`. A Laravel app, a Rust CLI and a static site use the same binary
//! with three different configurations.

#[macro_use]
extern crate rust_i18n;

mod ansi;
mod config;
mod git;
mod i18n;
mod ops;
mod state;
mod tmpl;
mod ui;
mod util;

rust_i18n::i18n!("locales", fallback = "en");

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use config::Project;
use ops::App;

#[derive(Parser)]
#[command(
    name = "wt",
    version,
    about = "Git worktree manager, configured by a per-project wt.toml",
    after_help = "Without a subcommand: opens the interactive interface."
)]
struct Cli {
    /// Starting directory used to find the project (default: current directory).
    #[arg(short = 'C', long, global = true, value_name = "DIR")]
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
        branch: Option<String>,
        /// Option passed to hooks, available as {{opt.key}}. Repeatable.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Starts a worktree (up hooks).
    Up {
        slug: String,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Stops a worktree (down hooks). The checkout and state are kept.
    Down { slug: String },
    /// Lists the worktrees and their state.
    Ls,
    /// Details of one worktree.
    Show { slug: String },
    /// Removes a worktree (pre_rm/post_rm hooks). The branch is kept.
    Rm {
        slug: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Opens a worktree in an editor.
    Ide {
        slug: String,
        /// Editor command. Defaults to WT_IDE, [editor] command, EDITOR, then PATH.
        editor: Option<String>,
    },
    /// Opens one of the worktree's addresses in the browser ([open]).
    Open {
        slug: String,
        /// Address to open: a full URL, or a fragment of a label ("tenant acme").
        /// Without a target, the first address — the application's.
        target: Option<String>,
        /// List the available addresses instead of opening one.
        #[arg(long, short)]
        list: bool,
    },
    /// Runs a wt.toml task on a worktree.
    Run {
        task: String,
        slug: String,
        /// Extra arguments, available as {{args}}.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Lists the tasks declared in wt.toml.
    Tasks,
    /// Prints a worktree's path (`cd "$(wt path demo)"`).
    Path { slug: String },
    /// Prints the worktree root and the wt.toml in use.
    Root,
    /// Writes a shell completion script to stdout.
    ///
    /// `wt completions zsh > ~/.zfunc/_wt`
    Completions { shell: Shell },
}

fn main() {
    if let Err(e) = try_main() {
        eprintln!("\x1b[31m{}:\x1b[0m {e:#}", t!("label.error"));
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    i18n::init();
    let cli = Cli::parse();
    let start = match cli.dir {
        Some(d) => d,
        None => std::env::current_dir().with_context(|| t!("err.no_cwd").to_string())?,
    };

    // Both of these must work outside a project: `init` creates the config, and
    // completions are generated at package build time, far from any repository.
    match &cli.cmd {
        Some(Cmd::Init { preset, force }) => return cmd_init(&start, *preset, *force),
        Some(Cmd::Completions { shell }) => {
            clap_complete::generate(*shell, &mut Cli::command(), "wt", &mut std::io::stdout());
            return Ok(());
        }
        _ => {}
    }

    let app = Arc::new(App::new(Project::load(&start)?)?);

    match cli.cmd {
        None => ui::run(Arc::clone(&app)),
        Some(Cmd::Init { .. }) | Some(Cmd::Completions { .. }) => unreachable!(),
        Some(Cmd::New { slug, branch, set }) => app.cmd_new(&slug, branch.as_deref(), &set),
        Some(Cmd::Up { slug, set }) => app.cmd_up(&slug, &set),
        Some(Cmd::Down { slug }) => app.cmd_down(&slug),
        Some(Cmd::Ls) => app.cmd_ls(),
        Some(Cmd::Show { slug }) => {
            let wt = app
                .list()
                .into_iter()
                .find(|w| w.slug == slug)
                .with_context(|| t!("err.unknown_worktree", slug = slug).to_string())?;
            for line in app.preview(&wt) {
                println!("{line}");
            }
            Ok(())
        }
        Some(Cmd::Rm { slug, yes }) => app.cmd_rm(&slug, yes),
        Some(Cmd::Ide { slug, editor }) => app.cmd_ide(&slug, editor.as_deref()),
        Some(Cmd::Open { slug, target, list }) => app.cmd_open(&slug, target.as_deref(), list),
        Some(Cmd::Run { task, slug, args }) => app.cmd_run(&task, &slug, &args),
        Some(Cmd::Tasks) => app.cmd_tasks(),
        Some(Cmd::Path { slug }) => {
            println!("{}", app.dir(&slug).display());
            Ok(())
        }
        Some(Cmd::Root) => {
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
