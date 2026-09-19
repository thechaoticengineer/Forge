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

/// Plan-review criteria for a milestone plan: one item per covered scenario ID
/// (S31). Empty for a plan without a `feature` object.
pub(crate) fn scenario_criteria(plan_feature: &Value) -> Vec<String> {
    if !plan_feature.is_object() { return Vec::new(); }
    scenario_ids(plan_feature).into_iter()
        .map(|id| format!("an executable test traceable to {id} exists and passes")).collect()
}

/// `acceptance` followed by the scenario criteria of `plan_feature`, one per
/// line. The same string feeds the review prompt, the mock review, verdict
/// normalization and every later revalidation, so a missing or failed scenario
/// criterion prevents approval. Unchanged for a plan without a feature.
pub(crate) fn with_scenario_criteria(acceptance: &str, plan_feature: &Value) -> String {
    let criteria = scenario_criteria(plan_feature);
    if criteria.is_empty() { return acceptance.to_string(); }
    let mut lines: Vec<&str> = acceptance.lines().collect();
    lines.extend(criteria.iter().map(String::as_str));
    lines.join("\n")
}

/// The reviewer section listing every registered business test file (S32, D7,
/// D13). None when no feature registers a file, so prompts stay unchanged.
pub(crate) fn registry_section(registered: &[String]) -> Option<String> {
    if registered.is_empty() { return None; }
    Some(format!(
        "\nREGISTERED BUSINESS TESTS (the `Business tests:` lines of every feature in docs/features/):\n\
        You must run every listed business test with the matching project command (cargo test for Rust test modules, \
        node --test for .mjs files, qmltestrunner -input for QML test files, the repository's documented command otherwise) \
        and record each run in project_checks. Any failing business test prevents approval.\n{}\n",
        registered.iter().map(|file| format!("- {file}")).collect::<Vec<_>>().join("\n")))
}

/// The reviewer section listing registered business test files that the diff
/// under review modifies, deletes or renames away (S33, D14). None when the diff
/// touches none. The engine only discloses these files; it never blocks them.
pub(crate) fn changed_section(changed: &[String]) -> Option<String> {
    if changed.is_empty() { return None; }
    Some(format!(
        "\nCHANGED BUSINESS TESTS (registered files this diff modifies, deletes or renames):\n{}\n\
        Reject unless each change only adds tests or implements a scenario change approved in the feature spec, \
        and say in your verdict which of the two applies to each file.\n",
        changed.iter().map(|file| format!("- {file}")).collect::<Vec<_>>().join("\n")))
}

/// Implementer, fixer and plan-fixer paragraph (S33): registered business tests
/// keep passing and approved-scenario conflicts are escalated to the architect,
/// never solved by editing the test. `escalation` names the channel. None when
/// no business test is registered and the plan has no feature.
pub(crate) fn conflict_paragraph(registered: &[String], plan_feature: &Value, escalation: &str) -> Option<String> {
    if registered.is_empty() && !plan_feature.is_object() { return None; }
    let files = if registered.is_empty() { "(none registered yet)".to_string() } else { registered.join(", ") };
    Some(format!(
        "\nBUSINESS TEST RULES: registered business test files: {files}. They must keep passing. \
        If the implementation conflicts with an approved scenario or its business test, escalate it to the architect \
        as an architectural context gap ({escalation}). Never change, skip, weaken or delete a business test to resolve such a conflict.\n"))
}

/// Paths a `git diff --name-status -z <base>` output modifies, deletes, changes
/// type of, or renames away (the rename source). Added and copied files never
/// count: they leave every existing path in place.
pub(crate) fn affected_paths(name_status_z: &str) -> Vec<String> {
    let mut fields = name_status_z.split('\0').filter(|f| !f.is_empty());
    let mut paths = Vec::new();
    while let Some(status) = fields.next() {
        let Some(path) = fields.next() else { break };
        match status.chars().next() {
            Some('M' | 'D' | 'T') => paths.push(path.to_string()),
            Some('R') => { paths.push(path.to_string()); fields.next(); }
            Some('C') => { fields.next(); }
            _ => {}
        }
    }
    paths
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
    fn name_status_counts_modified_deleted_type_changed_and_rename_sources() {
        let output = "M\0tests/a.rs\0D\0tests/b.rs\0A\0tests/new.rs\0T\0tests/link.rs\0\
            R087\0tests/old.rs\0tests/moved.rs\0C100\0tests/src.rs\0tests/copy.rs\0M\0tests/with space.rs\0";
        assert_eq!(affected_paths(output),
            ["tests/a.rs", "tests/b.rs", "tests/link.rs", "tests/old.rs", "tests/with space.rs"]);
        assert!(affected_paths("").is_empty());
        assert!(affected_paths("A\0only/added.rs\0").is_empty());
    }

    #[test]
    fn sections_are_absent_without_registered_files_or_feature() {
        assert!(registry_section(&[]).is_none());
        assert!(changed_section(&[]).is_none());
        assert!(conflict_paragraph(&[], &Value::Null, "x").is_none());
        let files = vec!["tests/a.rs".to_string()];
        assert!(registry_section(&files).unwrap().to_lowercase().contains("must run"));
        assert!(changed_section(&files).unwrap().contains("- tests/a.rs"));
        assert!(conflict_paragraph(&[], &json!({"slug":"f"}), "x").is_some());
    }

    #[test]
    fn scenario_criteria_extend_acceptance_only_for_milestone_plans() {
        assert_eq!(with_scenario_criteria("a\nb", &Value::Null), "a\nb");
        let feature = json!({"slug":"f","milestone":"M1","scenario_ids":["S1","S2"]});
        assert_eq!(with_scenario_criteria("a", &feature),
            "a\nan executable test traceable to S1 exists and passes\nan executable test traceable to S2 exists and passes");
        assert_eq!(with_scenario_criteria("", &feature).lines().count(), 2);
    }

    /// Plans a two-stage goal, turns it into a milestone plan whose roles all
    /// review per stage, then approves and runs it with `verdicts` queued.
    fn run_per_stage_milestone_plan(verdicts: Value) -> crate::test_support::QueueTest {
        use crate::test_support::{QueueTest, api_request, wait_for_worker};
        let test = QueueTest::new(false);
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["review_cadence"] = json!({"architect": "per_stage", "reviewer": "per_stage"});
            settings["max_fix_rounds"] = json!(0);
            settings["mock_verdicts"] = verdicts;
            settings["mock_plan_output"] = json!({"goal": "G", "status": "draft", "stages": [
                {"id": 1, "title": "Tests", "instructions": "Write tests for S1 and S2.", "acceptance": "Tests exist.", "commit": "test: t"},
                {"id": 2, "title": "Build", "instructions": "Implement it.", "acceptance": "It works.", "commit": "feat: b"}]});
        }
        let (code, resp) = api_request(&test.app.app, "POST", "/api/plan", json!({"goal": "G"}));
        assert_eq!(code, 200, "{resp}");
        wait_for_worker(&test.app);
        let mut plan = test.app.load_plan().unwrap();
        plan["feature"] = json!({"slug": "demo", "milestone": "M1", "title": "Alpha", "scenario_ids": ["S1", "S2"]});
        test.app.save_plan(&plan).unwrap();
        for path in ["/api/approve", "/api/run"] {
            let (code, resp) = api_request(&test.app.app, "POST", path, json!({}));
            assert_eq!(code, 200, "{resp}");
        }
        wait_for_worker(&test.app);
        test
    }

    #[test]
    fn per_stage_only_milestone_plan_evidences_scenarios_in_its_final_stage() {
        let test = run_per_stage_milestone_plan(json!([]));
        let plan = test.app.load_plan().unwrap();
        assert_eq!(plan["status"], "done", "{plan}");
        assert!(plan["plan_review"].is_null() || plan["plan_review"]["status"].is_null(), "no plan review runs");
        let settings = test.app.app.settings.lock().unwrap();
        let prompts: Vec<&str> = settings["test_review_sessions"].as_array().unwrap().iter()
            .filter(|r| r["role"] == "reviewer").filter_map(|r| r["prompt"].as_str()).collect();
        assert_eq!(prompts.len(), 2, "one reviewer per stage");
        let criteria = |prompt: &str| prompt[prompt.rfind("CRITERIA TO EVIDENCE:").unwrap()..].lines().next().unwrap().to_string();
        assert!(!criteria(prompts[0]).contains("traceable to"), "only the final stage carries scenario criteria");
        for id in ["S1", "S2"] {
            assert!(criteria(prompts[1]).contains(&format!("an executable test traceable to {id} exists and passes")));
        }
        let verdict = &plan["stages"][1]["last_verdict"];
        assert!(verdict["criteria"].as_array().unwrap().iter()
            .any(|c| c["criterion"] == "an executable test traceable to S2 exists and passes"));
    }

    #[test]
    fn per_stage_only_milestone_plan_cannot_finish_without_scenario_evidence() {
        // The final stage's approvals evidence only the stage acceptance, never the
        // scenario criteria, so no correction can make them valid.
        let unevidenced = json!({"approved": true, "summary": "Looks fine", "issues": [], "checks": ["ran"],
            "criteria": [{"criterion": "It works.", "status": "passed", "evidence": "ok"}]});
        // The first stage gets an ordinary approval; its criteria are filled in by the mock.
        let mut verdicts = vec![json!({"approved": true, "summary": "ok", "issues": [], "checks": ["ran"]})];
        verdicts.extend(std::iter::repeat_n(unevidenced, 12));
        let test = run_per_stage_milestone_plan(json!(verdicts));
        let plan = test.app.load_plan().unwrap();
        assert_ne!(plan["status"], "done", "{plan}");
        assert_eq!(plan["stages"][0]["status"], "committed");
        assert_ne!(plan["stages"][1]["status"], "committed", "{}", plan["stages"][1]);
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
