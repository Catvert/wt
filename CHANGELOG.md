# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `WT_ROOT`: the worktree root for projects whose `wt.toml` sets no `root`, with the same
  template (`{{main}}`, `{{repo}}`). A machine can gather every project's worktrees elsewhere
  without touching shared `wt.toml` files; an explicit `root` still wins.

### Fixed

- Paseo plugin: worktrees created under a custom `worktrees.root` (Paseo's `config.json`)
  were neither adopted nor cleaned up after archiving; only `$PASEO_HOME/worktrees` was
  recognised.

## [0.10.0] — 2026-10-27

### Added

- `--json` on `wt ls`, `wt show`, `wt open --list`, `wt root` and `wt tasks`: stable fields,
  independent of `WT_LANG`, for tools driving wt from outside. `checkout` gives where an
  adopted worktree really is, `root --json` what the wt.toml declares and its tasks.
- `integrations/paseo/`: a Paseo plugin built on it. It adopts the worktrees Paseo creates
  (unless the project's `paseo.json` does it in `worktree.setup`), adds a `wt` button and
  panel to the workspace (addresses, ports, options, probes, tasks, `up`/`down` in a Paseo
  terminal) and runs `wt gc --apply` once Paseo has deleted an archived worktree.

## [0.9.0] — 2026-10-19

### Added

- `wt gc` cleans up after worktrees removed without `wt rm` (a `git worktree remove`, a
  deleted directory): the removal hooks run with the options recorded in the state, `pre_rm`
  in an empty temporary directory with `$WT_GC=1`, then the git metadata and the state go.
  Dry run by default, `--apply` to act, `--forget <slug>` to drop an entry without hooks,
  `wt gc <slug>…` to limit it to some entries. Ambiguous cases (directory still present,
  locked, moved, unreadable or old state without options) are reported, never acted upon.
  The contract is in `docs/gc.md`.
- `wt new <slug> --adopt <path>` takes over a git worktree another tool created (an agent
  orchestrator, an IDE): `<root>/<slug>` becomes a link to it, then directories, copies,
  ports, state and `post_new` hooks run as for a new worktree. Copies already present in
  the checkout are kept; a failing hook removes the link and the state again. `wt rm`
  removes the adopted checkout and the link; when the tool deletes the checkout itself,
  `wt gc` treats the dangling link as an orphan.

### Changed

- `wt new` records `format = 1` in the state, so that `wt gc` can trust an empty option
  set.

## [0.8.0] — 2026-08-23

### Added

- `wt pick` opens the one-shot Skim workflow.

### Changed

- Running `wt` without a subcommand opens the Ratatui dashboard again, as it did before
  0.6.0. The Skim workflow moves to `wt pick`; `wt tui` still names the dashboard.

## [0.7.0] — 2026-08-22

### Added

- `wt` is now a library as well as a binary. `config`, `git`, `ops`, `state`, `tmpl`,
  `util` and `fuzzy` are public modules; a graphical front-end can create worktrees, run
  tasks and read a `wt.toml` without parsing the output of a CLI written for a human.
- A `cli` feature, on by default, carries the command line, the Skim workflow and the
  Ratatui dashboard along with their dependencies. `default-features = false` builds the
  library alone, without clap, ratatui or skim.
- An `[lsp.<name>]` section: the language servers a project's code wants, with their
  command, arguments, environment and the extensions they serve. `wt` neither starts nor
  supervises them — they are read by a front-end embedding the library, which is why a
  project declares them in the file it already uses rather than in a second one.

## [0.6.0] — 2026-08-15

### Added

- A focused, embedded Skim interface for the default `wt` invocation: fuzzy-select a
  worktree, then an action, with previews and the same searchable branch, task, editor,
  address and `[[prompt]]` choices. It requires no separate `fzf` or `sk` executable.
- `wt tui` opens the persistent Ratatui dashboard that `wt` used to open directly.

### Changed

- Running `wt` without a subcommand now opens the one-shot Skim workflow. The selected
  action gets the terminal after the picker closes; this suits shells, terminal editors
  and interactive tasks without temporarily suspending a dashboard.
- Subcommands now open a Skim picker for omitted enumerable arguments: worktrees for
  commands such as `wt open`, and the task followed by the worktree for `wt run`.

### Fixed

- Selecting a worktree in the Skim workflow no longer offers worktree creation again in
  its action menu.

## [0.5.0] — 2026-08-05

### Added

- A worktree branch that tracks nothing is wired to `origin/<branch>` at creation, so
  the first `git push` from the worktree needs no `-u origin …`. Branches created from
  an `origin/…` start point keep the tracking git already set up, and without an
  `origin` remote nothing is invented.

## [0.4.0] — 2026-08-01

### Added

- Tasks can declare questions: `prompt = ["name"]` on a `[tasks.*]` entry lists
  `[[prompt]]` entries the interface asks before the run — same pickers as `new`/`up`,
  `source` included — and the answers become the task's `{{args}}` (a `multi` value is
  split on its separator). They are per-run inputs: always asked, never remembered in
  the worktree's options. On the command line, arguments are passed directly and
  nothing is asked. A prompt meant only for tasks takes `ask = "task"`, which no phase
  ever triggers.
- Going back a step in the interface: `⌫` returns to the previous question — the branch,
  the start point, the slug, an answer already given — with what was chosen there under
  the cursor again, and the questions that followed asked anew. It only steps back with
  nothing left to delete (an empty search, an empty field); the window shows `⌫  back`
  when it applies. `ESC` still gives up on the action as a whole.
- `^U` empties an input field and `^W` cuts its last word, as they already did in a
  picker's search box. A control key no longer types its letter into the field.

### Changed

- `c` in the interface now opens the shell in a **terminal window of its own**: the list
  stays where it is, and the session outlives it. The emulator is `WT_TERMINAL_WINDOW`,
  then `[editor] terminal_window`, then whichever known one is installed — Windows
  Terminal under WSL, ghostty, WezTerm, kitty, Alacritty, foot, GNOME Terminal, Konsole,
  xfce4-terminal, xterm, Terminal.app on macOS. The window gets the same shell and the
  same `$WT_*` variables as before. A machine with no emulator — a bare TTY, an ssh
  session — keeps the old behaviour, the interface stepping aside for the session, and
  says so in its help; `WT_TERMINAL_WINDOW=""` asks for it outright. `wt shell` on the
  command line is unchanged.

## [0.3.0] — 2026-07-27

### Added

- Getting into a worktree without going through a hook: `wt shell [slug]` opens a shell
  at its root (with the worktree's `$WT_*` variables exported, as the hooks get them),
  and `c` does the same from the interface. Both take a fragment of a slug and ask when
  it leaves several worktrees.
- `wt cd [slug]` moves the current shell instead of nesting one, via the function
  `wt shell-init <bash|zsh|fish>` writes. Without the integration installed, `wt cd`
  prints the path and says which line is missing.
- Completion of what only the project knows: worktree slugs (`wt cd <TAB>`), tasks
  (`wt run <TAB>`) and branches (`wt new demo <TAB>`, `--from <TAB>`), each with its
  branch, description or commit subject as the shown detail.

### Changed

- `wt completions <shell>` now writes a script that asks the binary for candidates
  instead of one carrying a frozen list — that is what lets it complete slugs. Sourcing
  `COMPLETE=<shell> wt` at shell startup is the alternative, and cannot fall behind the
  binary. Re-generate the script after upgrading wt.

### Fixed

- Cancelling `wt rm` at the confirmation prompt printed `err.cancelled` instead of the
  message: the key was missing from both locales.

## [0.2.0] — 2026-07-26

### Added

- Fuzzy search in every picker (branches, tasks, editors, addresses, `[[prompt]]`
  answers): typing filters the list fzf-style, spaces narrow, matches are ranked and
  highlighted, and a counter shows what is left.
- Start point for a new branch: the interface asks which branch it comes off (the main
  repository's checked-out branch preselected), and `wt new` takes `--from <ref>`.
  Previously a new branch always started at the main repository's HEAD.

### Changed

- Pickers now treat printable keys as search input. Navigation moved to the arrows and
  to `^N`/`^P` (`^J`/`^K`), ticking a box in a multiple choice moved from `SPACE` to
  `TAB`, and `q` no longer closes a picker — `ESC` does, after clearing the search.

## [0.1.0] — 2026-07-26

First public release.

### Added

- Worktree lifecycle driven by a per-project `wt.toml`: create, start, stop, remove.
- Inheritance from the main repository: hardlink, copy or symlink, declared per entry.
- Automatic port allocation, stable across restarts and unique across worktrees.
- Shell hooks at four moments (`post_new`, `up`, `down`, `pre_rm`/`post_rm`) plus
  on-demand `[tasks]`.
- Per-worktree state (branch, ports, last start's options) kept outside the checkout.
- Interactive interface: list, preview, action output streamed in a panel with ANSI
  colours interpreted, mouse support, project-declared questions (`[[prompt]]`).
- Multiple browser addresses per worktree (`[open] source`).
- English and French interface, selected from `WT_LANG` or the POSIX locale.
- Shell completions (`wt completions`), Nix flake with overlay and dev shell.
