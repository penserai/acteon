//! Model identity and artifact governance for deterministic inference.
//!
//! A [`ModelLock`] binds a runtime package set, immutable model revision,
//! artifact manifest, and named inference contracts. Consumers can reject
//! drift before sending inference traffic and record the verified lock digest
//! alongside each decision.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The model-lock schema version supported by this crate.
pub const MODEL_LOCK_SCHEMA_VERSION: u16 = 1;

/// An immutable model identity.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelIdentity {
    /// Registry or repository containing the model.
    pub repository: String,
    /// Name exposed by the inference runtime.
    pub name: String,
    /// Immutable repository revision or content identifier.
    pub revision: String,
}

/// A named, content-addressed inference contract.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockedContract {
    /// Path relative to the lock file's application root.
    pub path: PathBuf,
    /// Lowercase SHA-256 digest prefixed with `sha256:`.
    pub digest: String,
}

/// Portable policy for a governed model deployment.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelLock {
    /// Lock-file schema version.
    pub schema_version: u16,
    /// Exact runtime package or component versions.
    pub runtime: BTreeMap<String, String>,
    /// Approved model identity.
    pub model: ModelIdentity,
    /// Model-relative artifact paths and their SHA-256 digests.
    pub artifacts: BTreeMap<PathBuf, String>,
    /// Named inference contracts and their application-relative files.
    pub contracts: BTreeMap<String, LockedContract>,
}

/// A parsed and structurally validated model lock with its content digest.
#[derive(Debug, Clone, Serialize)]
pub struct VerifiedModelLock {
    /// SHA-256 digest of the exact lock bytes.
    pub lock_digest: String,
    /// Validated lock policy.
    #[serde(flatten)]
    pub policy: ModelLock,
}

/// Errors raised while loading or enforcing a model lock.
#[derive(Debug, Error)]
pub enum ModelGovernanceError {
    /// A governed file could not be read.
    #[error("model governance I/O failed for {path}: {source}")]
    Io {
        /// File involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The lock file was not valid JSON.
    #[error("model governance JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// The lock uses a schema this crate cannot enforce.
    #[error("unsupported model lock schema version {0}")]
    UnsupportedSchema(u16),
    /// A required field was empty.
    #[error("model lock contains an empty value for {0}")]
    EmptyValue(String),
    /// A path was absolute or could escape its governed root.
    #[error("model lock path is not a safe relative path: {0}")]
    UnsafePath(PathBuf),
    /// A digest was not a canonical SHA-256 value.
    #[error("model lock digest for {0} is not a lowercase SHA-256 value")]
    InvalidDigest(String),
    /// A requested model differs from the approved model.
    #[error("requested model {actual} does not match approved model {expected}")]
    ModelMismatch {
        /// Approved model name.
        expected: String,
        /// Requested model name.
        actual: String,
    },
    /// Installed runtime components differ from the lock.
    #[error("installed runtime components differ from the model lock")]
    RuntimeMismatch {
        /// Versions required by the lock.
        expected: BTreeMap<String, String>,
        /// Versions reported by the runtime.
        actual: BTreeMap<String, String>,
    },
    /// Files in a governed directory differ from the lock manifest.
    #[error("files under {root} differ from the model lock")]
    FileSetMismatch {
        /// Governed directory.
        root: PathBuf,
        /// Paths required by the lock.
        expected: BTreeSet<PathBuf>,
        /// Paths found on disk.
        actual: BTreeSet<PathBuf>,
    },
    /// A governed file's content differs from its lock entry.
    #[error("digest mismatch for {path}: expected {expected}, got {actual}")]
    DigestMismatch {
        /// Governed file.
        path: PathBuf,
        /// Digest required by the lock.
        expected: String,
        /// Digest computed from the file.
        actual: String,
    },
    /// The serving runtime loaded a different model set.
    #[error("serving runtime loaded models {actual:?}; policy requires only {expected}")]
    LoadedModelMismatch {
        /// Approved model name.
        expected: String,
        /// Models reported by the runtime.
        actual: Vec<String>,
    },
    /// The serving runtime reported a different revision map.
    #[error("serving runtime revisions differ from the model lock")]
    RevisionSetMismatch,
    /// The serving runtime loaded an unapproved revision.
    #[error("served revision {actual} is not approved revision {expected}")]
    UnapprovedRevision {
        /// Approved revision.
        expected: String,
        /// Served revision.
        actual: String,
    },
}

impl VerifiedModelLock {
    /// Load and structurally validate a JSON model lock.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ModelGovernanceError> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| ModelGovernanceError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }

    /// Parse and structurally validate model-lock bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ModelGovernanceError> {
        let policy: ModelLock = serde_json::from_slice(bytes)?;
        policy.validate()?;
        Ok(Self {
            lock_digest: sha256(bytes),
            policy,
        })
    }

    /// Require a caller-selected model to match the lock.
    pub fn verify_model(&self, requested: &str) -> Result<(), ModelGovernanceError> {
        if requested != self.policy.model.name {
            return Err(ModelGovernanceError::ModelMismatch {
                expected: self.policy.model.name.clone(),
                actual: requested.to_owned(),
            });
        }
        Ok(())
    }

    /// Require the installed runtime map to equal the locked map exactly.
    pub fn verify_runtime(
        &self,
        installed: &BTreeMap<String, String>,
    ) -> Result<(), ModelGovernanceError> {
        if installed != &self.policy.runtime {
            return Err(ModelGovernanceError::RuntimeMismatch {
                expected: self.policy.runtime.clone(),
                actual: installed.clone(),
            });
        }
        Ok(())
    }

    /// Verify the exact file set and content of a model artifact directory.
    pub fn verify_artifacts(&self, root: impl AsRef<Path>) -> Result<(), ModelGovernanceError> {
        verify_exact_manifest(root.as_ref(), &self.policy.artifacts)
    }

    /// Load every named contract after verifying its content digest.
    pub fn load_contracts(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<BTreeMap<String, Vec<u8>>, ModelGovernanceError> {
        let root = root.as_ref();
        self.policy
            .contracts
            .iter()
            .map(|(name, contract)| {
                let path = root.join(&contract.path);
                let bytes = fs::read(&path).map_err(|source| ModelGovernanceError::Io {
                    path: path.clone(),
                    source,
                })?;
                verify_digest(&path, &contract.digest, &bytes)?;
                Ok((name.clone(), bytes))
            })
            .collect()
    }

    /// Require the serving runtime to expose only the locked model and revision.
    pub fn verify_served_identity(
        &self,
        loaded: &[String],
        revisions: &BTreeMap<String, String>,
    ) -> Result<(), ModelGovernanceError> {
        let model = &self.policy.model;
        if loaded != [model.name.as_str()] {
            return Err(ModelGovernanceError::LoadedModelMismatch {
                expected: model.name.clone(),
                actual: loaded.to_vec(),
            });
        }
        if revisions.len() != 1 || !revisions.contains_key(&model.name) {
            return Err(ModelGovernanceError::RevisionSetMismatch);
        }
        let actual = &revisions[&model.name];
        if actual != &model.revision {
            return Err(ModelGovernanceError::UnapprovedRevision {
                expected: model.revision.clone(),
                actual: actual.clone(),
            });
        }
        Ok(())
    }
}

impl ModelLock {
    fn validate(&self) -> Result<(), ModelGovernanceError> {
        if self.schema_version != MODEL_LOCK_SCHEMA_VERSION {
            return Err(ModelGovernanceError::UnsupportedSchema(self.schema_version));
        }
        require_nonempty("model.repository", &self.model.repository)?;
        require_nonempty("model.name", &self.model.name)?;
        require_nonempty("model.revision", &self.model.revision)?;
        if self.runtime.is_empty() {
            return Err(ModelGovernanceError::EmptyValue("runtime".to_owned()));
        }
        if self.artifacts.is_empty() {
            return Err(ModelGovernanceError::EmptyValue("artifacts".to_owned()));
        }
        if self.contracts.is_empty() {
            return Err(ModelGovernanceError::EmptyValue("contracts".to_owned()));
        }
        for (name, version) in &self.runtime {
            require_nonempty("runtime component name", name)?;
            require_nonempty(&format!("runtime.{name}"), version)?;
        }
        for (path, digest) in &self.artifacts {
            validate_relative_path(path)?;
            validate_sha256(&path.display().to_string(), digest)?;
        }
        for (name, contract) in &self.contracts {
            require_nonempty("contract name", name)?;
            validate_relative_path(&contract.path)?;
            validate_sha256(name, &contract.digest)?;
        }
        Ok(())
    }
}

fn verify_exact_manifest(
    root: &Path,
    manifest: &BTreeMap<PathBuf, String>,
) -> Result<(), ModelGovernanceError> {
    let actual = relative_files(root)?;
    let expected = manifest.keys().cloned().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(ModelGovernanceError::FileSetMismatch {
            root: root.to_owned(),
            expected,
            actual,
        });
    }
    for (relative, expected) in manifest {
        let path = root.join(relative);
        let bytes = fs::read(&path).map_err(|source| ModelGovernanceError::Io {
            path: path.clone(),
            source,
        })?;
        verify_digest(&path, expected, &bytes)?;
    }
    Ok(())
}

fn relative_files(root: &Path) -> Result<BTreeSet<PathBuf>, ModelGovernanceError> {
    let mut pending = vec![root.to_owned()];
    let mut files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| ModelGovernanceError::Io {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ModelGovernanceError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|source| ModelGovernanceError::Io {
                    path: path.clone(),
                    source,
                })?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| ModelGovernanceError::UnsafePath(path.clone()))?;
                files.insert(relative.to_owned());
            }
        }
    }
    Ok(files)
}

fn validate_relative_path(path: &Path) -> Result<(), ModelGovernanceError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ModelGovernanceError::UnsafePath(path.to_owned()));
    }
    Ok(())
}

fn require_nonempty(field: &str, value: &str) -> Result<(), ModelGovernanceError> {
    if value.trim().is_empty() {
        return Err(ModelGovernanceError::EmptyValue(field.to_owned()));
    }
    Ok(())
}

fn verify_digest(path: &Path, expected: &str, bytes: &[u8]) -> Result<(), ModelGovernanceError> {
    let actual = sha256(bytes);
    if expected != actual {
        return Err(ModelGovernanceError::DigestMismatch {
            path: path.to_owned(),
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(())
}

fn validate_sha256(name: &str, digest: &str) -> Result<(), ModelGovernanceError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(ModelGovernanceError::InvalidDigest(name.to_owned()));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ModelGovernanceError::InvalidDigest(name.to_owned()));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_json(contract_digest: &str, artifact_digest: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "runtime": {"inference-engine": "1.2.3"},
            "model": {
                "repository": "acme/models",
                "name": "classifier",
                "revision": "0123456789abcdef"
            },
            "artifacts": {"weights.bin": artifact_digest},
            "contracts": {
                "classify": {"path": "contracts/classify.json", "digest": contract_digest}
            }
        }))
        .unwrap()
    }

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "acteon-model-governance-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("contracts")).unwrap();
        fs::create_dir_all(root.join("model")).unwrap();
        root
    }

    #[test]
    fn verifies_runtime_artifacts_contracts_and_served_identity() {
        let root = temporary_root();
        fs::write(root.join("contracts/classify.json"), b"contract").unwrap();
        fs::write(root.join("model/weights.bin"), b"weights").unwrap();
        let lock =
            VerifiedModelLock::from_bytes(&lock_json(&sha256(b"contract"), &sha256(b"weights")))
                .unwrap();

        lock.verify_model("classifier").unwrap();
        lock.verify_runtime(&BTreeMap::from([(
            "inference-engine".to_owned(),
            "1.2.3".to_owned(),
        )]))
        .unwrap();
        lock.verify_artifacts(root.join("model")).unwrap();
        let contracts = lock.load_contracts(&root).unwrap();
        assert_eq!(contracts["classify"], b"contract");
        lock.verify_served_identity(
            &["classifier".to_owned()],
            &BTreeMap::from([("classifier".to_owned(), "0123456789abcdef".to_owned())]),
        )
        .unwrap();
        assert!(lock.lock_digest.starts_with("sha256:"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_runtime_drift_and_extra_artifacts() {
        let root = temporary_root();
        fs::write(root.join("model/weights.bin"), b"weights").unwrap();
        fs::write(root.join("model/unlocked.bin"), b"extra").unwrap();
        let lock =
            VerifiedModelLock::from_bytes(&lock_json(&sha256(b"contract"), &sha256(b"weights")))
                .unwrap();

        assert!(matches!(
            lock.verify_runtime(&BTreeMap::new()),
            Err(ModelGovernanceError::RuntimeMismatch { .. })
        ));
        assert!(matches!(
            lock.verify_artifacts(root.join("model")),
            Err(ModelGovernanceError::FileSetMismatch { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_contract_tampering_and_path_traversal() {
        let root = temporary_root();
        fs::write(root.join("contracts/classify.json"), b"changed").unwrap();
        let lock =
            VerifiedModelLock::from_bytes(&lock_json(&sha256(b"contract"), &sha256(b"weights")))
                .unwrap();
        assert!(matches!(
            lock.load_contracts(&root),
            Err(ModelGovernanceError::DigestMismatch { .. })
        ));

        let unsafe_lock = String::from_utf8(lock_json(&sha256(b"contract"), &sha256(b"weights")))
            .unwrap()
            .replace("contracts/classify.json", "../outside.json");
        assert!(matches!(
            VerifiedModelLock::from_bytes(unsafe_lock.as_bytes()),
            Err(ModelGovernanceError::UnsafePath(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
