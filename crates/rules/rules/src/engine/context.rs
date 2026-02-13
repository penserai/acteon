use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use acteon_core::Action;
use acteon_state::StateStore;

use crate::error::RuleError;

/// Keys that were actually accessed during rule evaluation.
#[derive(Debug, Default)]
struct AccessedKeys {
    env_keys: HashSet<String>,
    state_keys: HashSet<String>,
}

/// Tracks which environment and state keys are accessed during evaluation.
///
/// Shared via `Arc` across timezone-overridden context copies so that a single
/// tracker captures accesses from all rules in the trace.
#[derive(Debug, Default)]
pub struct AccessTracker {
    inner: Mutex<AccessedKeys>,
    /// Last semantic match detail captured during evaluation.
    pub(crate) last_semantic: Mutex<Option<SemanticMatchDetail>>,
}

impl AccessTracker {
    /// Record that an environment key was accessed.
    pub fn record_env_key(&self, key: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.env_keys.insert(key.to_owned());
        }
    }

    /// Record that a state key was accessed.
    pub fn record_state_key(&self, key: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.state_keys.insert(key.to_owned());
        }
    }

    /// Drain the tracked environment keys into a sorted `Vec`.
    pub fn drain_env_keys(&self) -> Vec<String> {
        if let Ok(mut guard) = self.inner.lock() {
            let mut keys: Vec<String> = guard.env_keys.drain().collect();
            keys.sort();
            keys
        } else {
            Vec::new()
        }
    }

    /// Drain the tracked state keys into a sorted `Vec`.
    pub fn drain_state_keys(&self) -> Vec<String> {
        if let Ok(mut guard) = self.inner.lock() {
            let mut keys: Vec<String> = guard.state_keys.drain().collect();
            keys.sort();
            keys
        } else {
            Vec::new()
        }
    }

    /// Take the last captured semantic match detail, if any.
    pub fn take_semantic_detail(&self) -> Option<SemanticMatchDetail> {
        self.last_semantic.lock().ok().and_then(|mut g| g.take())
    }

    /// Store a semantic match detail.
    pub fn set_semantic_detail(&self, detail: SemanticMatchDetail) {
        if let Ok(mut guard) = self.last_semantic.lock() {
            *guard = Some(detail);
        }
    }
}

/// Detail about a semantic match evaluation, used for explainability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SemanticMatchDetail {
    /// The text that was extracted and compared.
    pub extracted_text: String,
    /// The topic the text was compared against.
    pub topic: String,
    /// The computed similarity score.
    pub similarity: f64,
    /// The threshold that was configured on the rule.
    pub threshold: f64,
}

/// Trait for providing embedding-based similarity evaluation.
///
/// Implementations compute the cosine similarity between a text string and a
/// topic description using vector embeddings. The trait is defined in
/// `acteon-rules` so the rule engine can call it without depending on any
/// specific embedding provider.
#[async_trait]
pub trait EmbeddingEvalSupport: Send + Sync + std::fmt::Debug {
    /// Compute the similarity between `text` and `topic`.
    ///
    /// Returns a value in `[0.0, 1.0]` where higher means more similar.
    async fn similarity(&self, text: &str, topic: &str) -> Result<f64, RuleError>;
}

/// The evaluation context supplied to the rule engine when evaluating expressions.
///
/// It provides access to the action being evaluated, the state store for
/// stateful lookups (counters, dedup, etc.), environment variables, and the
/// current timestamp.
pub struct EvalContext<'a> {
    /// The action being evaluated.
    pub action: &'a Action,
    /// The state store for stateful rule conditions.
    pub state: &'a dyn StateStore,
    /// Environment variables and external configuration.
    pub environment: &'a HashMap<String, String>,
    /// The current timestamp for time-based evaluations.
    pub now: DateTime<Utc>,
    /// Optional embedding support for semantic matching.
    pub embedding: Option<Arc<dyn EmbeddingEvalSupport>>,
    /// Optional timezone for evaluating `time.*` fields in local time.
    ///
    /// When `None`, `time.*` fields use UTC (backward-compatible default).
    pub timezone: Option<Tz>,
    /// Lazily cached `time.*` map so that multiple rules sharing the same
    /// context only allocate one `HashMap`.  Uses `OnceLock` (not `OnceCell`)
    /// because `&EvalContext` is held across `.await` points, requiring `Sync`.
    pub(crate) time_map_cache: OnceLock<super::value::Value>,
    /// Optional tracker for recording which keys are accessed during evaluation.
    ///
    /// Only populated in playground/trace mode to avoid overhead in the hot path.
    pub access_tracker: Option<Arc<AccessTracker>>,
}

impl<'a> EvalContext<'a> {
    /// Create a new evaluation context.
    pub fn new(
        action: &'a Action,
        state: &'a dyn StateStore,
        environment: &'a HashMap<String, String>,
    ) -> Self {
        Self {
            action,
            state,
            environment,
            now: Utc::now(),
            embedding: None,
            timezone: None,
            time_map_cache: OnceLock::new(),
            access_tracker: None,
        }
    }

    /// Create a new evaluation context with a specific timestamp.
    #[must_use]
    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self.time_map_cache = OnceLock::new();
        self
    }

    /// Set the embedding support for semantic matching.
    #[must_use]
    pub fn with_embedding(mut self, embedding: Arc<dyn EmbeddingEvalSupport>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set the timezone for evaluating `time.*` fields.
    #[must_use]
    pub fn with_timezone(mut self, tz: Tz) -> Self {
        self.timezone = Some(tz);
        self.time_map_cache = OnceLock::new();
        self
    }

    /// Set the access tracker for recording key accesses.
    #[must_use]
    pub fn with_access_tracker(mut self, tracker: Arc<AccessTracker>) -> Self {
        self.access_tracker = Some(tracker);
        self
    }
}
