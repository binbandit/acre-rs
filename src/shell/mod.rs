//! The bridge between Acre and the parent shell: directive files, generated integrations, navigation.

use serde::{Deserialize, Serialize};

pub mod directive;
pub mod generator;
pub mod navigation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SupportedShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}
