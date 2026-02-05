pub mod engine;
pub mod error;
pub mod frontend;
pub mod ir;

pub use engine::{EvalContext, RuleEngine, RuleVerdict};
pub use error::RuleError;
pub use frontend::RuleFrontend;
pub use ir::expr::Expr;
pub use ir::rule::{Rule, RuleAction, RuleSource};
