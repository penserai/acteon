//! Built-in functions available in rule expressions.

use chrono::Utc;
use regex::Regex;

use crate::engine::executor::Value;
use crate::error::RuleError;

/// Dispatch a built-in function call by name.
pub fn call_builtin(name: &str, args: &[Value]) -> Result<Value, RuleError> {
    match name {
        "len" => builtin_len(args),
        "lower" => builtin_lower(args),
        "upper" => builtin_upper(args),
        "contains" => builtin_contains(args),
        "starts_with" => builtin_starts_with(args),
        "ends_with" => builtin_ends_with(args),
        "matches" => builtin_matches(args),
        "now" => builtin_now(args),
        "duration" => builtin_duration(args),
        "format" => builtin_format(args),
        "abs" => builtin_abs(args),
        "min" => builtin_min(args),
        "max" => builtin_max(args),
        "to_string" => builtin_to_string(args),
        "to_int" => builtin_to_int(args),
        _ => Err(RuleError::UndefinedFunction(name.to_owned())),
    }
}

/// Ensure the argument list has exactly `n` elements.
fn expect_args(name: &str, args: &[Value], n: usize) -> Result<(), RuleError> {
    if args.len() != n {
        return Err(RuleError::TypeError(format!(
            "{name}() expects {n} argument(s), got {}",
            args.len()
        )));
    }
    Ok(())
}

/// `len(value)` - returns the length of a string or list.
fn builtin_len(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("len", args, 1)?;
    match &args[0] {
        Value::String(s) => Ok(Value::Int(i64::try_from(s.len()).unwrap_or(i64::MAX))),
        Value::List(v) => Ok(Value::Int(i64::try_from(v.len()).unwrap_or(i64::MAX))),
        Value::Map(m) => Ok(Value::Int(i64::try_from(m.len()).unwrap_or(i64::MAX))),
        other => Err(RuleError::TypeError(format!(
            "len() expects string, list, or map, got {}",
            other.type_name()
        ))),
    }
}

/// `lower(string)` - convert a string to lowercase.
fn builtin_lower(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("lower", args, 1)?;
    match &args[0] {
        Value::String(s) => Ok(Value::String(s.to_lowercase())),
        other => Err(RuleError::TypeError(format!(
            "lower() expects string, got {}",
            other.type_name()
        ))),
    }
}

/// `upper(string)` - convert a string to uppercase.
fn builtin_upper(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("upper", args, 1)?;
    match &args[0] {
        Value::String(s) => Ok(Value::String(s.to_uppercase())),
        other => Err(RuleError::TypeError(format!(
            "upper() expects string, got {}",
            other.type_name()
        ))),
    }
}

/// `contains(haystack, needle)` - check if a string contains a substring.
fn builtin_contains(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("contains", args, 2)?;
    match (&args[0], &args[1]) {
        (Value::String(haystack), Value::String(needle)) => {
            Ok(Value::Bool(haystack.contains(needle.as_str())))
        }
        (Value::List(list), needle) => Ok(Value::Bool(list.contains(needle))),
        (a, b) => Err(RuleError::TypeError(format!(
            "contains() expects (string, string) or (list, value), got ({}, {})",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `starts_with(string, prefix)` - check if a string starts with a prefix.
fn builtin_starts_with(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("starts_with", args, 2)?;
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(prefix)) => {
            Ok(Value::Bool(s.starts_with(prefix.as_str())))
        }
        (a, b) => Err(RuleError::TypeError(format!(
            "starts_with() expects (string, string), got ({}, {})",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `ends_with(string, suffix)` - check if a string ends with a suffix.
fn builtin_ends_with(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("ends_with", args, 2)?;
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(suffix)) => Ok(Value::Bool(s.ends_with(suffix.as_str()))),
        (a, b) => Err(RuleError::TypeError(format!(
            "ends_with() expects (string, string), got ({}, {})",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `matches(string, pattern)` - check if a string matches a regular expression.
fn builtin_matches(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("matches", args, 2)?;
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(pattern)) => {
            let re = Regex::new(pattern).map_err(|e| RuleError::InvalidRegex(e.to_string()))?;
            Ok(Value::Bool(re.is_match(s)))
        }
        (a, b) => Err(RuleError::TypeError(format!(
            "matches() expects (string, string), got ({}, {})",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `now()` - returns the current Unix timestamp in seconds.
fn builtin_now(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("now", args, 0)?;
    Ok(Value::Int(Utc::now().timestamp()))
}

/// `duration(seconds)` - returns a duration value in seconds (identity for numeric).
#[allow(clippy::cast_possible_truncation)]
fn builtin_duration(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("duration", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Int(*f as i64)),
        other => Err(RuleError::TypeError(format!(
            "duration() expects number, got {}",
            other.type_name()
        ))),
    }
}

/// `format(template, args...)` - simple string formatting.
/// The template uses `{}` as placeholders, replaced left-to-right.
fn builtin_format(args: &[Value]) -> Result<Value, RuleError> {
    if args.is_empty() {
        return Err(RuleError::TypeError(
            "format() requires at least 1 argument".into(),
        ));
    }
    let template = match &args[0] {
        Value::String(s) => s.clone(),
        other => {
            return Err(RuleError::TypeError(format!(
                "format() first argument must be string, got {}",
                other.type_name()
            )))
        }
    };

    let mut result = template;
    for arg in &args[1..] {
        if let Some(pos) = result.find("{}") {
            let replacement = arg.display_string();
            result.replace_range(pos..pos + 2, &replacement);
        }
    }

    Ok(Value::String(result))
}

/// `abs(number)` - returns the absolute value.
fn builtin_abs(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("abs", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        other => Err(RuleError::TypeError(format!(
            "abs() expects number, got {}",
            other.type_name()
        ))),
    }
}

/// `min(a, b)` - returns the smaller of two numbers.
#[allow(clippy::cast_precision_loss)]
fn builtin_min(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("min", args, 2)?;
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.min(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.min(*b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float((*a as f64).min(*b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.min(*b as f64))),
        (a, b) => Err(RuleError::TypeError(format!(
            "min() expects numbers, got ({}, {})",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `max(a, b)` - returns the larger of two numbers.
#[allow(clippy::cast_precision_loss)]
fn builtin_max(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("max", args, 2)?;
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.max(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.max(*b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float((*a as f64).max(*b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.max(*b as f64))),
        (a, b) => Err(RuleError::TypeError(format!(
            "max() expects numbers, got ({}, {})",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `to_string(value)` - convert any value to its string representation.
fn builtin_to_string(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("to_string", args, 1)?;
    Ok(Value::String(args[0].display_string()))
}

/// `to_int(value)` - convert a value to an integer.
#[allow(clippy::cast_possible_truncation)]
fn builtin_to_int(args: &[Value]) -> Result<Value, RuleError> {
    expect_args("to_int", args, 1)?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Int(*f as i64)),
        Value::Bool(b) => Ok(Value::Int(i64::from(*b))),
        Value::String(s) => s
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|e| RuleError::TypeError(format!("to_int() cannot parse '{s}': {e}"))),
        other => Err(RuleError::TypeError(format!(
            "to_int() cannot convert {} to int",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn len_string() {
        let result = call_builtin("len", &[Value::String("hello".into())]).unwrap();
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn len_list() {
        let result =
            call_builtin("len", &[Value::List(vec![Value::Int(1), Value::Int(2)])]).unwrap();
        assert_eq!(result, Value::Int(2));
    }

    #[test]
    fn len_map() {
        let mut m = std::collections::HashMap::new();
        m.insert("a".into(), Value::Int(1));
        let result = call_builtin("len", &[Value::Map(m)]).unwrap();
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn lower_upper() {
        let result = call_builtin("lower", &[Value::String("HELLO".into())]).unwrap();
        assert_eq!(result, Value::String("hello".into()));

        let result = call_builtin("upper", &[Value::String("hello".into())]).unwrap();
        assert_eq!(result, Value::String("HELLO".into()));
    }

    #[test]
    fn contains_string() {
        let result = call_builtin(
            "contains",
            &[
                Value::String("hello world".into()),
                Value::String("world".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(true));

        let result = call_builtin(
            "contains",
            &[
                Value::String("hello world".into()),
                Value::String("xyz".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn contains_list() {
        let result = call_builtin(
            "contains",
            &[
                Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
                Value::Int(2),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn starts_with_ends_with() {
        let result = call_builtin(
            "starts_with",
            &[
                Value::String("hello world".into()),
                Value::String("hello".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(true));

        let result = call_builtin(
            "ends_with",
            &[
                Value::String("hello world".into()),
                Value::String("world".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn matches_regex() {
        let result = call_builtin(
            "matches",
            &[
                Value::String("user-123".into()),
                Value::String(r"^user-\d+$".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(true));

        let result = call_builtin(
            "matches",
            &[
                Value::String("admin".into()),
                Value::String(r"^user-\d+$".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn matches_invalid_regex() {
        let result = call_builtin(
            "matches",
            &[
                Value::String("test".into()),
                Value::String("[invalid".into()),
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn now_returns_timestamp() {
        let result = call_builtin("now", &[]).unwrap();
        assert!(matches!(result, Value::Int(_)));
    }

    #[test]
    fn duration_from_int() {
        let result = call_builtin("duration", &[Value::Int(3600)]).unwrap();
        assert_eq!(result, Value::Int(3600));
    }

    #[test]
    fn format_basic() {
        let result = call_builtin(
            "format",
            &[
                Value::String("Hello, {}!".into()),
                Value::String("world".into()),
            ],
        )
        .unwrap();
        assert_eq!(result, Value::String("Hello, world!".into()));
    }

    #[test]
    fn abs_values() {
        assert_eq!(
            call_builtin("abs", &[Value::Int(-42)]).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            call_builtin("abs", &[Value::Int(42)]).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn min_max_values() {
        assert_eq!(
            call_builtin("min", &[Value::Int(3), Value::Int(7)]).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            call_builtin("max", &[Value::Int(3), Value::Int(7)]).unwrap(),
            Value::Int(7)
        );
    }

    #[test]
    fn to_string_conversion() {
        assert_eq!(
            call_builtin("to_string", &[Value::Int(42)]).unwrap(),
            Value::String("42".into())
        );
        assert_eq!(
            call_builtin("to_string", &[Value::Bool(true)]).unwrap(),
            Value::String("true".into())
        );
    }

    #[test]
    fn to_int_conversion() {
        assert_eq!(
            call_builtin("to_int", &[Value::String("42".into())]).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            call_builtin("to_int", &[Value::Float(3.9)]).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            call_builtin("to_int", &[Value::Bool(true)]).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn undefined_function() {
        let result = call_builtin("nonexistent", &[]);
        assert!(matches!(result, Err(RuleError::UndefinedFunction(_))));
    }

    #[test]
    fn wrong_arg_count() {
        let result = call_builtin("len", &[]);
        assert!(matches!(result, Err(RuleError::TypeError(_))));

        let result = call_builtin("len", &[Value::String("a".into()), Value::Int(1)]);
        assert!(matches!(result, Err(RuleError::TypeError(_))));
    }

    #[test]
    fn type_error_on_wrong_type() {
        let result = call_builtin("lower", &[Value::Int(42)]);
        assert!(matches!(result, Err(RuleError::TypeError(_))));

        let result = call_builtin("len", &[Value::Bool(true)]);
        assert!(matches!(result, Err(RuleError::TypeError(_))));
    }
}
