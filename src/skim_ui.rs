//! The default, fzf-style interface powered by the embedded `skim` library.
//!
//! This interface is intentionally one-shot: choose a worktree, choose an action, then
//! let that action own the terminal.  The persistent dashboard remains available as
//! `wt tui`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};

use anyhow::{bail, Result};
use skim::prelude::*;

use crate::config::{Ask, Prompt, PromptKind};
use crate::ops::{self, App};

/// An item whose machine-readable value is separate from what Skim searches and draws.
/// This avoids parsing aligned, translated display lines after a choice is made.
struct Choice {
    key: String,
    text: String,
    preview: String,
    disabled: bool,
}

impl Choice {
    fn new(key: impl Into<String>, label: impl AsRef<str>, detail: impl AsRef<str>) -> Self {
        let label = label.as_ref();
        let detail = detail.as_ref();
        Self {
            key: key.into(),
            text: if detail.is_empty() {
                label.to_string()
            } else {
                format!("{label:<30} {detail}")
            },
            preview: String::new(),
            disabled: false,
        }
    }

    fn preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = preview.into();
        self
    }

    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl SkimItem for Choice {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.text)
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        ItemPreview::Text(self.preview.clone())
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.key)
    }

    fn disabled(&self) -> bool {
        self.disabled
    }
}

pub fn run(app: &App) -> Result<()> {
    // Skim owns the keyboard and renders on stderr.  Failing before entering raw mode
    // gives scripts a useful error instead of a half-drawn interface or a hanging read.
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("{}", t!("err.interactive_tty"));
    }

    let Some(target) = pick_worktree(app)? else {
        return Ok(());
    };
    if target == "__new__" {
        return create(app);
    }

    let Some(action) = pick_one(t!("ui.actions").as_ref(), action_choices(app), None)? else {
        return Ok(());
    };
    dispatch(app, &target, &action)
}

fn pick_worktree(app: &App) -> Result<Option<String>> {
    let mut items = vec![
        Choice::new("__new__", action_label(t!("action.new")), "").preview(format!(
            "{}: {}\n{}: {}",
            t!("label.config"),
            app.project.config_path.display(),
            t!("label.root"),
            app.root.display()
        )),
    ];
    items.extend(worktree_choices(app));

    pick_one(t!("skim.worktree").as_ref(), items, Some("right:55%:wrap"))
}

fn worktree_choices(app: &App) -> Vec<Choice> {
    let mut items = Vec::new();
    for wt in app.list() {
        let branch = crate::git::current_branch(&wt.path);
        let mut detail = Vec::new();
        if let Some(up) = app.is_up(&wt) {
            detail.push(
                if up {
                    t!("state.started")
                } else {
                    t!("state.stopped")
                }
                .to_string(),
            );
        }
        if !wt.state.ports.is_empty() {
            detail.push(
                wt.state
                    .ports
                    .iter()
                    .map(|(name, port)| format!("{name}:{port}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        let suffix = if detail.is_empty() {
            branch
        } else {
            format!("{branch}  ·  {}", detail.join("  ·  "))
        };
        let preview = app.preview(&wt).join("\n");
        items.push(Choice::new(wt.slug.clone(), wt.slug, suffix).preview(preview));
    }
    items
}

/// Chooses a worktree for a subcommand whose slug was omitted.
pub fn choose_worktree(app: &App) -> Result<Option<String>> {
    let items = worktree_choices(app);
    if items.is_empty() {
        bail!("{}", t!("info.no_worktrees", path = app.root.display()));
    }
    pick_one(t!("skim.worktree").as_ref(), items, Some("right:55%:wrap"))
}

/// Chooses a task for `wt run` when its task argument was omitted.
pub fn choose_task(app: &App) -> Result<Option<String>> {
    let choices = app
        .project
        .config
        .tasks
        .iter()
        .map(|(name, task)| Choice::new(name.clone(), name, &task.description))
        .collect::<Vec<_>>();
    if choices.is_empty() {
        bail!(
            "{}",
            t!("info.no_tasks", path = app.project.config_path.display())
        );
    }
    pick_one(t!("ui.task").as_ref(), choices, None)
}

fn action_choices(app: &App) -> Vec<Choice> {
    let mut items = Vec::new();
    if app.has_up() {
        items.push(Choice::new(
            "up",
            action_label(t!("action.start")),
            "hooks up",
        ));
        items.push(Choice::new(
            "up-opts",
            action_label(t!("action.start_opts")),
            "--set key=value",
        ));
    }
    if app.has_down() {
        items.push(Choice::new(
            "down",
            action_label(t!("action.stop")),
            "hooks down",
        ));
    }
    items.push(Choice::new(
        "shell",
        action_label(t!("action.shell")),
        t!("action.shell_detail"),
    ));
    items.push(Choice::new("ide", action_label(t!("action.editor")), ""));
    if app.has_open() {
        items.push(Choice::new("open", action_label(t!("action.browser")), ""));
    }
    if app.has_tasks() {
        items.push(Choice::new(
            "task",
            action_label(t!("action.task")),
            "wt.toml [tasks]",
        ));
    }
    items.push(Choice::new(
        "rm",
        action_label(t!("action.remove")),
        t!("action.remove_detail"),
    ));
    items
}

/// Ratatui's action labels start with their one-key shortcut (`n · …`). In Skim every
/// printable key feeds the fuzzy query, so displaying those prefixes would promise a
/// shortcut that deliberately does not exist here.
fn action_label(label: impl AsRef<str>) -> String {
    let label = label.as_ref();
    label
        .split_once(" · ")
        .map_or(label, |(_, description)| description)
        .to_string()
}

fn dispatch(app: &App, slug: &str, action: &str) -> Result<()> {
    match action {
        "up" => {
            let Some(sets) = phase_options(app, Ask::Up, slug)? else {
                return Ok(());
            };
            app.cmd_up(slug, &sets)
        }
        "up-opts" => {
            let Some(raw) = read_line(&t!("ui.start_options", slug = slug), Some(""))? else {
                return Ok(());
            };
            let sets = raw
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            app.cmd_up(slug, &sets)
        }
        "down" => app.cmd_down(slug),
        // The Skim interface has already finished, so the shell can naturally use this
        // terminal.  `exit` returns to the caller rather than to an idle dashboard.
        "shell" => app.cmd_shell(slug),
        "ide" => open_editor(app, slug),
        "open" => open_link(app, slug),
        "task" => run_task(app, slug),
        "rm" => {
            if confirm(&t!("confirm.remove_long", slug = slug))? {
                app.cmd_rm(slug, true)
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

fn create(app: &App) -> Result<()> {
    let mut branches = vec![Choice::new(
        "__new__",
        t!("ui.new_branch"),
        t!("ui.new_branch_hint"),
    )];
    for branch in crate::git::branches(&app.project.main) {
        let used = branch.used_by.is_some();
        let label = if used {
            format!("⊘ {}", branch.name)
        } else {
            branch.name.clone()
        };
        let detail = if used {
            t!("ui.already_checked_out").to_string()
        } else {
            format!("{} · {}", branch.date, branch.subject)
        };
        branches.push(Choice::new(branch.name, label, detail).disabled(used));
    }
    let Some(picked) = pick_one(t!("ui.branch_title").as_ref(), branches, None)? else {
        return Ok(());
    };

    let (branch, from, suggestion) = if picked == "__new__" {
        let head = crate::git::current_branch(&app.project.main);
        let mut bases = crate::git::branches(&app.project.main)
            .into_iter()
            .map(|branch| {
                let current = branch.name == head;
                let label = if current {
                    format!("● {}", branch.name)
                } else {
                    branch.name.clone()
                };
                let detail = if current {
                    format!(
                        "{} · {} · {}",
                        t!("ui.head_here"),
                        branch.date,
                        branch.subject
                    )
                } else {
                    format!("{} · {}", branch.date, branch.subject)
                };
                (current, Choice::new(branch.name, label, detail))
            })
            .collect::<Vec<_>>();
        // ENTER accepts the repository's HEAD, just like the Ratatui flow.
        bases.sort_by_key(|(current, _)| !current);
        let bases = bases
            .into_iter()
            .map(|(_, choice)| choice)
            .collect::<Vec<_>>();
        let has_bases = !bases.is_empty();
        let from = if bases.is_empty() {
            None
        } else {
            pick_one(t!("ui.base_title").as_ref(), bases, None)?
        };
        if has_bases && from.is_none() {
            return Ok(());
        }
        (None, from, String::new())
    } else {
        let suggestion = ops::slugify(picked.trim_start_matches("origin/"));
        (Some(picked), None, suggestion)
    };

    let title = match (&branch, &from) {
        (Some(branch), _) => t!("ui.slug_for", branch = branch).to_string(),
        (None, Some(base)) => t!("ui.slug_from", base = base).to_string(),
        _ => t!("ui.slug_title").to_string(),
    };
    let Some(slug) = read_line(&title, Some(&suggestion))? else {
        return Ok(());
    };
    let slug = slug.trim().to_string();
    if slug.is_empty() {
        return Ok(());
    }

    let Some(sets) = phase_options_with(app, Ask::New, &slug, BTreeMap::new())? else {
        return Ok(());
    };
    app.cmd_new(&slug, branch.as_deref(), from.as_deref(), &sets)?;

    if app.has_up() && confirm(&t!("confirm.start_now", slug = slug))? {
        let Some(sets) = phase_options(app, Ask::Up, &slug)? else {
            return Ok(());
        };
        app.cmd_up(&slug, &sets)?;
    }
    Ok(())
}

fn open_editor(app: &App, slug: &str) -> Result<()> {
    let choices = app
        .editors()
        .into_iter()
        .map(|editor| Choice::new(editor.clone(), editor, ""))
        .collect::<Vec<_>>();
    let Some(editor) = pick_one(t!("ui.editor").as_ref(), choices, None)? else {
        return Ok(());
    };
    app.cmd_ide(slug, Some(&editor))
}

fn open_link(app: &App, slug: &str) -> Result<()> {
    let state = crate::state::load(&app.root, slug);
    let links = app.links(slug, &state);
    match links.as_slice() {
        [] => {
            ops::warn(&t!("ui.no_url"));
            Ok(())
        }
        [_] => app.cmd_open(slug, None, false),
        _ => {
            let choices = links
                .into_iter()
                .map(|link| Choice::new(link.url.clone(), link.label, link.url))
                .collect::<Vec<_>>();
            let Some(url) = pick_one(t!("ui.open_title", slug = slug).as_ref(), choices, None)?
            else {
                return Ok(());
            };
            app.cmd_open(slug, Some(&url), false)
        }
    }
}

fn run_task(app: &App, slug: &str) -> Result<()> {
    let choices = app
        .project
        .config
        .tasks
        .iter()
        .map(|(name, task)| Choice::new(name.clone(), name, &task.description))
        .collect::<Vec<_>>();
    let Some(task) = pick_one(t!("ui.task").as_ref(), choices, None)? else {
        return Ok(());
    };

    let prompts = app.task_prompts(&task);
    if prompts.is_empty() {
        return app.cmd_run(&task, slug, &[]);
    }
    let known = crate::state::load(&app.root, slug).opts;
    let Some(answers) = answer_prompts(app, prompts, Ask::Task, slug, known)? else {
        return Ok(());
    };
    let args = app
        .task_prompts(&task)
        .iter()
        .filter_map(|prompt| {
            answers
                .get(&prompt.name)
                .map(|value| (prompt.separator.as_str(), value.as_str()))
        })
        .flat_map(|(separator, value)| value.split(separator))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if args.is_empty() {
        return Ok(());
    }
    app.cmd_run(&task, slug, &args)
}

fn phase_options(app: &App, phase: Ask, slug: &str) -> Result<Option<Vec<String>>> {
    let known = crate::state::load(&app.root, slug).opts;
    phase_options_with(app, phase, slug, known)
}

fn phase_options_with(
    app: &App,
    phase: Ask,
    slug: &str,
    known: BTreeMap<String, String>,
) -> Result<Option<Vec<String>>> {
    let prompts = app.prompts_for(phase, &known);
    let Some(answers) = answer_prompts(app, prompts, phase, slug, known)? else {
        return Ok(None);
    };
    Ok(Some(
        answers
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect(),
    ))
}

fn answer_prompts(
    app: &App,
    prompts: Vec<Prompt>,
    phase: Ask,
    slug: &str,
    mut answers: BTreeMap<String, String>,
) -> Result<Option<BTreeMap<String, String>>> {
    for prompt in prompts {
        if !app.prompt_applies(&prompt, slug, &answers, phase) {
            continue;
        }
        if prompt.kind == PromptKind::Text {
            let Some(value) = read_line(prompt.title(), prompt.default.as_deref())? else {
                return Ok(None);
            };
            answers.insert(prompt.name, value);
            continue;
        }

        let options = app.prompt_choices(&prompt, slug, &answers, phase);
        if options.is_empty() {
            ops::warn(&t!("ui.no_choices", question = prompt.title()));
            continue;
        }
        let defaults = prompt
            .default
            .as_deref()
            .into_iter()
            .flat_map(|value| value.split(&prompt.separator))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut choices = options
            .into_iter()
            .map(|option| {
                let label = if option.label.is_empty() {
                    option.value.clone()
                } else {
                    option.label
                };
                Choice::new(option.value, label, option.detail)
            })
            .collect::<Vec<_>>();
        let multi = prompt.kind == PromptKind::Multi;
        // A single-choice default belongs under the cursor. Multiple defaults, on the
        // other hand, are marked with Skim's native pre-selection support.
        if !multi {
            if let Some(index) = choices
                .iter()
                .position(|choice| defaults.contains(&choice.key))
            {
                choices.swap(0, index);
            }
        }
        let presets = if multi { defaults.as_slice() } else { &[] };
        let Some(values) = pick(prompt.title(), choices, multi, presets, None)? else {
            return Ok(None);
        };
        let value = if multi {
            values.join(&prompt.separator)
        } else {
            values.into_iter().next().unwrap_or_default()
        };
        answers.insert(prompt.name, value);
    }
    Ok(Some(answers))
}

fn pick_one(
    title: &str,
    choices: Vec<Choice>,
    preview_window: Option<&str>,
) -> Result<Option<String>> {
    Ok(pick(title, choices, false, &[], preview_window)?.and_then(|mut values| values.pop()))
}

fn pick(
    title: &str,
    choices: Vec<Choice>,
    multi: bool,
    defaults: &[String],
    preview_window: Option<&str>,
) -> Result<Option<Vec<String>>> {
    if choices.is_empty() {
        return Ok(None);
    }
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("{}", t!("err.interactive_tty"));
    }
    let selected = choices
        .iter()
        .filter(|choice| defaults.contains(&choice.key))
        .map(|choice| choice.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let hint = if multi {
        t!("skim.hint_multi")
    } else {
        t!("skim.hint")
    };
    let mut builder = SkimOptionsBuilder::default();
    builder
        .height("85%")
        .reverse(true)
        .cycle(true)
        .multi(multi)
        .prompt(format!("{title} › "))
        .header(hint.to_string());
    if !selected.is_empty() {
        builder.pre_select_items(selected);
    }
    if let Some(layout) = preview_window {
        // An empty global command enables the pane; every Choice supplies its own text.
        builder.preview("").preview_window(layout);
    }
    let options = builder.build().map_err(|error| anyhow::anyhow!(error))?;
    let output = Skim::run_items(options, choices).map_err(|error| anyhow::anyhow!("{error:#}"))?;
    if output.is_abort {
        return Ok(None);
    }
    Ok(Some(
        output
            .selected_items
            .into_iter()
            .map(|item| item.output().into_owned())
            .collect(),
    ))
}

/// A cooked-terminal text field used only for genuinely free-form input.  Choices stay
/// in Skim, where they remain searchable and defaults can be preselected.
fn read_line(question: &str, default: Option<&str>) -> Result<Option<String>> {
    match default.filter(|value| !value.is_empty()) {
        Some(value) => eprint!("{question} [{value}] › "),
        None => eprint!("{question} › "),
    }
    io::stderr().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Ok(None);
    }
    let answer = answer.trim().to_string();
    Ok(Some(if answer.is_empty() {
        default.unwrap_or_default().to_string()
    } else {
        answer
    }))
}

fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [{}] ", t!("hint.yes_no"));
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "o" | "O"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_keeps_display_and_return_value_separate() {
        let choice = Choice::new("machine-value", "Visible label", "detail");
        assert!(choice.text().contains("Visible label"));
        assert!(choice.text().contains("detail"));
        assert_eq!(choice.output(), "machine-value");
    }

    #[test]
    fn skim_actions_do_not_advertise_ratatui_shortcuts() {
        assert_eq!(action_label("n · create a worktree"), "create a worktree");
        assert_eq!(action_label("plain label"), "plain label");
    }

    #[test]
    fn selecting_a_worktree_only_offers_actions_for_that_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let project = crate::config::Project {
            main: dir.path().to_path_buf(),
            config_path: dir.path().join("wt.toml"),
            config: crate::config::Config::default(),
        };
        let app = App::new(project).unwrap();

        let keys = action_choices(&app)
            .into_iter()
            .map(|choice| choice.key)
            .collect::<Vec<_>>();

        assert_eq!(keys, ["shell", "ide", "rm"]);
    }
}
