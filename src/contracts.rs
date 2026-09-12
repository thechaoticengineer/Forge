//! Version-one persistence and provider boundary contracts.
//! Routing and dual-review policy records remain available for later stages.
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionReference {
    pub provider: String,
    pub reference: String,
    pub checkpoint_reference: String,
    pub resume_policy: ResumePolicy,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResumePolicy {
    ForkFromCheckpoint,
    ExactIfCommitted,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Planner,
    Architect,
    Implementer,
    Reviewer,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EffectiveModel {
    pub provider: String,
    pub model: String,
    /// Exact provider-native value; never reinterpret one provider's effort as another's.
    pub native_effort: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReviewPolicy {
    pub version: u64,
    pub required_roles: Vec<Role>,
    pub scope: String,
    pub rationale: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Invocation {
    pub version: u64,
    pub id: String,
    pub plan_id: String,
    pub revision: u64,
    pub stage_id: Option<i64>,
    pub attempt_id: Option<String>,
    pub role: Role,
    pub effective: EffectiveModel,
    pub session: Option<SessionReference>,
    pub prompt: String,
    pub review_policy: ReviewPolicy,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InvocationResult {
    pub version: u64,
    pub invocation_id: String,
    pub effective: EffectiveModel,
    pub session: Option<SessionReference>,
    pub output: Value,
    pub usage: Value,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Availability {
    Verified,
    Unverified,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Provenance {
    pub capability_policy_version: String,
    pub catalogue_revision: String,
    pub official_sources: Vec<String>,
    pub checked_unix: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Catalogue {
    pub version: u64,
    pub revision: String,
    pub refreshed_unix: i64,
    pub models: Vec<CatalogueModel>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CatalogueModel {
    pub provider: String,
    pub model: String,
    pub native_efforts: Vec<String>,
    pub capability_tiers: Vec<String>,
    pub availability: Availability,
    pub provenance: Provenance,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionKind {
    Proposal,
    Agreement,
    Selection,
    Invalidation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ModelRecord {
    pub version: u64,
    pub id: String,
    pub kind: SelectionKind,
    pub proposal_ids: Vec<String>,
    pub agreement_id: Option<String>,
    pub plan_id: String,
    pub revision: u64,
    pub stage_id: i64,
    pub relevant_inputs: Value,
    pub effective: Option<EffectiveModel>,
    pub planner_reason: String,
    pub architect_reason: String,
    pub provenance: Provenance,
    pub availability: Availability,
    pub trigger: String,
    pub superseded_agreement: Option<String>,
    pub unix: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Decision {
    pub version: u64,
    pub id: String,
    pub plan_id: String,
    pub stage_id: Option<i64>,
    pub revision: u64,
    pub summary: String,
    pub rationale: String,
    pub alternatives: Vec<Alternative>,
    pub status: DecisionStatus,
    pub supersedes: Option<String>,
    pub created_unix: i64,
    pub updated_unix: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Alternative {
    pub description: String,
    pub tradeoffs: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Guidance {
    pub version: u64,
    pub id: String,
    pub stage_id: i64,
    pub revision: u64,
    pub relevant_inputs: Value,
    pub text: String,
    pub valid: bool,
    pub unix: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReviewRecord {
    pub version: u64,
    pub id: String,
    pub role: Role,
    pub stage_id: i64,
    pub revision: u64,
    pub attempt_id: String,
    pub round: u64,
    pub verdict: Value,
    pub policy: ReviewPolicy,
    pub unix: i64,
}
