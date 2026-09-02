use std::path::Path;

use crate::environment::roots::inspect_ignored;
use crate::error::Result;
use crate::model::{CloneMode, EnvironmentPlan, EnvironmentSnapshot, EnvironmentState};

pub fn inspect_environment(root: &Path, plan: &EnvironmentPlan) -> Result<EnvironmentSnapshot> {
    let layout = inspect_ignored(root, &plan.cache_roots)?;
    let state = if plan.cache_roots.is_empty()
        || (!plan.required_roots.is_empty()
            && plan.required_roots.iter().all(|root| layout.has_cache_root(root)))
    {
        EnvironmentState::Ready
    } else if !layout.cache_roots.is_empty() {
        EnvironmentState::Warm
    } else {
        EnvironmentState::Cold
    };
    Ok(EnvironmentSnapshot {
        fingerprint: plan.fingerprint.clone(),
        state,
        cache_roots: plan.cache_roots.clone(),
        required_roots: plan.required_roots.clone(),
        present_roots: layout.cache_roots,
        source: None,
        cloned_files: None,
        cloned_bytes: None,
        clone_mode: Some(CloneMode::None),
    })
}
