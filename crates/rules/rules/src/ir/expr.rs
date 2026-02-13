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

    // Semantic matching
    /// Check if a text field semantically matches a topic description.
    ///
    /// Uses vector embeddings and cosine similarity to determine whether the
    /// text is close enough to the topic. Requires an `EmbeddingEvalSupport`
    /// implementation in the evaluation context.
    SemanticMatch {
        /// Topic description to match against (e.g. "Infrastructure issues, server problems").
        topic: String,
        /// Minimum cosine similarity threshold (0.0 to 1.0).
        threshold: f64,
        /// Optional expression resolving to the text to match. When `None`,
        /// the entire action payload is stringified.
        text_field: Option<Box<Expr>>,
    },
}

impl Expr {
    /// Returns a human-readable pseudo-code representation of the expression.
    #[allow(clippy::too_many_lines)]
    pub fn to_source(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
            Self::Float(f) => f.to_string(),
            Self::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
            Self::List(items) => {
                let inner = items
                    .iter()
                    .map(Self::to_source)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            Self::Map(entries) => {
                let inner = entries
                    .iter()
                    .map(|(k, v)| format!("\"{}\": {}", k.replace('"', "\\\""), v.to_source()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{inner}}}")
            }
            Self::Ident(name) => name.clone(),
            Self::Field(base, field) => format!("{}.{}", base.to_source(), field),
            Self::Index(base, index) => format!("{}[{}]", base.to_source(), index.to_source()),
            Self::Unary(op, expr) => {
                let symbol = match op {
                    UnaryOp::Not => "!",
                    UnaryOp::Neg => "-",
                };
                format!("{}{}", symbol, expr.to_source())
            }
            Self::Binary(op, lhs, rhs) => {
                let symbol = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "%",
                    BinaryOp::Eq => "==",
                    BinaryOp::Ne => "!=",
                    BinaryOp::Lt => "<",
                    BinaryOp::Le => "<=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Ge => ">=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                    BinaryOp::Contains => "contains",
                    BinaryOp::StartsWith => "starts_with",
                    BinaryOp::EndsWith => "ends_with",
                    BinaryOp::Matches => "matches",
                    BinaryOp::In => "in",
                };
                format!("({} {} {})", lhs.to_source(), symbol, rhs.to_source())
            }
            Self::Ternary(cond, then, els) => format!(
                "({} ? {} : {})",
                cond.to_source(),
                then.to_source(),
                els.to_source()
            ),
            Self::Call(name, args) => {
                let inner = args
                    .iter()
                    .map(Self::to_source)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({inner})")
            }
            Self::All(exprs) => {
                let inner = exprs
                    .iter()
                    .map(Self::to_source)
                    .collect::<Vec<_>>()
                    .join(" && ");
                format!("all({inner})")
            }
            Self::Any(exprs) => {
                let inner = exprs
                    .iter()
                    .map(Self::to_source)
                    .collect::<Vec<_>>()
                    .join(" || ");
                format!("any({inner})")
            }
            Self::StateGet(key) => format!("StateGet(\"{key}\")"),
            Self::StateCounter(key) => format!("StateCounter(\"{key}\")"),
            Self::StateTimeSince(key) => format!("StateTimeSince(\"{key}\")"),
            Self::HasActiveEvent {
                event_type,
                label_value,
            } => match label_value {
                Some(v) => {
                    let v_src = v.to_source();
                    format!("HasActiveEvent(\"{event_type}\", {v_src})")
                }
                None => format!("HasActiveEvent(\"{event_type}\")"),
            },
            Self::GetEventState(fp) => {
                let fp_src = fp.to_source();
                format!("GetEventState({fp_src})")
            }
            Self::EventInState { fingerprint, state } => {
                let fp_src = fingerprint.to_source();
                format!("EventInState({fp_src}, \"{state}\")")
            }
            Self::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                let text = text_field
                    .as_ref()
                    .map(|f| {
                        let f_src = f.to_source();
                        format!(", text={f_src}")
                    })
                    .unwrap_or_default();
                format!("SemanticMatch(\"{topic}\", threshold={threshold}{text})")
            }
        }
    }

    /// Returns `true` if this expression is a constant (literal) value.
    pub fn is_constant(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Bool(_) | Self::Int(_) | Self::Float(_) | Self::String(_)
        )
    }

    /// Collect all `SemanticMatch` topic strings from this expression tree.
    ///
    /// Walks the AST recursively and returns every topic used in a
    /// `SemanticMatch` node. Useful for pre-warming the embedding cache
    /// after loading rules.
    pub fn semantic_topics(&self) -> Vec<&str> {
        let mut topics = Vec::new();
        self.collect_semantic_topics(&mut topics);
        topics
    }

    fn collect_semantic_topics<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::SemanticMatch {
                topic, text_field, ..
            } => {
                out.push(topic);
                if let Some(expr) = text_field {
                    expr.collect_semantic_topics(out);
                }
            }
            Self::Binary(_, lhs, rhs) => {
                lhs.collect_semantic_topics(out);
                rhs.collect_semantic_topics(out);
            }
            Self::Unary(_, expr) | Self::Field(expr, _) | Self::GetEventState(expr) => {
                expr.collect_semantic_topics(out);
            }
            Self::Index(expr, idx) => {
                expr.collect_semantic_topics(out);
                idx.collect_semantic_topics(out);
            }
            Self::Ternary(cond, then, els) => {
                cond.collect_semantic_topics(out);
                then.collect_semantic_topics(out);
                els.collect_semantic_topics(out);
            }
            Self::All(exprs) | Self::Any(exprs) | Self::List(exprs) => {
                for e in exprs {
                    e.collect_semantic_topics(out);
                }
            }
            Self::Call(_, args) => {
                for a in args {
                    a.collect_semantic_topics(out);
                }
            }
            Self::Map(pairs) => {
                for (_, v) in pairs {
                    v.collect_semantic_topics(out);
                }
            }
            Self::HasActiveEvent { label_value, .. } => {
                if let Some(expr) = label_value {
                    expr.collect_semantic_topics(out);
                }
            }
            Self::EventInState { fingerprint, .. } => {
                fingerprint.collect_semantic_topics(out);
            }
            // Leaf nodes — no children to visit.
            Self::Null
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::String(_)
            | Self::Ident(_)
            | Self::StateGet(_)
            | Self::StateCounter(_)
            | Self::StateTimeSince(_) => {}
        }
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

    #[test]
    fn semantic_match_expr() {
        let expr = Expr::SemanticMatch {
            topic: "Infrastructure issues".into(),
            threshold: 0.75,
            text_field: Some(Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "payload".into(),
                )),
                "message".into(),
            ))),
        };
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }

    #[test]
    fn semantic_match_without_text_field() {
        let expr = Expr::SemanticMatch {
            topic: "Server outage".into(),
            threshold: 0.8,
            text_field: None,
        };
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{expr:?}"), format!("{back:?}"));
    }

    #[test]
    fn semantic_topics_single() {
        let expr = Expr::SemanticMatch {
            topic: "billing".into(),
            threshold: 0.7,
            text_field: None,
        };
        assert_eq!(expr.semantic_topics(), vec!["billing"]);
    }

    #[test]
    fn semantic_topics_nested_in_binary() {
        let expr = Expr::Binary(
            BinaryOp::Or,
            Box::new(Expr::SemanticMatch {
                topic: "billing".into(),
                threshold: 0.7,
                text_field: None,
            }),
            Box::new(Expr::SemanticMatch {
                topic: "infrastructure".into(),
                threshold: 0.8,
                text_field: None,
            }),
        );
        assert_eq!(expr.semantic_topics(), vec!["billing", "infrastructure"]);
    }

    #[test]
    fn semantic_topics_empty_for_non_semantic() {
        let expr = Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Ident("x".into())),
            Box::new(Expr::Int(42)),
        );
        assert!(expr.semantic_topics().is_empty());
    }
}
