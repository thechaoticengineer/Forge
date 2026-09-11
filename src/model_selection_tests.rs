use super::*;
use crate::{app::App, test_support::QueueTest};
use std::sync::Arc;

fn configured(fixture: &QueueTest) -> Ctx {
    let mut settings = crate::plan::default_settings();
    for role in ["planner", "architect", "reviewer"] {
        settings[role] = json!("claude");
    }
    settings["model_catalogue"]["entries"] = json!([
        {"provider":"claude","model":"claude-fable-5-1[1m]","tier":"strong"},
        {"provider":"claude","model":"opus[1m]","tier":"strong"},
        {"provider":"codex","model":"small","tier":"basic"},
        {"provider":"codex","model":"large","tier":"strong"}
    ]);
    Arc::new(App::new(fixture.app.project(), settings)).context(fixture.app.project())
}

const LIMIT: &str =
    "claude exited with exit status: 1: You've reached your Fable limit. Switch to another model.";

#[test]
fn all_fresh_roles_share_fallback_and_report_the_actual_choice() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    for role in ["planner", "architect", "chat", "enhance", "reviewer"] {
        let requirements = ctx.model_requirements(role, Some("codex")).unwrap();
        let mut calls = vec![];
        let (output, choice) = ctx
            .with_selected_model(&requirements, None, |choice| {
                calls.push(choice.clone());
                if choice.1.contains("fable") {
                    Err(LIMIT.into())
                } else {
                    Ok("done")
                }
            })
            .unwrap();
        assert_eq!(output, "done");
        assert_eq!(calls.len(), 2, "{role}");
        assert_eq!(
            choice,
            (
                "claude".into(),
                "opus[1m]".into(),
                "provider_default".into()
            )
        );
        assert_eq!(calls.last(), Some(&choice));
    }
}

#[test]
fn shared_fallback_is_bounded_and_preserves_explicit_choices_and_failure_kind() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    let requirements = ctx.model_requirements("planner", None).unwrap();
    let mut calls = 0;
    let error = ctx
        .with_selected_model::<()>(&requirements, None, |choice| {
            calls += 1;
            Err(if choice.1.contains("fable") {
                LIMIT.into()
            } else {
                "claude exited with exit status: 1: You've reached your Opus limit.".into()
            })
        })
        .unwrap_err();
    assert_eq!(calls, 2);
    assert!(error.contains("no eligible strong planner"));
    for error in [
        "authentication failed",
        "Selected model is at capacity",
        "invalid response JSON",
    ] {
        let mut calls = 0;
        let result = ctx.with_selected_model::<()>(&requirements, None, |_| {
            calls += 1;
            Err(error.into())
        });
        assert_eq!(result.unwrap_err(), error);
        assert_eq!(calls, 1);
    }
    for role in ["planner", "architect", "chat", "enhance", "reviewer"] {
        let key = format!("{}_model", if matches!(role, "chat" | "enhance") { "planner" } else { role });
        ctx.app.settings.lock().unwrap()[&key] = json!("claude-fable-5-1[1m]");
        let requirements = ctx.model_requirements(role, Some("codex")).unwrap();
        let mut calls = 0;
        assert_eq!(
            ctx.with_selected_model::<()>(&requirements, None, |_| {
                calls += 1;
                Err(LIMIT.into())
            })
            .unwrap_err(),
            LIMIT
        );
        assert_eq!(calls, 1);
        ctx.app.settings.lock().unwrap()[&key] = json!("");
    }
    ctx.app.settings.lock().unwrap()["automatic_routing"] = json!(false);
    let requirements = ctx.model_requirements("planner", None).unwrap();
    assert_eq!(
        ctx.with_selected_model::<()>(&requirements, None, |_| Err(LIMIT.into()))
            .unwrap_err(),
        LIMIT
    );
    ctx.session.stop_requested.store(true, Ordering::SeqCst);
    assert!(
        ctx.with_selected_model::<()>(&requirements, None, |_| panic!(
            "must not launch after stop"
        ))
        .unwrap_err()
        .contains("stopped")
    );
}

#[test]
fn two_models_can_cover_three_capability_classes() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    for (tier, expected) in [
        (Tier::Basic, "small"),
        (Tier::Standard, "large"),
        (Tier::Strong, "large"),
    ] {
        let requirements = ModelRequirements::for_class("implementer", "codex", tier);
        assert_eq!(ctx.select_model(&requirements, &[]).unwrap().1, expected);
    }
    let requirements = ModelRequirements::for_class("implementer", "codex", Tier::Strong);
    assert!(
        ctx.select_model(&requirements, &[("codex".into(), "large".into())])
            .is_err()
    );
}

fn selection_failure(ctx: &Ctx, requirements: &ModelRequirements) -> String {
    ctx.with_selected_model::<()>(requirements, None, |_| {
        panic!("selection failure must not invoke a provider")
    })
    .unwrap_err()
}

#[test]
fn stage_roles_have_caller_owned_capability_floors() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    for role in ["implementer", "fixer", "future_stage_role"] {
        let requirements = ctx.model_requirements(role, None).unwrap();
        assert_eq!(requirements.minimum_tier, None, "{role}");
        assert_eq!(ctx.select_model(&requirements, &[]).unwrap().1, "small");
        for (tier, expected) in [
            (Tier::Basic, "small"),
            (Tier::Standard, "large"),
            (Tier::Strong, "large"),
        ] {
            let requirements = requirements.clone().requiring_at_least(tier);
            assert_eq!(ctx.select_model(&requirements, &[]).unwrap().1, expected);
        }
        let requirements = requirements
            .requiring_at_least(Tier::Strong)
            .requiring_at_least(Tier::Basic);
        assert_eq!(requirements.minimum_tier, Some(Tier::Strong));
    }
}

#[test]
fn fixed_roles_still_require_strong_models() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
        {"provider":"claude","model":"basic","tier":"basic"},
        {"provider":"claude","model":"standard","tier":"standard"}
    ]);
    for role in ["planner", "architect", "chat", "enhance", "reviewer"] {
        let requirements = ctx.model_requirements(role, Some("codex")).unwrap();
        assert_eq!(requirements.minimum_tier, Some(Tier::Strong), "{role}");
        let label = if role == "reviewer" {
            "independent reviewer"
        } else {
            role
        };
        assert_eq!(
            selection_failure(&ctx, &requirements),
            format!("no eligible strong {label} model")
        );
    }
    ctx.app.settings.lock().unwrap()["reviewer_model"] = json!("basic");
    let requirements = ctx.model_requirements("reviewer", Some("codex")).unwrap();
    assert_eq!(requirements.minimum_tier, None);
    assert!(!requirements.fallback);
    assert_eq!(ctx.select_model(&requirements, &[]).unwrap().1, "basic");
}

#[test]
fn pinned_models_report_configured_and_required_tiers() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    for role in ["planner", "architect", "chat", "enhance", "implementer"] {
        let configured_role = if matches!(role, "chat" | "enhance") {
            "planner"
        } else {
            role
        };
        ctx.app.settings.lock().unwrap()[configured_role] = json!("codex");
        for (model, tier) in [("small", "basic"), ("unregistered", "unclassified")] {
            ctx.app.settings.lock().unwrap()[format!("{configured_role}_model")] = json!(model);
            let requirements = ctx
                .model_requirements(role, None)
                .unwrap()
                .requiring_at_least(Tier::Strong);
            assert_eq!(
                selection_failure(&ctx, &requirements),
                format!(
                    "pinned {role} model codex/{model}: configured tier {tier}; this work requires at least strong - change the registry tier or the pin"
                )
            );
        }
    }
}

#[test]
fn pinned_models_preserve_catalogue_rejection_reasons() {
    for (model, effort, rejection, expected) in [
        (
            "invalid model!",
            "provider_default",
            "",
            "invalid model identifier",
        ),
        (
            "small",
            "unsupported",
            "",
            "unsupported or unknown native effort; use provider_default",
        ),
        (
            "small",
            "high",
            "effort",
            "native effort was rejected during execution; use provider_default",
        ),
        (
            "small",
            "provider_default",
            "model",
            "provider/model is known unavailable",
        ),
    ] {
        let f = QueueTest::new(false);
        let ctx = configured(&f);
        {
            let mut settings = ctx.app.settings.lock().unwrap();
            settings["implementer_model"] = json!(model);
            settings["model_catalogue"]["entries"][2]["effort"] = json!(effort);
        }
        let policy = Policy::from_settings(&ctx.app.settings.lock().unwrap()).unwrap();
        match rejection {
            "effort" => ctx
                .app
                .catalogue
                .reject_effort(&policy, Provider::Codex, model, effort),
            "model" => ctx.app.catalogue.observe(
                &policy,
                Provider::Codex,
                model,
                Err(crate::catalogue::Failure::new(
                    crate::catalogue::FailureKind::Rejected,
                )),
            ),
            _ => {}
        }
        let facts = ctx.model_facts(&policy, Provider::Codex, model, None);
        assert_eq!(facts["error"], expected);
        let mut requirements = ctx.model_requirements("implementer", None).unwrap();
        for fallback in [false, true] {
            requirements.fallback = fallback;
            assert_eq!(
                selection_failure(&ctx, &requirements),
                format!("pinned implementer model codex/{model}: {expected}")
            );
        }
    }
}

#[test]
fn pinned_models_report_setup_errors_and_preserve_reviewer_independence() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    ctx.app.settings.lock().unwrap()["implementer_model"] = json!("small");
    ctx.app.settings.lock().unwrap()["implementer"] = json!("invalid");
    let requirements = ctx.model_requirements("implementer", None).unwrap();
    assert_eq!(
        selection_failure(&ctx, &requirements),
        "pinned implementer model invalid/small: invalid model provider"
    );
    ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["tier"] = json!("invalid");
    let cause = Policy::from_settings(&ctx.app.settings.lock().unwrap()).unwrap_err();
    assert_eq!(
        selection_failure(&ctx, &requirements),
        format!("pinned implementer model invalid/small: {cause}")
    );
    ctx.app.settings.lock().unwrap()["reviewer_model"] = json!("opus[1m]");
    for (implementer, cause) in [
        (None, "cannot resolve independent reviewer provider"),
        (
            Some("claude"),
            "independent reviewer must be configured as codex, the other provider relative to claude",
        ),
    ] {
        let error = ctx
            .model_requirements("reviewer", implementer)
            .err()
            .unwrap();
        assert_eq!(
            error,
            format!("pinned reviewer model claude/opus[1m]: {cause}")
        );
    }
}

#[test]
fn excluded_pin_is_terminal_even_when_fallback_is_enabled() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    ctx.app.settings.lock().unwrap()["implementer_model"] = json!("small");
    let mut requirements = ctx.model_requirements("implementer", None).unwrap();
    for fallback in [false, true] {
        requirements.fallback = fallback;
        assert_eq!(
            ctx.select_model(&requirements, &[("codex".into(), "small".into())])
                .unwrap_err(),
            "pinned implementer model codex/small: model was already attempted"
        );
    }
}

#[test]
fn automatic_cost_order_is_stable_and_requires_complete_tier_preferences() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    for (preferences, expected) in [
        (json!([30, 10, 10]), "second"),
        (json!([10, 10, 30]), "first"),
        (json!([30, 10, null]), "first"),
        (json!([null, 10, 30]), "first"),
        (json!([30, null, 10]), "first"),
    ] {
        ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"strong","tier":"strong","relative_cost_preference":0},
            {"provider":"codex","model":"first","tier":"basic","relative_cost_preference":preferences[0]},
            {"provider":"codex","model":"standard-expensive","tier":"standard","relative_cost_preference":20},
            {"provider":"codex","model":"second","tier":"basic","relative_cost_preference":preferences[1]},
            {"provider":"codex","model":"third","tier":"basic","relative_cost_preference":preferences[2]},
            {"provider":"codex","model":"standard-cheap","tier":"standard","relative_cost_preference":5}
        ]);
        let requirements = ctx.model_requirements("implementer", None).unwrap();
        assert_eq!(
            ctx.select_model(&requirements, &[]).unwrap().1,
            expected,
            "{preferences}"
        );
        assert_eq!(
            ctx.select_model(&requirements.requiring_at_least(Tier::Standard), &[])
                .unwrap()
                .1,
            "standard-cheap"
        );
    }
}

#[test]
fn explicit_model_and_native_effort_are_not_replaced_by_cost_order() {
    let f = QueueTest::new(false);
    let ctx = configured(&f);
    {
        let mut settings = ctx.app.settings.lock().unwrap();
        settings["implementer_model"] = json!("large");
        settings["model_catalogue"]["entries"][3]["effort"] = json!("high");
        settings["model_catalogue"]["entries"][3]["relative_cost_preference"] = json!(100);
        settings["model_catalogue"]["entries"][2]["relative_cost_preference"] = json!(0);
    }
    let requirements = ctx.model_requirements("implementer", None).unwrap();
    assert!(!requirements.fallback);
    let mut calls = 0;
    let (_, choice) = ctx
        .with_selected_model(&requirements, None, |_| {
            calls += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(choice, ("codex".into(), "large".into(), "high".into()));
}

#[test]
fn quota_blocked_pin_is_terminal_even_when_fallback_is_enabled() {
    const BRIDGE_ENV: &str = "FORGE_SELECTION_TEST_QUOTA_BRIDGE";
    let f = QueueTest::new(false);
    let Ok(bridge) = std::env::var(BRIDGE_ENV) else {
        // Isolate PATH in a child test process: never change global test state or
        // depend on an installed Claude CLI to populate the real quota service.
        use std::os::unix::fs::PermissionsExt;
        let bridge = f.path.join("quota-bridge");
        let response = json!({
            "bridge_version":1,"source":"claude_code_usage",
            "cli_version":"2.1.263 (Claude Code)","sdk_version":"0.3.261",
            "usage":{"available":true,"extra_usage_enabled":false,"windows":[{
                "name":"Fable weekly","model":"Fable","used_percent":100,
                "resets_at":"future","resets_unix":crate::util::unix_timestamp()+3600
            }]}
        });
        for (path, script) in [
            (
                f.path.join("claude"),
                "#!/bin/sh\nprintf '%s\\n' '2.1.263 (Claude Code)'\n".to_string(),
            ),
            (
                bridge.clone(),
                format!("#!/bin/sh\nprintf '%s\\n' '{response}'\n"),
            ),
        ] {
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "model_selection::tests::quota_blocked_pin_is_terminal_even_when_fallback_is_enabled", "--nocapture"])
            .env("PATH", std::env::join_paths(std::iter::once(f.path.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap())
            .env(BRIDGE_ENV, bridge)
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    };
    let ctx = configured(&f);
    let model = "claude-fable-5-1[1m]";
    {
        let mut settings = ctx.app.settings.lock().unwrap();
        settings["planner_model"] = json!(model);
        settings["model_catalogue"]["claude_bridge"] = json!(bridge);
    }
    let reason = ctx.app.quota.check(&bridge, model).unwrap_err();
    assert!(reason.contains("Fable weekly quota exhausted"), "{reason}");
    let mut requirements = ctx.model_requirements("planner", None).unwrap();
    for fallback in [false, true] {
        requirements.fallback = fallback;
        assert_eq!(
            selection_failure(&ctx, &requirements),
            format!("pinned planner model claude/{model}: {reason}")
        );
    }
}
