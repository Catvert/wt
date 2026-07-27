//! `wt` — a git worktree manager driven by a `wt.toml` file.
//!
//! The binary knows nothing about the project's language: it creates worktrees,
//! allocates ports, copies what it is told to, and runs the commands declared in the
//! project's `wt.toml`. A Laravel app, a Rust CLI and a static site use the same binary
//! with three different configurations.

#[macro_use]
extern crate rust_i18n;

mod ansi;
mod complete;
mod config;
mod fuzzy;
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
    after_help = "Without a subcommand: opens the interactive interface."
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
        /// Option passed to hooks, available as {{opt.key}}. Repeatable.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Starts a worktree (up hooks).
    Up {
        #[arg(add = complete::slugs())]
        slug: String,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Stops a worktree (down hooks). The checkout and state are kept.
    Down {
        #[arg(add = complete::slugs())]
        slug: String,
    },
    /// Lists the worktrees and their state.
    Ls,
    /// Details of one worktree.
    Show {
        #[arg(add = complete::slugs())]
        slug: String,
    },
    /// Removes a worktree (pre_rm/post_rm hooks). The branch is kept.
    Rm {
        #[arg(add = complete::slugs())]
        slug: String,
        #[arg(long, short)]
        yes: bool,
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
        slug: String,
        /// Editor command. Defaults to WT_IDE, [editor] command, EDITOR, then PATH.
        editor: Option<String>,
    },
    /// Opens one of the worktree's addresses in the browser ([open]).
    Open {
        #[arg(add = complete::slugs())]
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
        #[arg(add = complete::tasks())]
        task: String,
        #[arg(add = complete::slugs())]
        slug: String,
        /// Extra arguments, available as {{args}}.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Lists the tasks declared in wt.toml.
    Tasks,
    /// Prints a worktree's path (`cd "$(wt path demo)"`).
    Path {
        #[arg(add = complete::slugs())]
        slug: String,
    },
    /// Prints the worktree root and the wt.toml in use.
    Root,
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
    complete::serve();
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
            branch,
            from,
            set,
        }) => app.cmd_new(&slug, branch.as_deref(), from.as_deref(), &set),
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
        Some(Cmd::Shell { slug }) => {
            let slug = app.choose_slug(slug.as_deref())?;
            app.cmd_shell(&slug)
        }
        Some(Cmd::Cd { slug }) => app.cmd_cd(slug.as_deref()),
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
