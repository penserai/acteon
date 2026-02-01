use std::collections::HashMap;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use acteon_core::Action;
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_rules::{EvalContext, RuleEngine};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

fn test_action() -> Action {
    Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Hello",
            "priority": 5
        }),
    )
}

fn simple_expression() -> Expr {
    // action.action_type == "send_email"
    Expr::Binary(
        BinaryOp::Eq,
        Box::new(Expr::Field(
            Box::new(Expr::Ident("action".into())),
            "action_type".into(),
        )),
        Box::new(Expr::String("send_email".into())),
    )
}

fn complex_expression() -> Expr {
    // (action.action_type == "send_email")
    //   && (action.payload.priority > 3)
    //   && (action.payload.to starts_with "user")
    Expr::All(vec![
        Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "action_type".into(),
            )),
            Box::new(Expr::String("send_email".into())),
        ),
        Expr::Binary(
            BinaryOp::Gt,
            Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "payload".into(),
                )),
                "priority".into(),
            )),
            Box::new(Expr::Int(3)),
        ),
        Expr::Binary(
            BinaryOp::StartsWith,
            Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "payload".into(),
                )),
                "to".into(),
            )),
            Box::new(Expr::String("user".into())),
        ),
    ])
}

fn bench_simple_expression(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let action = test_action();
    let store = MemoryStateStore::new();
    let env = HashMap::new();
    let expr = simple_expression();

    c.bench_function("eval_simple_expression", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx = EvalContext::new(&action, &store, &env);
                let result = acteon_rules::engine::executor::eval(black_box(&expr), &ctx).await;
                black_box(result)
            })
        });
    });
}

fn bench_complex_expression(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let action = test_action();
    let store = MemoryStateStore::new();
    let env = HashMap::new();
    let expr = complex_expression();

    c.bench_function("eval_complex_expression", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx = EvalContext::new(&action, &store, &env);
                let result = acteon_rules::engine::executor::eval(black_box(&expr), &ctx).await;
                black_box(result)
            })
        });
    });
}

fn bench_state_access(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let action = test_action();
    let store = MemoryStateStore::new();
    let env = HashMap::new();

    // Pre-populate state: a string value and a counter.
    rt.block_on(async {
        let state_key = StateKey::new("notifications", "tenant-1", KeyKind::State, "user-pref");
        store.set(&state_key, "dark-mode", None).await.unwrap();

        let counter_key =
            StateKey::new("notifications", "tenant-1", KeyKind::Counter, "email-count");
        store.increment(&counter_key, 42, None).await.unwrap();
    });

    // state_get("user-pref") != null && state_counter("email-count") > 10
    let expr = Expr::All(vec![
        Expr::Binary(
            BinaryOp::Ne,
            Box::new(Expr::StateGet("user-pref".into())),
            Box::new(Expr::Null),
        ),
        Expr::Binary(
            BinaryOp::Gt,
            Box::new(Expr::StateCounter("email-count".into())),
            Box::new(Expr::Int(10)),
        ),
    ]);

    c.bench_function("eval_state_access", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx = EvalContext::new(&action, &store, &env);
                let result = acteon_rules::engine::executor::eval(black_box(&expr), &ctx).await;
                black_box(result)
            })
        });
    });
}

fn bench_rule_engine_evaluate(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let action = test_action();
    let store = MemoryStateStore::new();
    let env = HashMap::new();

    let rules = vec![
        Rule::new(
            "check-type",
            Expr::Binary(
                BinaryOp::Eq,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "action_type".into(),
                )),
                Box::new(Expr::String("spam".into())),
            ),
            RuleAction::Suppress,
        )
        .with_priority(1),
        Rule::new(
            "check-priority",
            Expr::Binary(
                BinaryOp::Gt,
                Box::new(Expr::Field(
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("action".into())),
                        "payload".into(),
                    )),
                    "priority".into(),
                )),
                Box::new(Expr::Int(10)),
            ),
            RuleAction::Deny,
        )
        .with_priority(2),
        Rule::new("fallback-allow", Expr::Bool(true), RuleAction::Allow).with_priority(100),
    ];

    let engine = RuleEngine::new(rules);

    c.bench_function("rule_engine_evaluate_3_rules", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ctx = EvalContext::new(&action, &store, &env);
                let verdict = engine.evaluate(black_box(&ctx)).await;
                black_box(verdict)
            })
        });
    });
}

criterion_group!(
    benches,
    bench_simple_expression,
    bench_complex_expression,
    bench_state_access,
    bench_rule_engine_evaluate,
);
criterion_main!(benches);
