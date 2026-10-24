//! Bounded CEL compilation and a backend-independent search predicate.
//!
//! This is a compiler, not a per-row CEL interpreter. Only indexed operations
//! are accepted. Presence-aware truth pairs preserve missing-field behavior
//! under negation and Boolean composition.
use acteon_rules::ir::expr::{BinaryOp, Expr, UnaryOp};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::search::{SearchFields, exact, keyed_token, tokens};
use crate::{Error, Result, TextMode, TextSearch, TimelineEvent, TimelineQuery};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Field {
    Group,
    Kind,
    Source,
    Id,
    Exact,
    Paths,
    Tokens,
    KeyedTokens,
}
impl Field {
    pub(crate) fn column(self) -> &'static str {
        match self {
            Self::Group => "event_group",
            Self::Kind => "event_type",
            Self::Source => "source",
            Self::Id => "event_id",
            Self::Exact => "exact",
            Self::Paths => "paths",
            Self::Tokens => "tokens",
            Self::KeyedTokens => "keyed_tokens",
        }
    }
    pub(crate) fn is_array(self) -> bool {
        matches!(
            self,
            Self::Exact | Self::Paths | Self::Tokens | Self::KeyedTokens
        )
    }
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Comparison {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}
impl Comparison {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
    pub(crate) fn elastic(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Lt => "lt",
            Self::Le => "lte",
            Self::Gt => "gt",
            Self::Ge => "gte",
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) enum Predicate {
    Constant(bool),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Term(Field, String),
    Time(Comparison, DateTime<Utc>),
}
impl Predicate {
    fn not(self) -> Self {
        Self::Not(Box::new(self))
    }
    pub(crate) fn matches(&self, event: &TimelineEvent, fields: &SearchFields) -> bool {
        match self {
            Self::Constant(value) => *value,
            Self::All(nodes) => nodes.iter().all(|n| n.matches(event, fields)),
            Self::Any(nodes) => nodes.iter().any(|n| n.matches(event, fields)),
            Self::Not(node) => !node.matches(event, fields),
            Self::Time(op, value) => match op {
                Comparison::Eq => event.event.occurred_at == *value,
                Comparison::Lt => event.event.occurred_at < *value,
                Comparison::Le => event.event.occurred_at <= *value,
                Comparison::Gt => event.event.occurred_at > *value,
                Comparison::Ge => event.event.occurred_at >= *value,
            },
            Self::Term(field, value) => match field {
                Field::Group => event.event.event_group == *value,
                Field::Kind => event.event.event_type == *value,
                Field::Source => event.source == *value,
                Field::Id => event.event_id == *value,
                Field::Exact => fields.exact.contains(value),
                Field::Paths => fields.paths.contains(value),
                Field::Tokens => fields.tokens.contains(value),
                Field::KeyedTokens => fields.keyed_tokens.contains(value),
            },
        }
    }
}
fn invalid(message: &str) -> Error {
    Error::Invalid(format!("CEL filter: {message}"))
}

// Separate namespaces in existing keyword arrays keep scalar equality distinct
// from the legacy flattened array membership index, without dynamic mappings.
fn cel_exact(kind: &str, path: &str, value: &Value) -> String {
    crate::digest(&("cel-v1", kind, path, value))
}
fn cel_path(path: &str) -> String {
    format!("cel-v1:{path}")
}
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}
pub(crate) fn index_attributes(value: &Value, path: &str, fields: &mut SearchFields) {
    fields.paths.push(cel_path(path));
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                index_attributes(value, &format!("{path}/{}", escape(key)), fields);
            }
        }
        Value::Array(values) => {
            fields.paths.push(format!("cel-array:{path}"));
            for value in values.iter().filter(|v| !v.is_array() && !v.is_object()) {
                fields.exact.push(cel_exact("member", path, value));
            }
        }
        scalar => fields.exact.push(cel_exact("scalar", path, scalar)),
    }
}

struct Truth {
    yes: Predicate,
    no: Predicate,
}
impl Truth {
    fn total(yes: Predicate) -> Self {
        Self {
            no: yes.clone().not(),
            yes,
        }
    }
    fn present(yes: Predicate, presence: Predicate) -> Self {
        Self {
            no: Predicate::All(vec![presence, yes.clone().not()]),
            yes,
        }
    }
    fn negate(self) -> Self {
        Self {
            yes: self.no,
            no: self.yes,
        }
    }
}
enum Reference {
    Attribute(String),
    Field(Field),
    Time,
}
fn reference(expr: &Expr) -> Result<Reference> {
    let mut node = expr;
    let mut segments = Vec::new();
    loop {
        match node {
            Expr::Field(base, name) => {
                segments.push(name.as_str());
                node = base;
            }
            Expr::Index(base, index) => {
                let Expr::String(name) = index.as_ref() else {
                    return Err(invalid("attribute subscripts must be constant strings"));
                };
                segments.push(name.as_str());
                node = base;
            }
            Expr::Ident(name) => {
                if name == "attributes" && !segments.is_empty() {
                    segments.reverse();
                    return Ok(Reference::Attribute(format!(
                        "/{}",
                        segments
                            .into_iter()
                            .map(escape)
                            .collect::<Vec<_>>()
                            .join("/")
                    )));
                }
                if !segments.is_empty() {
                    return Err(invalid("only attributes supports field traversal"));
                }
                return match name.as_str() {
                    "eventGroup" => Ok(Reference::Field(Field::Group)),
                    "eventType" => Ok(Reference::Field(Field::Kind)),
                    "source" => Ok(Reference::Field(Field::Source)),
                    "eventId" => Ok(Reference::Field(Field::Id)),
                    "occurredAt" => Ok(Reference::Time),
                    _ => Err(invalid(
                        "unknown field; use eventGroup, eventType, source, eventId, occurredAt, or attributes",
                    )),
                };
            }
            _ => return Err(invalid("expected a searchable field")),
        }
    }
}
fn scalar(expr: &Expr) -> Result<Value> {
    match expr {
        Expr::Null => Ok(Value::Null),
        Expr::Bool(v) => Ok((*v).into()),
        Expr::Int(v) => Ok((*v).into()),
        Expr::String(v) => Ok(v.clone().into()),
        Expr::Float(v) if v.is_finite() => Ok(Value::from(*v)),
        Expr::Unary(UnaryOp::Neg, value) => match scalar(value)? {
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.checked_neg()
                        .map(Value::from)
                        .ok_or_else(|| invalid("integer overflow"))
                } else {
                    n.as_f64()
                        .map(|v| Value::from(-v))
                        .ok_or_else(|| invalid("invalid number"))
                }
            }
            _ => Err(invalid("negation requires a numeric literal")),
        },
        _ => Err(invalid("comparison requires a scalar literal")),
    }
}
fn timestamp(expr: &Expr) -> Result<DateTime<Utc>> {
    if let Expr::Call(name, args) = expr
        && name == "timestamp"
        && let [Expr::String(value)] = args.as_slice()
    {
        let time = DateTime::parse_from_rfc3339(value)
            .map_err(|_| invalid("timestamp requires an RFC3339 string"))?
            .with_timezone(&Utc);
        if time.timestamp_nanos_opt().is_some_and(|n| n >= 0) {
            return Ok(time);
        }
    }
    Err(invalid(
        "occurredAt requires timestamp(\"RFC3339\") in the 1970–2262 range",
    ))
}
fn equality(field: &Reference, value: &Expr) -> Result<Truth> {
    Ok(match field {
        Reference::Attribute(path) => Truth::present(
            Predicate::Term(Field::Exact, cel_exact("scalar", path, &scalar(value)?)),
            Predicate::Term(Field::Paths, cel_path(path)),
        ),
        Reference::Field(field) => {
            let Expr::String(value) = value else {
                return Err(invalid(
                    "infrastructure string fields require a string literal",
                ));
            };
            Truth::total(Predicate::Term(*field, value.clone()))
        }
        Reference::Time => Truth::total(Predicate::Time(Comparison::Eq, timestamp(value)?)),
    })
}
fn text_predicate(text: &TextSearch) -> Predicate {
    let nodes = tokens(&text.text)
        .into_iter()
        .map(|token| {
            text.path.as_ref().map_or_else(
                || Predicate::Term(Field::Tokens, token.clone()),
                |path| Predicate::Term(Field::KeyedTokens, keyed_token(path, &token)),
            )
        })
        .collect();
    match text.mode {
        TextMode::All => Predicate::All(nodes),
        TextMode::Any => Predicate::Any(nodes),
    }
}
fn text_call(name: &str, args: &[Expr]) -> Result<Truth> {
    let (path, value) = match args {
        [Expr::String(value)] => (None, value),
        [field, Expr::String(value)] => {
            let Reference::Attribute(path) = reference(field)? else {
                return Err(invalid("scoped text search requires an attribute"));
            };
            (Some(path), value)
        }
        _ => {
            return Err(invalid(
                "textMatch/textAny takes a string, or an attribute and a string",
            ));
        }
    };
    if tokens(value).is_empty() {
        return Err(invalid("text search requires searchable words"));
    }
    Ok(Truth::total(text_predicate(&TextSearch {
        path,
        text: value.clone(),
        mode: if name == "textMatch" {
            TextMode::All
        } else {
            TextMode::Any
        },
    })))
}
// Bound the entire parsed tree before compiling or cloning subtrees.
fn check_complexity(expr: &Expr, count: &mut usize, depth: usize) -> Result<()> {
    *count += 1;
    if *count > 256 || depth > 32 {
        return Err(invalid("expression exceeds 256 nodes or depth 32"));
    }
    match expr {
        Expr::Binary(_, a, b) | Expr::Index(a, b) => {
            check_complexity(a, count, depth + 1)?;
            check_complexity(b, count, depth + 1)?;
        }
        Expr::Unary(_, a) | Expr::Field(a, _) => check_complexity(a, count, depth + 1)?,
        Expr::Call(_, args) | Expr::List(args) => {
            for arg in args {
                check_complexity(arg, count, depth + 1)?;
            }
        }
        Expr::Ident(_)
        | Expr::Null
        | Expr::Bool(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::String(_) => (),
        _ => return Err(invalid("unsupported expression")),
    }
    Ok(())
}
fn compile(expr: &Expr) -> Result<Truth> {
    match expr {
        Expr::Bool(value) => Ok(Truth::total(Predicate::Constant(*value))),
        Expr::Unary(UnaryOp::Not, node) => Ok(compile(node)?.negate()),
        Expr::Binary(op @ (BinaryOp::And | BinaryOp::Or), left, right) => {
            let a = compile(left)?;
            let b = compile(right)?;
            Ok(if matches!(op, BinaryOp::And) {
                Truth {
                    yes: Predicate::All(vec![a.yes, b.yes]),
                    no: Predicate::Any(vec![a.no, b.no]),
                }
            } else {
                Truth {
                    yes: Predicate::Any(vec![a.yes, b.yes]),
                    no: Predicate::All(vec![a.no, b.no]),
                }
            })
        }
        Expr::Binary(op @ (BinaryOp::Eq | BinaryOp::Ne), left, right) => {
            let result = equality(&reference(left)?, right)?;
            Ok(if matches!(op, BinaryOp::Ne) {
                result.negate()
            } else {
                result
            })
        }
        Expr::Binary(
            op @ (BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge),
            left,
            right,
        ) => {
            if !matches!(reference(left)?, Reference::Time) {
                return Err(invalid(
                    "range comparisons are supported only for occurredAt",
                ));
            }
            let op = match op {
                BinaryOp::Lt => Comparison::Lt,
                BinaryOp::Le => Comparison::Le,
                BinaryOp::Gt => Comparison::Gt,
                _ => Comparison::Ge,
            };
            Ok(Truth::total(Predicate::Time(op, timestamp(right)?)))
        }
        Expr::Binary(BinaryOp::In, left, right) => {
            if let Expr::List(values) = right.as_ref() {
                let field = reference(left)?;
                if values.is_empty() {
                    return Ok(Truth::present(
                        Predicate::Constant(false),
                        match &field {
                            Reference::Attribute(path) => {
                                Predicate::Term(Field::Paths, cel_path(path))
                            }
                            Reference::Field(_) | Reference::Time => Predicate::Constant(true),
                        },
                    ));
                }
                let tests = values
                    .iter()
                    .map(|value| equality(&field, value))
                    .collect::<Result<Vec<_>>>()?;
                let (yes, no) = tests.into_iter().map(|t| (t.yes, t.no)).unzip();
                Ok(Truth {
                    yes: Predicate::Any(yes),
                    no: Predicate::All(no),
                })
            } else {
                let Reference::Attribute(path) = reference(right)? else {
                    return Err(invalid(
                        "membership requires a literal list or attribute array",
                    ));
                };
                Ok(Truth::present(
                    Predicate::Term(Field::Exact, cel_exact("member", &path, &scalar(left)?)),
                    Predicate::Term(Field::Paths, format!("cel-array:{path}")),
                ))
            }
        }
        Expr::Call(name, args) if name == "has" => {
            let [field] = args.as_slice() else {
                return Err(invalid("has requires one attribute"));
            };
            let Reference::Attribute(path) = reference(field)? else {
                return Err(invalid("has requires an attribute"));
            };
            Ok(Truth::total(Predicate::Term(Field::Paths, cel_path(&path))))
        }
        Expr::Call(name, args) if name == "textMatch" || name == "textAny" => text_call(name, args),
        _ => Err(invalid("unsupported operator or function")),
    }
}

// The shared rules parser preserves unrecognized escapes literally. A query
// compiler must reject those instead of silently changing CEL string meaning.
fn validate_strings(filter: &str) -> Result<()> {
    let mut quoted = false;
    let mut chars = filter.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' if quoted => {
                if !matches!(chars.next(), Some('n' | 't' | '\\' | '"')) {
                    return Err(invalid(
                        "unsupported string escape; use literal Unicode or escaped quotes, backslashes, n, t",
                    ));
                }
            }
            c if quoted && c.is_control() => {
                return Err(invalid("control characters in strings must be escaped"));
            }
            _ => (),
        }
    }
    Ok(())
}

pub(crate) fn prepare(query: &TimelineQuery) -> Result<Predicate> {
    let mut nodes = Vec::new();
    for (field, value) in [
        (Field::Group, &query.event_group),
        (Field::Kind, &query.event_type),
    ] {
        if let Some(value) = value {
            nodes.push(Predicate::Term(field, value.clone()));
        }
    }
    if let Some(from) = query.from {
        nodes.push(Predicate::Time(Comparison::Ge, from));
    }
    if let Some(to) = query.to {
        nodes.push(Predicate::Time(Comparison::Le, to));
    }
    nodes.extend(
        query
            .equals
            .iter()
            .map(|e| Predicate::Term(Field::Exact, exact(&e.path, &e.value))),
    );
    nodes.extend(
        query
            .exists
            .iter()
            .map(|path| Predicate::Term(Field::Paths, path.clone())),
    );
    if let Some(text) = &query.text {
        nodes.push(text_predicate(text));
    }
    if let Some(filter) = &query.filter {
        if filter.len() > 4096 {
            return Err(invalid("expression exceeds 4096 bytes"));
        }
        validate_strings(filter)?;
        let expr = acteon_rules_cel::parse_cel_expr(filter).map_err(|e| invalid(&e.to_string()))?;
        check_complexity(&expr, &mut 0, 0)?;
        nodes.push(compile(&expr)?.yes);
    }
    Ok(Predicate::All(nodes))
}
