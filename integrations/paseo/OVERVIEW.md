Connects Paseo workspaces to [wt](https://github.com/Catvert/wt), the git worktree manager
driven by a per-project `wt.toml`. Paseo creates and deletes the worktrees; wt gives each one
its ports, copies, dependencies and hooks, and knows its addresses.

## What it does

- **Adoption.** When Paseo creates a worktree for a project that has a `wt.toml`, the plugin
  runs `wt new <slug> --adopt <worktree>`: the worktree gets its ports, copies and `post_new`
  hooks as if wt had created it. Projects whose `paseo.json` declares a `worktree.setup` are
  left to it. A checkout wt does not know yet can be adopted by hand from the panel.
- **Header button.** Each adopted workspace gets a `wt · <port>` button. Its popover lists
  the worktree's addresses (the `[open]` URL and `[open] source` entries, the machine's
  `wt-proxy-url` when there is one, otherwise one `localhost` address per port) and starts
  or stops it.
- **Panel.** The `wt` panel, in a tab or in the Explorer, adds the options, the
  `[status.info]` probes, the uncommitted changes and the `wt.toml` tasks. On desktop an
  address opens in a Paseo browser tab, the arrow next to it in the system browser.
- **Terminals.** `wt up`, `wt down` and every task run in a Paseo terminal of the
  workspace, so `[[prompt]]` questions can be answered and the output stays visible.
- **Cleanup.** When a workspace is archived, the plugin waits for Paseo to delete the
  checkout, then runs `wt gc --apply <slug>`: removal hooks, state and port go.
- **Command Center.** "wt : ouvrir l'application du worktree" and "wt : panneau du worktree".

## Setup

`wt` 0.10 or later must be on the daemon's `PATH` (the plugin relies on `--json`). Paseo
0.13.2 or later.

## Limits

Adoption and cleanup only concern worktrees Paseo created under its own home
(`$PASEO_HOME/worktrees`). If Paseo leaves files behind when deleting a worktree, `wt gc`
is not run and the plugin log says so.
