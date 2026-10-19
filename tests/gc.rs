//! `wt gc` end to end, on a throwaway repository whose hooks only write to a log file.
//! No database, no container: the hooks say what they would have dropped.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::prelude::*;
use tempfile::TempDir;

struct Fixture {
    main: TempDir,
    root: TempDir,
    log: PathBuf,
    _logdir: TempDir,
}

impl Fixture {
    fn main(&self) -> &Path {
        self.main.path()
    }
    fn dir(&self, slug: &str) -> PathBuf {
        self.root.path().join(slug)
    }
    fn state(&self, slug: &str) -> PathBuf {
        self.root.path().join(".wt").join(format!("{slug}.toml"))
    }
    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
    fn wt(&self, args: &[&str]) -> Output {
        Command::cargo_bin("wt")
            .unwrap()
            .current_dir(self.main())
            .env("WT_LANG", "en")
            .args(args)
            .output()
            .unwrap()
    }
    fn wt_ok(&self, args: &[&str]) -> String {
        let out = self.wt(args);
        assert!(
            out.status.success(),
            "wt {args:?}: {}{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        format!("{}{}", text(&out.stdout), text(&out.stderr))
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(self.main())
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }
    fn git_worktrees(&self) -> String {
        self.git(&["worktree", "list", "--porcelain"])
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Removal hooks that log their slug, options, `$WT_GC` and working directory, and drop a
/// relative marker file: if `gc` ran them in the main checkout, the marker would show it.
fn fixture_with(pre_rm: &str) -> Fixture {
    let main = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let logdir = tempfile::tempdir().unwrap();
    let log = logdir.path().join("hooks.log");
    let config = format!(
        r#"root = "{root}"
branch = "wt/{{{{slug}}}}"

[hooks]
post_new = ["echo \"new {{{{slug}}}} db={{{{opt.db}}}}\" >> {log}"]
pre_rm = [{pre_rm}]
post_rm = ["echo \"post_rm {{{{slug}}}} db={{{{opt.db}}}} gc=$WT_GC\" >> {log}"]
"#,
        root = root.path().display(),
        log = log.display(),
    );
    let p = main.path();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "wt@example.com"],
        &["config", "user.name", "wt"],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    fs::write(p.join("wt.toml"), config).unwrap();
    fs::write(p.join("file.txt"), "hello\n").unwrap();
    let f = Fixture {
        main,
        root,
        log,
        _logdir: logdir,
    };
    f.git(&["add", "-A"]);
    f.git(&["commit", "-qm", "init"]);
    f
}

fn fixture() -> Fixture {
    fixture_with(
        r#""echo \"pre_rm {{slug}} db={{opt.db}} gc=$WT_GC cwd=$(pwd)\" >> LOG", "touch relative-marker""#,
    )
    .with_log_placeholder()
}

impl Fixture {
    /// `fixture_with` cannot know the log path before creating it: swap the placeholder.
    fn with_log_placeholder(self) -> Self {
        let path = self.main().join("wt.toml");
        let body = fs::read_to_string(&path)
            .unwrap()
            .replace(">> LOG", &format!(">> {}", self.log.display()));
        fs::write(&path, body).unwrap();
        self.git(&["commit", "-qam", "log path"]);
        self
    }
}

#[test]
fn new_records_the_state_format() {
    let f = fixture();
    f.wt_ok(&["new", "demo", "--set", "db=isolated"]);
    let state = fs::read_to_string(f.state("demo")).unwrap();
    assert!(state.contains("format = 1"), "{state}");
    assert!(state.contains("db = \"isolated\""), "{state}");
}

#[test]
fn an_orphan_is_cleaned_and_a_live_worktree_left_alone() {
    let f = fixture();
    f.wt_ok(&["new", "gone", "--set", "db=isolated"]);
    f.wt_ok(&["new", "alive", "--set", "db=shared"]);
    // Removed by hand: git keeps a prunable record, wt keeps its state.
    fs::remove_dir_all(f.dir("gone")).unwrap();

    // Dry run: says everything, changes nothing.
    let before_log = f.log();
    let out = f.wt_ok(&["gc"]);
    assert!(out.contains("orphan gone"), "{out}");
    assert!(out.contains("db=isolated"), "{out}");
    assert!(out.contains("pre_rm · echo"), "{out}");
    assert!(out.contains("--apply"), "{out}");
    assert!(!out.contains("alive"), "a live worktree was listed: {out}");
    assert_eq!(f.log(), before_log, "the dry run ran a hook");
    assert!(f.state("gone").exists());
    assert!(f.git_worktrees().contains("/gone"));

    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("gone cleaned up"), "{out}");
    let log = f.log();
    assert!(log.contains("pre_rm gone db=isolated gc=1"), "{log}");
    assert!(log.contains("post_rm gone db=isolated gc=1"), "{log}");
    assert!(!log.contains("pre_rm alive"), "{log}");
    assert!(!log.contains("post_rm alive"), "{log}");
    // pre_rm ran somewhere harmless, not in the main checkout.
    assert!(!f.main().join("relative-marker").exists());
    assert!(
        !log.contains(&format!("cwd={}\n", f.main().display())),
        "{log}"
    );

    assert!(!f.state("gone").exists(), "the state was not forgotten");
    assert!(!f.git_worktrees().contains("/gone"), "git still lists it");
    // The branch survives, as with `wt rm`.
    assert!(f.git(&["branch", "--list", "wt/gone"]).contains("wt/gone"));
    // The live one is untouched.
    assert!(f.dir("alive").join("file.txt").exists());
    assert!(f.state("alive").exists());

    let out = f.wt_ok(&["gc"]);
    assert!(out.contains("nothing to clean"), "{out}");
}

#[test]
fn a_worktree_removed_through_git_is_still_cleaned() {
    let f = fixture();
    f.wt_ok(&["new", "viagit", "--set", "db=isolated"]);
    f.git(&[
        "worktree",
        "remove",
        "--force",
        f.dir("viagit").to_str().unwrap(),
    ]);
    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("viagit cleaned up"), "{out}");
    assert!(f.log().contains("pre_rm viagit db=isolated gc=1"));
    assert!(!f.state("viagit").exists());
}

#[test]
fn an_old_state_without_options_is_reported_not_guessed() {
    let f = fixture();
    f.wt_ok(&["new", "old"]);
    f.wt_ok(&["new", "oldopts", "--set", "db=isolated"]);
    // As written by wt 0.8.0: no `format` key.
    fs::write(f.state("old"), "branch = \"wt/old\"\n").unwrap();
    fs::write(
        f.state("oldopts"),
        "branch = \"wt/oldopts\"\n\n[opts]\ndb = \"isolated\"\n",
    )
    .unwrap();
    fs::remove_dir_all(f.dir("old")).unwrap();
    fs::remove_dir_all(f.dir("oldopts")).unwrap();

    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("skipped old"), "{out}");
    assert!(out.contains("0.8.0"), "{out}");
    assert!(out.contains("--forget old"), "{out}");
    let log = f.log();
    assert!(!log.contains("pre_rm old "), "hooks ran on a guess: {log}");
    assert!(f.state("old").exists());
    // Old, but with its options recorded: that is exactly what `wt rm` would use.
    assert!(log.contains("pre_rm oldopts db=isolated gc=1"), "{log}");
    assert!(!f.state("oldopts").exists());

    // --forget: dry run first, then without any hook.
    let out = f.wt_ok(&["gc", "--forget", "old"]);
    assert!(out.contains("would forget old"), "{out}");
    assert!(f.state("old").exists());
    let out = f.wt_ok(&["gc", "--apply", "--forget", "old"]);
    assert!(out.contains("old forgotten"), "{out}");
    assert!(!f.state("old").exists());
    assert!(!f.log().contains("pre_rm old "));
}

#[test]
fn a_new_state_without_options_is_trusted() {
    let f = fixture();
    f.wt_ok(&["new", "plain"]);
    fs::remove_dir_all(f.dir("plain")).unwrap();
    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("plain cleaned up"), "{out}");
    assert!(f.log().contains("pre_rm plain db= gc=1"));
}

#[test]
fn a_directory_still_present_is_never_cleaned() {
    let f = fixture();
    f.wt_ok(&["new", "debris", "--set", "db=isolated"]);
    // git forgets it, but files remain at the path (ignored files, a copy…).
    f.git(&[
        "worktree",
        "remove",
        "--force",
        f.dir("debris").to_str().unwrap(),
    ]);
    fs::create_dir_all(f.dir("debris")).unwrap();
    fs::write(f.dir("debris").join("leftover"), "x").unwrap();

    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("skipped debris"), "{out}");
    assert!(out.contains("does not register it"), "{out}");
    assert!(!f.log().contains("pre_rm debris"));
    assert!(f.state("debris").exists());

    // Nor can it be forgotten.
    let out = f.wt(&["gc", "--apply", "--forget", "debris"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("exists"));
    assert!(f.state("debris").exists());
}

#[test]
fn a_moved_or_locked_worktree_is_left_alone() {
    let f = fixture();
    f.wt_ok(&["new", "moved", "--set", "db=isolated"]);
    f.wt_ok(&["new", "locked", "--set", "db=isolated"]);
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("moved-here");
    f.git(&[
        "worktree",
        "move",
        f.dir("moved").to_str().unwrap(),
        target.to_str().unwrap(),
    ]);
    f.git(&["worktree", "lock", f.dir("locked").to_str().unwrap()]);
    fs::remove_dir_all(f.dir("locked")).unwrap();

    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("skipped moved"), "{out}");
    assert!(out.contains("moved-here"), "{out}");
    assert!(out.contains("skipped locked"), "{out}");
    let log = f.log();
    assert!(!log.contains("pre_rm"), "{log}");
    assert!(f.state("moved").exists());
    assert!(f.state("locked").exists());
}

#[test]
fn a_git_leftover_without_state_only_loses_its_git_record() {
    let f = fixture();
    let path = f.dir("byhand");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "byhand",
        path.to_str().unwrap(),
    ]);
    fs::remove_dir_all(&path).unwrap();
    let out = f.wt_ok(&["gc"]);
    assert!(out.contains("git leftover byhand"), "{out}");
    assert!(f.git_worktrees().contains("/byhand"));
    f.wt_ok(&["gc", "--apply"]);
    assert!(!f.git_worktrees().contains("/byhand"));
    assert!(!f.log().contains("pre_rm"));
}

#[test]
fn an_unreadable_state_runs_nothing() {
    let f = fixture();
    f.wt_ok(&["new", "broken", "--set", "db=isolated"]);
    fs::write(f.state("broken"), "branch = [not toml").unwrap();
    fs::remove_dir_all(f.dir("broken")).unwrap();
    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("skipped broken"), "{out}");
    assert!(out.contains("unreadable"), "{out}");
    assert!(!f.log().contains("pre_rm"));
    assert!(f.state("broken").exists());
}

#[test]
fn a_failing_hook_keeps_the_state_for_a_retry() {
    let f = fixture_with(r#""test -e FLAG""#);
    let flag = f.log.with_file_name("flag");
    let path = f.main().join("wt.toml");
    let body = fs::read_to_string(&path)
        .unwrap()
        .replace("test -e FLAG", &format!("test -e {}", flag.display()));
    fs::write(&path, body).unwrap();
    f.git(&["commit", "-qam", "flag"]);

    f.wt_ok(&["new", "flaky", "--set", "db=isolated"]);
    f.wt_ok(&["new", "other", "--set", "db=isolated"]);
    fs::remove_dir_all(f.dir("flaky")).unwrap();
    fs::remove_dir_all(f.dir("other")).unwrap();

    let out = f.wt(&["gc", "--apply"]);
    assert!(
        !out.status.success(),
        "a failed cleanup must fail the command"
    );
    let err = text(&out.stderr);
    assert!(err.contains("flaky: cleanup failed"), "{err}");
    assert!(
        err.contains("other: cleanup failed"),
        "both entries are attempted: {err}"
    );
    assert!(f.state("flaky").exists());
    assert!(!f.log().contains("post_rm flaky"));

    // The cause is fixed: the next run retries and succeeds.
    fs::write(&flag, "").unwrap();
    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("flaky cleaned up"), "{out}");
    assert!(!f.state("flaky").exists());
    assert!(f.log().contains("post_rm flaky db=isolated gc=1"));
}

#[test]
fn rm_does_not_set_the_gc_variable() {
    let f = fixture();
    f.wt_ok(&["new", "normal", "--set", "db=x"]);
    f.wt_ok(&["rm", "normal", "-y"]);
    let log = f.log();
    assert!(log.contains("pre_rm normal db=x gc= "), "{log}");
}

// --------------------------------------------------------------------------------------
// Checkouts adopted with `wt new --adopt`: created by another tool, outside the root.
// --------------------------------------------------------------------------------------

impl Fixture {
    /// A git worktree created the way another tool would: outside the root, on its own
    /// branch, without `wt` knowing.
    fn foreign_worktree(&self, outside: &TempDir, name: &str) -> PathBuf {
        let path = outside.path().join(name);
        self.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("tool/{name}"),
            path.to_str().unwrap(),
        ]);
        fs::canonicalize(path).unwrap()
    }
}

#[test]
fn an_adopted_checkout_is_linked_provisioned_and_removed_by_rm() {
    let f = fixture();
    let outside = tempfile::tempdir().unwrap();
    let ext = f.foreign_worktree(&outside, "agent-1");

    let out = f.wt_ok(&[
        "new",
        "agent",
        "--adopt",
        ext.to_str().unwrap(),
        "--set",
        "db=shared",
    ]);
    assert!(out.contains("adopted"), "{out}");
    assert_eq!(fs::read_link(f.dir("agent")).unwrap(), ext);
    assert!(f.log().contains("new agent db=shared"), "{}", f.log());
    let state = fs::read_to_string(f.state("agent")).unwrap();
    assert!(state.contains("branch = \"tool/agent-1\""), "{state}");
    assert!(
        state.contains(&format!("path = \"{}\"", ext.display())),
        "{state}"
    );
    assert!(f.wt_ok(&["ls"]).contains("agent"));

    // Alive: gc has nothing to say about it.
    let out = f.wt_ok(&["gc"]);
    assert!(out.contains("nothing to clean"), "{out}");

    f.wt_ok(&["rm", "agent", "--yes"]);
    assert!(
        f.log().contains("pre_rm agent db=shared gc="),
        "{}",
        f.log()
    );
    assert!(!ext.exists(), "the adopted checkout survived rm");
    assert!(
        f.dir("agent").symlink_metadata().is_err(),
        "the link survived rm"
    );
    assert!(!f.state("agent").exists());
    assert!(!f.git_worktrees().contains("agent-1"));
}

#[test]
fn adopt_refuses_what_is_not_a_free_worktree_of_the_repository() {
    let f = fixture();
    let outside = tempfile::tempdir().unwrap();
    let plain = outside.path().join("plain");
    fs::create_dir_all(&plain).unwrap();

    let out = f.wt(&["new", "plain", "--adopt", plain.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("not a git worktree"),
        "{}",
        text(&out.stderr)
    );
    assert!(f.dir("plain").symlink_metadata().is_err());

    let out = f.wt(&["new", "main", "--adopt", f.main().to_str().unwrap()]);
    assert!(!out.status.success());

    let ext = f.foreign_worktree(&outside, "twice");
    f.wt_ok(&["new", "first", "--adopt", ext.to_str().unwrap()]);
    let out = f.wt(&["new", "second", "--adopt", ext.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("already adopted as 'first'"),
        "{}",
        text(&out.stderr)
    );
    assert!(f.dir("second").symlink_metadata().is_err());
    assert!(!f.state("second").exists());

    let out = f.wt(&[
        "new",
        "both",
        "--adopt",
        ext.to_str().unwrap(),
        "--from",
        "main",
    ]);
    assert!(!out.status.success(), "--adopt and --from are exclusive");
}

#[test]
fn an_adopted_checkout_deleted_by_its_tool_is_cleaned_with_its_link() {
    let f = fixture();
    let outside = tempfile::tempdir().unwrap();
    let ext = f.foreign_worktree(&outside, "agent-2");
    f.wt_ok(&[
        "new",
        "deleted",
        "--adopt",
        ext.to_str().unwrap(),
        "--set",
        "db=isolated",
    ]);
    // What the other tool does: its own `git worktree remove`, the link left dangling.
    f.git(&["worktree", "remove", "--force", ext.to_str().unwrap()]);
    assert!(f.dir("deleted").symlink_metadata().is_ok());

    let out = f.wt_ok(&["gc"]);
    assert!(out.contains("orphan deleted"), "{out}");
    assert!(out.contains("link · remove"), "{out}");

    let out = f.wt_ok(&["gc", "--apply"]);
    assert!(out.contains("deleted cleaned up"), "{out}");
    assert!(
        f.log().contains("pre_rm deleted db=isolated gc=1"),
        "{}",
        f.log()
    );
    assert!(
        f.dir("deleted").symlink_metadata().is_err(),
        "the dangling link survived"
    );
    assert!(!f.state("deleted").exists());
}

#[test]
fn gc_can_be_limited_to_some_slugs() {
    let f = fixture();
    f.wt_ok(&["new", "one", "--set", "db=isolated"]);
    f.wt_ok(&["new", "two", "--set", "db=isolated"]);
    fs::remove_dir_all(f.dir("one")).unwrap();
    fs::remove_dir_all(f.dir("two")).unwrap();

    let out = f.wt_ok(&["gc", "--apply", "one"]);
    assert!(out.contains("one cleaned up"), "{out}");
    assert!(!out.contains("two"), "{out}");
    assert!(!f.state("one").exists());
    assert!(f.state("two").exists());
}
