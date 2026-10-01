use super::ir::{ArithmeticOp, ComparisonOp, Expr, Function};
use std::borrow::Borrow;
use std::iter::Peekable;

use super::tokenize::{Token, tokenize};

pub fn try_parse(s: &str) -> Result<Expr, String> {
    let mut tokens = tokenize(s).peekable();
    let expr = parse_expression(&mut tokens)?;
    // The parse_expression must not consume the EOF token.
    let token = next(&mut tokens)?;
    match token {
        Token::Eof => {}
        next => return unexpected(next, &[Token::Eof]),
    }
    expr.validate()?;
    Ok(expr)
}

fn parse_expression<'src>(
    input: &mut Peekable<impl Iterator<Item = Result<Token<'src>, String>>>,
) -> Result<Expr, String> {
    parse_comparison(input)
}

fn parse_comparison<'src>(
    input: &mut Peekable<impl Iterator<Item = Result<Token<'src>, String>>>,
) -> Result<Expr, String> {
    let lhs = parse_arithmetic(input)?;
    let op = match peek(input)? {
        Token::Lt => ComparisonOp::LessThan,
        Token::Lte => ComparisonOp::LessThanOrEqual,
        Token::Gt => ComparisonOp::GreaterThan,
        Token::Gte => ComparisonOp::GreaterThanOrEqual,
        Token::Eq => ComparisonOp::Equal,
        Token::Neq => ComparisonOp::NotEqual,
        _ => return Ok(lhs),
    };
    input.next();
    let rhs = parse_arithmetic(input)?;
    Ok(Expr::ComparisonBinary(Box::new(lhs), op, Box::new(rhs)))
}

fn parse_arithmetic<'src>(
    input: &mut Peekable<impl Iterator<Item = Result<Token<'src>, String>>>,
) -> Result<Expr, String> {
    let mut lhs = parse_atom(input)?;
    loop {
        let op = match peek(input)? {
            Token::Plus => ArithmeticOp::Add,
            Token::Minus => ArithmeticOp::Subtract,
            _ => return Ok(lhs),
        };
        input.next();
        let rhs = parse_atom(input)?;
        lhs = Expr::ArithmeticBinary(Box::new(lhs), op, Box::new(rhs));
    }
}

fn parse_atom<'src>(
    input: &mut Peekable<impl Iterator<Item = Result<Token<'src>, String>>>,
) -> Result<Expr, String> {
    match next(input)? {
        Token::LParen => {
            let expr = parse_expression(input)?;
            match next(input)? {
                Token::RParen => Ok(expr),
                other => unexpected(other, &[Token::RParen]),
            }
        }
        Token::Int(value) => value.parse::<i64>().map_or_else(
            |e| Err(format!("Invalid integer: {e}")),
            |n| Ok(Expr::Integer(n)),
        ),
        Token::Ident(ident) => {
            if peek(input)? == &Token::LParen {
                input.next();
                let mut args = Vec::new();
                if peek(input)? == &Token::RParen {
                    input.next();
                } else {
                    loop {
                        args.push(parse_expression(input)?);
                        match peek(input)? {
                            Token::Comma => {
                                input.next();
                            }
                            Token::RParen => {
                                input.next();
                                break;
                            }
                            other => unexpected(other, &[Token::Comma, Token::RParen])?,
                        }
                    }
                }
                Ok(Expr::Call(
                    match ident {
                        "min" => Function::Min,
                        "max" => Function::Max,
                        "if" => Function::If,
                        other => return Err(format!("Unknown function: {other}")),
                    },
                    args,
                ))
            } else {
                Ok(Expr::Variable(ident.to_string()))
            }
        }
        other => unexpected(
            other,
            &[Token::LParen, Token::Int(".."), Token::Ident("..")],
        ),
    }
}

fn next<'src>(
    input: &mut Peekable<impl Iterator<Item = Result<Token<'src>, String>>>,
) -> Result<Token<'src>, String> {
    input
        .next()
        .unwrap_or_else(|| Err("Unexpected end of input".to_string()))
}

fn peek<'src>(
    input: &mut Peekable<impl Iterator<Item = Result<Token<'src>, String>>>,
) -> Result<&'_ Token<'src>, &'_ str> {
    match input.peek() {
        None => Err("Unexpected end of input"),
        Some(Ok(next)) => Ok(next),
        Some(Err(next)) => Err(next.as_str()),
    }
}

fn unexpected<'a, V>(
    unexpected: impl Borrow<Token<'a>>,
    expected: &[Token<'_>],
) -> Result<V, String> {
    Err(format!(
        "Unexpected token {:?}, expected: {}",
        unexpected.borrow(),
        expected
            .iter()
            .map(|t| format!("{t:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_constant() {
        assert_eq!(parse("0"), constant(0));
        assert_eq!(parse("1"), constant(1));
        assert_eq!(parse("42"), constant(42));
        assert_eq!(parse("9223372036854775807"), constant(9223372036854775807));
        // value out of range for i64
        assert_eq!(
            try_parse("9223372036854775808").unwrap_err(),
            "Invalid integer: number too large to fit in target type"
        );
        // unary negative value not supported
        assert_eq!(
            try_parse("-1").unwrap_err(),
            "Unexpected token Minus, expected: LParen, Int(\"..\"), Ident(\"..\")"
        );
    }

    #[test]
    fn test_parse_ident() {
        assert_eq!(parse("a"), var("a"));
        assert_eq!(parse("c"), var("c"));
        assert_eq!(parse("abcd"), var("abcd"));
        assert_eq!(parse("AbCd"), var("AbCd"));
        assert_eq!(
            try_parse("a a").unwrap_err(),
            "Unexpected token Ident(\"a\"), expected: Eof"
        );
        assert_eq!(
            try_parse("a 1").unwrap_err(),
            "Unexpected token Int(\"1\"), expected: Eof"
        );
    }

    #[test]
    fn test_parse_arithmetic() {
        assert_eq!(parse("a + b"), add(var("a"), var("b")));
        assert_eq!(parse("a + b + c"), add(add(var("a"), var("b")), var("c")));
        assert_eq!(
            parse("a - b + c - d"),
            subtract(add(subtract(var("a"), var("b")), var("c")), var("d"))
        );
        assert_eq!(
            parse("a - (b + c) - d"),
            subtract(subtract(var("a"), add(var("b"), var("c"))), var("d"))
        );
        // unary minus is not supported
        assert_eq!(
            try_parse("- b").unwrap_err(),
            "Unexpected token Minus, expected: LParen, Int(\"..\"), Ident(\"..\")"
        );

        // dangling operator
        assert_eq!(
            try_parse("a +").unwrap_err(),
            "Unexpected token Eof, expected: LParen, Int(\"..\"), Ident(\"..\")"
        );
    }

    #[test]
    fn test_parse_comparison() {
        assert_eq!(parse("a < b"), lt(var("a"), var("b")));
        assert_eq!(parse("a <= b"), lte(var("a"), var("b")));
        assert_eq!(parse("a > b"), gt(var("a"), var("b")));
        assert_eq!(parse("a >= b"), gte(var("a"), var("b")));
        assert_eq!(parse("a == b"), eq(var("a"), var("b")));
        assert_eq!(parse("a != b"), neq(var("a"), var("b")));
    }

    #[test]
    fn test_parse_function() {
        assert_eq!(parse("min(a, b)"), min(vec![var("a"), var("b")]));
        assert_eq!(parse("max(a, b)"), max(vec![var("a"), var("b")]));
        assert_eq!(
            parse("if(a < b, a, b)"),
            if_(lt(var("a"), var("b")), var("a"), var("b"))
        );
        // closing paren is required
        assert_eq!(
            try_parse("min(a, b").unwrap_err(),
            "Unexpected token Eof, expected: Comma, RParen"
        );
        // trailing comma is not accepted
        assert_eq!(
            try_parse("min(a, b, )").unwrap_err(),
            "Unexpected token RParen, expected: LParen, Int(\"..\"), Ident(\"..\")"
        );
        // upper-case function name is not accepted
        assert_eq!(try_parse("MIN(a, b)").unwrap_err(), "Unknown function: MIN");
        // at least two arguments are required
        assert_eq!(
            try_parse("min()").unwrap_err(),
            "Expected at least two arguments to function min, got 0"
        );
        assert_eq!(
            try_parse("min(a)").unwrap_err(),
            "Expected at least two arguments to function min, got 1"
        );
    }

    #[test]
    fn test_parse_paren() {
        assert_eq!(parse("(a + b)"), add(var("a"), var("b")));
        assert_eq!(parse("(((((((a))))) + (b)))"), add(var("a"), var("b")));
        // unmatched opening paren
        assert_eq!(
            try_parse("( a + b").unwrap_err(),
            "Unexpected token Eof, expected: RParen"
        );
        // unmatched opening paren
        assert_eq!(
            try_parse("(((a + b))").unwrap_err(),
            "Unexpected token Eof, expected: RParen"
        );
        // unmatched closing paren
        assert_eq!(
            try_parse("a + b )").unwrap_err(),
            "Unexpected token RParen, expected: Eof"
        );
    }

    #[test]
    fn test_parse_nested() {
        assert_eq!(
            parse("max(1 + min(0, 2 - 3), 5, 0 - 9 + 4 + 4)"),
            max(vec![
                add(
                    constant(1),
                    min(vec![constant(0), subtract(constant(2), constant(3))])
                ),
                constant(5),
                add(
                    add(subtract(constant(0), constant(9)), constant(4)),
                    constant(4)
                )
            ])
        );
    }

    fn parse(s: &str) -> Expr {
        try_parse(s).unwrap()
    }

    fn var(s: impl Into<String>) -> Expr {
        Expr::Variable(s.into())
    }

    fn constant(v: i64) -> Expr {
        Expr::Integer(v)
    }

    fn add(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ArithmeticBinary(Box::new(lhs), ArithmeticOp::Add, Box::new(rhs))
    }

    fn subtract(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ArithmeticBinary(Box::new(lhs), ArithmeticOp::Subtract, Box::new(rhs))
    }

    fn lt(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ComparisonBinary(Box::new(lhs), ComparisonOp::LessThan, Box::new(rhs))
    }

    fn lte(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ComparisonBinary(Box::new(lhs), ComparisonOp::LessThanOrEqual, Box::new(rhs))
    }

    fn gt(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ComparisonBinary(Box::new(lhs), ComparisonOp::GreaterThan, Box::new(rhs))
    }

    fn gte(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ComparisonBinary(
            Box::new(lhs),
            ComparisonOp::GreaterThanOrEqual,
            Box::new(rhs),
        )
    }

    fn eq(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ComparisonBinary(Box::new(lhs), ComparisonOp::Equal, Box::new(rhs))
    }

    fn neq(lhs: Expr, rhs: Expr) -> Expr {
        Expr::ComparisonBinary(Box::new(lhs), ComparisonOp::NotEqual, Box::new(rhs))
    }

    fn min(args: Vec<Expr>) -> Expr {
        Expr::Call(Function::Min, args)
    }

    fn max(args: Vec<Expr>) -> Expr {
        Expr::Call(Function::Max, args)
    }

    fn if_(cond: Expr, then: Expr, else_: Expr) -> Expr {
        Expr::Call(Function::If, vec![cond, then, else_])
    }
}
