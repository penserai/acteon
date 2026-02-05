use serde::{Deserialize, Serialize};

/// Unary operators supported in rule expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    /// Logical negation (`!expr`).
    Not,
    /// Arithmetic negation (`-expr`).
    Neg,
}

/// Binary operators supported in rule expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    // Arithmetic
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Modulo.
    Mod,

    // Comparison
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Le,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Ge,

    // Logical
    /// Logical AND (short-circuit).
    And,
    /// Logical OR (short-circuit).
    Or,

    // String operations
    /// String contains.
    Contains,
    /// String starts with.
    StartsWith,
    /// String ends with.
    EndsWith,
    /// Regex match.
    Matches,
    /// Membership test (value in collection).
    In,
}

/// The expression AST for rule conditions.
///
/// Expressions are evaluated recursively against an `EvalContext` to produce
/// a runtime `Value`. The tree is designed to be serializable so that rules
/// can be stored and transferred as JSON/YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// The null literal.
    Null,
    /// A boolean literal.
    Bool(bool),
    /// A 64-bit signed integer literal.
    Int(i64),
    /// A 64-bit floating-point literal.
    Float(f64),
    /// A string literal.
    String(String),
    /// A list of expressions.
    List(Vec<Expr>),
    /// A map of string keys to expressions.
    Map(Vec<(String, Expr)>),
    /// A variable reference by name.
    Ident(String),
    /// Field access: `expr.field`.
    Field(Box<Expr>, String),
    /// Index access: `expr[index]`.
    Index(Box<Expr>, Box<Expr>),
    /// A unary operation.
    Unary(UnaryOp, Box<Expr>),
    /// A binary operation.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// A ternary (conditional) expression: `condition ? then : else`.
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// A function call: `name(args...)`.
    Call(String, Vec<Expr>),
    /// All sub-expressions must evaluate to true (logical AND over a list).
    All(Vec<Expr>),
    /// Any sub-expression must evaluate to true (logical OR over a list).
    Any(Vec<Expr>),

    // State access expressions
    /// Retrieve a state value by key pattern.
    StateGet(String),
    /// Retrieve a counter value from the state store.
    StateCounter(String),
    /// Compute the duration since the last state update for the given key.
    StateTimeSince(String),

    // Event state machine expressions (for inhibition)
    /// Check if an active event exists with the given type.
    /// Usage: `HasActiveEvent("cluster_down")` or `HasActiveEvent("cluster_down", action.metadata.cluster)`
    HasActiveEvent {
        /// Event type to check for.
        event_type: String,
        /// Optional label value to match (from action field path).
        label_value: Option<Box<Expr>>,
    },
    /// Get the current state of an event by fingerprint.
    /// Returns the state name (e.g., "open", "closed") or null if not found.
    GetEventState(Box<Expr>),
    /// Check if an event is in a specific state.
    /// Usage: `EventInState(action.fingerprint, "firing")`
    EventInState {
        /// Expression evaluating to the fingerprint.
        fingerprint: Box<Expr>,
        /// The state name to check for.
        state: String,
    },
}

impl Expr {
    /// Returns `true` if this expression is a constant (literal) value.
    pub fn is_constant(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Bool(_) | Self::Int(_) | Self::Float(_) | Self::String(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_constant() {
        assert!(Expr::Null.is_constant());
        assert!(Expr::Bool(true).is_constant());
        assert!(Expr::Int(42).is_constant());
        assert!(Expr::Float(3.14).is_constant());
        assert!(Expr::String("hello".into()).is_constant());
        assert!(!Expr::Ident("x".into()).is_constant());
        assert!(!Expr::StateGet("key".into()).is_constant());
    }

    #[test]
    fn expr_serde_roundtrip() {
        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::Int(1)),
            Box::new(Expr::Int(2)),
        );
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();

        // Verify structure via debug format
        let original = format!("{expr:?}");
        let deserialized = format!("{back:?}");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn complex_expr_construction() {
        // (action.type == "send_email") && (action.payload.priority > 5)
        let expr = Expr::Binary(
            BinaryOp::And,
            Box::new(Expr::Binary(
                BinaryOp::Eq,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "type".into(),
                )),
                Box::new(Expr::String("send_email".into())),
            )),
            Box::new(Expr::Binary(
                BinaryOp::Gt,
                Box::new(Expr::Field(
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("action".into())),
                        "payload".into(),
                    )),
                    "priority".into(),
                )),
                Box::new(Expr::Int(5)),
            )),
        );

        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }

    #[test]
    fn list_and_map_construction() {
        let list = Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)]);
        let map = Expr::Map(vec![
            ("a".into(), Expr::Int(1)),
            ("b".into(), Expr::String("hello".into())),
        ]);

        assert!(!list.is_constant());
        assert!(!map.is_constant());

        let json = serde_json::to_string(&list).unwrap();
        let _: Expr = serde_json::from_str(&json).unwrap();

        let json = serde_json::to_string(&map).unwrap();
        let _: Expr = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn ternary_expr() {
        let expr = Expr::Ternary(
            Box::new(Expr::Bool(true)),
            Box::new(Expr::String("yes".into())),
            Box::new(Expr::String("no".into())),
        );
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }

    #[test]
    fn has_active_event_expr() {
        let expr = Expr::HasActiveEvent {
            event_type: "cluster_down".into(),
            label_value: Some(Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "metadata".into(),
                )),
                "cluster".into(),
            ))),
        };
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }

    #[test]
    fn get_event_state_expr() {
        let expr = Expr::GetEventState(Box::new(Expr::String("fingerprint-123".into())));
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }

    #[test]
    fn event_in_state_expr() {
        let expr = Expr::EventInState {
            fingerprint: Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "fingerprint".into(),
            )),
            state: "firing".into(),
        };
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }
}
