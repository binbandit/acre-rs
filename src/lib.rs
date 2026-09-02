//! Acre: warm, reusable Git workspaces. The binary is a thin wrapper over this crate so tests can drive it.

pub mod cli;
pub mod commands;
pub mod environment;
pub mod error;
pub mod git;
pub mod model;
pub mod provider;
pub mod shell;
pub mod state;
pub mod ui;
pub mod util;
pub mod workspace;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
