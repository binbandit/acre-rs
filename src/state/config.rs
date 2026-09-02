//! User configuration: defaults, loading with overrides, validation, and `acre config set`.

use std::path::Path;

use serde_json::Value;

use crate::error::{AcreError, Result, exit};
use crate::model::{
    AcreConfig, EnvironmentConfig, PoolConfig, RepoConfig, RepoEnvironmentConfig, SafetyConfig,
};
use crate::state::paths::{config_path, default_acre_root};
use crate::state::storage::{read_json, write_json};
use crate::util::validate_relative_path;

impl Default for AcreConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            root: default_acre_root(),
            pool: PoolConfig::default(),
            environment: EnvironmentConfig::default(),
            safety: SafetyConfig::default(),
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_slots: 1,
            max_slots: 4,
            replenish: true,
            idle_retention_days: 30,
        }
    }
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self {
            cache_roots: Vec::new(),
            required_roots: Vec::new(),
            seed_files: vec![".env".to_owned(), ".env.local".to_owned()],
            excluded_roots: Vec::new(),
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            detect_processes: true,
            block_unknown_ignored_files: true,
        }
    }
}

/// The user's configuration over the defaults; a missing file means all defaults.
pub fn load_config() -> Result<AcreConfig> {
    let config = read_json::<AcreConfig>(&config_path())?.unwrap_or_default();
    validate_acre_config(&config)?;
    Ok(config)
}

pub fn save_config(config: &AcreConfig) -> Result<()> {
    validate_acre_config(config)?;
    write_json(&config_path(), config)
}

pub fn load_repo_config(repository_root: &Path) -> Result<RepoConfig> {
    let path = repository_root.join(".acre.json");
    let config = read_json::<RepoConfig>(&path)?.unwrap_or_default();
    if let Some(environment) = &config.environment {
        validate_environment_paths(
            environment
                .cache_roots
                .iter()
                .chain(environment.required_roots.iter())
                .chain(environment.seed_files.iter())
                .chain(environment.excluded_roots.iter()),
            ".acre.json",
        )?;
    }
    Ok(config)
}

pub fn validate_acre_config(config: &AcreConfig) -> Result<()> {
    if config.schema_version != 1 {
        return invalid("schemaVersion must be 1.");
    }
    if config.root.as_os_str().is_empty() {
        return invalid("root must be a non-empty path.");
    }
    if config.pool.max_slots == 0 {
        return invalid("pool.maxSlots must be a positive integer.");
    }
    if config.pool.min_slots > config.pool.max_slots {
        return invalid("pool.minSlots cannot exceed pool.maxSlots.");
    }
    validate_environment_paths(
        config
            .environment
            .cache_roots
            .iter()
            .chain(config.environment.required_roots.iter())
            .chain(config.environment.seed_files.iter())
            .chain(config.environment.excluded_roots.iter()),
        "Acre configuration",
    )
}

pub fn write_default_repo_config(path: &Path) -> Result<()> {
    let value = RepoConfig {
        environment: Some(RepoEnvironmentConfig {
            cache_roots: Vec::new(),
            required_roots: Vec::new(),
            seed_files: vec![".env".to_owned(), ".env.local".to_owned()],
            excluded_roots: Vec::new(),
        }),
    };
    write_json(path, &value)
}

pub fn ensure_default_config_file() -> Result<()> {
    let path = config_path();
    if !path.exists() {
        save_config(&AcreConfig::default())?;
    }
    Ok(())
}

pub fn set_config_value(config: &mut AcreConfig, key: &str, raw: &str) -> Result<Value> {
    // Try JSON first so `true` and `[".env"]` parse; anything else is a plain string.
    let value = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
    match key {
        "root" => {
            config.root = value
                .as_str()
                .map(Into::into)
                .ok_or_else(|| invalid_error("root must be a string."))?;
        }
        "pool.minSlots" => config.pool.min_slots = as_usize(&value, key)?,
        "pool.maxSlots" => config.pool.max_slots = as_usize(&value, key)?,
        "pool.replenish" => config.pool.replenish = as_bool(&value, key)?,
        "pool.idleRetentionDays" => config.pool.idle_retention_days = as_u64(&value, key)?,
        "environment.cacheRoots" => config.environment.cache_roots = as_strings(&value, key)?,
        "environment.requiredRoots" => config.environment.required_roots = as_strings(&value, key)?,
        "environment.seedFiles" => config.environment.seed_files = as_strings(&value, key)?,
        "environment.excludedRoots" => config.environment.excluded_roots = as_strings(&value, key)?,
        "safety.detectProcesses" => config.safety.detect_processes = as_bool(&value, key)?,
        "safety.blockUnknownIgnoredFiles" => {
            config.safety.block_unknown_ignored_files = as_bool(&value, key)?
        }
        _ => return Err(invalid_error(format!("Unknown configuration key: {key}"))),
    }
    validate_acre_config(config)?;
    Ok(value)
}

fn validate_environment_paths(values: impl IntoIterator<Item = impl AsRef<str>>, source: &str) -> Result<()> {
    for value in values {
        let value = value.as_ref();
        if !validate_relative_path(value) {
            return invalid(format!("{source} contains an unsafe repository path: {value}"));
        }
    }
    Ok(())
}

fn as_usize(value: &Value, key: &str) -> Result<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid_error(format!("{key} must be a non-negative integer.")))
}

fn as_u64(value: &Value, key: &str) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| invalid_error(format!("{key} must be a non-negative integer.")))
}

fn as_bool(value: &Value, key: &str) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| invalid_error(format!("{key} must be a boolean.")))
}

fn as_strings(value: &Value, key: &str) -> Result<Vec<String>> {
    value
        .as_array()
        .filter(|values| values.iter().all(Value::is_string))
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .ok_or_else(|| invalid_error(format!("{key} must be an array of strings.")))
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> AcreError {
    AcreError::new("ACRE_CONFIG_INVALID", message, exit::USAGE)
}
