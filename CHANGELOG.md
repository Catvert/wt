# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
