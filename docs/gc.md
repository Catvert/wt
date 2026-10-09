# `wt gc` — cleaning up after worktrees removed without `wt rm`

A worktree removed with `git worktree remove`, or whose directory was deleted by hand,
leaves behind what its hooks created: databases, containers, proxy routes… and the
state file `wt` keeps for it. `wt gc` finds those leftovers and runs the cleanup that
`wt rm` would have run.

```
wt gc                      # dry run: lists what it would do, changes nothing
wt gc --apply              # does it
wt gc --forget <slug>      # dry run of forgetting an entry without running its hooks
wt gc --apply --forget <slug>
```

## What is examined

- the state files `<root>/.wt/<slug>.toml` — every worktree created by `wt new` has one;
- the git worktrees registered under `<root>` (`git worktree list --porcelain`).

## Classification

Each slug falls into exactly one class. Only the first two are ever acted upon.

| Class | Condition | `--apply` |
|---|---|---|
| **orphan** | state file present, `<root>/<slug>` absent (no directory, file or link), state readable and trusted (see below), branch not checked out in any live worktree, git entry (if any) not locked | removal hooks, git metadata, state forgotten |
| **git leftover** | git still registers `<root>/<slug>`, the directory is gone, no state file | git metadata only (`git worktree remove <path>`); no hook: `wt` never recorded options for it |
| alive | `<root>/<slug>` exists and git registers it | nothing — never listed |
| not a worktree | `<root>/<slug>` exists but git does not register it (debris left by a `git worktree remove` that kept ignored files, a hand-made copy…) | nothing: remove the directory yourself, then run `wt gc` again |
| locked | git marks the worktree `locked` (removable media, network share) | nothing: `git worktree unlock` it, or reconnect the disk |
| moved | the state's branch is checked out in a live worktree elsewhere (`git worktree move`, or a re-created checkout) | nothing: its resources may still be in use |
| unreadable | the state file cannot be parsed | nothing: fix or remove the file, or `--forget` |
| old state | state written by `wt` ≤ 0.8.0 (no `format` key), **no recorded options**, and the project declares removal hooks | nothing: see below |

A resource whose worktree still exists is never touched: the only paths that run hooks
are the ones where `<root>/<slug>` does not exist at all.

### Old states

`wt` ≤ 0.8.0 already recorded the `--set` options (and the interface's answers) in the
state, so an old state *with* options is used as is — exactly what `wt rm` would do. An
old state *without* options is ambiguous: created without any option, or by a version
that did not keep them? Removal hooks often depend on options (`{{opt.db}}`), and
running them with a guessed value could delete the wrong thing, so `wt gc` refuses and
says so. Clean up by hand, then `wt gc --apply --forget <slug>`.

From this version on, `wt new` writes `format = 1` in the state: an empty option set is
then known to be really empty. Other commands keep the `format` they find — a `wt up`
does not turn an old state into a trusted one. Older `wt` binaries ignore the key, and
drop it if they rewrite the file, which only makes the state stricter.

## What `--apply` does for an orphan

In this order, stopping at the first failure:

1. `pre_rm` hooks, with the recorded options, ports and branch — the same variables as
   `wt rm`. **Working directory: a fresh, empty temporary directory**, since the worktree
   no longer exists: a hook using relative paths finds nothing instead of acting on the
   main checkout. `{{path}}` / `$WT_PATH` still name the former location.
2. git metadata: `git worktree remove <path>` if git still registers it (no `--force`:
   git refuses if the directory came back).
3. `post_rm` hooks, working directory = main repository, as with `wt rm`.
4. the state file is forgotten — last, so that a failed cleanup can be retried.

During `wt gc`, hooks also receive `$WT_GC=1` (`{{gc}}` = `1`): a hook can skip what only
makes sense with a checkout (`test -n "$WT_GC" || docker compose … down`).

On failure the entry keeps its state file, the next entries are still processed, and
`wt gc` exits with status 1. A later `wt gc --apply` retries the whole sequence: removal
hooks should be idempotent (dropping a database that is already gone must succeed).

The branch is kept, as with `wt rm`.

## `--forget <slug>`

Forgets an entry's state without running any hook — for a cleanup done by hand, or an
entry `wt gc` refuses (old state, unreadable). Refused when `<root>/<slug>` exists or
when `wt` has no state for it. Repeatable; when given, only the named entries are
handled. Like everything else, it only acts with `--apply`.

## Exit status

- dry run: 0, whatever it found;
- `--apply`: 0 if every action succeeded, 1 otherwise.
