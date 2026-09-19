//! Compact feature context for milestone plans (M3, D15).
//!
//! A milestone plan records `feature: {slug, milestone, title, scenario_ids}`.
//! Everything an agent prompt says about that feature is derived here from
//! those engine-owned identifiers and titles only, so a prompt never grows
//! with the feature files and never embeds their contents: agents read
//! `docs/features/<slug>/` from the repository themselves.

use serde_json::{Value, json};

fn scenario_ids(feature: &Value) -> Vec<&str> {
    feature["scenario_ids"].as_array().into_iter().flatten().filter_map(Value::as_str).collect()
}

/// The repository folder of a feature, derived from its slug.
fn folder(feature: &Value) -> String {
    format!("docs/features/{}/", feature["slug"].as_str().unwrap_or(""))
}

/// The plan's feature object as a compact JSON value for work-status contexts.
/// None for a plan without a `feature` object.
pub(crate) fn feature_summary(plan_feature: &Value) -> Option<Value> {
    plan_feature.is_object().then(|| json!({
        "slug": plan_feature["slug"], "folder": folder(plan_feature),
        "milestone": plan_feature["milestone"], "title": plan_feature["title"],
        "scenario_ids": plan_feature["scenario_ids"],
    }))
}

/// A compact text block with the feature folder, milestone, covered scenario
/// IDs and the rules a milestone plan follows. None for a plan without a
/// `feature` object.
pub(crate) fn feature_reference(plan_feature: &Value) -> Option<String> {
    if !plan_feature.is_object() { return None; }
    let slug = plan_feature["slug"].as_str().unwrap_or("");
    let ids = scenario_ids(plan_feature).join(", ");
    Some(format!(
        "FEATURE REFERENCE (this plan implements one milestone of a feature spec):\n\
        - feature: {slug}\n\
        - folder: {}\n\
        - milestone: {} — {}\n\
        - covered scenario IDs: {ids}\n\
        Read scenarios.md, decisions.md and milestones.md in that folder from the repository yourself; they are not included here.\n\
        Rules for this plan:\n\
        (a) The first stage turns every covered scenario into an executable test named after its scenario ID that fails before implementation, and names every covered scenario ID ({ids}) in its instructions or acceptance.\n\
        (b) The final stage sets the milestone's `Status: implemented` and its `Business tests:` line in {}milestones.md to name the test files that cover its scenarios (comma or `and` separated repository paths, each optionally followed by a parenthesized note), inside the reviewed commit range.\n",
        folder(plan_feature),
        plan_feature["milestone"].as_str().unwrap_or(""),
        plan_feature["title"].as_str().unwrap_or(""),
        folder(plan_feature),
    ))
}

/// Covered scenario IDs that the first stage names neither in its instructions
/// nor in its acceptance. IDs match as whole tokens: `S3` is not in `S30`.
pub(crate) fn missing_first_stage_ids(plan_feature: &Value, first_stage: &Value) -> Vec<String> {
    let text = format!("{} {}", first_stage["instructions"].as_str().unwrap_or(""),
        first_stage["acceptance"].as_str().unwrap_or(""));
    scenario_ids(plan_feature).into_iter().filter(|id| !contains_token(&text, id))
        .map(str::to_owned).collect()
}

fn contains_token(text: &str, token: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(token).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(word)
            && !text[at + token.len()..].chars().next().is_some_and(word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_ids_match_as_whole_tokens() {
        assert!(contains_token("covers S3, S30.", "S3"));
        assert!(!contains_token("covers S30 and S31", "S3"));
        assert!(!contains_token("XS3", "S3"));
        assert!(contains_token("(S3)", "S3"));
    }

    #[test]
    fn reference_is_none_without_a_feature_object() {
        assert!(feature_reference(&Value::Null).is_none());
        assert!(feature_summary(&json!("x")).is_none());
    }

    #[test]
    fn missing_ids_are_named_in_order() {
        let feature = json!({"slug":"f","milestone":"M1","title":"T","scenario_ids":["S3","S30","S31"]});
        let stage = json!({"instructions":"tests for S30","acceptance":"S3 passes"});
        assert_eq!(missing_first_stage_ids(&feature, &stage), vec!["S31"]);
    }

    #[test]
    fn revision_of_a_milestone_plan_gets_the_reference_and_the_first_stage_rule() {
        use crate::test_support::{QueueTest, editable_stage};
        use std::sync::atomic::Ordering;
        let test = QueueTest::new(false);
        let mut stage = editable_stage(1);
        stage["instructions"] = json!("Write tests for S30 and S31.");
        let original = json!({"goal": "Goal", "status": "approved", "stages": [stage, editable_stage(2)],
            "feature": {"slug": "demo", "milestone": "M1", "title": "Alpha", "scenario_ids": ["S30", "S31"]}});
        test.app.save_plan(&original).unwrap();
        let original = test.app.load_plan().unwrap();
        let incomplete = json!({"goal": "Goal", "status": "draft", "stages": [
            {"id": 1, "title": "T", "instructions": "Write tests for S3 and S31.", "acceptance": "", "commit": "x"},
            editable_stage(2)]});
        let complete = json!({"goal": "Goal", "status": "draft", "stages": [
            {"id": 1, "title": "T", "instructions": "Write tests for S30 and S31.", "acceptance": "", "commit": "x"},
            editable_stage(2)]});
        test.app.app.settings.lock().unwrap()["mock_plan_output"] = json!([incomplete, complete]);
        test.app.acquire_busy().unwrap();
        test.app.revise_worker(&original, "Keep it");
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        let settings = test.app.app.settings.lock().unwrap();
        let prompts: Vec<&str> = settings["mock_agent_requests"].as_array().unwrap().iter()
            .filter(|r| r["role"] == "planner").filter_map(|r| r["prompt"].as_str()).collect();
        assert_eq!(prompts.len(), 2, "the incomplete revision is corrected once");
        assert!(prompts[0].contains("FEATURE REFERENCE") && prompts[0].contains("docs/features/demo/"));
        assert!(prompts[1].contains("missing: S30"), "whole-token matching: S3 does not name S30");
        drop(settings);
        assert_eq!(test.app.load_plan().unwrap()["feature"]["milestone"], "M1");
    }
}
