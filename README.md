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

Shell completions are installed by the package, slug completion included.

### Binary cache (no local compilation)

Builds are pushed to [Cachix](https://cachix.org) by CI, so `nix run` / `nix profile
install` download the binary instead of compiling it.

The flake advertises the substituter, but Nix only honours a flake's `nixConfig` after
an interactive confirmation — it is **silently ignored** in scripts and CI. Configure it
on the machine instead. On NixOS:

```nix
{
  nix.settings = {
    substituters = [ "https://catvert.cachix.org" ];
    trusted-public-keys = [
      "catvert.cachix.org-1:R5plivdLnx2WtmZkBryZwUF51Uvl6TJldhFGYOcyPXg="
    ];
  };
}
```

Outside NixOS — or for a one-off — `cachix use catvert` writes the same thing into
`~/.config/nix/nix.conf`, and a single build accepts it inline:

```bash
nix build github:Catvert/wt \
  --option extra-substituters https://catvert.cachix.org \
  --option extra-trusted-public-keys "catvert.cachix.org-1:R5plivdLnx2WtmZkBryZwUF51Uvl6TJldhFGYOcyPXg="
```

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
wt new fix --from dev   # same, but the branch starts at dev
wt shell demo           # a shell inside the worktree — claude, a build, a rebase…
wt                      # interactive interface
```

## Commands

| Command | Effect |
|---|---|
| `wt` | interactive interface (list, preview, actions) |
| `wt init [--preset plain\|web] [--force]` | writes an example `wt.toml` |
| `wt new <slug> [branch] [--from ref] [--set k=v]` | checkout + directories + copies + `post_new` hooks |
| `wt up <slug> [--set k=v]` | `up` hooks (and port allocation, if any) |
| `wt down <slug>` | `down` hooks — the checkout and state are kept |
| `wt ls` / `wt show <slug>` | worktree state |
| `wt rm <slug> [-y]` | `pre_rm` hooks, worktree removal, `post_rm` hooks |
| `wt shell [slug]` | opens a shell at the worktree root |
| `wt cd [slug]` | changes directory to the worktree (needs `wt shell-init`) |
| `wt shell-init <bash\|zsh\|fish>` | the shell function `wt cd` needs |
| `wt ide <slug> [editor]` | opens the worktree in an editor |
| `wt open <slug> [target] [--list]` | opens an address in the browser (WSL included) |
| `wt run <task> <slug> [args…]` | runs a `wt.toml` task |
| `wt tasks` / `wt root` / `wt path <slug>` | introspection |
| `wt completions <shell>` | completion script (slugs, tasks and branches included) |

`wt` works from the main repository **and from inside a worktree**: the configuration is
always the main repository's, not that of the branch currently checked out.

### Interface shortcuts

`↑↓`/`jk` move · `ENTER` action menu · `n` create · `s` start · `S` start with options ·
`d` stop · `c` shell · `e` editor · `o` browser · `t` task · `r` remove · `g` refresh ·
`m` mouse · `?` help · `q` quit.

**Mouse** (on by default): click selects a row, double-click opens the action menu, the
wheel scrolls the list, a picker or the output panel. In a multiple-choice list a click
toggles. `m` releases mouse capture so the terminal gets its native text selection back,
long enough to copy a line.

The footer, the action menu and the help only show what the `wt.toml` declares: without
`[hooks] up`, no "start"; without `[open] url`, no "browser".

### Creating a worktree

`n` — or "create a worktree" in the action menu — asks three questions in a row: **which
branch** (an existing one, or `＋ new branch`); for a new one, **where it starts from** —
`dev`, `master`, a colleague's branch… — with the main repository's checked-out branch
preselected and marked `●`; then the worktree's **slug**. The `wt.toml` questions, if
any, come next.

On the command line that is `wt new <slug> [branch] --from <ref>`. `--from` takes
anything git takes as a start point (a local branch, `origin/dev`, a tag, a commit) and
only applies to a branch that **does not exist yet**: a branch already written has its
own history, and wt refuses rather than create something else than what was asked.

### Getting into a worktree

Most of what one does in a worktree is not a hook: a `claude`, a `git rebase -i`, a build
one wants to watch. Two ways in, depending on whether you mean to come back.

`wt shell <slug>` opens a shell at the worktree root and needs nothing installed. `exit`
returns where you were:

```bash
wt shell demo
claude              # in ../my-project-wt/demo
exit
```

`wt cd <slug>` moves the **current** shell instead — no nesting, no `exit`. A process
cannot change its parent's directory, so this one needs a shell function; `wt shell-init`
writes it:

```bash
eval "$(wt shell-init bash)"   # in ~/.bashrc — or `zsh` in ~/.zshrc
wt shell-init fish > ~/.config/fish/functions/wt.fish
```

The function only intercepts `wt cd`; every other command goes to the binary untouched.
Without it, `wt cd demo` still prints the path and says which line is missing.

In the interface it is `c`, or "shell in the worktree" in the action menu: the interface
steps aside and comes back when the session ends.

Both take **a fragment of a slug** — `wt cd auth` finds `fix-auth` — and both ask when
what was typed leaves several worktrees, or when nothing was typed at all:

```
$ wt cd fix
  1) fix-auth             wt/fix-auth
  2) hotfix               wt/hotfix
which worktree? [1-2, ENTER for 1]
```

wt never guesses between candidates: a wrong directory is discovered three commands
later. With no terminal to ask on — a script, a pipe — the command fails and names them.

A shell opened by `wt shell` (or by `c`) inherits the worktree's variables, the same ones
the hooks get: `$WT_SLUG`, `$WT_PATH`, `$WT_PORT_VITE`… Which shell it is comes from
`WT_TERMINAL`, then `[editor] terminal`, then `$SHELL`. `wt cd`, being your own shell,
exports nothing.

For a command run often enough to deserve a name, a task beats either — a three-line
`[tasks.claude]` with `interactive = true`, then `wt run claude demo`.

### Completion

`wt cd <TAB>` offers the project's worktrees, `wt run <TAB>` the `wt.toml`'s tasks, and
`wt new demo <TAB>` the repository's branches — with the branch, the task's description
or the commit subject shown alongside, where the shell displays it.

The script does not carry the list: it asks the binary on every TAB, which is the only
way for it to be right after a `wt new`. Install it either way:

```bash
wt completions zsh > ~/.zfunc/_wt         # a file, as before
echo 'source <(COMPLETE=bash wt)' >> ~/.bashrc   # or at shell startup
```

The second form is regenerated at every shell launch, so it can never be a version
behind the binary. The first is what a package manager installs — the Nix package does
exactly that, where the script and the binary come from the same build and cannot drift.

Outside a project — or when the `wt.toml` is broken — the completion offers nothing at
all rather than an error: a TAB is not the place to learn something is wrong.

### Searching a picker

Every picker — branches, tasks, editors, addresses, and the `wt.toml` questions —
**filters as you type**, the way `fzf` does: type `acme` and three hundred tenants
become three. The letters need not be adjacent (`fab` finds `feature/acme-billing`),
case is ignored, and **a space narrows** rather than searching: `acme prod` keeps only
what contains both. The search covers both displayed columns, label and detail.

Results are ranked by relevance — a whole word before scattered letters, the start of a
word before its middle — and the matched characters are highlighted. The counter at the
bottom right shows what is left.

Since typing feeds the search, moving around is done with the arrows or with `^N`/`^P`
(`^J`/`^K`), `TAB` ticks a box in a multiple choice, `^U` clears the search, and `ESC`
clears it first, then closes the picker.

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
type = "multi"                            # TAB toggles, ENTER confirms
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
