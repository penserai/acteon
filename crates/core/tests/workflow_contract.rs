//! Server side of the cross-SDK workflow contract.
//!
//! The fixtures in `clients/contract-fixtures/workflow-contract.json` pin
//! the wire shapes shared by the worker SDKs (Python, Node.js) and this
//! crate: every directive an SDK can emit must parse into
//! [`WorkflowDirective`] (and round-trip byte-for-byte), the malformed
//! shapes must be rejected by `from_task_result` (the settle path turns
//! those into a workflow failure instead of a silent completion), and the
//! reserved constants must match. The SDK test suites consume the same
//! file, so a drift on either side fails somewhere.

use acteon_core::{CHILD_RESULT_SIGNAL_PREFIX, WORKFLOW_TASK_ACTION_TYPE, WorkflowDirective};

const FIXTURES: &str = include_str!("../../../clients/contract-fixtures/workflow-contract.json");

fn fixtures() -> serde_json::Value {
    serde_json::from_str(FIXTURES).expect("contract fixtures must be valid JSON")
}

#[test]
fn reserved_constants_match_the_contract() {
    let fixtures = fixtures();
    assert_eq!(
        fixtures["constants"]["workflow_task_action_type"],
        serde_json::json!(WORKFLOW_TASK_ACTION_TYPE)
    );
    assert_eq!(
        fixtures["constants"]["child_result_signal_prefix"],
        serde_json::json!(CHILD_RESULT_SIGNAL_PREFIX)
    );
}

#[test]
fn every_sdk_directive_parses_and_round_trips() {
    let fixtures = fixtures();
    let cases = fixtures["directives"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let json = &case["json"];

        // The settle path must accept it…
        let directive = WorkflowDirective::from_task_result(json)
            .unwrap_or_else(|| panic!("directive fixture `{name}` did not parse: {json}"));

        // …agree on the variant…
        let expected_tag = json["directive"].as_str().unwrap();
        let actual_tag = match &directive {
            WorkflowDirective::Complete { .. } => "complete",
            WorkflowDirective::Fail { .. } => "fail",
            WorkflowDirective::Sleep { .. } => "sleep",
            WorkflowDirective::AwaitSignal { .. } => "await_signal",
        };
        assert_eq!(actual_tag, expected_tag, "fixture `{name}`");

        // …and serialize back to the exact fixture shape (so directives the
        // server echoes, e.g. in task results, stay SDK-parseable).
        let round_tripped = serde_json::to_value(&directive).unwrap();
        assert_eq!(&round_tripped, json, "fixture `{name}` round trip");
    }
}

#[test]
fn malformed_directives_are_rejected_not_misread() {
    let fixtures = fixtures();
    let cases = fixtures["invalid_directives"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let reason = case["reason"].as_str().unwrap();
        let json = &case["json"];
        assert!(
            WorkflowDirective::from_task_result(json).is_none(),
            "invalid directive parsed ({reason}): {json}"
        );
        // Every invalid fixture still carries the `directive` key — that is
        // what routes it to a loud workflow failure in the settle path
        // rather than being mistaken for a plain completion result.
        assert!(
            json.get("directive").is_some(),
            "invalid fixture must carry a `directive` key ({reason})"
        );
    }
}
