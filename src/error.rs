//! One error type with stable codes and exit classes; the UI layer decides how it looks.

use std::fmt::{Display, Formatter};

use serde::Serialize;
use serde_json::{Value, json};

pub mod exit {
    pub const SUCCESS: i32 = 0;
    pub const INTERNAL: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const ENVIRONMENT: i32 = 3;
    pub const NOT_FOUND: i32 = 4;
    pub const CONFLICT: i32 = 5;
    pub const REFUSED: i32 = 6;
    pub const GIT: i32 = 7;
    pub const NETWORK: i32 = 8;
    pub const INTERRUPTED: i32 = 130;
    // A private handshake with the shell wrapper: cd, then call back with the token.
    pub const RESUME: i32 = 194;
}

#[derive(Debug)]
pub struct AcreError {
    pub code: &'static str,
    pub message: String,
    pub exit_code: i32,
    pub details: Value,
}

impl AcreError {
    pub fn new(code: &'static str, message: impl Into<String>, exit_code: i32) -> Self {
        Self {
            code,
            message: message.into(),
            exit_code,
            details: json!({}),
        }
    }

    pub fn with_details<T: Serialize>(mut self, details: T) -> Self {
        self.details = serde_json::to_value(details).unwrap_or_else(|_| json!({}));
        self
    }

    pub fn io(context: impl Into<String>, error: std::io::Error) -> Self {
        let context = context.into();
        Self::new("ACRE_IO", format!("{context}: {error}"), exit::ENVIRONMENT)
            .with_details(json!({ "kind": format!("{:?}", error.kind()) }))
    }

    pub fn json(context: impl Into<String>, error: serde_json::Error) -> Self {
        let context = context.into();
        Self::new("ACRE_JSON", format!("{context}: {error}"), exit::ENVIRONMENT)
    }
}

impl Display for AcreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AcreError {}

impl From<std::io::Error> for AcreError {
    fn from(error: std::io::Error) -> Self {
        Self::io("filesystem operation failed", error)
    }
}

impl From<serde_json::Error> for AcreError {
    fn from(error: serde_json::Error) -> Self {
        Self::json("could not read JSON", error)
    }
}

pub type Result<T> = std::result::Result<T, AcreError>;
