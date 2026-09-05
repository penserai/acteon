#![cfg(feature = "swarm")]

use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("acteon-scenario-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_acteon-scenario"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cli_replays_trials_preserves_input_and_rejects_forged_evidence() {
    let workspace = Workspace::new();
    std::fs::write(workspace.0.join("suite.json"), r#"{"schema_version":2,"seed":42,"backend":"memory","trials":1,"scenarios":["incident_response","refund_fulfillment","prompt_injection"]}"#).unwrap();
    let output = workspace.run(&["--manifest", "suite.json", "--output", "first"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for file in ["report.json", "manifest.json", "trace.jsonl", "junit.xml"] {
        assert!(
            workspace
                .0
                .join("first")
                .join(file)
                .metadata()
                .unwrap()
                .len()
                > 0
        );
    }
    let original = std::fs::read(workspace.0.join("first/report.json")).unwrap();
    let output = workspace.run(&["--replay", "first/report.json", "--output", "replay"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = workspace.run(&["--replay", "first/report.json", "--output", "first"]);
    assert!(!output.status.success());
    assert_eq!(
        original,
        std::fs::read(workspace.0.join("first/report.json")).unwrap()
    );
    let mut forged: serde_json::Value = serde_json::from_slice(&original).unwrap();
    forged["results"][0]["evidence"]["invariants"][0]["passed"] = false.into();
    std::fs::write(
        workspace.0.join("forged.json"),
        serde_json::to_vec(&forged).unwrap(),
    )
    .unwrap();
    let output = workspace.run(&["--replay", "forged.json", "--output", "forged-run"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("inconsistent"));
    assert!(!workspace.0.join("forged-run").exists());
    let mut changed_runner: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed_runner["provenance"]["runner_sha256"] = "different runner".into();
    std::fs::write(
        workspace.0.join("different-runner.json"),
        serde_json::to_vec(&changed_runner).unwrap(),
    )
    .unwrap();
    let output = workspace.run(&[
        "--replay",
        "different-runner.json",
        "--output",
        "different-runner",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("provenance differs"));
    assert!(!workspace.0.join("different-runner").exists());
}

#[test]
fn cli_rejects_unknown_versions_fields_and_unbounded_trials() {
    let workspace = Workspace::new();
    for manifest in [
        r#"{"schema_version":99}"#,
        r#"{"schema_version":2,"seed":42,"backend":"memory","trials":0,"scenarios":["incident_response"]}"#,
        r#"{"schema_version":2,"seed":42,"backend":"memory","trials":999999,"scenarios":["incident_response"]}"#,
        r#"{"schema_version":2,"seed":42,"backend":"memory","trials":1,"scenarios":["incident_response"],"disable_gates":true}"#,
    ] {
        std::fs::write(workspace.0.join("suite.json"), manifest).unwrap();
        assert!(
            !workspace
                .run(&["--manifest", "suite.json", "--output", "invalid"])
                .status
                .success()
        );
        assert!(!workspace.0.join("invalid").exists());
    }
}

#[test]
fn replay_preserves_reports_named_after_any_output_artifact() {
    let workspace = Workspace::new();
    // The overwrite guard must run before parsing or dispatch, so invalid JSON
    // is enough to distinguish preservation from a later parse failure.
    for name in ["report.json", "manifest.json", "trace.jsonl", "junit.xml"] {
        std::fs::write(workspace.0.join(name), b"original evidence").unwrap();
        let output = workspace.run(&["--replay", name, "--output", "."]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("output must differ"));
        assert_eq!(
            std::fs::read(workspace.0.join(name)).unwrap(),
            b"original evidence"
        );
    }
}

#[cfg(unix)]
#[test]
fn replay_preserves_hard_linked_input() {
    let workspace = Workspace::new();
    std::fs::write(workspace.0.join("saved.json"), b"original evidence").unwrap();
    std::fs::hard_link(
        workspace.0.join("saved.json"),
        workspace.0.join("manifest.json"),
    )
    .unwrap();
    let output = workspace.run(&["--replay", "saved.json", "--output", "."]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("output must differ"));
    assert_eq!(
        std::fs::read(workspace.0.join("saved.json")).unwrap(),
        b"original evidence"
    );
}

#[test]
fn cli_replays_virtual_time_and_rejects_remote_ttls_or_forged_clocks() {
    let workspace = Workspace::new();
    for scenario in ["deadline_safety", "worker_lifecycle"] {
        for backend in ["redis", "postgres"] {
            for version in [1, 2] {
                let mut manifest = serde_json::json!({"schema_version":version,"seed":42,"backend":backend,"scenarios":[scenario]});
                if version == 2 {
                    manifest["trials"] = 1.into();
                }
                std::fs::write(
                    workspace.0.join("suite.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
                let output = workspace.run(&["--manifest", "suite.json", "--output", "invalid"]);
                assert!(!output.status.success());
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("requires the memory backend")
                );
                assert!(!workspace.0.join("invalid").exists());
            }
        }
    }
    std::fs::write(workspace.0.join("suite.json"), r#"{"schema_version":2,"seed":42,"backend":"memory","trials":2,"scenarios":["deadline_safety","worker_lifecycle"]}"#).unwrap();
    let output = workspace.run(&["--manifest", "suite.json", "--output", "first"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = workspace.run(&["--replay", "first/report.json", "--output", "replay"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first = std::fs::read(workspace.0.join("first/report.json")).unwrap();
    assert_eq!(
        first,
        std::fs::read(workspace.0.join("replay/report.json")).unwrap()
    );
    let mut forged: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert!(
        forged["provenance"]["clock"]
            .as_str()
            .unwrap()
            .contains("manual UTC epoch")
    );
    forged["provenance"]["clock"] = "wall_clock".into();
    std::fs::write(
        workspace.0.join("forged-clock.json"),
        serde_json::to_vec(&forged).unwrap(),
    )
    .unwrap();
    let output = workspace.run(&["--replay", "forged-clock.json", "--output", "forged"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("provenance differs"));
    assert!(!workspace.0.join("forged").exists());
}
