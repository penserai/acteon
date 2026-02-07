//! Hand-written recursive descent parser for a CEL expression subset.
//!
//! The parser uses `nom` for low-level token recognition and implements
//! precedence climbing manually. The output is the [`Expr`] IR defined in
//! `acteon-rules`.

use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::{char, multispace0},
    combinator::{opt, recognize},
    multi::separated_list0,
    sequence::{delimited, tuple},
};

use acteon_rules::RuleError;
use acteon_rules::ir::expr::{BinaryOp, Expr, UnaryOp};

/// Parse a complete CEL expression string into an [`Expr`].
///
/// Returns a [`RuleError::Parse`] if the input cannot be parsed or has
/// trailing tokens.
pub fn parse_cel_expr(input: &str) -> Result<Expr, RuleError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(RuleError::Parse("empty expression".to_owned()));
    }
    let (rest, expr) =
        parse_ternary(input).map_err(|e| RuleError::Parse(format!("CEL parse error: {e}")))?;
    let rest = rest.trim();
    if !rest.is_empty() {
        return Err(RuleError::Parse(format!(
            "unexpected trailing input: {rest:?}"
        )));
    }
    Ok(expr)
}

// ---------------------------------------------------------------------------
// Whitespace helper
// ---------------------------------------------------------------------------

/// Consume optional whitespace around a parser.
fn ws<'a, F, O>(inner: F) -> impl FnMut(&'a str) -> IResult<&'a str, O>
where
    F: FnMut(&'a str) -> IResult<&'a str, O>,
{
    delimited(multispace0, inner, multispace0)
}

// ---------------------------------------------------------------------------
// Atoms (literals, identifiers, parenthesised expressions, lists, maps)
// ---------------------------------------------------------------------------

/// Parse an atom: literal, identifier, parenthesised expression, list, or map.
fn parse_atom(input: &str) -> IResult<&str, Expr> {
    let (input, _) = multispace0(input)?;
    alt((
        parse_null,
        parse_bool,
        parse_number,
        parse_string_literal,
        parse_list_literal,
        parse_map_literal,
        parse_paren,
        parse_function_or_ident,
    ))(input)
}

/// Parse the `null` keyword.
fn parse_null(input: &str) -> IResult<&str, Expr> {
    let (rest, _) = tag("null")(input)?;
    // Ensure it's not just a prefix of an identifier.
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((rest, Expr::Null))
}

/// Parse boolean literals `true` and `false`.
fn parse_bool(input: &str) -> IResult<&str, Expr> {
    let (rest, word) = alt((tag("true"), tag("false")))(input)?;
    // Ensure it's not just a prefix of an identifier.
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((rest, Expr::Bool(word == "true")))
}

/// Parse a number literal (integer or float).
fn parse_number(input: &str) -> IResult<&str, Expr> {
    let (rest, num_str) = recognize(tuple((
        opt(char('-')),
        take_while1(|c: char| c.is_ascii_digit()),
        opt(tuple((
            char('.'),
            take_while1(|c: char| c.is_ascii_digit()),
        ))),
    )))(input)?;

    // Do not consume a leading `-` in number parsing when it appears as part
    // of a binary expression such as `5 - 3`.  Number parsing should only
    // match bare numeric literals (without a leading sign); the unary
    // negation operator is handled separately at the unary-expression level.
    // We accept a leading minus only if the parser was called at a position
    // that starts with `-` AND the next character is a digit (i.e. this is
    // not a standalone unary minus before whitespace or an identifier).
    //
    // However, because parse_number is invoked from parse_atom, which is
    // always preceded by whitespace consumption, we should NOT greedily eat
    // a leading minus here -- let the unary layer deal with it.
    // For simplicity: if the source starts with `-`, reject and let the
    // unary handler take care of it.
    if num_str.starts_with('-') {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }

    if num_str.contains('.') {
        let f: f64 = num_str.parse().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Float))
        })?;
        Ok((rest, Expr::Float(f)))
    } else {
        let i: i64 = num_str.parse().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit))
        })?;
        Ok((rest, Expr::Int(i)))
    }
}

/// Parse a double-quoted string literal.
fn parse_string_literal(input: &str) -> IResult<&str, Expr> {
    let (input, _) = char('"')(input)?;
    let mut result = String::new();
    let mut chars = input.chars();
    let mut consumed = 0;
    loop {
        match chars.next() {
            Some('"') => {
                consumed += 1;
                return Ok((&input[consumed..], Expr::String(result)));
            }
            Some('\\') => {
                consumed += 1;
                match chars.next() {
                    Some('n') => {
                        result.push('\n');
                        consumed += 1;
                    }
                    Some('t') => {
                        result.push('\t');
                        consumed += 1;
                    }
                    Some('\\') => {
                        result.push('\\');
                        consumed += 1;
                    }
                    Some('"') => {
                        result.push('"');
                        consumed += 1;
                    }
                    Some(c) => {
                        result.push('\\');
                        result.push(c);
                        consumed += c.len_utf8();
                    }
                    None => {
                        return Err(nom::Err::Error(nom::error::Error::new(
                            input,
                            nom::error::ErrorKind::Char,
                        )));
                    }
                }
            }
            Some(c) => {
                result.push(c);
                consumed += c.len_utf8();
            }
            None => {
                return Err(nom::Err::Error(nom::error::Error::new(
                    input,
                    nom::error::ErrorKind::Char,
                )));
            }
        }
    }
}

/// Parse a list literal: `[expr, expr, ...]`.
fn parse_list_literal(input: &str) -> IResult<&str, Expr> {
    let (input, _) = char('[')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, items) = separated_list0(ws(char(',')), parse_ternary)(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(']')(input)?;
    Ok((input, Expr::List(items)))
}

/// Parse a map literal: `{key: value, ...}`.
///
/// Keys can be either string literals (`"key"`) or bare identifiers (`key`).
fn parse_map_literal(input: &str) -> IResult<&str, Expr> {
    let (input, _) = char('{')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, entries) = separated_list0(ws(char(',')), parse_map_entry)(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char('}')(input)?;
    Ok((input, Expr::Map(entries)))
}

/// Parse a single map entry: `key: value`.
fn parse_map_entry(input: &str) -> IResult<&str, (String, Expr)> {
    let (input, _) = multispace0(input)?;
    let (input, key) = alt((parse_map_key_string, parse_map_key_ident))(input)?;
    let (input, _) = ws(char(':'))(input)?;
    let (input, value) = parse_ternary(input)?;
    Ok((input, (key, value)))
}

/// Parse a string key in a map literal.
fn parse_map_key_string(input: &str) -> IResult<&str, String> {
    let (rest, expr) = parse_string_literal(input)?;
    match expr {
        Expr::String(s) => Ok((rest, s)),
        _ => unreachable!(),
    }
}

/// Parse an identifier key in a map literal.
fn parse_map_key_ident(input: &str) -> IResult<&str, String> {
    let (rest, ident) = parse_ident_str(input)?;
    Ok((rest, ident.to_owned()))
}

/// Parse a parenthesised expression.
fn parse_paren(input: &str) -> IResult<&str, Expr> {
    let (input, _) = char('(')(input)?;
    let (input, _) = multispace0(input)?;
    let (input, expr) = parse_ternary(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(')')(input)?;
    Ok((input, expr))
}

/// Parse a bare identifier string matching `[a-zA-Z_][a-zA-Z0-9_]*`.
fn parse_ident_str(input: &str) -> IResult<&str, &str> {
    let (rest, ident) = recognize(tuple((
        take_while1(|c: char| c.is_ascii_alphabetic() || c == '_'),
        take_while(|c: char| c.is_alphanumeric() || c == '_'),
    )))(input)?;
    Ok((rest, ident))
}

/// Parse a function call or a plain identifier.
///
/// Built-in functions `size`, `int`, and `string` are recognized here as
/// function calls if followed by `(`.
fn parse_function_or_ident(input: &str) -> IResult<&str, Expr> {
    let (rest, ident) = parse_ident_str(input)?;
    let (rest2, _) = multispace0(rest)?;

    // Check if this is a function call.
    if rest2.starts_with('(') {
        let (rest3, _) = char('(')(rest2)?;
        let (rest3, _) = multispace0(rest3)?;
        let (rest3, args) = separated_list0(ws(char(',')), parse_ternary)(rest3)?;
        let (rest3, _) = multispace0(rest3)?;
        let (rest3, _) = char(')')(rest3)?;

        let expr = match ident {
            "size" => {
                // size(expr) -> Call("len", [expr])
                Expr::Call("len".to_owned(), args)
            }
            "int" => Expr::Call("to_int".to_owned(), args),
            "string" => Expr::Call("to_string".to_owned(), args),
            #[allow(clippy::cast_precision_loss)]
            "semantic_match" if args.len() == 2 || args.len() == 3 => {
                let mut args_iter = args.into_iter();
                let topic_expr = args_iter.next().expect("checked length");
                let threshold_expr = args_iter.next().expect("checked length");
                let text_field_expr = args_iter.next();

                let Expr::String(topic) = topic_expr else {
                    let mut full = vec![topic_expr, threshold_expr];
                    full.extend(text_field_expr);
                    return Ok((rest3, Expr::Call("semantic_match".to_owned(), full)));
                };
                let threshold = match threshold_expr {
                    Expr::Float(f) => f,
                    Expr::Int(i) => i as f64,
                    other => {
                        let mut full = vec![Expr::String(topic), other];
                        full.extend(text_field_expr);
                        return Ok((rest3, Expr::Call("semantic_match".to_owned(), full)));
                    }
                };
                Expr::SemanticMatch {
                    topic,
                    threshold,
                    text_field: text_field_expr.map(Box::new),
                }
            }
            _ => Expr::Call(ident.to_owned(), args),
        };
        return Ok((rest3, expr));
    }

    Ok((rest, Expr::Ident(ident.to_owned())))
}

// ---------------------------------------------------------------------------
// Postfix: field access, index access, method calls
// ---------------------------------------------------------------------------

/// Parse postfix operations: `.field`, `[index]`, `.method(args)`.
fn parse_postfix(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut expr) = parse_atom(input)?;

    loop {
        let (next, _) = multispace0(input)?;

        // Field access or method call: `.ident` or `.ident(args)`
        if let Ok((rest, _)) = char::<&str, nom::error::Error<&str>>('.')(next) {
            let (rest, _) = multispace0(rest)?;
            let (rest, field) = parse_ident_str(rest)?;
            let (rest2, _) = multispace0(rest)?;

            // Check for method call.
            if rest2.starts_with('(') {
                let (rest3, _) = char('(')(rest2)?;
                let (rest3, _) = multispace0(rest3)?;
                let (rest3, args) = separated_list0(ws(char(',')), parse_ternary)(rest3)?;
                let (rest3, _) = multispace0(rest3)?;
                let (rest3, _) = char(')')(rest3)?;

                expr = compile_method_call(expr, field, args);
                input = rest3;
                continue;
            }

            // Plain field access.
            expr = Expr::Field(Box::new(expr), field.to_owned());
            input = rest;
            continue;
        }

        // Index access: `[expr]`
        if let Ok((rest, _)) = char::<&str, nom::error::Error<&str>>('[')(next) {
            let (rest, _) = multispace0(rest)?;
            let (rest, index_expr) = parse_ternary(rest)?;
            let (rest, _) = multispace0(rest)?;
            let (rest, _) = char(']')(rest)?;
            expr = Expr::Index(Box::new(expr), Box::new(index_expr));
            input = rest;
            continue;
        }

        input = next;
        break;
    }

    Ok((input, expr))
}

/// Compile a method call into the appropriate Expr node.
fn compile_method_call(receiver: Expr, method: &str, args: Vec<Expr>) -> Expr {
    match method {
        "contains" if args.len() == 1 => Expr::Binary(
            BinaryOp::Contains,
            Box::new(receiver),
            Box::new(args.into_iter().next().expect("checked length")),
        ),
        "startsWith" if args.len() == 1 => Expr::Binary(
            BinaryOp::StartsWith,
            Box::new(receiver),
            Box::new(args.into_iter().next().expect("checked length")),
        ),
        "endsWith" if args.len() == 1 => Expr::Binary(
            BinaryOp::EndsWith,
            Box::new(receiver),
            Box::new(args.into_iter().next().expect("checked length")),
        ),
        "matches" if args.len() == 1 => Expr::Binary(
            BinaryOp::Matches,
            Box::new(receiver),
            Box::new(args.into_iter().next().expect("checked length")),
        ),
        "size" if args.is_empty() => Expr::Call("len".to_owned(), vec![receiver]),
        #[allow(clippy::cast_precision_loss)]
        "semanticMatch" if args.len() == 1 || args.len() == 2 => {
            let mut args_iter = args.into_iter();
            let topic_expr = args_iter.next().expect("checked length");
            let Expr::String(topic) = topic_expr else {
                return Expr::Call("semanticMatch".to_owned(), {
                    let mut full = vec![receiver];
                    full.push(topic_expr);
                    full.extend(args_iter);
                    full
                });
            };
            let threshold = if let Some(t) = args_iter.next() {
                match t {
                    Expr::Float(f) => f,
                    Expr::Int(i) => i as f64,
                    _ => {
                        return Expr::Call("semanticMatch".to_owned(), {
                            let mut full = vec![receiver, Expr::String(topic), t];
                            full.extend(args_iter);
                            full
                        });
                    }
                }
            } else {
                0.8
            };
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field: Some(Box::new(receiver)),
            }
        }
        _ => {
            // Generic method call: receiver becomes the first argument.
            let mut full_args = vec![receiver];
            full_args.extend(args);
            Expr::Call(method.to_owned(), full_args)
        }
    }
}

// ---------------------------------------------------------------------------
// Unary operators
// ---------------------------------------------------------------------------

/// Parse unary operators `!` and `-`.
fn parse_unary(input: &str) -> IResult<&str, Expr> {
    let (input, _) = multispace0(input)?;

    if let Ok((rest, _)) = char::<&str, nom::error::Error<&str>>('!')(input) {
        let (rest, _) = multispace0(rest)?;
        let (rest, operand) = parse_unary(rest)?;
        return Ok((rest, Expr::Unary(UnaryOp::Not, Box::new(operand))));
    }

    // Unary minus: only if followed by a non-space character and NOT a digit
    // directly (numbers handle their own sign). We need to be careful not to
    // confuse unary minus with the binary subtract operator.
    if let Some(after_minus) = input.strip_prefix('-') {
        let after_ws = after_minus.trim_start();
        // This is a unary minus if the next non-whitespace character is:
        // - an alphabetic char or underscore (identifier)
        // - '(' (parenthesised expression)
        // - '!' (nested unary)
        // - a digit (negative number literal)
        // - '-' (nested unary minus)
        if let Some(c) = after_ws.chars().next()
            && (c.is_alphanumeric() || c == '_' || c == '(' || c == '!' || c == '-')
        {
            let (rest, _) = char('-')(input)?;
            let (rest, _) = multispace0(rest)?;
            let (rest, operand) = parse_unary(rest)?;
            return Ok((rest, Expr::Unary(UnaryOp::Neg, Box::new(operand))));
        }
    }

    parse_postfix(input)
}

// ---------------------------------------------------------------------------
// Binary operators with precedence climbing
// ---------------------------------------------------------------------------

/// Precedence level 6: `*`, `/`, `%`
fn parse_mul(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut left) = parse_unary(input)?;

    loop {
        let (next, _) = multispace0(input)?;
        let op = if next.starts_with('*') {
            Some((BinaryOp::Mul, 1))
        } else if next.starts_with('/') {
            Some((BinaryOp::Div, 1))
        } else if next.starts_with('%') {
            Some((BinaryOp::Mod, 1))
        } else {
            None
        };

        if let Some((op, len)) = op {
            let rest = &next[len..];
            let (rest, _) = multispace0(rest)?;
            let (rest, right) = parse_unary(rest)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
            input = rest;
        } else {
            input = next;
            break;
        }
    }

    Ok((input, left))
}

/// Precedence level 5: `+`, `-`
fn parse_add(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut left) = parse_mul(input)?;

    loop {
        let (next, _) = multispace0(input)?;
        let op = if next.starts_with('+') {
            Some((BinaryOp::Add, 1))
        } else if next.starts_with('-') {
            // Distinguish binary minus from unary minus.
            // Binary minus: the `-` follows a completed expression and the
            // next token after `-` is something that can start an expression.
            // We accept it as binary if `-` is not followed by `>` (arrow)
            // and the previous expression was completed (which it always is
            // here since we have `left`).
            Some((BinaryOp::Sub, 1))
        } else {
            None
        };

        if let Some((op, len)) = op {
            let rest = &next[len..];
            let (rest, _) = multispace0(rest)?;
            // For subtraction, the right-hand side should be parsed without
            // consuming a leading unary minus first -- we let parse_unary
            // handle that.
            let (rest, right) = parse_mul(rest)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
            input = rest;
        } else {
            input = next;
            break;
        }
    }

    Ok((input, left))
}

/// Precedence level 4: `<`, `>`, `<=`, `>=`, `in`
fn parse_relational(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut left) = parse_add(input)?;

    loop {
        let (next, _) = multispace0(input)?;
        let op = if next.starts_with("<=") {
            Some((BinaryOp::Le, 2))
        } else if next.starts_with(">=") {
            Some((BinaryOp::Ge, 2))
        } else if next.starts_with('<') {
            Some((BinaryOp::Lt, 1))
        } else if next.starts_with('>') {
            Some((BinaryOp::Gt, 1))
        } else {
            None
        };

        if let Some((op, len)) = op {
            let rest = &next[len..];
            let (rest, _) = multispace0(rest)?;
            let (rest, right) = parse_add(rest)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
            input = rest;
        } else {
            // Check for `in` keyword.
            if let Ok((after_in, _)) = tag::<&str, &str, nom::error::Error<&str>>("in")(next) {
                // Make sure `in` is not a prefix of an identifier.
                if after_in
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                {
                    break;
                }
                let (rest, _) = multispace0(after_in)?;
                let (rest, right) = parse_add(rest)?;
                left = Expr::Binary(BinaryOp::In, Box::new(left), Box::new(right));
                input = rest;
            } else {
                input = next;
                break;
            }
        }
    }

    Ok((input, left))
}

/// Precedence level 3: `==`, `!=`
fn parse_equality(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut left) = parse_relational(input)?;

    loop {
        let (next, _) = multispace0(input)?;
        let op = if next.starts_with("==") {
            Some((BinaryOp::Eq, 2))
        } else if next.starts_with("!=") {
            Some((BinaryOp::Ne, 2))
        } else {
            None
        };

        if let Some((op, len)) = op {
            let rest = &next[len..];
            let (rest, _) = multispace0(rest)?;
            let (rest, right) = parse_relational(rest)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
            input = rest;
        } else {
            input = next;
            break;
        }
    }

    Ok((input, left))
}

/// Precedence level 2: `&&`
fn parse_and(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut left) = parse_equality(input)?;

    loop {
        let (next, _) = multispace0(input)?;
        if let Some(stripped) = next.strip_prefix("&&") {
            let (rest, _) = multispace0(stripped)?;
            let (rest, right) = parse_equality(rest)?;
            left = Expr::Binary(BinaryOp::And, Box::new(left), Box::new(right));
            input = rest;
        } else {
            input = next;
            break;
        }
    }

    Ok((input, left))
}

/// Precedence level 1: `||`
fn parse_or(input: &str) -> IResult<&str, Expr> {
    let (mut input, mut left) = parse_and(input)?;

    loop {
        let (next, _) = multispace0(input)?;
        if let Some(stripped) = next.strip_prefix("||") {
            let (rest, _) = multispace0(stripped)?;
            let (rest, right) = parse_and(rest)?;
            left = Expr::Binary(BinaryOp::Or, Box::new(left), Box::new(right));
            input = rest;
        } else {
            input = next;
            break;
        }
    }

    Ok((input, left))
}

/// Top-level expression: ternary `condition ? then : else`
fn parse_ternary(input: &str) -> IResult<&str, Expr> {
    let (input, cond) = parse_or(input)?;
    let (input, _) = multispace0(input)?;

    if let Ok((rest, _)) = char::<&str, nom::error::Error<&str>>('?')(input) {
        let (rest, _) = multispace0(rest)?;
        let (rest, then_expr) = parse_ternary(rest)?;
        let (rest, _) = multispace0(rest)?;
        let (rest, _) = char(':')(rest)?;
        let (rest, _) = multispace0(rest)?;
        let (rest, else_expr) = parse_ternary(rest)?;
        Ok((
            rest,
            Expr::Ternary(Box::new(cond), Box::new(then_expr), Box::new(else_expr)),
        ))
    } else {
        Ok((input, cond))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Literal tests ---

    #[test]
    fn parse_null_literal() {
        let expr = parse_cel_expr("null").unwrap();
        assert!(matches!(expr, Expr::Null));
    }

    #[test]
    fn parse_bool_true() {
        let expr = parse_cel_expr("true").unwrap();
        assert!(matches!(expr, Expr::Bool(true)));
    }

    #[test]
    fn parse_bool_false() {
        let expr = parse_cel_expr("false").unwrap();
        assert!(matches!(expr, Expr::Bool(false)));
    }

    #[test]
    fn parse_integer() {
        let expr = parse_cel_expr("42").unwrap();
        assert!(matches!(expr, Expr::Int(42)));
    }

    #[test]
    fn parse_zero() {
        let expr = parse_cel_expr("0").unwrap();
        assert!(matches!(expr, Expr::Int(0)));
    }

    #[test]
    fn parse_float() {
        let expr = parse_cel_expr("3.14").unwrap();
        match expr {
            Expr::Float(f) => assert!((f - 3.14).abs() < f64::EPSILON),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn parse_string() {
        let expr = parse_cel_expr(r#""hello world""#).unwrap();
        assert!(matches!(expr, Expr::String(s) if s == "hello world"));
    }

    #[test]
    fn parse_string_with_escapes() {
        let expr = parse_cel_expr(r#""line1\nline2""#).unwrap();
        assert!(matches!(expr, Expr::String(s) if s == "line1\nline2"));
    }

    #[test]
    fn parse_empty_string() {
        let expr = parse_cel_expr(r#""""#).unwrap();
        assert!(matches!(expr, Expr::String(s) if s.is_empty()));
    }

    // --- Identifier tests ---

    #[test]
    fn parse_identifier() {
        let expr = parse_cel_expr("action").unwrap();
        assert!(matches!(expr, Expr::Ident(s) if s == "action"));
    }

    #[test]
    fn parse_identifier_with_underscore() {
        let expr = parse_cel_expr("my_var").unwrap();
        assert!(matches!(expr, Expr::Ident(s) if s == "my_var"));
    }

    // --- Field access ---

    #[test]
    fn parse_field_access() {
        let expr = parse_cel_expr("action.action_type").unwrap();
        match expr {
            Expr::Field(base, field) => {
                assert!(matches!(*base, Expr::Ident(s) if s == "action"));
                assert_eq!(field, "action_type");
            }
            other => panic!("expected Field, got {other:?}"),
        }
    }

    #[test]
    fn parse_nested_field_access() {
        let expr = parse_cel_expr("action.payload.to").unwrap();
        match expr {
            Expr::Field(base, field) => {
                assert_eq!(field, "to");
                match *base {
                    Expr::Field(inner, ref mid) => {
                        assert!(matches!(*inner, Expr::Ident(s) if s == "action"));
                        assert_eq!(mid, "payload");
                    }
                    other => panic!("expected inner Field, got {other:?}"),
                }
            }
            other => panic!("expected Field, got {other:?}"),
        }
    }

    // --- Index access ---

    #[test]
    fn parse_index_access() {
        let expr = parse_cel_expr("items[0]").unwrap();
        match expr {
            Expr::Index(base, index) => {
                assert!(matches!(*base, Expr::Ident(s) if s == "items"));
                assert!(matches!(*index, Expr::Int(0)));
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn parse_string_index_access() {
        let expr = parse_cel_expr(r#"map["key"]"#).unwrap();
        match expr {
            Expr::Index(base, index) => {
                assert!(matches!(*base, Expr::Ident(s) if s == "map"));
                assert!(matches!(*index, Expr::String(s) if s == "key"));
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    // --- Unary operators ---

    #[test]
    fn parse_not_operator() {
        let expr = parse_cel_expr("!true").unwrap();
        match expr {
            Expr::Unary(UnaryOp::Not, inner) => {
                assert!(matches!(*inner, Expr::Bool(true)));
            }
            other => panic!("expected Unary(Not, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_neg_operator() {
        let expr = parse_cel_expr("-42").unwrap();
        match expr {
            Expr::Unary(UnaryOp::Neg, inner) => {
                assert!(matches!(*inner, Expr::Int(42)));
            }
            other => panic!("expected Unary(Neg, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_double_not() {
        let expr = parse_cel_expr("!!false").unwrap();
        match expr {
            Expr::Unary(UnaryOp::Not, inner) => {
                assert!(matches!(*inner, Expr::Unary(UnaryOp::Not, _)));
            }
            other => panic!("expected Unary(Not, Unary(Not, ...)), got {other:?}"),
        }
    }

    // --- Binary operators ---

    #[test]
    fn parse_add() {
        let expr = parse_cel_expr("1 + 2").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Add, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(1)));
                assert!(matches!(*rhs, Expr::Int(2)));
            }
            other => panic!("expected Binary(Add, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_sub() {
        let expr = parse_cel_expr("5 - 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Sub, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(5)));
                assert!(matches!(*rhs, Expr::Int(3)));
            }
            other => panic!("expected Binary(Sub, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_mul() {
        let expr = parse_cel_expr("2 * 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Mul, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(2)));
                assert!(matches!(*rhs, Expr::Int(3)));
            }
            other => panic!("expected Binary(Mul, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_div() {
        let expr = parse_cel_expr("10 / 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Div, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(10)));
                assert!(matches!(*rhs, Expr::Int(3)));
            }
            other => panic!("expected Binary(Div, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_modulo() {
        let expr = parse_cel_expr("10 % 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Mod, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(10)));
                assert!(matches!(*rhs, Expr::Int(3)));
            }
            other => panic!("expected Binary(Mod, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_eq() {
        let expr = parse_cel_expr("x == 5").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Eq, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "x"));
                assert!(matches!(*rhs, Expr::Int(5)));
            }
            other => panic!("expected Binary(Eq, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_ne() {
        let expr = parse_cel_expr("x != 5").unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Ne, _, _)));
    }

    #[test]
    fn parse_lt() {
        let expr = parse_cel_expr("x < 5").unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Lt, _, _)));
    }

    #[test]
    fn parse_le() {
        let expr = parse_cel_expr("x <= 5").unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Le, _, _)));
    }

    #[test]
    fn parse_gt() {
        let expr = parse_cel_expr("x > 5").unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Gt, _, _)));
    }

    #[test]
    fn parse_ge() {
        let expr = parse_cel_expr("x >= 5").unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Ge, _, _)));
    }

    #[test]
    fn parse_and() {
        let expr = parse_cel_expr("a && b").unwrap();
        match expr {
            Expr::Binary(BinaryOp::And, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "a"));
                assert!(matches!(*rhs, Expr::Ident(s) if s == "b"));
            }
            other => panic!("expected Binary(And, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_or() {
        let expr = parse_cel_expr("a || b").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Or, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "a"));
                assert!(matches!(*rhs, Expr::Ident(s) if s == "b"));
            }
            other => panic!("expected Binary(Or, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_in_operator() {
        let expr = parse_cel_expr("x in [1, 2, 3]").unwrap();
        match expr {
            Expr::Binary(BinaryOp::In, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "x"));
                assert!(matches!(*rhs, Expr::List(items) if items.len() == 3));
            }
            other => panic!("expected Binary(In, ...), got {other:?}"),
        }
    }

    // --- Precedence tests ---

    #[test]
    fn precedence_mul_over_add() {
        // 1 + 2 * 3 should be 1 + (2 * 3)
        let expr = parse_cel_expr("1 + 2 * 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Add, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(1)));
                assert!(matches!(*rhs, Expr::Binary(BinaryOp::Mul, _, _)));
            }
            other => panic!("expected Add(1, Mul(2, 3)), got {other:?}"),
        }
    }

    #[test]
    fn precedence_and_over_or() {
        // a || b && c should be a || (b && c)
        let expr = parse_cel_expr("a || b && c").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Or, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "a"));
                assert!(matches!(*rhs, Expr::Binary(BinaryOp::And, _, _)));
            }
            other => panic!("expected Or(a, And(b, c)), got {other:?}"),
        }
    }

    #[test]
    fn precedence_eq_over_and() {
        // a == 1 && b == 2 should be (a == 1) && (b == 2)
        let expr = parse_cel_expr("a == 1 && b == 2").unwrap();
        match expr {
            Expr::Binary(BinaryOp::And, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Binary(BinaryOp::Eq, _, _)));
                assert!(matches!(*rhs, Expr::Binary(BinaryOp::Eq, _, _)));
            }
            other => panic!("expected And(Eq(a, 1), Eq(b, 2)), got {other:?}"),
        }
    }

    #[test]
    fn precedence_comparison_over_eq() {
        // a < 5 == true should be (a < 5) == true
        let expr = parse_cel_expr("a < 5 == true").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Eq, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Binary(BinaryOp::Lt, _, _)));
                assert!(matches!(*rhs, Expr::Bool(true)));
            }
            other => panic!("expected Eq(Lt(a, 5), true), got {other:?}"),
        }
    }

    #[test]
    fn parentheses_override_precedence() {
        // (1 + 2) * 3 should be Mul(Add(1, 2), 3)
        let expr = parse_cel_expr("(1 + 2) * 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Mul, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Binary(BinaryOp::Add, _, _)));
                assert!(matches!(*rhs, Expr::Int(3)));
            }
            other => panic!("expected Mul(Add(1, 2), 3), got {other:?}"),
        }
    }

    // --- Ternary ---

    #[test]
    fn parse_ternary_expr() {
        let expr = parse_cel_expr("x > 0 ? x : 0").unwrap();
        match expr {
            Expr::Ternary(cond, then_expr, else_expr) => {
                assert!(matches!(*cond, Expr::Binary(BinaryOp::Gt, _, _)));
                assert!(matches!(*then_expr, Expr::Ident(s) if s == "x"));
                assert!(matches!(*else_expr, Expr::Int(0)));
            }
            other => panic!("expected Ternary, got {other:?}"),
        }
    }

    #[test]
    fn parse_nested_ternary() {
        // a ? b ? c : d : e => a ? (b ? c : d) : e  (right-associative)
        let expr = parse_cel_expr("a ? b ? c : d : e").unwrap();
        match expr {
            Expr::Ternary(_, then_expr, _) => {
                assert!(matches!(*then_expr, Expr::Ternary(_, _, _)));
            }
            other => panic!("expected nested Ternary, got {other:?}"),
        }
    }

    // --- Method calls ---

    #[test]
    fn parse_contains_method() {
        let expr = parse_cel_expr(r#"name.contains("test")"#).unwrap();
        match expr {
            Expr::Binary(BinaryOp::Contains, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "name"));
                assert!(matches!(*rhs, Expr::String(s) if s == "test"));
            }
            other => panic!("expected Binary(Contains, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_starts_with_method() {
        let expr = parse_cel_expr(r#"name.startsWith("pre")"#).unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::StartsWith, _, _)));
    }

    #[test]
    fn parse_ends_with_method() {
        let expr = parse_cel_expr(r#"name.endsWith("fix")"#).unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::EndsWith, _, _)));
    }

    #[test]
    fn parse_matches_method() {
        let expr = parse_cel_expr(r#"name.matches("^test.*$")"#).unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Matches, _, _)));
    }

    #[test]
    fn parse_size_method() {
        let expr = parse_cel_expr("items.size()").unwrap();
        match expr {
            Expr::Call(name, args) => {
                assert_eq!(name, "len");
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], Expr::Ident(s) if s == "items"));
            }
            other => panic!("expected Call(len, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_chained_method_and_field() {
        let expr = parse_cel_expr(r#"action.payload.to.contains("@")"#).unwrap();
        match expr {
            Expr::Binary(BinaryOp::Contains, lhs, _) => {
                // lhs should be action.payload.to
                assert!(matches!(*lhs, Expr::Field(_, f) if f == "to"));
            }
            other => panic!("expected Binary(Contains, ...), got {other:?}"),
        }
    }

    // --- Function calls ---

    #[test]
    fn parse_size_function() {
        let expr = parse_cel_expr("size(items)").unwrap();
        match expr {
            Expr::Call(name, args) => {
                assert_eq!(name, "len");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected Call(len, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_int_function() {
        let expr = parse_cel_expr(r#"int("42")"#).unwrap();
        match expr {
            Expr::Call(name, args) => {
                assert_eq!(name, "to_int");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected Call(to_int, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_string_function() {
        let expr = parse_cel_expr("string(42)").unwrap();
        match expr {
            Expr::Call(name, args) => {
                assert_eq!(name, "to_string");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected Call(to_string, ...), got {other:?}"),
        }
    }

    // --- List and map literals ---

    #[test]
    fn parse_empty_list() {
        let expr = parse_cel_expr("[]").unwrap();
        assert!(matches!(expr, Expr::List(items) if items.is_empty()));
    }

    #[test]
    fn parse_list_with_items() {
        let expr = parse_cel_expr("[1, 2, 3]").unwrap();
        match expr {
            Expr::List(items) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], Expr::Int(1)));
                assert!(matches!(items[1], Expr::Int(2)));
                assert!(matches!(items[2], Expr::Int(3)));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_map() {
        let expr = parse_cel_expr("{}").unwrap();
        assert!(matches!(expr, Expr::Map(items) if items.is_empty()));
    }

    #[test]
    fn parse_map_with_string_keys() {
        let expr = parse_cel_expr(r#"{"a": 1, "b": 2}"#).unwrap();
        match expr {
            Expr::Map(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].0, "a");
                assert!(matches!(entries[0].1, Expr::Int(1)));
                assert_eq!(entries[1].0, "b");
                assert!(matches!(entries[1].1, Expr::Int(2)));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn parse_map_with_bare_keys() {
        let expr = parse_cel_expr("{key: true}").unwrap();
        match expr {
            Expr::Map(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0, "key");
                assert!(matches!(entries[0].1, Expr::Bool(true)));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    // --- Complex expressions ---

    #[test]
    fn parse_complex_condition() {
        let expr = parse_cel_expr(
            r#"action.action_type == "send_email" && action.payload.to.contains("@")"#,
        )
        .unwrap();
        match expr {
            Expr::Binary(BinaryOp::And, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Binary(BinaryOp::Eq, _, _)));
                assert!(matches!(*rhs, Expr::Binary(BinaryOp::Contains, _, _)));
            }
            other => panic!("expected And(Eq, Contains), got {other:?}"),
        }
    }

    #[test]
    fn parse_expression_with_whitespace() {
        let expr = parse_cel_expr("  1  +  2  ").unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::Add, _, _)));
    }

    #[test]
    fn parse_deeply_nested() {
        let expr = parse_cel_expr("((((42))))").unwrap();
        assert!(matches!(expr, Expr::Int(42)));
    }

    // --- Error cases ---

    #[test]
    fn parse_empty_input() {
        assert!(parse_cel_expr("").is_err());
    }

    #[test]
    fn parse_trailing_garbage() {
        assert!(parse_cel_expr("1 + 2 @@@").is_err());
    }

    #[test]
    fn parse_unclosed_paren() {
        assert!(parse_cel_expr("(1 + 2").is_err());
    }

    #[test]
    fn parse_unclosed_string() {
        assert!(parse_cel_expr(r#""unclosed"#).is_err());
    }

    // --- Subtraction edge cases ---

    #[test]
    fn parse_sub_after_ident() {
        let expr = parse_cel_expr("a - b").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Sub, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Ident(s) if s == "a"));
                assert!(matches!(*rhs, Expr::Ident(s) if s == "b"));
            }
            other => panic!("expected Binary(Sub, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_sub_chain() {
        // 10 - 3 - 2 should be (10 - 3) - 2 (left associative)
        let expr = parse_cel_expr("10 - 3 - 2").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Sub, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Binary(BinaryOp::Sub, _, _)));
                assert!(matches!(*rhs, Expr::Int(2)));
            }
            other => panic!("expected Sub(Sub(10, 3), 2), got {other:?}"),
        }
    }

    #[test]
    fn parse_unary_neg_in_expression() {
        // 5 + -3 should be 5 + (-(3))
        let expr = parse_cel_expr("5 + -3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Add, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Int(5)));
                assert!(matches!(*rhs, Expr::Unary(UnaryOp::Neg, _)));
            }
            other => panic!("expected Add(5, Neg(3)), got {other:?}"),
        }
    }

    // --- in keyword edge case ---

    #[test]
    fn parse_ident_starting_with_in() {
        // "index" should parse as an identifier, not "in" + "dex"
        let expr = parse_cel_expr("index").unwrap();
        assert!(matches!(expr, Expr::Ident(s) if s == "index"));
    }

    #[test]
    fn parse_null_not_prefix() {
        // "nullable" should parse as an identifier
        let expr = parse_cel_expr("nullable").unwrap();
        assert!(matches!(expr, Expr::Ident(s) if s == "nullable"));
    }

    #[test]
    fn parse_true_not_prefix() {
        // "trueval" should parse as an identifier
        let expr = parse_cel_expr("trueval").unwrap();
        assert!(matches!(expr, Expr::Ident(s) if s == "trueval"));
    }

    // --- Associativity ---

    #[test]
    fn left_associative_arithmetic() {
        // 1 + 2 + 3 should be (1 + 2) + 3
        let expr = parse_cel_expr("1 + 2 + 3").unwrap();
        match expr {
            Expr::Binary(BinaryOp::Add, lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Binary(BinaryOp::Add, _, _)));
                assert!(matches!(*rhs, Expr::Int(3)));
            }
            other => panic!("expected Add(Add(1, 2), 3), got {other:?}"),
        }
    }

    // --- Mixed list expressions ---

    #[test]
    fn parse_list_with_expressions() {
        let expr = parse_cel_expr("[1 + 2, x, true]").unwrap();
        match expr {
            Expr::List(items) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(items[0], Expr::Binary(BinaryOp::Add, _, _)));
                assert!(matches!(items[1], Expr::Ident(_)));
                assert!(matches!(items[2], Expr::Bool(true)));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    // --- Semantic match ---

    #[test]
    fn parse_semantic_match_method_with_threshold() {
        let expr = parse_cel_expr(r#"action.payload.message.semanticMatch("server issues", 0.75)"#)
            .unwrap();
        match expr {
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                assert_eq!(topic, "server issues");
                assert!((threshold - 0.75).abs() < f64::EPSILON);
                assert!(text_field.is_some());
            }
            other => panic!("expected SemanticMatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_semantic_match_method_default_threshold() {
        let expr =
            parse_cel_expr(r#"action.payload.message.semanticMatch("server issues")"#).unwrap();
        match expr {
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                assert_eq!(topic, "server issues");
                assert!((threshold - 0.8).abs() < f64::EPSILON);
                assert!(text_field.is_some());
            }
            other => panic!("expected SemanticMatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_semantic_match_function_two_args() {
        let expr = parse_cel_expr(r#"semantic_match("billing issues", 0.7)"#).unwrap();
        match expr {
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                assert_eq!(topic, "billing issues");
                assert!((threshold - 0.7).abs() < f64::EPSILON);
                assert!(text_field.is_none());
            }
            other => panic!("expected SemanticMatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_semantic_match_function_three_args() {
        let expr =
            parse_cel_expr(r#"semantic_match("billing issues", 0.7, action.payload.msg)"#).unwrap();
        match expr {
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                assert_eq!(topic, "billing issues");
                assert!((threshold - 0.7).abs() < f64::EPSILON);
                assert!(text_field.is_some());
            }
            other => panic!("expected SemanticMatch, got {other:?}"),
        }
    }

    // --- Time-based expressions ---

    #[test]
    fn parse_time_hour_field_access() {
        let expr = parse_cel_expr("time.hour").unwrap();
        match expr {
            Expr::Field(base, field) => {
                assert!(matches!(*base, Expr::Ident(s) if s == "time"));
                assert_eq!(field, "hour");
            }
            other => panic!("expected Field(Ident(time), hour), got {other:?}"),
        }
    }

    #[test]
    fn parse_time_business_hours_condition() {
        let expr =
            parse_cel_expr("time.hour >= 9 && time.hour < 17 && time.weekday_num <= 5").unwrap();
        // Should be: And(And(Ge(time.hour, 9), Lt(time.hour, 17)), Le(time.weekday_num, 5))
        match expr {
            Expr::Binary(BinaryOp::And, _, _) => {} // nested AND structure
            other => panic!("expected And(...), got {other:?}"),
        }
    }

    #[test]
    fn parse_time_weekday_comparison() {
        let expr = parse_cel_expr(r#"time.weekday != "Saturday""#).unwrap();
        match expr {
            Expr::Binary(BinaryOp::Ne, lhs, rhs) => {
                match *lhs {
                    Expr::Field(base, ref field) => {
                        assert!(matches!(*base, Expr::Ident(ref s) if s == "time"));
                        assert_eq!(field, "weekday");
                    }
                    ref other => panic!("expected Field, got {other:?}"),
                }
                assert!(matches!(*rhs, Expr::String(ref s) if s == "Saturday"));
            }
            other => panic!("expected Binary(Ne, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_time_combined_with_action() {
        let expr = parse_cel_expr(
            r#"action.action_type == "send_email" && time.hour >= 9 && time.hour < 17"#,
        )
        .unwrap();
        assert!(matches!(expr, Expr::Binary(BinaryOp::And, _, _)));
    }
}
