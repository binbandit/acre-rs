//! One handler per command. Handlers parse intent, call the workspace lifecycle, and render results.

pub mod config;
pub mod done;
pub mod internal;
pub mod machine;
pub mod new;
pub mod open;
pub mod opened;
pub mod setup;
pub mod shell;
pub mod system;
