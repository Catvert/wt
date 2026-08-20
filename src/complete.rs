//! Shell completion — the part a generated script cannot know.
//!
//! `wt cd <TAB>` has to answer with *this* project's worktrees, and `wt run <TAB>` with
//! *this* `wt.toml`'s tasks. A script written once at install time knows the commands
//! and the flags, never the slugs. `clap_complete`'s dynamic engine calls the binary
//! back on every TAB instead, so the list is whatever the project has right now.
//!
//! Everything here answers with an empty list rather than an error: a TAB pressed
//! outside a repository, or in one whose `wt.toml` is broken, must do nothing at all.
//! A completion is nobody's way of learning something is wrong.

use anyhow::{Context, Result};
use clap_complete::env::Shells;
use clap_complete::{ArgValueCandidates, CompletionCandidate};

use crate::config::Project;
use crate::ops::App;

/// The variable the generated script sets to ask for candidates instead of a run.
const VAR: &str = "COMPLETE";

/// Answers `COMPLETE=<shell> wt …` and exits; a normal run goes straight through.
///
/// Must come before anything writes to stdout: what we print there *is* the candidate
/// list the shell reads.
///
/// La fabrique de commande vient du binaire : `Cli` est sa grammaire, et la
/// bibliothèque n'a pas à la connaître pour savoir énumérer des slugs.
pub fn serve(factory: fn() -> clap::Command) {
    clap_complete::CompleteEnv::with_factory(factory)
        // Call `wt` from the PATH rather than argv[0]: a script written by
        // `wt completions` outlives the path the binary happened to be run from.
        .completer("wt")
        .complete();
}

/// Writes the shell code that wires TAB to this binary (`wt completions <shell>`).
///
/// Sourcing `COMPLETE=<shell> wt` at shell startup does the same thing and can never
/// fall out of step with the binary; this exists for packagers, who install the script
/// and the binary as one, and for anyone who would rather not spawn wt in their rc file.
pub fn registration(shell: &str, out: &mut dyn std::io::Write) -> Result<()> {
    let shells = Shells::builtins();
    let completer = shells
        .completer(shell)
        .with_context(|| t!("err.no_completions", shell = shell).to_string())?;
    completer.write_registration(VAR, "wt", "wt", "wt", out)?;
    Ok(())
}

/// The project's worktrees, for every command that takes a slug.
pub fn slugs() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        let Some(app) = app() else {
            return Vec::new();
        };
        app.list()
            .into_iter()
            // The recorded branch, never `git`: a TAB must not spawn a process per
            // worktree to decorate a list that is about to be filtered anyway.
            .map(|w| candidate(&w.slug, &w.state.branch))
            .collect()
    })
}

/// The `wt.toml`'s tasks, for `wt run <TAB>`.
pub fn tasks() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        let Some(app) = app() else {
            return Vec::new();
        };
        app.project
            .config
            .tasks
            .iter()
            .map(|(name, task)| candidate(name, &task.description))
            .collect()
    })
}

/// The main repository's branches, for `wt new [branch]`.
///
/// One already checked out elsewhere is still listed — hiding it would leave the user
/// wondering where their branch went — but its help says why `wt new` will refuse it.
pub fn branches() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        branch_list(|b, _head| {
            if b.used_by.is_some() {
                t!("ui.already_checked_out").to_string()
            } else {
                b.subject.clone()
            }
        })
    })
}

/// The same branches, for `--from <ref>`.
///
/// Nothing is warned about here: a branch checked out elsewhere is a perfectly good
/// place for a new one to start, and the frequent case — starting off `dev` while `main`
/// is checked out — is exactly that.
pub fn start_points() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        branch_list(|b, head| {
            if b.name == head {
                format!("{} · {}", t!("ui.head_here"), b.subject)
            } else {
                b.subject.clone()
            }
        })
    })
}

/// The branch list both use, with the main repository's HEAD handed to the help so
/// neither has to load the project a second time.
fn branch_list(help: impl Fn(&crate::git::BranchInfo, &str) -> String) -> Vec<CompletionCandidate> {
    let Some(app) = app() else {
        return Vec::new();
    };
    let head = crate::git::current_branch(&app.project.main);
    crate::git::branches(&app.project.main)
        .into_iter()
        .map(|b| candidate(&b.name, &help(&b, &head)))
        .collect()
}

/// The project the shell is sitting in, or nothing at all.
fn app() -> Option<App> {
    let start = std::env::current_dir().ok()?;
    App::new(Project::load(&start).ok()?).ok()
}

fn candidate(value: &str, help: &str) -> CompletionCandidate {
    let candidate = CompletionCandidate::new(value);
    if help.is_empty() {
        candidate
    } else {
        candidate.help(Some(help.to_owned().into()))
    }
}
