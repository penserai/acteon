use acteon_rules::RuleError;
use acteon_rules::ir::expr::{BinaryOp, Expr};

/// Parse a dotted field path string into a chain of `Expr::Ident` and `Expr::Field`.
///
/// For example, `"action.payload.to"` becomes:
/// ```text
/// Expr::Field(
///     Expr::Field(
///         Expr::Ident("action"),
///         "payload",
///     ),
///     "to",
/// )
/// ```
pub fn parse_field_path(path: &str) -> Result<Expr, RuleError> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() || segments[0].is_empty() {
        return Err(RuleError::Parse(format!("empty field path: '{path}'")));
    }

    let mut expr = Expr::Ident(segments[0].to_owned());
    for segment in &segments[1..] {
        if segment.is_empty() {
            return Err(RuleError::Parse(format!(
                "empty segment in field path: '{path}'"
            )));
        }
        expr = Expr::Field(Box::new(expr), (*segment).to_owned());
    }
    Ok(expr)
}

/// Compile a template string that may contain `{{ ... }}` interpolation markers.
///
/// If the entire string is a single template expression (e.g. `"{{ action.payload.to }}"`),
/// the result is the parsed field path expression directly.
///
/// If the string contains a mix of literal text and template expressions, the result
/// is a chain of `Binary(Add, ...)` operations for string concatenation.
///
/// If the string contains no template markers, the result is `Expr::String(...)`.
pub fn compile_template(input: &str) -> Result<Expr, RuleError> {
    let trimmed = input.trim();

    // Fast path: no template markers at all.
    if !trimmed.contains("{{") {
        return Ok(Expr::String(input.to_owned()));
    }

    let mut parts: Vec<Expr> = Vec::new();
    let mut remaining = input;

    while let Some(start) = remaining.find("{{") {
        // Capture any literal text before the opening marker.
        if start > 0 {
            parts.push(Expr::String(remaining[..start].to_owned()));
        }

        let after_open = &remaining[start + 2..];
        let end = after_open.find("}}").ok_or_else(|| {
            RuleError::Parse("unclosed template expression: missing '}}'".to_owned())
        })?;

        let inner = after_open[..end].trim();
        if inner.is_empty() {
            return Err(RuleError::Parse(
                "empty template expression: '{{ }}'".to_owned(),
            ));
        }

        parts.push(parse_field_path(inner)?);

        // Advance past the closing `}}`.
        remaining = &after_open[end + 2..];
    }

    // Capture any trailing literal text.
    if !remaining.is_empty() {
        parts.push(Expr::String(remaining.to_owned()));
    }

    // If only one part and it is already an expression (not a string literal),
    // return it directly instead of wrapping in concatenation.
    if parts.len() == 1 {
        return Ok(parts.into_iter().next().expect("length checked"));
    }

    // Fold parts into a left-associative chain of Binary::Add for string concatenation.
    let mut iter = parts.into_iter();
    let mut result = iter.next().expect("at least one part");
    for part in iter {
        result = Expr::Binary(BinaryOp::Add, Box::new(result), Box::new(part));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_ident() {
        let expr = parse_field_path("action").unwrap();
        assert!(matches!(expr, Expr::Ident(s) if s == "action"));
    }

    #[test]
    fn parse_two_segments() {
        let expr = parse_field_path("action.payload").unwrap();
        match expr {
            Expr::Field(base, field) => {
                assert_eq!(field, "payload");
                assert!(matches!(*base, Expr::Ident(s) if s == "action"));
            }
            other => panic!("expected Field, got {other:?}"),
        }
    }

    #[test]
    fn parse_three_segments() {
        let expr = parse_field_path("action.payload.to").unwrap();
        match expr {
            Expr::Field(base, field) => {
                assert_eq!(field, "to");
                match *base {
                    Expr::Field(inner_base, inner_field) => {
                        assert_eq!(inner_field, "payload");
                        assert!(matches!(*inner_base, Expr::Ident(s) if s == "action"));
                    }
                    other => panic!("expected nested Field, got {other:?}"),
                }
            }
            other => panic!("expected Field, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_path_errors() {
        assert!(parse_field_path("").is_err());
    }

    #[test]
    fn parse_path_with_empty_segment_errors() {
        assert!(parse_field_path("action..payload").is_err());
    }

    #[test]
    fn template_plain_string() {
        let expr = compile_template("hello world").unwrap();
        assert!(matches!(expr, Expr::String(s) if s == "hello world"));
    }

    #[test]
    fn template_single_expression() {
        let expr = compile_template("{{ action.payload.to }}").unwrap();
        // Should be a Field chain, not wrapped in string concat.
        match expr {
            Expr::Field(_, ref field) => assert_eq!(field, "to"),
            other => panic!("expected Field, got {other:?}"),
        }
    }

    #[test]
    fn template_mixed_literal_and_expression() {
        let expr = compile_template("Hello {{ action.name }}!").unwrap();
        // Should be: Add(Add(String("Hello "), Field(...)), String("!"))
        match expr {
            Expr::Binary(BinaryOp::Add, _, rhs) => {
                assert!(matches!(*rhs, Expr::String(s) if s == "!"));
            }
            other => panic!("expected Binary(Add, ...), got {other:?}"),
        }
    }

    #[test]
    fn template_unclosed_marker_errors() {
        assert!(compile_template("{{ action.name").is_err());
    }

    #[test]
    fn template_empty_expression_errors() {
        assert!(compile_template("{{  }}").is_err());
    }

    #[test]
    fn template_multiple_expressions() {
        let expr = compile_template("{{ action.a }}-{{ action.b }}").unwrap();
        // Should be: Add(Add(Field(..., "a"), String("-")), Field(..., "b"))
        assert!(matches!(expr, Expr::Binary(BinaryOp::Add, _, _)));
    }
}
