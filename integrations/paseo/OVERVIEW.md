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
  `wt-proxy-url` when there is one, otherwise one `localhost` address per port), asks the
  `wt up` questions described below, and starts or stops it.
- **Panel.** The `wt` panel, in a tab or in the Explorer, adds the options, the
  `[status.info]` probes, the uncommitted changes and the `wt.toml` tasks. The `wt up`
  questions with fixed choices (`[[prompt]]` without `source` or `when`) are asked there, as
  in the popover but with each choice's detail:
  checkboxes for `multi` and yes/no choices, pills otherwise, preset to the worktree's saved
  answers or the `wt.toml` default. Start passes them with `--set`; on a running worktree,
  changing one offers to apply it (`wt up` again). The other questions stay in the terminal. On desktop an
  address opens in a Paseo browser tab, the arrow next to it in the system browser.
- **Tools.** The `[open] source` lines with a third column, an icon name
  (`url<TAB>label<TAB>Database`), are the worktree's tools (database admin, mail catcher,
  error tracker…): a row of buttons at the top of the popover and the panel, opening in a
  Paseo browser tab. Paseo gives that tab only to panels: the popover borrows it from a `wt`
  panel already shown on the same host (in a tab or the Explorer), and falls back to the
  system browser until then.
- **Automatic stop.** Before a start, pills choose how long the worktree may run (1 h to
  8 h, or never; preset to the daemon's `WT_STOP_AFTER`), passed as `wt up --stop-after`.
  On a started worktree the panel shows the time left and the same pills keep it that much
  longer from now (`wt keep`). A timer running `wt down --expired` does the stopping.
- **Git.** Two more buttons in that row: **VS Code**, a web editor opened on the worktree when
  the daemon has `WT_WEB_EDITOR_URL` (`{path}` stands for the checkout, URL-encoded: for
  example openvscode-server's `https://code.example/?folder={path}`), and **lazygit** in a
  Paseo terminal of the workspace, when it is on the daemon's `PATH`.
- **Terminals.** `wt up`, `wt down` and every task run in a Paseo terminal of the
  workspace, so `[[prompt]]` questions can be answered and the output stays visible.
- **Cleanup.** When a workspace is archived, the plugin waits for Paseo to delete the
  checkout, then runs `wt gc --apply <slug>`: removal hooks, state and port go.
- **Command Center.** "wt : ouvrir l'application du worktree" and "wt : panneau du worktree".

## Setup

`wt` 0.10 or later must be on the daemon's `PATH` (the plugin relies on `--json`); the
start-up questions in the panel and the tools need the next release (`prompts` in
`wt root --json`, `icon` in `links`). Paseo
0.13.2 or later.

## Limits

Adoption and cleanup only concern worktrees Paseo created under its worktrees root:
`worktrees.root` of its `config.json` when set, `$PASEO_HOME/worktrees` otherwise. Like the
daemon, the plugin reads it at startup. If Paseo leaves files behind when deleting a
worktree, `wt gc` is not run and the plugin log says so.
