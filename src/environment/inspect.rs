use std::path::Path;

use crate::model::{CloneMode, EnvironmentPlan, EnvironmentSnapshot, EnvironmentState};

pub fn inspect_environment(root: &Path, plan: &EnvironmentPlan) -> EnvironmentSnapshot {
    let present_roots: Vec<String> = plan
        .cache_roots
        .iter()
        .filter(|cache_root| root.join(cache_root).exists())
        .cloned()
        .collect();
    let state = if plan.cache_roots.is_empty()
        || (!plan.required_roots.is_empty()
            && plan
                .required_roots
                .iter()
                .all(|root| present_roots.contains(root)))
    {
        EnvironmentState::Ready
    } else if !present_roots.is_empty() {
        EnvironmentState::Warm
    } else {
        EnvironmentState::Cold
    };
    EnvironmentSnapshot {
        fingerprint: plan.fingerprint.clone(),
        state,
        cache_roots: plan.cache_roots.clone(),
        required_roots: plan.required_roots.clone(),
        present_roots,
        source: None,
        cloned_files: None,
        cloned_bytes: None,
        clone_mode: Some(CloneMode::None),
    }
}
