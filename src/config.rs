//! Reading and validating a project's `wt.toml`.
//!
//! The file describes *what the project wants* (where worktrees live, which files to
//! copy, which ports to allocate, which commands to run). The binary knows nothing about
//! Docker, Laravel or npm: anything stack-specific goes through shell hooks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub const CONFIG_NAME: &str = "wt.toml";

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Display name. Defaults to the main repository's directory name.
    pub project: Option<String>,
    /// Directory holding the worktrees. Templated. Defaults to `{{main}}/../{{repo}}-wt`.
    pub root: Option<String>,
    /// Branch name template used by `wt new <slug>`. Defaults to `wt/{{slug}}`.
    pub branch: Option<String>,

    /// Variables reusable in every template (they may reference one another; the
    /// resolution is iterative).
    #[serde(default)]
    pub vars: BTreeMap<String, String>,

    /// Directories created in the worktree right after checkout (unversioned caches).
    #[serde(default)]
    pub dirs: Vec<String>,

    /// Files and directories inherited from the main repository.
    #[serde(default, rename = "copy")]
    pub copies: Vec<CopySpec>,

    /// Automatically allocated ports, exposed as `{{port.<name>}}`.
    #[serde(default)]
    pub ports: BTreeMap<String, PortSpec>,

    /// Questions the interface asks before `new` / `up`. Their answers become options
    /// (`{{opt.<name>}}`), exactly like a `--set` on the command line.
    #[serde(default, rename = "prompt")]
    pub prompts: Vec<Prompt>,

    #[serde(default)]
    pub hooks: Hooks,

    /// Ad-hoc commands: `wt run <name> <slug>`.
    #[serde(default)]
    pub tasks: BTreeMap<String, Task>,

    /// Language servers a front-end may start for this project's code.
    ///
    /// `wt` does nothing with them: it neither launches nor supervises a
    /// language server, and the command line has no use for one. They are here
    /// because this is the file a project already uses to say what it needs —
    /// a graphical front-end embedding the library reads them alongside the
    /// tasks and the ports, and no project has to learn a second file.
    ///
    /// Server-specific settings do **not** belong here: a language server
    /// almost always has a configuration file of its own, in the project, that
    /// it reads and watches itself.
    #[serde(default)]
    pub lsp: BTreeMap<String, LanguageServer>,

    #[serde(default)]
    pub status: Status,

    #[serde(default)]
    pub editor: Editor,

    #[serde(default)]
    pub open: Open,
}

/// A question asked before an action, whose answer becomes an option.
///
/// This is what lets a project have a real start-up dialogue — "shared or isolated
/// databases?", "which tenants to mount?" — without the binary knowing what a tenant is:
/// the list of choices can come from a shell command.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    /// Name of the produced option: `{{opt.<name>}}` and `WT_OPT_<NAME>`.
    pub name: String,
    /// Displayed label. Defaults to the name.
    #[serde(default)]
    pub question: String,
    #[serde(default, rename = "type")]
    pub kind: PromptKind,
    /// When to ask. Defaults to start-up.
    #[serde(default)]
    pub ask: Ask,
    /// Fixed choices.
    #[serde(default)]
    pub options: Vec<PromptOption>,
    /// Computed choices: a shell command writing one line per choice,
    /// `value<TAB>label<TAB>detail` (the last two columns are optional).
    pub source: Option<String>,
    /// Preselected value (for `multi`, several separated by `separator`).
    pub default: Option<String>,
    /// Separator for `multi` values. Defaults to a comma.
    #[serde(default = "comma")]
    pub separator: String,
    /// Shell command: the question is only asked when it exits 0. Answers already given
    /// are in the environment, which is what lets questions chain.
    pub when: Option<String>,
    /// Ask again even when the value is already known (by default an option already
    /// answered, or given with `--set`, is not asked again).
    #[serde(default)]
    pub always: bool,
}

fn comma() -> String {
    ",".to_string()
}

impl Prompt {
    pub fn title(&self) -> &str {
        if self.question.is_empty() {
            &self.name
        } else {
            &self.question
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PromptKind {
    /// One value out of N.
    #[default]
    Choice,
    /// Several values out of N, joined by `separator`.
    Multi,
    /// Yes / no, producing `1` or `0`.
    Confirm,
    /// Free-form input.
    Text,
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Ask {
    /// Before `wt up` (default).
    #[default]
    Up,
    /// Before `wt new`.
    New,
    Both,
    /// Never asked by a phase: only when a task lists the prompt in its `prompt` field.
    Task,
}

impl Ask {
    pub fn covers(self, phase: Ask) -> bool {
        self == phase || self == Ask::Both
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PromptOption {
    pub value: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopySpec {
    /// Path relative to the main repository.
    pub from: String,
    /// Path relative to the worktree. Defaults to `from`.
    pub to: Option<String>,
    #[serde(default)]
    pub mode: CopyMode,
    /// Do not fail when the source is missing. Defaults to true.
    #[serde(default = "yes")]
    pub optional: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CopyMode {
    /// Hardlink tree: near-instant, near-zero disk. Package managers (composer, npm)
    /// replace files, which cleanly breaks the link.
    #[default]
    Hardlink,
    /// Real copy, for what the worktree will modify (a `.env`, typically).
    Copy,
    /// Symbolic link to the source.
    Symlink,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortSpec {
    /// First port tried; incremented until a free, unreserved one is found.
    pub base: u16,
    /// When to allocate: at worktree creation, or at start-up (default).
    #[serde(default)]
    pub allocate: Allocate,
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Allocate {
    New,
    #[default]
    Up,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// After the checkout, the directories and the copies.
    #[serde(default)]
    pub post_new: Commands,
    /// `wt up` — starting the services.
    #[serde(default)]
    pub up: Commands,
    /// `wt down` — stopping the services; the checkout is kept.
    #[serde(default)]
    pub down: Commands,
    /// Before removing the worktree (the checkout still exists: databases, containers…).
    #[serde(default)]
    pub pre_rm: Commands,
    /// After removal (the directory is gone, cwd = main repository).
    #[serde(default)]
    pub post_rm: Commands,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    #[serde(default)]
    pub description: String,
    pub run: Commands,
    /// `worktree` (default) or `main`.
    #[serde(default)]
    pub cwd: Cwd,
    /// The task needs the terminal (shell, `logs -f`, editor, full-screen watcher).
    /// The interface then steps aside instead of showing the output in a panel, because
    /// a panel neither forwards keystrokes nor renders direct output.
    #[serde(default)]
    pub interactive: bool,
    /// Names of `[[prompt]]` entries the interface asks before the run; the answers
    /// become the task's `{{args}}` (multi values split on the prompt's separator).
    /// On the command line, arguments are passed directly and nothing is asked.
    #[serde(default)]
    pub prompt: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Cwd {
    #[default]
    Worktree,
    Main,
}

/// A language server declared by the project.
///
/// The table's key names it (`[lsp.php]`), and doubles as the LSP `languageId`
/// announced for the files it serves — `language` overrides that when the two
/// differ.
///
/// Which of several servers a given file belongs to is the front-end's call,
/// not ours: `extensions` is the raw material, and a file such as
/// `page.blade.php` matches two entries at once. The rule that settles it — the
/// longest extension wins — belongs where the files are opened.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LanguageServer {
    /// The program to run. Templated like everything else, so a server living
    /// in the project (`{{main}}/vendor/bin/…`) can be named.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment added to the server's own, values templated as well.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Extensions served, without the leading dot: `"php"`, `"blade.php"`.
    /// Empty means the front-end decides, usually by the server's own name.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// The `languageId` announced to the server. Defaults to the table's key.
    pub language: Option<String>,
}

impl LanguageServer {
    /// The `languageId` for this server, `name` being its key in the table.
    pub fn language_id<'a>(&'a self, name: &'a str) -> &'a str {
        self.language.as_deref().unwrap_or(name)
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Status {
    /// Shell command whose exit code 0 means "worktree started".
    /// When absent, a worktree is simply "created".
    pub up: Option<String>,
    /// Extra lines shown in the TUI preview (`name = shell command`).
    #[serde(default)]
    pub info: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Editor {
    /// Default command. `WT_IDE` in the environment still takes precedence.
    pub command: Option<String>,
    /// Shell opened in the worktree when a terminal is requested.
    /// Defaults to `WT_TERMINAL`, then `$SHELL`.
    pub terminal: Option<String>,
    /// Terminal emulator opening that shell in a window of its own.
    /// Defaults to `WT_TERMINAL_WINDOW`, then whichever known emulator is installed.
    /// With none, the shell takes over the terminal the interface runs in.
    pub terminal_window: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Open {
    /// Main URL, offered first by `wt open <slug>`.
    pub url: Option<String>,
    /// Label for that URL. Defaults to "application".
    pub label: Option<String>,
    /// Extra links computed on demand: a shell command writing one line per link,
    /// `url<TAB>label`. This is what allows one address per tenant, per service or per
    /// environment — the binary does not need to know what those are.
    pub source: Option<String>,
}

/// One command or a list of them — `run = "…"` and `run = ["…", "…"]` are both
/// accepted, because both forms come naturally when writing.
#[derive(Debug, Default, Clone)]
pub struct Commands(pub Vec<String>);

impl Commands {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for Commands {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        Ok(match OneOrMany::deserialize(d)? {
            OneOrMany::One(s) => Commands(vec![s]),
            OneOrMany::Many(v) => Commands(v),
        })
    }
}

/// A loaded project: the main repository, its config, and the path of the file read.
pub struct Project {
    pub main: PathBuf,
    pub config_path: PathBuf,
    pub config: Config,
}

impl Project {
    /// Loads the main repository's `wt.toml`.
    ///
    /// Called from a worktree, `crate::git::main_repo` walks back to the main repository:
    /// the config is the project's, not that of the branch currently checked out.
    pub fn load(start: &Path) -> Result<Self> {
        let main = crate::git::main_repo(start)?;
        let config_path = match std::env::var_os("WT_CONFIG") {
            Some(p) => PathBuf::from(p),
            None => main.join(CONFIG_NAME),
        };
        if !config_path.exists() {
            bail!(
                "{}",
                t!("err.no_config", file = CONFIG_NAME, path = main.display())
            );
        }
        let raw = fs::read_to_string(&config_path)
            .with_context(|| t!("err.read_failed", path = config_path.display()).to_string())?;
        let config: Config = toml::from_str(&raw)
            .with_context(|| t!("err.invalid_config", path = config_path.display()).to_string())?;
        Ok(Project {
            main,
            config_path,
            config,
        })
    }

    pub fn name(&self) -> String {
        self.config.project.clone().unwrap_or_else(|| {
            self.main
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "projet".into())
        })
    }

    /// Worktree root: absolute path, template resolved.
    pub fn root(&self) -> Result<PathBuf> {
        let repo = self
            .main
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut base = BTreeMap::new();
        base.insert("main".to_string(), self.main.display().to_string());
        base.insert("repo".to_string(), repo.clone());
        let tpl = self
            .config
            .root
            .clone()
            .unwrap_or_else(|| format!("{{{{main}}}}/../{repo}-wt"));
        let rendered = crate::tmpl::render(&tpl, &base);
        let path = PathBuf::from(&rendered);
        let path = if path.is_absolute() {
            path
        } else {
            self.main.join(path)
        };
        Ok(crate::util::normalize(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Templates and examples ship with the binary: if they do not parse back, `wt init`
    /// writes a broken config. This happened once with a `dirs` placed after `[vars]`,
    /// which TOML then attaches to that table.
    #[test]
    fn templates_and_examples_are_valid() {
        let fichiers = [
            (
                "templates/plain.toml",
                include_str!("../templates/plain.toml"),
            ),
            ("templates/web.toml", include_str!("../templates/web.toml")),
            ("wt.toml", include_str!("../wt.toml")),
            (
                "examples/rust-cli.toml",
                include_str!("../examples/rust-cli.toml"),
            ),
            (
                "examples/python-cli.toml",
                include_str!("../examples/python-cli.toml"),
            ),
            (
                "examples/node-vite.toml",
                include_str!("../examples/node-vite.toml"),
            ),
            (
                "examples/laravel-sail.toml",
                include_str!("../examples/laravel-sail.toml"),
            ),
        ];
        for (nom, contenu) in fichiers {
            toml::from_str::<Config>(contenu).unwrap_or_else(|e| panic!("{nom} : {e}"));
        }
    }

    /// A declared language server is data, and the defaults are what a project
    /// leaves out: no arguments, no environment, and a `languageId` that is the
    /// table's key.
    #[test]
    fn a_language_server_defaults_to_its_key() {
        let c: Config = toml::from_str(
            r#"
[lsp.php]
command = "phpantom_lsp"
extensions = ["php", "blade.php"]

[lsp.rust]
command = "rust-analyzer"
language = "rust"
env = { RA_LOG = "info" }
args = ["--log-file", "/tmp/ra.log"]
"#,
        )
        .unwrap();

        let php = &c.lsp["php"];
        assert_eq!(php.command, "phpantom_lsp");
        assert_eq!(php.language_id("php"), "php");
        assert!(php.args.is_empty());
        assert!(php.env.is_empty());
        assert_eq!(php.extensions, ["php", "blade.php"]);

        let rust = &c.lsp["rust"];
        assert_eq!(rust.language_id("rust"), "rust");
        assert_eq!(rust.env["RA_LOG"], "info");
        assert_eq!(rust.args.len(), 2);
    }

    /// A project that declares none is the normal case, and the field must not
    /// force one on it.
    #[test]
    fn no_language_server_is_the_default() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.lsp.is_empty());
    }

    /// A project may have no configuration at all: no ports, no services, no URL.
    #[test]
    fn an_empty_config_is_valid() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.ports.is_empty());
        assert!(c.hooks.up.is_empty());
        assert!(c.open.url.is_none());
        assert!(c.status.up.is_none());
    }
}
