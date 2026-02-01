//! CEL (Common Expression Language) frontend for Acteon rules.
//!
//! This crate provides a [`CelFrontend`] that parses YAML rule files
//! where the `condition` field is a CEL expression string, compiling
//! it to the `Expr` IR defined in `acteon-rules`.

pub mod frontend;
pub mod parser;

pub use frontend::CelFrontend;
pub use parser::parse_cel_expr;
