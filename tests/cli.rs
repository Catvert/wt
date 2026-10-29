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
    // WT_ROOT from the machine running the tests would move every worktree root.
    cmd.current_dir(dir)
        .env("WT_LANG", "en")
        .env_remove("WT_ROOT");
    cmd
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn help_exposes_tui_and_default_mode_rejects_a_pipe() {
    let project = project("branch = \"wt/{{slug}}\"\n");
    let help = wt(project.path()).arg("--help").output().unwrap();
    let help = stdout(&help);
    assert!(help.contains("tui"), "{help}");

    // `Command::output` gives the child pipes, not a TTY. The default dashboard must
    // fail clearly instead of entering raw mode or waiting for input forever.
    let out = wt(project.path()).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("needs a terminal"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_missing_subcommand_argument_opens_the_fuzzy_picker() {
    let project = project("branch = \"wt/{{slug}}\"\n");
    wt(project.path()).args(["new", "demo"]).output().unwrap();

    // A captured process has no TTY, so Skim cannot actually be driven here. Reaching
    // its explicit terminal error proves `open` accepted the missing slug instead of
    // letting Clap reject the command as an incomplete invocation.
    let out = wt(project.path()).arg("open").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("needs a terminal"), "{stderr}");
    assert!(!stderr.contains("required arguments"), "{stderr}");

    wt(project.path())
        .args(["rm", "demo", "-y"])
        .output()
        .unwrap();
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

/// A new branch does not have to start at whatever the main repository has checked out:
/// `--from` picks the base, which is what makes a worktree branched off `dev` possible
/// from a `main` checkout.
#[test]
fn a_new_branch_starts_where_it_is_told() {
    let project = project(BASIC);
    let dir = project.path();
    let root = dir
        .parent()
        .unwrap()
        .join(format!("{}-wt", dir.file_name().unwrap().to_string_lossy()));

    // A `dev` branch one commit ahead of `main`.
    git(dir, &["checkout", "-q", "-b", "dev"]);
    fs::write(dir.join("only-on-dev.txt"), "dev\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "dev commit"]);
    git(dir, &["checkout", "-q", "main"]);

    let out = wt(dir)
        .args(["new", "feat", "--from", "dev"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        root.join("feat").join("only-on-dev.txt").exists(),
        "the worktree did not start from dev"
    );

    // Without --from the base is still the main repository's HEAD.
    wt(dir).args(["new", "plain"]).output().unwrap();
    assert!(!root.join("plain").join("only-on-dev.txt").exists());

    // A start point cannot rewrite where an existing branch begins: better to say so
    // than to silently check out something else.
    let out = wt(dir)
        .args(["new", "again", "dev", "--from", "main"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("already exists"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for slug in ["feat", "plain"] {
        wt(dir).args(["rm", slug, "-y"]).output().unwrap();
    }
}

#[test]
fn a_new_branch_is_wired_to_origin() {
    let project = project(BASIC);
    let dir = project.path();
    let config = |key: &str| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "--get", key])
            .output()
            .expect("git");
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };

    // Without a remote there is nothing to wire to — no config must be invented.
    wt(dir).args(["new", "solo"]).output().unwrap();
    assert_eq!(config("branch.wt/solo.remote"), None);

    // With an origin, the fresh branch is ready for `git push` without `-u`.
    let remote = tempfile::tempdir().expect("tempdir");
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        dir,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    wt(dir).args(["new", "feat"]).output().unwrap();
    assert_eq!(config("branch.wt/feat.remote"), Some("origin".into()));
    assert_eq!(
        config("branch.wt/feat.merge"),
        Some("refs/heads/wt/feat".into())
    );

    for slug in ["solo", "feat"] {
        wt(dir).args(["rm", slug, "-y"]).output().unwrap();
    }
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
fn wt_root_moves_the_default_root_but_not_an_explicit_one() {
    const WT_ROOT: &str = "{{main}}/../worktrees/{{repo}}";
    let root_of = |project: &TempDir| {
        let out = wt(project.path())
            .args(["root", "--json"])
            .env("WT_ROOT", WT_ROOT)
            .output()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
        std::path::PathBuf::from(json["root"].as_str().expect("root"))
    };

    let default = project("");
    let parent = default.path().parent().unwrap();
    let expected = parent
        .join("worktrees")
        .join(default.path().file_name().unwrap());
    assert_eq!(root_of(&default), expected);

    // The worktree really goes there, and is found again to be removed.
    let new = wt(default.path())
        .args(["new", "demo"])
        .env("WT_ROOT", WT_ROOT)
        .output()
        .unwrap();
    assert!(
        new.status.success(),
        "{}",
        String::from_utf8_lossy(&new.stderr)
    );
    assert!(expected.join("demo").join("file.txt").exists());
    let rm = wt(default.path())
        .args(["rm", "demo", "-y"])
        .env("WT_ROOT", WT_ROOT)
        .output()
        .unwrap();
    assert!(
        rm.status.success(),
        "{}",
        String::from_utf8_lossy(&rm.stderr)
    );
    assert!(!expected.join("demo").exists());
    let _ = fs::remove_dir_all(&expected);
    // Shared with other tests running in parallel: only if empty.
    let _ = fs::remove_dir(parent.join("worktrees"));

    let explicit = project("root = \"{{main}}/../{{repo}}-mine\"\n");
    let repo = explicit.path().file_name().unwrap().to_string_lossy();
    assert_eq!(
        root_of(&explicit),
        explicit
            .path()
            .parent()
            .unwrap()
            .join(format!("{repo}-mine"))
    );
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

/// `wt cd` prints where to go — the shell function is what goes there. A fragment is
/// enough as long as it leaves one worktree; when it leaves several, a command with
/// nobody to ask must fail rather than guess.
#[test]
fn cd_prints_the_path_of_the_worktree_meant() {
    let project = project(BASIC);
    let dir = project.path();
    let root = dir
        .parent()
        .unwrap()
        .join(format!("{}-wt", dir.file_name().unwrap().to_string_lossy()));

    for slug in ["fix-auth", "hotfix"] {
        wt(dir).args(["new", slug]).output().unwrap();
    }

    let out = wt(dir).args(["cd", "fix-auth"]).output().unwrap();
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(
        Path::new(stdout(&out).trim()),
        root.join("fix-auth"),
        "the path is the only thing on stdout"
    );

    // A fragment, not the whole slug: `auth` only matches one of the two.
    let out = wt(dir).args(["cd", "auth"]).output().unwrap();
    assert_eq!(Path::new(stdout(&out).trim()), root.join("fix-auth"));

    // `fix` matches both, and a captured stdout means no terminal to ask on.
    let out = wt(dir).args(["cd", "fix"]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("fix-auth") && err.contains("hotfix"), "{err}");

    let out = wt(dir).args(["cd", "ghost"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("ghost"));

    for slug in ["fix-auth", "hotfix"] {
        wt(dir).args(["rm", slug, "-y"]).output().unwrap();
    }
}

/// The shell function is what makes `wt cd` change the caller's directory. It must be
/// available before any project exists — an rc file is read once, everywhere.
#[test]
fn shell_init_writes_a_function_outside_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    for (shell, needle) in [
        ("bash", "wt() {"),
        ("zsh", "wt() {"),
        ("fish", "function wt"),
    ] {
        let out = wt(dir.path()).args(["shell-init", shell]).output().unwrap();
        assert!(out.status.success(), "shell-init {shell} failed");
        let body = stdout(&out);
        assert!(body.contains(needle), "{shell}: {body}");
        // It must call the binary, not itself.
        assert!(body.contains("command wt cd"), "{shell}: {body}");

        // Hand-written shell: parse it with the shell itself, when the machine has one.
        // A typo here breaks a login shell, which is worse than a broken command.
        let file = dir.path().join(format!("init.{shell}"));
        fs::write(&file, &body).unwrap();
        let syntax_only = if shell == "fish" {
            "--no-execute"
        } else {
            "-n"
        };
        match Command::new(shell).arg(syntax_only).arg(&file).output() {
            Ok(out) => assert!(
                out.status.success(),
                "{shell} rejects its own snippet: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
            // Not installed here: nothing to check against, and nothing to fail over.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("running {shell}: {e}"),
        }
    }
}

#[test]
fn completions_work_outside_a_repository() {
    // The Nix build generates them far from any git repository.
    let dir = tempfile::tempdir().unwrap();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = wt(dir.path())
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(out.status.success(), "completions {shell} failed");
        let script = stdout(&out);
        // The script hooks the shell up to `wt` itself — that is what makes the
        // candidates the project's own, rather than a list frozen at install time.
        assert!(script.contains("COMPLETE"), "{shell}: {script}");
        assert!(script.contains("wt"), "{shell}: {script}");
    }
}

/// What the shell asks for on TAB: slugs from the worktree root, tasks from the
/// `wt.toml`. A frozen script could not know either.
#[test]
fn completion_candidates_come_from_the_project() {
    let project = project(BASIC);
    let dir = project.path();
    for slug in ["alpha", "beta"] {
        wt(dir).args(["new", slug]).output().unwrap();
    }

    // How the generated fish snippet calls back in: the words so far, then the word
    // being completed.
    let complete = |words: &[&str]| -> String {
        let out = wt(dir)
            .env("COMPLETE", "fish")
            .arg("--")
            .args(words)
            .output()
            .unwrap();
        assert!(out.status.success(), "completing {words:?} failed");
        stdout(&out)
    };

    let slugs = complete(&["wt", "cd", ""]);
    assert!(slugs.contains("alpha"), "{slugs}");
    assert!(slugs.contains("beta"), "{slugs}");
    // Prefix filtering is the shell's convention, and the engine's.
    let filtered = complete(&["wt", "shell", "al"]);
    assert!(
        filtered.contains("alpha") && !filtered.contains("beta"),
        "{filtered}"
    );

    let tasks = complete(&["wt", "run", ""]);
    assert!(tasks.contains("hello"), "{tasks}");

    // Nothing to offer outside a project, and above all no error: a TAB is not the
    // place to learn that.
    let elsewhere = tempfile::tempdir().unwrap();
    let out = wt(elsewhere.path())
        .env("COMPLETE", "fish")
        .args(["--", "wt", "cd", ""])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!stdout(&out).contains("alpha"), "{}", stdout(&out));

    for slug in ["alpha", "beta"] {
        wt(dir).args(["rm", slug, "-y"]).output().unwrap();
    }
}

#[test]
fn json_output_is_stable_and_untranslated() {
    let project = project(
        r#"
branch = "wt/{{slug}}"

[ports.dev]
base = 4900
allocate = "new"

[open]
url = "http://localhost:{{port.dev}}"
label = "app"
source = "printf 'http://admin.localhost\tadmin\nhttp://mail.localhost\tmails\tmail\n'"

[status]
up = "test -f .up"

[status.info]
greeting = "echo hi"

[tasks.hello]
description = "says hello"
run = "echo hello"

[[prompt]]
name = "services"
question = "services"
type = "multi"
default = "queue"
options = [{ value = "queue", label = "Horizon" }, { value = "reverb" }]

[[prompt]]
name = "tenants"
ask = "both"
type = "multi"
source = "echo demo"
"#,
    );
    let dir = project.path();
    wt(dir)
        .args(["new", "demo", "--set", "tenants=demo"])
        .output()
        .unwrap();
    let json = |args: &[&str]| -> serde_json::Value {
        // French on purpose: the JSON must not follow the locale.
        let out = wt(dir).env("WT_LANG", "fr").args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("valid JSON")
    };

    let ls = json(&["ls", "--json"]);
    let demo = &ls[0];
    assert_eq!(demo["slug"], "demo");
    assert_eq!(demo["branch"], "wt/demo");
    assert_eq!(demo["up"], false);
    assert_eq!(demo["adopted"], false);
    assert_eq!(demo["ports"]["dev"], 4900);
    assert_eq!(demo["url"], "http://localhost:4900");
    assert_eq!(demo["checkout"], demo["dir"]);

    let show = json(&["show", "demo", "--json"]);
    assert_eq!(show["slug"], "demo");
    assert_eq!(show["info"]["greeting"], "hi");
    assert_eq!(show["links"][0]["label"], "app");
    assert_eq!(show["links"][1]["url"], "http://admin.localhost");
    assert_eq!(show["links"][1]["icon"], serde_json::Value::Null);
    assert_eq!(show["links"][2]["label"], "mails");
    assert_eq!(show["links"][2]["icon"], "mail");
    assert_eq!(show["changes"].as_array().unwrap().len(), 0);

    let links = json(&["open", "demo", "--list", "--json"]);
    assert_eq!(links, show["links"]);

    let root = json(&["root", "--json"]);
    assert_eq!(root["has_ports"], true);
    assert_eq!(root["has_up"], false);
    assert_eq!(root["tasks"][0]["name"], "hello");
    assert_eq!(json(&["tasks", "--json"]), root["tasks"]);
    let services = &root["prompts"][0];
    assert_eq!(services["name"], "services");
    assert_eq!(services["type"], "multi");
    assert_eq!(services["ask"], "up");
    assert_eq!(services["default"], "queue");
    assert_eq!(services["separator"], ",");
    assert_eq!(services["has_source"], false);
    assert_eq!(services["options"][0]["label"], "Horizon");
    assert_eq!(services["options"][1]["label"], "");
    assert_eq!(root["prompts"][1]["ask"], "both");
    assert_eq!(root["prompts"][1]["has_source"], true);
    assert_eq!(root["prompts"][1]["question"], "tenants");

    // `--json` only makes sense with `--list`: opening a browser has no output to format.
    assert!(!wt(dir)
        .args(["open", "demo", "--json"])
        .output()
        .unwrap()
        .status
        .success());

    wt(dir).args(["rm", "demo", "-y"]).output().unwrap();
}

#[test]
fn automatic_stop_deadline() {
    let project = project(
        r#"
[hooks]
up = ["touch .up"]
down = ["rm -f .up"]

[status]
up = "test -f .up"
"#,
    );
    let dir = project.path();
    let run = |args: &[&str]| {
        let out = wt(dir)
            .env_remove("WT_STOP_AFTER")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    let stop_at = || -> serde_json::Value {
        serde_json::from_slice::<serde_json::Value>(&run(&["ls", "--json"]).stdout).unwrap()[0]
            ["stop_at"]
            .clone()
    };
    run(&["new", "demo"]);

    // No duration given and no WT_STOP_AFTER: no deadline.
    run(&["up", "demo"]);
    assert_eq!(stop_at(), serde_json::Value::Null);

    run(&["up", "demo", "--stop-after", "2h"]);
    let at = stop_at().as_u64().expect("a deadline");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(at > now + 7000 && at <= now + 7200, "{at} vs {now}");

    // Not due: --expired leaves it alone.
    run(&["down", "--expired"]);
    assert_eq!(stop_at().as_u64(), Some(at));

    run(&["keep", "demo", "never"]);
    assert_eq!(stop_at(), serde_json::Value::Null);

    // WT_STOP_AFTER is the default; due after a second.
    let out = wt(dir)
        .env("WT_STOP_AFTER", "1s")
        .args(["up", "demo"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(stop_at().is_u64());
    std::thread::sleep(std::time::Duration::from_millis(1500));
    run(&["down", "--expired"]);
    assert_eq!(stop_at(), serde_json::Value::Null);
    let ls: serde_json::Value = serde_json::from_slice(&run(&["ls", "--json"]).stdout).unwrap();
    assert_eq!(ls[0]["up"], false);

    let bad = wt(dir).args(["keep", "demo", "2x"]).output().unwrap();
    assert!(!bad.status.success());
}
