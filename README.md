# wt

A git worktree manager, configured **per project** in a `wt.toml` file — the way `just`
uses a `justfile`.

*[Version française](README.fr.md)*

The binary knows nothing about Docker, Laravel or npm. It does five things:

- creates and removes git worktrees;
- copies what a worktree should inherit from the main repository (hardlinks for
  `vendor/`, `node_modules/`… — nearly free on disk);
- runs the shell commands the project declares at the moments that matter (creation,
  start, stop, removal) and on demand (`[tasks]`);
- tracks per-worktree state (branch, last start's options, ports if the project asks for
  any);
- shows all of it in an interactive interface.

Anything stack-specific goes through shell hooks: a Laravel app, a Rust CLI and a Python
data pipeline use the same binary with three different `wt.toml` files.

**Nothing is assumed to be a web app.** Ports, URLs and status probes are optional: a
project that declares none sees no "PORTS" column, no state and no "browser" action. For
many repositories a three-line `wt.toml` is enough:

```toml
branch = "wt/{{slug}}"

[tasks]
test = { description = "tests", run = "cargo test {{args}}" }
```

## Install

### Nix / NixOS

```bash
nix run github:Catvert/wt                 # try it without installing
nix profile install github:Catvert/wt     # install for the current user
```

In a NixOS configuration or with home-manager:

```nix
{
  inputs.wt.url = "github:Catvert/wt";

  # environment.systemPackages = [ inputs.wt.packages.${system}.default ];
  # or, with the overlay:
  # nixpkgs.overlays = [ inputs.wt.overlays.default ];
  # environment.systemPackages = [ pkgs.wt ];
}
```

Shell completions are installed by the package.

### Binary cache (no local compilation)

Builds are pushed to [Cachix](https://cachix.org) by CI, so `nix run` / `nix profile
install` download the binary instead of compiling it.

Nix offers the cache on its own when it reads the flake (it asks for confirmation unless
you are a `trusted-user`). To accept it once and for all, on NixOS:

```nix
{
  nix.settings = {
    substituters = [ "https://wt.cachix.org" ];
    trusted-public-keys = [ "wt.cachix.org-1:REPLACE_ME" ];
  };
}
```

Outside NixOS, `cachix use wt` writes the same thing into `~/.config/nix/nix.conf`.

### From source

```bash
cargo install --git https://github.com/Catvert/wt
```

### Prebuilt binary

Each release ships `x86_64-unknown-linux-gnu` and `x86_64-unknown-linux-musl` (statically
linked) archives, with checksums. Shell completions:

```bash
wt completions zsh > ~/.zfunc/_wt      # bash | zsh | fish | elvish | powershell
```

## Getting started

```bash
cd my-project
wt init                 # a wt.toml with no services; --preset web for a port + URL
$EDITOR wt.toml
wt new demo             # creates ../my-project-wt/demo on branch wt/demo
wt                      # interactive interface
```

## Commands

| Command | Effect |
|---|---|
| `wt` | interactive interface (list, preview, actions) |
| `wt init [--preset plain\|web] [--force]` | writes an example `wt.toml` |
| `wt new <slug> [branch] [--set k=v]` | checkout + directories + copies + `post_new` hooks |
| `wt up <slug> [--set k=v]` | `up` hooks (and port allocation, if any) |
| `wt down <slug>` | `down` hooks — the checkout and state are kept |
| `wt ls` / `wt show <slug>` | worktree state |
| `wt rm <slug> [-y]` | `pre_rm` hooks, worktree removal, `post_rm` hooks |
| `wt ide <slug> [editor]` | opens the worktree in an editor |
| `wt open <slug> [target] [--list]` | opens an address in the browser (WSL included) |
| `wt run <task> <slug> [args…]` | runs a `wt.toml` task |
| `wt tasks` / `wt root` / `wt path <slug>` | introspection |
| `wt completions <shell>` | shell completion script |

`wt` works from the main repository **and from inside a worktree**: the configuration is
always the main repository's, not that of the branch currently checked out.

### Interface shortcuts

`↑↓`/`jk` move · `ENTER` action menu · `n` create · `s` start · `S` start with options ·
`d` stop · `e` editor · `o` browser · `t` task · `r` remove · `g` refresh · `m` mouse ·
`?` help · `q` quit.

**Mouse** (on by default): click selects a row, double-click opens the action menu, the
wheel scrolls the list, a picker or the output panel. In a multiple-choice list a click
toggles. `m` releases mouse capture so the terminal gets its native text selection back,
long enough to copy a line.

The footer, the action menu and the help only show what the `wt.toml` declares: without
`[hooks] up`, no "start"; without `[open] url`, no "browser".

### Action output

Creations, starts, stops, removals and tasks run **without leaving the interface**: their
output (stdout and stderr) scrolls in a panel as it comes, with `↑↓` / `PgUp` / `PgDn` to
scroll back and `ENTER` to close once the action is over. The list and preview refresh on
their own.

Command colours are **interpreted**, not printed raw: a hook writing `\033[36m…\033[0m`,
or a `docker`/`cargo` colouring its output, looks like it does in a terminal (256-colour
and RGB palettes included). Progress bars that rewrite themselves with `\r` only show
their final state.

A task that needs the terminal — a shell, a `logs -f`, a full-screen watcher — declares
`interactive = true`: the interface then steps aside while it runs and takes over again
afterwards. The editor does the same.

### Offered follow-ups

- **after a creation**, if the project has `[hooks] up`, the panel asks "start the
  services now?" — `o` chains (including the `wt.toml` questions), any other key closes;
- **after a GUI editor opens**, the interface offers a terminal at the worktree root —
  something an IDE window does not give you. The shell is `WT_TERMINAL`, then
  `[editor] terminal` from the `wt.toml`, then `$SHELL`. (An editor living in the
  terminal, like `nvim`, replaces the process: nothing can be chained after it, so the
  question is not asked.)

## The `wt.toml` file

Every section is optional. This one shows everything at once, as a reference — a project
without a server simply leaves `[ports]`, `[status]` and `[open]` out.

```toml
root   = "{{main}}/../{{repo}}-wt"   # default
branch = "wt/{{slug}}"               # default

[vars]
host = "{{slug}}.wt.localhost"       # vars may reference one another

dirs = ["storage/framework/views"]   # created after the checkout

[[copy]]
from = "node_modules"                # to = from by default
mode = "hardlink"                    # hardlink | copy | symlink

[ports.vite]
base = 5200                          # first port tried
allocate = "up"                      # "up" (default) | "new"

[hooks]
post_new = ["npm install"]
up       = ["docker compose -p {{repo}}-{{slug}} up -d"]
down     = ["docker compose -p {{repo}}-{{slug}} down"]
pre_rm   = ["docker compose -p {{repo}}-{{slug}} down -v"]
post_rm  = []

[status]
up = "docker compose -p {{repo}}-{{slug}} ps -q app | grep -q ."   # exit 0 = started

[status.info]                        # extra preview lines
size = "du -sh . | cut -f1"

[open]
url = "http://{{host}}"              # main address
label = "application"                # its label in the picker
source = "./scripts/urls.sh"         # extra addresses: url<TAB>label

[editor]
command = "phpstorm"                 # WT_IDE from the environment still wins
terminal = "zsh"                     # shell offered after the editor opens

[tasks.shell]
description = "shell inside the container"
interactive = true
run = "docker compose -p {{repo}}-{{slug}} exec app bash"
```

### Examples

| File | Profile |
|---|---|
| `examples/rust-cli.toml` | binary/library — **no services, no ports** |
| `examples/python-cli.toml` | script or data processing — venv per worktree, data symlinked |
| `examples/node-vite.toml` | one dev server per worktree |
| `examples/laravel-sail.toml` | multi-tenant, shared Caddy, isolated databases |

### Variables

| Variable | Value |
|---|---|
| `{{slug}}` `{{branch}}` `{{path}}` | the worktree |
| `{{main}}` `{{root}}` `{{repo}}` `{{project}}` | the project |
| `{{port.<name>}}` | port allocated for `[ports.<name>]` (if the project declares any) |
| `{{opt.<name>}}` | option passed with `--set <name>=<value>` |
| `{{args}}` | arguments of `wt run` (tasks only) |

The same values are exported to the hooks' environment: `WT_SLUG`, `WT_PATH`,
`WT_PORT_VITE`, `WT_OPT_TENANTS`… Handy as soon as a hook outgrows one line.

An unknown key is left as-is (`{{port.web}}` visible in the failing command beats an
argument silently gone), and `awk '{print $1}'` passes through the engine unharmed.

### `--set` options

`wt up demo --set tenants=acme,globex --set services=queue,reverb` makes
`{{opt.tenants}}` and `{{opt.services}}` available to the hooks. They are **remembered**:
a later `wt up demo` repeats the same start, and a new `--set` only replaces the keys it
names.

### Questions asked before an action (`[[prompt]]`)

A project can declare the questions the interface should ask before `new` or `up`. The
answers become options — exactly like a `--set`, memorisation included.

```toml
[[prompt]]
name = "db"                  # -> {{opt.db}} and $WT_OPT_DB
ask = "new"                  # "up" (default) | "new" | "both"
question = "databases"
type = "choice"              # "choice" | "multi" | "confirm" | "text"
default = "shared"
options = [
    { value = "shared",   label = "shared",   detail = "no migration possible" },
    { value = "isolated", label = "isolated", detail = "required if the branch migrates" },
]

[[prompt]]
name = "tenants"
type = "multi"                            # SPACE toggles, ENTER confirms
separator = ","                           # how checked values are joined (default)
when = "test \"$WT_OPT_DB\" = isolated"   # only asked when the command exits 0
source = "my-script --list"               # one line per choice: value<TAB>label<TAB>detail
```

- **`source`** makes the list dynamic: the binary does not know what a tenant, a database
  or a device is — it runs the project's command and shows what it enumerates.
- **`when`** sees the answers already given (`$WT_OPT_*`) and the current phase
  (`$WT_PHASE` = `new` or `up`), which is what lets questions chain.
- An option **already known** is not asked again — that is what makes a repeated `wt up`
  reproduce the same setup silently. `always = true` forces the question.
- `default` preselects (or pre-checks in `multi`): the common case is one ENTER away.
- `ESC` during a question **cancels the whole action**: running it with half-collected
  answers would be worse than doing nothing.
- A `source` returning nothing does not stall the interface: the question is skipped and
  the action goes on.

On the command line nothing is ever asked — `wt new demo --set db=isolated` stays fully
scriptable.

### Several addresses (`[open] source`)

A worktree does not always have a single address: a multi-tenant application has one per
mounted tenant, a multi-service project one per service. `source` is a shell command
enumerating them, one line per link:

```
http://acme.demo.wt.localhost	tenant acme
http://globex.demo.wt.localhost	tenant globex
```

It sees the worktree's options (`$WT_OPT_TENANTS`…), so it can offer only what is really
mounted. It only runs when opening — never to render the worktree list — because it may
query a database or a container.

```bash
wt open demo                 # the first address (the application)
wt open demo globex          # the one whose label or URL contains "globex"
wt open demo --list          # show them all
```

In the interface, `o` opens directly when there is a single address, and offers a picker
as soon as there are several.

## What `wt` guarantees

- **Nothing is imposed.** Ports, state, URLs, hooks: everything is optional, and the
  interface shrinks to what the project declares. An empty `wt.toml` is viable.
- **Stable ports** — for projects that ask for them. Allocated once, kept while stopped,
  never reused by another worktree of the project: bookmarks and IDE configurations stay
  put.
- **State lives outside the checkout**, in `<root>/.wt/<slug>.toml`: nothing to add to
  the project's `.gitignore`, and the state survives a branch change.
- **The branch survives `wt rm`.** Removing a worktree must never make unmerged work
  disappear; `wt` prints the `git branch -d` command for you to run yourself.
- **A POSIX shell** (`sh`) runs the hooks, whatever the user's shell (override with
  `WT_SHELL`).

## Environment variables

| Variable | Effect |
|---|---|
| `WT_LANG` | interface language (`en`, `fr`); otherwise `LC_ALL`/`LC_MESSAGES`/`LANG`, then English |
| `WT_CONFIG` | path to an alternative `wt.toml` |
| `WT_IDE` | preferred editor (command or absolute path, Windows `.exe` included) |
| `WT_TERMINAL` | shell opened in the worktree (default `$SHELL`) |
| `WT_SHELL` | shell running the hooks (default `sh`) |

## Related projects

`wt.exe` is Windows Terminal, and crates.io hosts `wt-core` and `wt-cli`, two unrelated
worktree managers. This project is not published to crates.io.

## License

MIT.
