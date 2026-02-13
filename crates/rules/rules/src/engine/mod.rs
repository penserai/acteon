pub mod builtins;
pub mod context;
pub mod eval;
pub mod executor;
pub mod ops_event;
pub mod ops_semantic;
pub mod ops_state;
pub mod trace;
pub mod value;
pub mod verdict;

pub use context::{AccessTracker, EmbeddingEvalSupport, EvalContext, SemanticMatchDetail};
pub use executor::RuleEngine;
pub use value::Value;
pub use verdict::RuleVerdict;
