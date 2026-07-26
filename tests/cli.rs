//! End-to-end tests of the command line, on a throwaway git repository.
//!
//! The interactive interface is not covered here: driving it needs a pty, which belongs
//! to manual testing. Everything else — the lifecycle, the hooks, the state, the
//! locale — goes through the real binary.

use std::fs;
use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use tempfile::TempDir;

/// A git repository with a `wt.toml`, ready for `wt`.
fn project(config: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    // Commits need an identity, which CI machines do not have globally.
    git(path, &["config", "user.email", "wt@example.com"]);
    git(path, &["config", "user.name", "wt"]);
    fs::write(path.join("file.txt"), "hello\n").unwrap();
    fs::write(path.join("wt.toml"), config).unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

/// Runs `wt` in English unless told otherwise, so assertions do not depend on the
/// machine's locale.
fn wt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("wt").expect("binary");
    cmd.current_dir(dir).env("WT_LANG", "en");
    cmd
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const BASIC: &str = r#"
branch = "wt/{{slug}}"

[hooks]
post_new = ["echo created-{{slug}} > marker"]
up = ["echo up-{{opt.mode}} > .up"]
down = ["rm -f .up"]

[status]
up = "test -f .up"

[tasks.hello]
description = "says hello"
run = "echo hello-{{slug}}-{{args}}"
"#;

#[test]
fn full_lifecycle() {
    let project = project(BASIC);
    let dir = project.path();

    let out = wt(dir).args(["new", "demo"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout(&out).contains("created"));

    // The worktree lives next to the repository, never inside it.
    let worktree = dir.parent().unwrap().join(format!(
        "{}-wt/demo",
        dir.file_name().unwrap().to_string_lossy()
    ));
    assert!(worktree.join("file.txt").exists(), "checkout is missing");
    assert_eq!(
        fs::read_to_string(worktree.join("marker")).unwrap().trim(),
        "created-demo",
        "post_new hook did not run"
    );

    let out = wt(dir).args(["ls"]).output().unwrap();
    assert!(stdout(&out).contains("demo"));
    assert!(stdout(&out).contains("stopped"));

    // An option reaches the hooks, and is remembered for the next start.
    wt(dir)
        .args(["up", "demo", "--set", "mode=fast"])
        .output()
        .unwrap();
    assert_eq!(
        fs::read_to_string(worktree.join(".up")).unwrap().trim(),
        "up-fast"
    );
    assert!(stdout(&wt(dir).args(["ls"]).output().unwrap()).contains("started"));

    wt(dir).args(["down", "demo"]).output().unwrap();
    let out = wt(dir).args(["up", "demo"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(worktree.join(".up")).unwrap().trim(),
        "up-fast",
        "the option was not remembered"
    );

    let out = wt(dir)
        .args(["run", "hello", "demo", "x"])
        .output()
        .unwrap();
    assert!(stdout(&out).contains("hello-demo-x"));

    let out = wt(dir).args(["rm", "demo", "-y"]).output().unwrap();
    assert!(out.status.success());
    assert!(!worktree.exists(), "the worktree was left behind");
    // Removing a worktree must never delete unmerged work.
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["branch", "--list", "wt/demo"])
        .output()
        .unwrap();
    assert!(stdout(&out).contains("wt/demo"), "the branch was deleted");
}

#[test]
fn init_writes_a_usable_config() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);

    assert!(wt(dir.path())
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    assert!(dir.path().join("wt.toml").exists());
    // Writing over an existing config needs to be explicit.
    assert!(!wt(dir.path())
        .arg("init")
        .output()
        .unwrap()
        .status
        .success());
    assert!(wt(dir.path())
        .args(["init", "--force", "--preset", "web"])
        .output()
        .unwrap()
        .status
        .success());
    // The generated file must be readable back by wt itself.
    assert!(wt(dir.path()).arg("ls").output().unwrap().status.success());
}

#[test]
fn an_empty_config_is_enough() {
    let project = project("");
    assert!(wt(project.path())
        .args(["new", "bare"])
        .output()
        .unwrap()
        .status
        .success());
    let out = wt(project.path()).args(["ls"]).output().unwrap();
    // No [status] and no [ports]: neither column is shown.
    assert!(!stdout(&out).contains("STATE"));
    assert!(!stdout(&out).contains("PORTS"));
    wt(project.path())
        .args(["rm", "bare", "-y"])
        .output()
        .unwrap();
}

#[test]
fn ports_are_stable_and_unique() {
    let project = project(
        r#"
branch = "wt/{{slug}}"

[ports.web]
base = 45210

[hooks]
up = ["echo {{port.web}} > .port"]
"#,
    );
    let dir = project.path();
    let root = dir
        .parent()
        .unwrap()
        .join(format!("{}-wt", dir.file_name().unwrap().to_string_lossy()));

    for slug in ["one", "two"] {
        wt(dir).args(["new", slug]).output().unwrap();
        wt(dir).args(["up", slug]).output().unwrap();
    }
    let read = |slug: &str| fs::read_to_string(root.join(slug).join(".port")).unwrap();
    let (one, two) = (read("one"), read("two"));
    assert_ne!(one.trim(), two.trim(), "two worktrees share a port");

    // A restart must not move the port: bookmarks and IDE configs depend on it.
    wt(dir).args(["up", "one"]).output().unwrap();
    assert_eq!(read("one").trim(), one.trim());

    for slug in ["one", "two"] {
        wt(dir).args(["rm", slug, "-y"]).output().unwrap();
    }
}

#[test]
fn messages_follow_the_locale() {
    let project = project(BASIC);
    let dir = project.path();

    let english = wt(dir).args(["new", "loc"]).output().unwrap();
    assert!(stdout(&english).contains("created"));

    let french = Command::cargo_bin("wt")
        .unwrap()
        .current_dir(dir)
        .env("WT_LANG", "fr")
        .args(["rm", "loc", "-y"])
        .output()
        .unwrap();
    assert!(stdout(&french).contains("supprimé"), "{}", stdout(&french));
}

#[test]
fn unknown_things_fail_with_a_clear_message() {
    let project = project(BASIC);
    let dir = project.path();

    let out = wt(dir).args(["up", "ghost"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ghost"));

    let out = wt(dir).args(["new", "Bad_Slug"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("slug"));
}

#[test]
fn completions_work_outside_a_repository() {
    // The Nix build generates them far from any git repository.
    let dir = tempfile::tempdir().unwrap();
    let out = wt(dir.path())
        .args(["completions", "bash"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(stdout(&out).contains("_wt"));
}
