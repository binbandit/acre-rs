//! The only module that runs Git: discovery, porcelain parsing, and mutations.

pub mod content;
pub mod operations;
pub mod refs;
pub mod repository;
pub mod runner;
pub mod status;
pub mod worktrees;
