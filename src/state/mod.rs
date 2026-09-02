//! Everything Acre persists under its root: repository state, config, locks, sessions, pending operations.

pub mod config;
pub mod index;
pub mod lock;
pub mod operations;
pub mod paths;
pub mod repository;
pub mod shell;
pub mod storage;
