//! Unit tests for the guardrail rate limiter + name heuristics.

use std::time::{Duration, Instant};

use uuid::Uuid;

use super::names::{NameVerdict, assess_name};
use super::{ActionClass, ActionContext, GuardrailConfig, GuardrailDecision, Guardrails};

fn ctx() -> ActionContext<'static> {
    ActionContext::default()
}

#[test]
fn destructive_action_throttles_after_burst() {
    let g = Guardrails::new(GuardrailConfig::default());
    let actor = Uuid::now_v7();
    let t0 = Instant::now();
    // Burst capacity for destructive actions is 3.
    for _ in 0..3 {
        assert_eq!(
            g.check_at(actor, ActionClass::DeleteChannel, &ctx(), t0),
            GuardrailDecision::Allow
        );
    }
    // The 4th in the same instant is throttled.
    match g.check_at(actor, ActionClass::DeleteChannel, &ctx(), t0) {
        GuardrailDecision::Throttle { retry_after_secs, .. } => assert!(retry_after_secs >= 1),
        other => panic!("expected throttle, got {other:?}"),
    }
    // After enough time, a token refills (1 per 20s).
    assert_eq!(
        g.check_at(actor, ActionClass::DeleteChannel, &ctx(), t0 + Duration::from_secs(21)),
        GuardrailDecision::Allow
    );
}

#[test]
fn owner_is_not_blocked_by_default() {
    let g = Guardrails::new(GuardrailConfig::default());
    let owner = Uuid::now_v7();
    let t0 = Instant::now();
    let octx = ActionContext { is_owner: true, ..Default::default() };
    for _ in 0..10 {
        assert_eq!(
            g.check_at(owner, ActionClass::DeleteChannel, &octx, t0),
            GuardrailDecision::Allow
        );
    }
}

#[test]
fn owner_blocked_when_configured() {
    let g = Guardrails::new(GuardrailConfig { block_owner: true });
    let owner = Uuid::now_v7();
    let t0 = Instant::now();
    let octx = ActionContext { is_owner: true, ..Default::default() };
    for _ in 0..3 {
        let _ = g.check_at(owner, ActionClass::DeleteChannel, &octx, t0);
    }
    assert!(matches!(
        g.check_at(owner, ActionClass::DeleteChannel, &octx, t0),
        GuardrailDecision::Throttle { .. }
    ));
}

#[test]
fn additive_has_generous_budget() {
    let g = Guardrails::new(GuardrailConfig::default());
    let actor = Uuid::now_v7();
    let t0 = Instant::now();
    // Capacity 5 for create-channel.
    for _ in 0..5 {
        assert!(g.check_at(actor, ActionClass::CreateChannel, &ctx(), t0).allowed());
    }
    assert!(matches!(
        g.check_at(actor, ActionClass::CreateChannel, &ctx(), t0),
        GuardrailDecision::Throttle { .. }
    ));
}

#[test]
fn flags_random_channel_names() {
    assert!(matches!(assess_name("xk7qzwf", &[]), NameVerdict::Suspicious { .. }));
    assert!(matches!(assess_name("aaaaaa", &[]), NameVerdict::Suspicious { .. }));
    assert!(matches!(assess_name("99999999", &[]), NameVerdict::Suspicious { .. }));
    // Ordinary names pass.
    assert_eq!(assess_name("general", &[]), NameVerdict::Ok);
    assert_eq!(assess_name("off-topic", &[]), NameVerdict::Ok);
    assert_eq!(assess_name("dev", &[]), NameVerdict::Ok); // too short to judge
}

#[test]
fn flags_low_variance_spam_cluster() {
    let recent: Vec<String> = vec![
        "raid-1".to_owned(),
        "raid-2".to_owned(),
        "raid-3".to_owned(),
        "general".to_owned(),
    ];
    assert!(matches!(
        assess_name("raid-4", &recent),
        NameVerdict::Suspicious { .. }
    ));
    // A genuinely different name in the same server is fine.
    assert_eq!(assess_name("introductions", &recent), NameVerdict::Ok);
}

#[test]
fn create_channel_flagged_but_allowed_for_suspicious_name() {
    let g = Guardrails::new(GuardrailConfig::default());
    let actor = Uuid::now_v7();
    let recent: Vec<String> = vec!["spam-a".into(), "spam-b".into(), "spam-c".into()];
    let actx = ActionContext { name: Some("spam-d"), recent_names: &recent, is_owner: false };
    match g.check(actor, ActionClass::CreateChannel, &actx) {
        GuardrailDecision::AllowFlagged { .. } => {}
        other => panic!("expected AllowFlagged, got {other:?}"),
    }
}
