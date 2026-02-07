use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::RuleError;

/// Runtime value produced by expression evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// The null value.
    Null,
    /// A boolean value.
    Bool(bool),
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit floating-point number.
    Float(f64),
    /// A UTF-8 string.
    String(String),
    /// An ordered list of values.
    List(Vec<Value>),
    /// A string-keyed map of values.
    Map(HashMap<String, Value>),
}

impl Value {
    /// Convert a `serde_json::Value` into a runtime `Value`.
    pub fn from_json(json: serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Self::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Self::Float(f)
                } else {
                    Self::Null
                }
            }
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Array(arr) => {
                Self::List(arr.into_iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(obj) => Self::Map(
                obj.into_iter()
                    .map(|(k, v)| (k, Self::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// Returns `true` if this value is considered truthy.
    ///
    /// - `Null` is falsy.
    /// - `Bool` is its own truthiness.
    /// - `Int(0)` and `Float(0.0)` are falsy.
    /// - Empty strings, lists, and maps are falsy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::List(v) => !v.is_empty(),
            Self::Map(m) => !m.is_empty(),
        }
    }

    /// Returns a string representation of the value type.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::List(_) => "list",
            Self::Map(_) => "map",
        }
    }

    /// Returns a human-readable display string for the value.
    pub fn display_string(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
            Self::Float(f) => f.to_string(),
            Self::String(s) => s.clone(),
            Self::List(v) => format!("{v:?}"),
            Self::Map(m) => format!("{m:?}"),
        }
    }

    /// Access a field by name on this value (for Map values).
    pub(crate) fn field(&self, name: &str) -> Result<Self, RuleError> {
        match self {
            Self::Map(m) => Ok(m.get(name).cloned().unwrap_or(Self::Null)),
            _ => Err(RuleError::TypeError(format!(
                "cannot access field '{name}' on {}",
                self.type_name()
            ))),
        }
    }

    /// Access an index on this value (for List and Map values).
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    pub(crate) fn index(&self, idx: &Self) -> Result<Self, RuleError> {
        match (self, idx) {
            (Self::List(v), Self::Int(i)) => {
                let index = if *i < 0 {
                    (v.len() as i64 + i) as usize
                } else {
                    *i as usize
                };
                Ok(v.get(index).cloned().unwrap_or(Self::Null))
            }
            (Self::Map(m), Self::String(key)) => Ok(m.get(key).cloned().unwrap_or(Self::Null)),
            _ => Err(RuleError::TypeError(format!(
                "cannot index {} with {}",
                self.type_name(),
                idx.type_name()
            ))),
        }
    }
}
