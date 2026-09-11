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
