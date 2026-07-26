# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
