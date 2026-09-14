//! Snapshot-bound review facade with separated scope, verdict, and orchestration policy.
use super::Ctx;
use crate::agent::AgentRequest;
use crate::prompts::PLAN_REVIEW_PROMPT;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

#[path = "review_scope.rs"]
mod scope;
use scope::{
    classify_review_scope, dual_review_policy, review_snapshot, review_snapshot_against,
};

#[path = "review_verdict.rs"]
mod verdict;
use verdict::{
    acceptance_criteria_items, aggregate_review_gate, normalize_review_verdict,
    partition_review_policy_by_cadence, stage_required_roles,
};

#[path = "review_orchestration.rs"]
mod orchestration;
use orchestration::ReviewScope;

#[cfg(test)]
use crate::model_selection::claude_model_limit;

#[cfg(test)]
use crate::app::App;

#[cfg(test)]
use std::{fs, path::PathBuf, process::Command};

#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
#[path = "review_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "review_scope_tests.rs"]
mod scope_tests;

#[cfg(test)]
#[path = "review_verdict_tests.rs"]
mod verdict_tests;

#[cfg(test)]
#[path = "review_orchestration_tests.rs"]
mod orchestration_tests;

#[path = "plan_review.rs"]
mod plan_review;
