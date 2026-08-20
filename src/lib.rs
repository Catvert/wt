//! `wt` en bibliothèque.
//!
//! Le binaire n'est qu'un des consommateurs de ces modules : Claudhub s'en sert pour
//! lire le `wt.toml` d'un projet, créer un worktree et lancer ses tâches, sans jamais
//! passer par la ligne de commande — parser une sortie alignée, colorée et traduite
//! reviendrait à lire ce qui est fait pour un humain.
//!
//! Ce qui n'a de sens que dans un terminal — la CLI, l'interface skim, le tableau de
//! bord ratatui — vit derrière la caractéristique `cli`, active par défaut. Sans elle,
//! la bibliothèque ne tire ni clap, ni ratatui, ni skim : un consommateur graphique n'a
//! pas à les compiler pour créer un dossier.

#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

pub mod config;
pub mod fuzzy;
pub mod git;
pub mod i18n;
pub mod ops;
pub mod state;
pub mod tmpl;
pub mod util;

#[cfg(feature = "cli")]
pub mod ansi;
#[cfg(feature = "cli")]
pub mod complete;
#[cfg(feature = "cli")]
pub mod skim_ui;
#[cfg(feature = "cli")]
pub mod ui;
