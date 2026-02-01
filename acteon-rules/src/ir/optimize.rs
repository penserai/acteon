//! Basic optimizer passes for expression IR.
//!
//! Supported transformations:
//! - Constant folding for arithmetic and comparison on literals.
//! - Dead branch elimination for ternary expressions with constant conditions.
//! - Double negation removal (`!!x` -> `x`).
//! - All/Any simplification (flatten single-element, remove constant entries).

use super::expr::{BinaryOp, Expr, UnaryOp};

/// Run all optimization passes on an expression tree, returning the optimized form.
pub fn optimize(expr: Expr) -> Expr {
    let expr = fold_constants(expr);
    let expr = eliminate_dead_branches(expr);
    remove_double_negation(expr)
}

/// Constant folding: evaluate operations on literal values at compile time.
fn fold_constants(expr: Expr) -> Expr {
    match expr {
        Expr::Unary(op, inner) => {
            let inner = fold_constants(*inner);
            match (&op, &inner) {
                (UnaryOp::Not, Expr::Bool(b)) => Expr::Bool(!b),
                (UnaryOp::Neg, Expr::Int(n)) => Expr::Int(-n),
                (UnaryOp::Neg, Expr::Float(f)) => Expr::Float(-f),
                _ => Expr::Unary(op, Box::new(inner)),
            }
        }
        Expr::Binary(op, lhs, rhs) => {
            let lhs = fold_constants(*lhs);
            let rhs = fold_constants(*rhs);
            fold_binary(op, lhs, rhs)
        }
        Expr::Ternary(cond, then_branch, else_branch) => {
            let cond = fold_constants(*cond);
            let then_branch = fold_constants(*then_branch);
            let else_branch = fold_constants(*else_branch);
            Expr::Ternary(Box::new(cond), Box::new(then_branch), Box::new(else_branch))
        }
        Expr::All(exprs) => {
            let optimized: Vec<Expr> = exprs.into_iter().map(fold_constants).collect();
            Expr::All(optimized)
        }
        Expr::Any(exprs) => {
            let optimized: Vec<Expr> = exprs.into_iter().map(fold_constants).collect();
            Expr::Any(optimized)
        }
        Expr::Call(name, args) => {
            let args = args.into_iter().map(fold_constants).collect();
            Expr::Call(name, args)
        }
        Expr::List(items) => {
            let items = items.into_iter().map(fold_constants).collect();
            Expr::List(items)
        }
        Expr::Map(entries) => {
            let entries = entries
                .into_iter()
                .map(|(k, v)| (k, fold_constants(v)))
                .collect();
            Expr::Map(entries)
        }
        Expr::Field(inner, field) => {
            let inner = fold_constants(*inner);
            Expr::Field(Box::new(inner), field)
        }
        Expr::Index(base, index) => {
            let base = fold_constants(*base);
            let index = fold_constants(*index);
            Expr::Index(Box::new(base), Box::new(index))
        }
        other => other,
    }
}

/// Attempt to fold a binary operation on two constant operands.
fn fold_binary(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
    match (&op, &lhs, &rhs) {
        // Integer arithmetic
        (BinaryOp::Add, Expr::Int(a), Expr::Int(b)) => Expr::Int(a.wrapping_add(*b)),
        (BinaryOp::Sub, Expr::Int(a), Expr::Int(b)) => Expr::Int(a.wrapping_sub(*b)),
        (BinaryOp::Mul, Expr::Int(a), Expr::Int(b)) => Expr::Int(a.wrapping_mul(*b)),
        (BinaryOp::Div, Expr::Int(a), Expr::Int(b)) if *b != 0 => Expr::Int(a / b),
        (BinaryOp::Mod, Expr::Int(a), Expr::Int(b)) if *b != 0 => Expr::Int(a % b),

        // Float arithmetic
        (BinaryOp::Add, Expr::Float(a), Expr::Float(b)) => Expr::Float(a + b),
        (BinaryOp::Sub, Expr::Float(a), Expr::Float(b)) => Expr::Float(a - b),
        (BinaryOp::Mul, Expr::Float(a), Expr::Float(b)) => Expr::Float(a * b),
        (BinaryOp::Div, Expr::Float(a), Expr::Float(b)) if *b != 0.0 => Expr::Float(a / b),

        // Integer comparison
        (BinaryOp::Eq, Expr::Int(a), Expr::Int(b)) => Expr::Bool(a == b),
        (BinaryOp::Ne, Expr::Int(a), Expr::Int(b)) => Expr::Bool(a != b),
        (BinaryOp::Lt, Expr::Int(a), Expr::Int(b)) => Expr::Bool(a < b),
        (BinaryOp::Le, Expr::Int(a), Expr::Int(b)) => Expr::Bool(a <= b),
        (BinaryOp::Gt, Expr::Int(a), Expr::Int(b)) => Expr::Bool(a > b),
        (BinaryOp::Ge, Expr::Int(a), Expr::Int(b)) => Expr::Bool(a >= b),

        // String comparison
        (BinaryOp::Eq, Expr::String(a), Expr::String(b)) => Expr::Bool(a == b),
        (BinaryOp::Ne, Expr::String(a), Expr::String(b)) => Expr::Bool(a != b),

        // Boolean logic
        (BinaryOp::And, Expr::Bool(a), Expr::Bool(b)) => Expr::Bool(*a && *b),
        (BinaryOp::Or, Expr::Bool(a), Expr::Bool(b)) => Expr::Bool(*a || *b),

        // Short-circuit: `false && x` -> `false`, `true || x` -> `true`
        (BinaryOp::And, Expr::Bool(false), _) => Expr::Bool(false),
        (BinaryOp::Or, Expr::Bool(true), _) => Expr::Bool(true),
        // Identity: `true && x` -> `x`, `false || x` -> `x`
        (BinaryOp::And, Expr::Bool(true), _) | (BinaryOp::Or, Expr::Bool(false), _) => rhs,

        // String operations on constants
        (BinaryOp::Contains, Expr::String(haystack), Expr::String(needle)) => {
            Expr::Bool(haystack.contains(needle.as_str()))
        }
        (BinaryOp::StartsWith, Expr::String(haystack), Expr::String(prefix)) => {
            Expr::Bool(haystack.starts_with(prefix.as_str()))
        }
        (BinaryOp::EndsWith, Expr::String(haystack), Expr::String(suffix)) => {
            Expr::Bool(haystack.ends_with(suffix.as_str()))
        }

        _ => Expr::Binary(op, Box::new(lhs), Box::new(rhs)),
    }
}

/// Dead branch elimination: replace ternary expressions whose condition
/// is a known constant with the appropriate branch.
fn eliminate_dead_branches(expr: Expr) -> Expr {
    match expr {
        Expr::Ternary(cond, then_branch, else_branch) => {
            let cond = eliminate_dead_branches(*cond);
            let then_branch = eliminate_dead_branches(*then_branch);
            let else_branch = eliminate_dead_branches(*else_branch);

            match &cond {
                Expr::Bool(true) => then_branch,
                Expr::Bool(false) => else_branch,
                _ => Expr::Ternary(Box::new(cond), Box::new(then_branch), Box::new(else_branch)),
            }
        }
        Expr::All(exprs) => {
            let optimized: Vec<Expr> = exprs.into_iter().map(eliminate_dead_branches).collect();

            // If any is false, the whole All is false.
            if optimized.iter().any(|e| matches!(e, Expr::Bool(false))) {
                return Expr::Bool(false);
            }

            // Remove constant trues.
            let filtered: Vec<Expr> = optimized
                .into_iter()
                .filter(|e| !matches!(e, Expr::Bool(true)))
                .collect();

            match filtered.len() {
                0 => Expr::Bool(true),
                1 => filtered.into_iter().next().expect("length checked"),
                _ => Expr::All(filtered),
            }
        }
        Expr::Any(exprs) => {
            let optimized: Vec<Expr> = exprs.into_iter().map(eliminate_dead_branches).collect();

            // If any is true, the whole Any is true.
            if optimized.iter().any(|e| matches!(e, Expr::Bool(true))) {
                return Expr::Bool(true);
            }

            // Remove constant falses.
            let filtered: Vec<Expr> = optimized
                .into_iter()
                .filter(|e| !matches!(e, Expr::Bool(false)))
                .collect();

            match filtered.len() {
                0 => Expr::Bool(false),
                1 => filtered.into_iter().next().expect("length checked"),
                _ => Expr::Any(filtered),
            }
        }
        Expr::Unary(op, inner) => {
            let inner = eliminate_dead_branches(*inner);
            Expr::Unary(op, Box::new(inner))
        }
        Expr::Binary(op, lhs, rhs) => {
            let lhs = eliminate_dead_branches(*lhs);
            let rhs = eliminate_dead_branches(*rhs);
            Expr::Binary(op, Box::new(lhs), Box::new(rhs))
        }
        Expr::Call(name, args) => {
            let args = args.into_iter().map(eliminate_dead_branches).collect();
            Expr::Call(name, args)
        }
        Expr::Field(inner, field) => {
            let inner = eliminate_dead_branches(*inner);
            Expr::Field(Box::new(inner), field)
        }
        Expr::Index(base, index) => {
            let base = eliminate_dead_branches(*base);
            let index = eliminate_dead_branches(*index);
            Expr::Index(Box::new(base), Box::new(index))
        }
        other => other,
    }
}

/// Remove double negations: `!!x` -> `x`.
fn remove_double_negation(expr: Expr) -> Expr {
    match expr {
        Expr::Unary(UnaryOp::Not, inner) => {
            let inner = remove_double_negation(*inner);
            match inner {
                Expr::Unary(UnaryOp::Not, double_inner) => *double_inner,
                other => Expr::Unary(UnaryOp::Not, Box::new(other)),
            }
        }
        Expr::Unary(op, inner) => {
            let inner = remove_double_negation(*inner);
            Expr::Unary(op, Box::new(inner))
        }
        Expr::Binary(op, lhs, rhs) => {
            let lhs = remove_double_negation(*lhs);
            let rhs = remove_double_negation(*rhs);
            Expr::Binary(op, Box::new(lhs), Box::new(rhs))
        }
        Expr::Ternary(cond, then_branch, else_branch) => {
            let cond = remove_double_negation(*cond);
            let then_branch = remove_double_negation(*then_branch);
            let else_branch = remove_double_negation(*else_branch);
            Expr::Ternary(Box::new(cond), Box::new(then_branch), Box::new(else_branch))
        }
        Expr::All(exprs) => Expr::All(exprs.into_iter().map(remove_double_negation).collect()),
        Expr::Any(exprs) => Expr::Any(exprs.into_iter().map(remove_double_negation).collect()),
        Expr::Call(name, args) => {
            let args = args.into_iter().map(remove_double_negation).collect();
            Expr::Call(name, args)
        }
        Expr::Field(inner, field) => {
            let inner = remove_double_negation(*inner);
            Expr::Field(Box::new(inner), field)
        }
        Expr::Index(base, index) => {
            let base = remove_double_negation(*base);
            let index = remove_double_negation(*index);
            Expr::Index(Box::new(base), Box::new(index))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_integer_arithmetic() {
        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::Int(2)),
            Box::new(Expr::Int(3)),
        );
        assert!(matches!(optimize(expr), Expr::Int(5)));

        let expr = Expr::Binary(
            BinaryOp::Sub,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(4)),
        );
        assert!(matches!(optimize(expr), Expr::Int(6)));

        let expr = Expr::Binary(
            BinaryOp::Mul,
            Box::new(Expr::Int(3)),
            Box::new(Expr::Int(7)),
        );
        assert!(matches!(optimize(expr), Expr::Int(21)));

        let expr = Expr::Binary(
            BinaryOp::Div,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(3)),
        );
        assert!(matches!(optimize(expr), Expr::Int(3)));

        let expr = Expr::Binary(
            BinaryOp::Mod,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(3)),
        );
        assert!(matches!(optimize(expr), Expr::Int(1)));
    }

    #[test]
    fn fold_no_divide_by_zero() {
        // Division by zero should not be folded.
        let expr = Expr::Binary(
            BinaryOp::Div,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(0)),
        );
        assert!(matches!(optimize(expr), Expr::Binary(BinaryOp::Div, _, _)));
    }

    #[test]
    fn fold_float_arithmetic() {
        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::Float(1.5)),
            Box::new(Expr::Float(2.5)),
        );
        match optimize(expr) {
            Expr::Float(f) => assert!((f - 4.0).abs() < f64::EPSILON),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn fold_integer_comparison() {
        let expr = Expr::Binary(BinaryOp::Eq, Box::new(Expr::Int(3)), Box::new(Expr::Int(3)));
        assert!(matches!(optimize(expr), Expr::Bool(true)));

        let expr = Expr::Binary(BinaryOp::Ne, Box::new(Expr::Int(3)), Box::new(Expr::Int(3)));
        assert!(matches!(optimize(expr), Expr::Bool(false)));

        let expr = Expr::Binary(BinaryOp::Lt, Box::new(Expr::Int(2)), Box::new(Expr::Int(3)));
        assert!(matches!(optimize(expr), Expr::Bool(true)));

        let expr = Expr::Binary(BinaryOp::Gt, Box::new(Expr::Int(2)), Box::new(Expr::Int(3)));
        assert!(matches!(optimize(expr), Expr::Bool(false)));

        let expr = Expr::Binary(BinaryOp::Le, Box::new(Expr::Int(3)), Box::new(Expr::Int(3)));
        assert!(matches!(optimize(expr), Expr::Bool(true)));

        let expr = Expr::Binary(BinaryOp::Ge, Box::new(Expr::Int(3)), Box::new(Expr::Int(3)));
        assert!(matches!(optimize(expr), Expr::Bool(true)));
    }

    #[test]
    fn fold_boolean_logic() {
        let expr = Expr::Binary(
            BinaryOp::And,
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Bool(false)),
        );
        assert!(matches!(optimize(expr), Expr::Bool(false)));

        let expr = Expr::Binary(
            BinaryOp::Or,
            Box::new(Expr::Bool(false)),
            Box::new(Expr::Bool(true)),
        );
        assert!(matches!(optimize(expr), Expr::Bool(true)));
    }

    #[test]
    fn fold_short_circuit_and() {
        // false && <anything> -> false
        let expr = Expr::Binary(
            BinaryOp::And,
            Box::new(Expr::Bool(false)),
            Box::new(Expr::Ident("x".into())),
        );
        assert!(matches!(optimize(expr), Expr::Bool(false)));

        // true && x -> x
        let expr = Expr::Binary(
            BinaryOp::And,
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Ident("x".into())),
        );
        assert!(matches!(optimize(expr), Expr::Ident(_)));
    }

    #[test]
    fn fold_short_circuit_or() {
        // true || <anything> -> true
        let expr = Expr::Binary(
            BinaryOp::Or,
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Ident("x".into())),
        );
        assert!(matches!(optimize(expr), Expr::Bool(true)));

        // false || x -> x
        let expr = Expr::Binary(
            BinaryOp::Or,
            Box::new(Expr::Bool(false)),
            Box::new(Expr::Ident("x".into())),
        );
        assert!(matches!(optimize(expr), Expr::Ident(_)));
    }

    #[test]
    fn fold_unary_not() {
        let expr = Expr::Unary(UnaryOp::Not, Box::new(Expr::Bool(true)));
        assert!(matches!(optimize(expr), Expr::Bool(false)));
    }

    #[test]
    fn fold_unary_neg() {
        let expr = Expr::Unary(UnaryOp::Neg, Box::new(Expr::Int(42)));
        assert!(matches!(optimize(expr), Expr::Int(-42)));
    }

    #[test]
    fn dead_branch_true_condition() {
        let expr = Expr::Ternary(
            Box::new(Expr::Bool(true)),
            Box::new(Expr::String("yes".into())),
            Box::new(Expr::String("no".into())),
        );
        match optimize(expr) {
            Expr::String(s) => assert_eq!(s, "yes"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn dead_branch_false_condition() {
        let expr = Expr::Ternary(
            Box::new(Expr::Bool(false)),
            Box::new(Expr::String("yes".into())),
            Box::new(Expr::String("no".into())),
        );
        match optimize(expr) {
            Expr::String(s) => assert_eq!(s, "no"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn double_negation_removal() {
        let expr = Expr::Unary(
            UnaryOp::Not,
            Box::new(Expr::Unary(UnaryOp::Not, Box::new(Expr::Ident("x".into())))),
        );
        assert!(matches!(optimize(expr), Expr::Ident(_)));
    }

    #[test]
    fn all_with_false_collapses() {
        let expr = Expr::All(vec![
            Expr::Bool(true),
            Expr::Bool(false),
            Expr::Ident("x".into()),
        ]);
        assert!(matches!(optimize(expr), Expr::Bool(false)));
    }

    #[test]
    fn all_removes_trues() {
        let expr = Expr::All(vec![
            Expr::Bool(true),
            Expr::Ident("x".into()),
            Expr::Bool(true),
        ]);
        // Should simplify to just Ident("x")
        assert!(matches!(optimize(expr), Expr::Ident(_)));
    }

    #[test]
    fn any_with_true_collapses() {
        let expr = Expr::Any(vec![
            Expr::Bool(false),
            Expr::Bool(true),
            Expr::Ident("x".into()),
        ]);
        assert!(matches!(optimize(expr), Expr::Bool(true)));
    }

    #[test]
    fn any_removes_falses() {
        let expr = Expr::Any(vec![
            Expr::Bool(false),
            Expr::Ident("x".into()),
            Expr::Bool(false),
        ]);
        assert!(matches!(optimize(expr), Expr::Ident(_)));
    }

    #[test]
    fn fold_string_operations() {
        let expr = Expr::Binary(
            BinaryOp::Contains,
            Box::new(Expr::String("hello world".into())),
            Box::new(Expr::String("world".into())),
        );
        assert!(matches!(optimize(expr), Expr::Bool(true)));

        let expr = Expr::Binary(
            BinaryOp::StartsWith,
            Box::new(Expr::String("hello world".into())),
            Box::new(Expr::String("hello".into())),
        );
        assert!(matches!(optimize(expr), Expr::Bool(true)));

        let expr = Expr::Binary(
            BinaryOp::EndsWith,
            Box::new(Expr::String("hello world".into())),
            Box::new(Expr::String("world".into())),
        );
        assert!(matches!(optimize(expr), Expr::Bool(true)));
    }

    #[test]
    fn nested_constant_folding() {
        // (2 + 3) * (10 - 4) should fold to 30
        let expr = Expr::Binary(
            BinaryOp::Mul,
            Box::new(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Int(2)),
                Box::new(Expr::Int(3)),
            )),
            Box::new(Expr::Binary(
                BinaryOp::Sub,
                Box::new(Expr::Int(10)),
                Box::new(Expr::Int(4)),
            )),
        );
        assert!(matches!(optimize(expr), Expr::Int(30)));
    }
}
