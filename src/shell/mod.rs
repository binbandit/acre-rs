//! The bridge between Acre and the parent shell: directive files, generated integrations, navigation.

use std::fmt;

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

impl fmt::Display for SupportedShell {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::Powershell => "powershell",
        })
    }
}
