use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

const LOCK_SCHEMA_VERSION: u16 = 1;
const QUESTION_FILES: [(&str, &str); 4] = [
    ("metrics", "metrics.json"),
    ("traces", "traces.json"),
    ("logs", "logs.json"),
    ("fusion", "fusion.json"),
];
const REQUIRED_RUNTIMES: [&str; 6] = [
    "laya",
    "torch",
    "transformers",
    "huggingface_hub",
    "safetensors",
    "numpy",
];
const ARTIFACT_FILES: [&str; 5] = [
    "encoder/config.json",
    "model.safetensors",
    "rl_agent_config.json",
    "tokenizer/tokenizer.json",
    "tokenizer/tokenizer_config.json",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelLock {
    schema_version: u16,
    runtime: BTreeMap<String, String>,
    checkpoint: CheckpointLock,
    artifacts: BTreeMap<String, String>,
    question_sets: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointLock {
    repository: String,
    name: String,
    revision: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelGovernance {
    pub schema_version: u16,
    pub lock_digest: String,
    pub runtime: BTreeMap<String, String>,
    pub repository: String,
    pub checkpoint: String,
    pub approved_revision: String,
    pub artifacts: BTreeMap<String, String>,
    pub question_sets: BTreeMap<String, String>,
}

pub struct GovernedQuestions {
    pub governance: ModelGovernance,
    pub questions: BTreeMap<String, Value>,
}

#[derive(Debug, Error)]
pub enum GovernanceError {
    #[error("model governance I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("model governance JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported model lock schema version {0}")]
    UnsupportedSchema(u16),
    #[error("model lock checkpoint {actual} does not match requested checkpoint {expected}")]
    CheckpointMismatch { expected: String, actual: String },
    #[error("model lock runtime keys do not match the required set")]
    RuntimeSetMismatch,
    #[error("model lock question-set files do not match the required set")]
    QuestionSetMismatch,
    #[error("model lock artifact files do not match the checkpoint manifest")]
    ArtifactSetMismatch,
    #[error("model lock digest for {0} is not a lowercase SHA-256 value")]
    InvalidDigest(String),
    #[error("model lock contains an empty value for {0}")]
    EmptyValue(String),
    #[error("question set {file} digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("Laya loaded checkpoints {actual:?}; policy requires only {expected}")]
    LoadedModelMismatch {
        expected: String,
        actual: Vec<String>,
    },
    #[error("Laya reported unexpected checkpoint revisions")]
    RevisionSetMismatch,
    #[error("Laya revision {actual} is not the approved revision {expected}")]
    UnapprovedRevision { expected: String, actual: String },
}

impl ModelGovernance {
    pub fn verify_health(
        &self,
        loaded: &[String],
        revisions: &BTreeMap<String, String>,
    ) -> Result<(), GovernanceError> {
        if loaded.len() != 1 || loaded[0] != self.checkpoint {
            return Err(GovernanceError::LoadedModelMismatch {
                expected: self.checkpoint.clone(),
                actual: loaded.to_vec(),
            });
        }
        if revisions.len() != 1 || !revisions.contains_key(&self.checkpoint) {
            return Err(GovernanceError::RevisionSetMismatch);
        }
        let actual = &revisions[&self.checkpoint];
        if actual != &self.approved_revision {
            return Err(GovernanceError::UnapprovedRevision {
                expected: self.approved_revision.clone(),
                actual: actual.clone(),
            });
        }
        Ok(())
    }
}

pub fn load_governed_questions(
    example_root: &Path,
    expected_checkpoint: &str,
) -> Result<GovernedQuestions, GovernanceError> {
    let lock_bytes = fs::read(example_root.join("model.lock.json"))?;
    let model_lock: ModelLock = serde_json::from_slice(&lock_bytes)?;
    if model_lock.schema_version != LOCK_SCHEMA_VERSION {
        return Err(GovernanceError::UnsupportedSchema(
            model_lock.schema_version,
        ));
    }
    if model_lock.checkpoint.name != expected_checkpoint {
        return Err(GovernanceError::CheckpointMismatch {
            expected: expected_checkpoint.to_owned(),
            actual: model_lock.checkpoint.name,
        });
    }

    let runtime_keys = model_lock
        .runtime
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if runtime_keys != BTreeSet::from(REQUIRED_RUNTIMES) {
        return Err(GovernanceError::RuntimeSetMismatch);
    }
    for (name, version) in &model_lock.runtime {
        require_nonempty(&format!("runtime.{name}"), version)?;
    }
    require_nonempty("checkpoint.repository", &model_lock.checkpoint.repository)?;
    require_nonempty("checkpoint.revision", &model_lock.checkpoint.revision)?;

    let artifact_files = model_lock
        .artifacts
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if artifact_files != BTreeSet::from(ARTIFACT_FILES) {
        return Err(GovernanceError::ArtifactSetMismatch);
    }
    for (file, digest) in &model_lock.artifacts {
        validate_sha256(file, digest)?;
    }

    let expected_files = QUESTION_FILES
        .iter()
        .map(|(_, file)| *file)
        .collect::<BTreeSet<_>>();
    let locked_files = model_lock
        .question_sets
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if locked_files != expected_files {
        return Err(GovernanceError::QuestionSetMismatch);
    }
    for (file, digest) in &model_lock.question_sets {
        validate_sha256(file, digest)?;
    }

    let mut questions = BTreeMap::new();
    for (id, file) in QUESTION_FILES {
        let bytes = fs::read(example_root.join("questions").join(file))?;
        let expected = &model_lock.question_sets[file];
        verify_digest(file, expected, &bytes)?;
        questions.insert(id.to_owned(), serde_json::from_slice(&bytes)?);
    }

    Ok(GovernedQuestions {
        governance: ModelGovernance {
            schema_version: model_lock.schema_version,
            lock_digest: sha256(&lock_bytes),
            runtime: model_lock.runtime,
            repository: model_lock.checkpoint.repository,
            checkpoint: expected_checkpoint.to_owned(),
            approved_revision: model_lock.checkpoint.revision,
            artifacts: model_lock.artifacts,
            question_sets: model_lock.question_sets,
        },
        questions,
    })
}

fn require_nonempty(field: &str, value: &str) -> Result<(), GovernanceError> {
    if value.trim().is_empty() {
        return Err(GovernanceError::EmptyValue(field.to_owned()));
    }
    Ok(())
}

fn verify_digest(file: &str, expected: &str, bytes: &[u8]) -> Result<(), GovernanceError> {
    let actual = sha256(bytes);
    if expected != actual {
        return Err(GovernanceError::DigestMismatch {
            file: file.to_owned(),
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(())
}

fn validate_sha256(name: &str, digest: &str) -> Result<(), GovernanceError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(GovernanceError::InvalidDigest(name.to_owned()));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(GovernanceError::InvalidDigest(name.to_owned()));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/neural-observability-detector")
    }

    #[test]
    fn repository_lock_approves_every_question_set() {
        let governed = load_governed_questions(&example_root(), "typed-decisions").unwrap();
        assert_eq!(governed.questions.len(), 4);
        assert_eq!(governed.governance.artifacts.len(), 5);
        assert_eq!(governed.governance.question_sets.len(), 4);
        assert!(governed.governance.lock_digest.starts_with("sha256:"));
    }

    #[test]
    fn altered_question_set_is_rejected() {
        let error = verify_digest("metrics.json", "sha256:0000", b"changed").unwrap_err();
        assert!(matches!(error, GovernanceError::DigestMismatch { .. }));
    }

    #[test]
    fn unapproved_laya_revision_is_rejected() {
        let governed = load_governed_questions(&example_root(), "typed-decisions").unwrap();
        let revisions = BTreeMap::from([(
            "typed-decisions".to_owned(),
            "unreviewed-revision".to_owned(),
        )]);
        let error = governed
            .governance
            .verify_health(&["typed-decisions".to_owned()], &revisions)
            .unwrap_err();
        assert!(matches!(error, GovernanceError::UnapprovedRevision { .. }));
    }
}
