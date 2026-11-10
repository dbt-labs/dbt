use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::LazyLock;

use crate::compiler::ast::{self, Comment, MacroKind, Spanned};
use crate::compiler::lexer::{Tokenizer, WhitespaceConfig};
use crate::compiler::tokens::{Span, Token};
use crate::error::{Error, ErrorKind};
use crate::layout::JinjaLayoutEventKind;
use crate::listener::{BlockNameKind, TokenizerEventListener};
use crate::syntax::SyntaxConfig;
use crate::value::Value;

const MAX_RECURSION: usize = 100;
const RESERVED_NAMES: [&str; 10] = [
    "true", "True", "TRUE", "false", "False", "FALSE", "none", "None", "loop", "self",
];

#[allow(clippy::incompatible_msrv)]
static ENDBLOCK_IDENT: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut m = HashSet::new();
    m.insert("endfor");
    m.insert("endwith");
    m.insert("endset");
    m.insert("endblock");
    m.insert("endautoescape");
    m.insert("endfilter");
    m.insert("endmacro");
    m.insert("endcall");
    m.insert("endtest");
    m.insert("endsnapshot");
    m.insert("enddocs");
    m.insert("endmaterialization");
    // 'If' statement related idents
    m.insert("endif");
    m.insert("else");
    m.insert("elif");
    m
});

fn unexpected<D: fmt::Display>(unexpected: D, expected: &str) -> Error {
    Error::new(
        ErrorKind::SyntaxError,
        format!("unexpected {unexpected}, expected {expected}"),
    )
}

fn unexpected_eof(expected: &str) -> Error {
    unexpected("end of input", expected)
}

fn make_const(value: Value, span: Span) -> ast::Expr<'static> {
    ast::Expr::Const(Spanned::new(ast::Const { value }, span))
}

fn syntax_error(msg: Cow<'static, str>) -> Error {
    Error::new(ErrorKind::SyntaxError, msg)
}

macro_rules! syntax_error {
    ($msg:expr, $filename:expr, $span:expr) => {{
        return Err(syntax_error(Cow::Borrowed($msg)).with_span(&PathBuf::from($filename), $span));
    }};
    ($msg:expr, $filename:expr, $span:expr, $($tt:tt)*) => {{
        return Err(syntax_error(Cow::Owned(format!($msg, $($tt)*))).with_span($filename, $span));
    }};
}

macro_rules! expect_token {
    ($parser:expr, $expectation:expr) => {{
        match ok!($parser.stream.next()) {
            Some(rv) => rv,
            None => return Err(unexpected_eof($expectation)),
        }
    }};
    ($parser:expr, $match:pat, $expectation:expr) => {{
        match ok!($parser.stream.next()) {
            Some((token @ $match, span)) => (token, span),
            Some((token, span)) => {
                return Err(unexpected(token, $expectation)
                    .with_span(&PathBuf::from($parser.filename()), &span))
            }
            None => return Err(unexpected_eof($expectation)),
        }
    }};
    ($parser:expr, $match:pat => $target:expr, $expectation:expr) => {{
        match ok!($parser.stream.next()) {
            Some(($match, span)) => ($target, span),
            Some((token, span)) => {
                return Err(unexpected(token, $expectation)
                    .with_span(&PathBuf::from($parser.filename()), &span))
            }
            None => return Err(unexpected_eof($expectation)),
        }
    }};
}

macro_rules! matches_token {
    ($p:expr, $match:pat) => {
        match $p.stream.current() {
            Err(err) => return Err(err),
            Ok(Some(($match, _))) => true,
            _ => false,
        }
    };
}

macro_rules! skip_token {
    ($p:expr, $match:pat) => {
        match $p.stream.current() {
            Err(err) => return Err(err),
            Ok(Some(($match, _))) => {
                let _ = $p.stream.next();
                true
            }
            _ => false,
        }
    };
}

enum SetParseResult<'a> {
    Set(ast::Set<'a>),
    SetBlock(ast::SetBlock<'a>),
}

pub struct TokenStream<'a> {
    tokenizer: Tokenizer<'a>,
    current: Option<Result<(Token<'a>, Span), Error>>,
    last_span: Span,
}

impl<'a> TokenStream<'a> {
    /// Tokenize a template
    pub fn new(
        source: &'a str,
        filename: &'a str,
        in_expr: bool,
        syntax_config: SyntaxConfig,
        whitespace_config: WhitespaceConfig,
    ) -> TokenStream<'a> {
        Self::new_with_tokenizer_listeners(
            source,
            filename,
            in_expr,
            syntax_config,
            whitespace_config,
            &[],
        )
    }

    /// Tokenize a template and notify listeners.
    pub fn new_with_tokenizer_listeners(
        source: &'a str,
        filename: &'a str,
        in_expr: bool,
        syntax_config: SyntaxConfig,
        whitespace_config: WhitespaceConfig,
        source_listeners: &[Rc<dyn TokenizerEventListener>],
    ) -> TokenStream<'a> {
        let mut tokenizer = Tokenizer::new_with_tokenizer_listeners(
            source,
            filename,
            in_expr,
            syntax_config,
            whitespace_config,
            source_listeners,
        );
        let current = tokenizer.next_token().transpose();
        TokenStream {
            tokenizer,
            current,
            last_span: Span::default(),
        }
    }

    /// Advance the stream.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<(Token<'a>, Span)>, Error> {
        let rv = self.current.take();
        self.current = self.tokenizer.next_token().transpose();
        if let Some(Ok((_, span))) = &rv {
            self.last_span = *span;
        }
        rv.transpose()
    }

    /// Look at the current token
    pub fn current(&mut self) -> Result<Option<(&Token<'a>, Span)>, Error> {
        match self.current {
            Some(Ok(ref tok)) => Ok(Some((&tok.0, tok.1))),
            Some(Err(_)) => Err(self.current.take().unwrap().unwrap_err()),
            None => Ok(None),
        }
    }

    /// Expands the span
    #[inline(always)]
    pub fn expand_span(&self, mut span: Span) -> Span {
        span.end_line = self.last_span.end_line;
        span.end_col = self.last_span.end_col;
        span.end_offset = self.last_span.end_offset;
        span
    }

    /// Returns the current span.
    #[inline(always)]
    pub fn current_span(&self) -> Span {
        if let Some(Ok((_, span))) = &self.current {
            *span
        } else {
            self.last_span
        }
    }

    /// Returns the last seen span.
    #[inline(always)]
    pub fn last_span(&self) -> Span {
        self.last_span
    }
}

pub struct Parser<'a> {
    stream: TokenStream<'a>,
    in_macro: bool,
    in_loop: bool,
    blocks: BTreeSet<&'a str>,
    depth: usize,
    string_interner: HashMap<String, &'a str>,
    ignore_unknown_stmts: bool,
    filename: PathBuf,
}

macro_rules! binop {
    ($func:ident, $next:ident, { $($tok:tt)* }) => {
        fn $func(&mut self) -> Result<ast::Expr<'a>, Error> {
            let span = self.stream.current_span();
            let mut left = ok!(self.$next());
            loop {
                let op = match ok!(self.stream.current()) {
                    $($tok)*
                    _ => break,
                };
                ok!(self.stream.next());
                let right = ok!(self.$next());
                left = ast::Expr::BinOp(Spanned::new(
                    ast::BinOp { op, left, right, },
                    self.stream.expand_span(span),
                ));
            }
            Ok(left)
        }
    };
}

macro_rules! unaryop {
    ($func:ident, $next:ident, { $($tok:tt)* }) => {
        fn $func(&mut self) -> Result<ast::Expr<'a>, Error> {
            let span = self.stream.current_span();
            let op = match ok!(self.stream.current()) {
                $($tok)*
                _ => return self.$next()
            };
            ok!(self.stream.next());
            let mut expr = ok!(self.$func());
            expr = ok!(self.parse_postfix(expr, span));
            Ok(ast::Expr::UnaryOp(Spanned::new(
                ast::UnaryOp { op, expr },
                self.stream.expand_span(span),
            )))
        }
    };
}

macro_rules! with_recursion_guard {
    ($parser:expr, $expr:expr) => {{
        $parser.depth += 1;
        if $parser.depth > MAX_RECURSION {
            return Err(syntax_error(Cow::Borrowed(
                "template exceeds maximum recursion limits",
            )));
        }
        let rv = $expr;
        $parser.depth -= 1;
        rv
    }};
}

impl<'a> Parser<'a> {
    /// Creates a new parser.
    ///
    /// `in_expr` is necessary to parse within an expression context. Otherwise,
    /// the parser starts out in template context.  This means that when
    /// [`parse`](Self::parse) is to be called, the `in_expr` argument must be
    /// `false` and for [`parse_standalone_expr`](Self::parse_standalone_expr)
    /// it must be `true`.
    pub fn new(
        source: &'a str,
        filename: &'a str,
        in_expr: bool,
        syntax_config: SyntaxConfig,
        whitespace_config: WhitespaceConfig,
    ) -> Parser<'a> {
        Self::new_with_tokenizer_listeners(
            source,
            filename,
            in_expr,
            syntax_config,
            whitespace_config,
            &[],
        )
    }

    /// Creates a new parser that notifies listeners as source tokens are emitted.
    pub fn new_with_tokenizer_listeners(
        source: &'a str,
        filename: &'a str,
        in_expr: bool,
        syntax_config: SyntaxConfig,
        whitespace_config: WhitespaceConfig,
        source_listeners: &[Rc<dyn TokenizerEventListener>],
    ) -> Parser<'a> {
        Parser {
            stream: TokenStream::new_with_tokenizer_listeners(
                source,
                filename,
                in_expr,
                syntax_config,
                whitespace_config,
                source_listeners,
            ),
            in_macro: false,
            in_loop: false,
            blocks: BTreeSet::new(),
            depth: 0,
            string_interner: HashMap::new(),
            ignore_unknown_stmts: false,
            filename: PathBuf::from(filename),
        }
    }

    /// Parses a template.
    pub fn parse(&mut self) -> Result<ast::Stmt<'a>, Error> {
        let span = self.stream.last_span();
        self.subparse(&|_| false, None).map(|children| {
            ast::Stmt::Template(Spanned::new(
                ast::Template { children },
                self.stream.expand_span(span),
            ))
        })
    }

    /// Parses an expression and asserts that there is no more input after it.
    pub fn parse_standalone_expr(&mut self) -> Result<ast::Expr<'a>, Error> {
        self.parse_expr().and_then(|result| {
            if ok!(self.stream.next()).is_some() {
                syntax_error!(
                    "unexpected input after expression",
                    &self.filename,
                    &self.stream.current_span()
                )
            } else {
                Ok(result)
            }
        })
    }

    /// Returns the current filename.
    pub fn filename(&self) -> &str {
        self.stream.tokenizer.filename()
    }

    fn parse_ifexpr(&mut self) -> Result<ast::Expr<'a>, Error> {
        let mut span = self.stream.last_span();
        let mut expr = ok!(self.parse_or());
        loop {
            if skip_token!(self, Token::Ident("if")) {
                let expr2 = ok!(self.parse_or());
                let expr3 = if skip_token!(self, Token::Ident("else")) {
                    Some(ok!(self.parse_ifexpr()))
                } else {
                    None
                };
                expr = ast::Expr::IfExpr(Spanned::new(
                    ast::IfExpr {
                        test_expr: expr2,
                        true_expr: expr,
                        false_expr: expr3,
                    },
                    self.stream.expand_span(span),
                ));
                span = self.stream.last_span();
            } else {
                break;
            }
        }
        Ok(expr)
    }

    binop!(parse_or, parse_and, {
        Some((Token::Ident("or"), _)) => ast::BinOpKind::ScOr,
    });
    binop!(parse_and, parse_not, {
        Some((Token::Ident("and"), _)) => ast::BinOpKind::ScAnd,
    });
    unaryop!(parse_not, parse_compare, {
        Some((Token::Ident("not"), _)) => ast::UnaryOpKind::Not,
    });

    fn parse_compare(&mut self) -> Result<ast::Expr<'a>, Error> {
        let mut span = self.stream.last_span();
        let mut expr = ok!(self.parse_math1());
        loop {
            let mut negated = false;
            let op = match ok!(self.stream.current()) {
                Some((Token::Eq, _)) => ast::BinOpKind::Eq,
                Some((Token::Ne, _)) => ast::BinOpKind::Ne,
                Some((Token::Lt, _)) => ast::BinOpKind::Lt,
                Some((Token::Lte, _)) => ast::BinOpKind::Lte,
                Some((Token::Gt, _)) => ast::BinOpKind::Gt,
                Some((Token::Gte, _)) => ast::BinOpKind::Gte,
                Some((Token::Ident("in"), _)) => ast::BinOpKind::In,
                Some((Token::Ident("not"), _)) => {
                    ok!(self.stream.next());
                    expect_token!(self, Token::Ident("in"), "in");
                    negated = true;
                    ast::BinOpKind::In
                }
                _ => break,
            };
            if !negated {
                ok!(self.stream.next());
            }
            expr = ast::Expr::BinOp(Spanned::new(
                ast::BinOp {
                    op,
                    left: expr,
                    right: ok!(self.parse_math1()),
                },
                self.stream.expand_span(span),
            ));
            if negated {
                expr = ast::Expr::UnaryOp(Spanned::new(
                    ast::UnaryOp {
                        op: ast::UnaryOpKind::Not,
                        expr,
                    },
                    self.stream.expand_span(span),
                ));
            }
            span = self.stream.last_span();
        }
        Ok(expr)
    }

    binop!(parse_math1, parse_concat, {
        Some((Token::Plus, _)) => ast::BinOpKind::Add,
        Some((Token::Minus, _)) => ast::BinOpKind::Sub,
    });
    binop!(parse_concat, parse_math2, {
        Some((Token::Tilde, _)) => ast::BinOpKind::Concat,
    });
    binop!(parse_math2, parse_pow, {
        Some((Token::Mul, _)) => ast::BinOpKind::Mul,
        Some((Token::Div, _)) => ast::BinOpKind::Div,
        Some((Token::FloorDiv, _)) => ast::BinOpKind::FloorDiv,
        Some((Token::Mod, _)) => ast::BinOpKind::Rem,
    });
    binop!(parse_pow, parse_unary, {
        Some((Token::Pow, _)) => ast::BinOpKind::Pow,
    });
    unaryop!(parse_unary_only, parse_primary, {
        Some((Token::Minus, _)) => ast::UnaryOpKind::Neg,
        Some((Token::Plus, _)) => ast::UnaryOpKind::Pos,
    });

    fn parse_unary(&mut self) -> Result<ast::Expr<'a>, Error> {
        let span = self.stream.current_span();
        let mut expr = ok!(self.parse_unary_only());
        expr = ok!(self.parse_postfix(expr, span));
        self.parse_filter_expr(expr)
    }

    fn parse_postfix(
        &mut self,
        expr: ast::Expr<'a>,
        mut span: Span,
    ) -> Result<ast::Expr<'a>, Error> {
        let mut expr = expr;
        loop {
            let next_span = self.stream.current_span();
            match ok!(self.stream.current()) {
                Some((Token::Dot, _)) => {
                    ok!(self.stream.next());
                    let (tok, _) = {
                        match ok!(self.stream.next()) {
                            Some((Token::Ident(name), span)) => (Token::Ident(name), span),
                            Some((Token::Int(number), span)) => (Token::Int(number), span),
                            Some((token, _)) => {
                                return Err(unexpected(token, "identifier or integer"))
                            }
                            None => return Err(unexpected_eof("identifier or integer")),
                        }
                    };
                    match tok {
                        Token::Ident(name) => {
                            expr = ast::Expr::GetAttr(Spanned::new(
                                ast::GetAttr { name, expr },
                                self.stream.expand_span(span),
                            ));
                        }
                        Token::Int(number) => {
                            let subscript_expr = make_const(Value::from(number), span);
                            expr = ast::Expr::GetItem(Spanned::new(
                                ast::GetItem {
                                    expr,
                                    subscript_expr,
                                },
                                self.stream.expand_span(span),
                            ));
                        }
                        _ => {}
                    }
                }
                Some((Token::BracketOpen, _)) => {
                    ok!(self.stream.next());

                    let mut start = None;
                    let mut stop = None;
                    let mut step = None;
                    let mut items = vec![];
                    let mut is_slice = false;
                    let mut is_tuple = false;

                    if !matches_token!(self, Token::Colon) {
                        start = Some(ok!(self.parse_expr()));
                    }

                    if skip_token!(self, Token::Colon) {
                        is_slice = true;
                        if !matches_token!(self, Token::BracketClose | Token::Colon) {
                            stop = Some(ok!(self.parse_expr()));
                        }
                        if skip_token!(self, Token::Colon)
                            && !matches_token!(self, Token::BracketClose)
                        {
                            step = Some(ok!(self.parse_expr()));
                        }
                    } else if skip_token!(self, Token::Comma) {
                        is_tuple = true;
                        loop {
                            if matches_token!(self, Token::BracketClose) {
                                break;
                            }
                            items.push(ok!(self.parse_expr()));
                            if !skip_token!(self, Token::Comma) {
                                break;
                            }
                        }
                    }

                    expect_token!(self, Token::BracketClose, "`]`");

                    if is_slice {
                        expr = ast::Expr::Slice(Spanned::new(
                            ast::Slice {
                                expr,
                                start,
                                stop,
                                step,
                            },
                            self.stream.expand_span(span),
                        ));
                    } else if is_tuple {
                        let items = start.into_iter().chain(items).collect();
                        expr = ast::Expr::GetItem(Spanned::new(
                            ast::GetItem {
                                expr,
                                subscript_expr: ast::Expr::List(Spanned::new(
                                    ast::List { items },
                                    self.stream.expand_span(span),
                                )),
                            },
                            self.stream.expand_span(span),
                        ));
                    } else {
                        assert!(items.is_empty());
                        expr = ast::Expr::GetItem(Spanned::new(
                            ast::GetItem {
                                expr,
                                subscript_expr: ok!(start.ok_or_else(|| {
                                    syntax_error(Cow::Borrowed("empty subscript"))
                                })),
                            },
                            self.stream.expand_span(span),
                        ));
                    }
                }
                Some((Token::ParenOpen, _)) => {
                    let args = ok!(self.parse_args());
                    expr = ast::Expr::Call(Spanned::new(
                        ast::Call { expr, args },
                        self.stream.expand_span(span),
                    ));
                }
                _ => break,
            }
            span = next_span;
        }
        Ok(expr)
    }

    fn parse_filter_expr(&mut self, expr: ast::Expr<'a>) -> Result<ast::Expr<'a>, Error> {
        let mut expr = expr;
        loop {
            match ok!(self.stream.current()) {
                Some((Token::Pipe, _)) => {
                    ok!(self.stream.next());
                    let (name, span) =
                        expect_token!(self, Token::Ident(name) => name, "identifier");
                    let args = if matches_token!(self, Token::ParenOpen) {
                        ok!(self.parse_args())
                    } else {
                        Vec::new()
                    };
                    expr = ast::Expr::Filter(Spanned::new(
                        ast::Filter {
                            name,
                            expr: Some(expr),
                            args,
                        },
                        self.stream.expand_span(span),
                    ));
                    // Allow postfix attribute/item access on the filter result,
                    // e.g. `rows|first.col` is valid Jinja2 and means `(rows|first).col`.
                    expr = ok!(self.parse_postfix(expr, span));
                }
                Some((Token::Ident("is"), _)) => {
                    ok!(self.stream.next());
                    let negated = skip_token!(self, Token::Ident("not"));
                    let (name, span) =
                        expect_token!(self, Token::Ident(name) => name, "identifier");
                    let args = if matches_token!(self, Token::ParenOpen) {
                        ok!(self.parse_args())
                    } else if matches_token!(
                        self,
                        Token::Ident(_)
                            | Token::Str(_)
                            | Token::String(_)
                            | Token::Int(_)
                            | Token::Int128(_)
                            | Token::Float(_)
                            | Token::Plus
                            | Token::Minus
                            | Token::BracketOpen
                            | Token::BraceOpen
                    ) && !matches_token!(
                        self,
                        Token::Ident("and")
                            | Token::Ident("or")
                            | Token::Ident("else")
                            | Token::Ident("is")
                    ) {
                        let span = self.stream.current_span();
                        let mut expr = ok!(self.parse_unary_only());
                        expr = ok!(self.parse_postfix(expr, span));
                        vec![ast::CallArg::Pos(expr)]
                    } else {
                        Vec::new()
                    };
                    expr = ast::Expr::Test(Spanned::new(
                        ast::Test { name, expr, args },
                        self.stream.expand_span(span),
                    ));
                    if negated {
                        expr = ast::Expr::UnaryOp(Spanned::new(
                            ast::UnaryOp {
                                op: ast::UnaryOpKind::Not,
                                expr,
                            },
                            self.stream.expand_span(span),
                        ));
                    }
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_args(&mut self) -> Result<Vec<ast::CallArg<'a>>, Error> {
        let mut args = Vec::new();
        let mut first_span = None;
        let mut has_kwargs = false;

        enum ArgType {
            Regular,
            Splat,
            KwargsSplat,
        }

        expect_token!(self, Token::ParenOpen, "`(`");
        loop {
            if skip_token!(self, Token::ParenClose) {
                break;
            }
            if !args.is_empty() || has_kwargs {
                expect_token!(self, Token::Comma, "`,`");
                if skip_token!(self, Token::ParenClose) {
                    break;
                }
            }

            let arg_type = if skip_token!(self, Token::Pow) {
                ArgType::KwargsSplat
            } else if skip_token!(self, Token::Mul) {
                ArgType::Splat
            } else {
                ArgType::Regular
            };

            let expr = ok!(self.parse_expr());

            match arg_type {
                ArgType::Regular => {
                    // keyword argument
                    match expr {
                        ast::Expr::Var(ref var) if skip_token!(self, Token::Assign) => {
                            if first_span.is_none() {
                                first_span = Some(var.span());
                            }
                            has_kwargs = true;
                            args.push(ast::CallArg::Kwarg(var.id, ok!(self.parse_expr())));
                        }
                        _ if has_kwargs => {
                            return Err(syntax_error(Cow::Borrowed(
                                "non-keyword arg after keyword arg",
                            )));
                        }
                        _ => {
                            args.push(ast::CallArg::Pos(expr));
                        }
                    }
                }
                ArgType::Splat => {
                    args.push(ast::CallArg::PosSplat(expr));
                }
                ArgType::KwargsSplat => {
                    args.push(ast::CallArg::KwargSplat(expr));
                    has_kwargs = true;
                }
            }

            // Set an arbitrary limit of max function parameters.  This is done
            // in parts because the opcodes can only express 2**16 as argument
            // count.
            if args.len() > 2000 {
                syntax_error!(
                    "Too many arguments in function call",
                    &self.filename,
                    &self.stream.current_span()
                )
            }
        }

        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<ast::Expr<'a>, Error> {
        with_recursion_guard!(self, self.parse_primary_impl())
    }

    fn parse_primary_impl(&mut self) -> Result<ast::Expr<'a>, Error> {
        let (token, span) = expect_token!(self, "expression");
        macro_rules! const_val {
            ($expr:expr) => {
                make_const(Value::from($expr), self.stream.expand_span(span))
            };
        }

        match token {
            // An identifier spelled like a literal keyword (`true`/`false`/`none`) is a valid
            // keyword-argument *name* when immediately followed by `=`, e.g. `namespace(none=0)`.
            // Python Jinja recognizes this at the token level in `parse_call_args`; here we must
            // avoid collapsing it into a `Const` so `parse_args` can still see an `Expr::Var`.
            // A bare literal followed by a single `=` is never a valid value expression, and
            // comparison uses `==` (`Token::Eq`), so this only affects the kwarg-name case.
            Token::Ident(
                name @ ("true" | "True" | "TRUE" | "false" | "False" | "FALSE" | "none" | "None"),
            ) if matches!(ok!(self.stream.current()), Some((Token::Assign, _))) => {
                Ok(ast::Expr::Var(Spanned::new(ast::Var { id: name }, span)))
            }
            Token::Ident("true" | "True" | "TRUE") => Ok(const_val!(true)),
            Token::Ident("false" | "False" | "FALSE") => Ok(const_val!(false)),
            Token::Ident("none" | "None") => Ok(const_val!(())),
            Token::Ident(name) => Ok(ast::Expr::Var(Spanned::new(ast::Var { id: name }, span))),
            Token::Str(_) | Token::String(_) => {
                let mut buf = match token {
                    Token::Str(s) => s.to_owned(),
                    Token::String(s) => s,
                    _ => unreachable!(),
                };
                loop {
                    match ok!(self.stream.current()) {
                        Some((Token::Str(s), _)) => buf.push_str(s),
                        Some((Token::String(s), _)) => buf.push_str(s),
                        _ => break,
                    }
                    ok!(self.stream.next());
                }
                Ok(const_val!(buf))
            }
            Token::Int(val) => Ok(const_val!(val)),
            Token::Int128(val) => Ok(const_val!(val)),
            Token::Float(val) => Ok(const_val!(val)),
            Token::ParenOpen => self.parse_tuple_or_expression(span),
            Token::BracketOpen => self.parse_list_expr(span),
            Token::BraceOpen => self.parse_map_expr(span),
            token => syntax_error!("unexpected {}", &self.filename, &span, token),
        }
    }

    fn parse_list_expr(&mut self, span: Span) -> Result<ast::Expr<'a>, Error> {
        let mut items = Vec::new();
        loop {
            if skip_token!(self, Token::BracketClose) {
                break;
            }
            if !items.is_empty() {
                expect_token!(self, Token::Comma, "`,`");
                if skip_token!(self, Token::BracketClose) {
                    break;
                }
            }
            items.push(ok!(self.parse_expr()));
        }
        Ok(ast::Expr::List(Spanned::new(
            ast::List { items },
            self.stream.expand_span(span),
        )))
    }

    fn parse_map_expr(&mut self, span: Span) -> Result<ast::Expr<'a>, Error> {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        loop {
            if skip_token!(self, Token::BraceClose) {
                break;
            }
            if !keys.is_empty() {
                expect_token!(self, Token::Comma, "`,`");
                if skip_token!(self, Token::BraceClose) {
                    break;
                }
            }
            keys.push(ok!(self.parse_expr()));
            expect_token!(self, Token::Colon, "`:`");
            values.push(ok!(self.parse_expr()));
        }
        Ok(ast::Expr::Map(Spanned::new(
            ast::Map { keys, values },
            self.stream.expand_span(span),
        )))
    }

    fn parse_tuple_or_expression(&mut self, span: Span) -> Result<ast::Expr<'a>, Error> {
        // MiniJinja does not really have tuples, but it treats the tuple
        // syntax the same as lists.
        if skip_token!(self, Token::ParenClose) {
            return Ok(ast::Expr::Tuple(Spanned::new(
                ast::Tuple { items: vec![] },
                self.stream.expand_span(span),
            )));
        }
        let mut expr = ok!(self.parse_expr());
        if matches_token!(self, Token::Comma) {
            let mut items = vec![expr];
            loop {
                if skip_token!(self, Token::ParenClose) {
                    break;
                }
                expect_token!(self, Token::Comma, "`,`");
                if skip_token!(self, Token::ParenClose) {
                    break;
                }
                items.push(ok!(self.parse_expr()));
            }
            expr = ast::Expr::Tuple(Spanned::new(
                ast::Tuple { items },
                self.stream.expand_span(span),
            ));
        } else {
            expect_token!(self, Token::ParenClose, "`)`");
        }
        Ok(expr)
    }

    fn parse_expr(&mut self) -> Result<ast::Expr<'a>, Error> {
        with_recursion_guard!(self, self.parse_ifexpr())
    }

    fn parse_expr_or_implied_tuple(&mut self) -> Result<ast::Expr<'a>, Error> {
        let first = ok!(self.parse_expr());
        if skip_token!(self, Token::Comma) {
            let mut items = vec![first];
            loop {
                if matches_token!(self, Token::VariableEnd) {
                    break;
                }
                items.push(ok!(self.parse_expr()));
                if !skip_token!(self, Token::Comma) {
                    break;
                }
            }
            Ok(ast::Expr::Tuple(Spanned::new(
                ast::Tuple { items },
                self.stream.expand_span(self.stream.last_span()),
            )))
        } else {
            Ok(first)
        }
    }

    fn parse_expr_noif_or_implied_tuple(&mut self) -> Result<ast::Expr<'a>, Error> {
        let first = ok!(self.parse_expr_noif());
        if skip_token!(self, Token::Comma) {
            let mut items = vec![first];
            loop {
                if matches_token!(self, Token::BlockEnd) || matches_token!(self, Token::Colon) {
                    break;
                }
                items.push(ok!(self.parse_expr_noif()));
                if !skip_token!(self, Token::Comma) {
                    break;
                }
            }
            Ok(ast::Expr::Tuple(Spanned::new(
                ast::Tuple { items },
                self.stream.expand_span(self.stream.last_span()),
            )))
        } else {
            Ok(first)
        }
    }

    fn parse_expr_noif(&mut self) -> Result<ast::Expr<'a>, Error> {
        self.parse_or()
    }

    fn parse_stmt(&mut self) -> Result<ast::Stmt<'a>, Error> {
        with_recursion_guard!(self, self.parse_stmt_unprotected())
    }

    fn parse_stmt_unprotected(&mut self) -> Result<ast::Stmt<'a>, Error> {
        let span = self.stream.last_span();
        let (token, _span) = expect_token!(self, "block keyword");

        macro_rules! respan {
            ($expr:expr) => {{
                let node = $expr;
                expect_token!(self, Token::BlockEnd, "end of block");
                Spanned::new(node, self.stream.expand_span(span))
            }};
        }

        let ident = match token {
            Token::Ident(ident) => ident,
            token => syntax_error!(
                "unknown {}, expected statement",
                &self.filename,
                &span,
                token
            ),
        };

        Ok(match ident {
            "for" => {
                let node = ok!(self.parse_for_stmt(span));
                ast::Stmt::ForLoop(Spanned::new(node, self.stream.expand_span(span)))
            }
            "if" => {
                let node = ok!(self.parse_if_cond(span, JinjaLayoutEventKind::BlockStart));
                ast::Stmt::IfCond(Spanned::new(node, self.stream.expand_span(span)))
            }
            "with" => {
                let node = ok!(self.parse_with_block(span));
                ast::Stmt::WithBlock(Spanned::new(node, self.stream.expand_span(span)))
            }
            "set" => match ok!(self.parse_set(span)) {
                SetParseResult::Set(rv) => ast::Stmt::Set(respan!(rv)),
                // `parse_set`'s `SetBlock` branch already consumes the
                // trailing `%}` itself (to compute its own `end_tag_span`),
                // so (unlike `Set`) `respan!` must not be used here.
                SetParseResult::SetBlock(rv) => {
                    ast::Stmt::SetBlock(Spanned::new(rv, self.stream.expand_span(span)))
                }
            },
            "autoescape" => {
                let node = ok!(self.parse_auto_escape(span));
                ast::Stmt::AutoEscape(Spanned::new(node, self.stream.expand_span(span)))
            }
            "filter" => {
                let node = ok!(self.parse_filter_block(span));
                ast::Stmt::FilterBlock(Spanned::new(node, self.stream.expand_span(span)))
            }
            #[cfg(feature = "multi_template")]
            "block" => {
                let node = ok!(self.parse_block(span));
                ast::Stmt::Block(Spanned::new(node, self.stream.expand_span(span)))
            }
            #[cfg(feature = "multi_template")]
            "extends" => ast::Stmt::Extends(respan!(ok!(self.parse_extends()))),
            #[cfg(feature = "multi_template")]
            "include" => ast::Stmt::Include(respan!(ok!(self.parse_include()))),
            #[cfg(feature = "multi_template")]
            "import" => ast::Stmt::Import(respan!(ok!(self.parse_import()))),
            #[cfg(feature = "multi_template")]
            "from" => ast::Stmt::FromImport(respan!(ok!(self.parse_from_import()))),
            #[cfg(feature = "macros")]
            "macro" => {
                let node = ok!(self.parse_macro(span));
                ast::Stmt::Macro((
                    Spanned::new(node, self.stream.expand_span(span)),
                    MacroKind::Macro,
                    BTreeMap::new(),
                ))
            }
            #[cfg(feature = "macros")]
            "test" => {
                let node = ok!(self.parse_test(span));
                ast::Stmt::Macro((
                    Spanned::new(node, self.stream.expand_span(span)),
                    MacroKind::Test,
                    BTreeMap::new(),
                ))
            }
            #[cfg(feature = "macros")]
            "snapshot" => {
                let node = ok!(self.parse_snapshot(span));
                ast::Stmt::Macro((
                    Spanned::new(node, self.stream.expand_span(span)),
                    MacroKind::Snapshot,
                    BTreeMap::new(),
                ))
            }
            #[cfg(feature = "macros")]
            // `parse_doc` consumes the closing `%}` itself, so `respan!` (which
            // insists on `BlockEnd` next) must not be used here.
            "docs" => {
                let node = ok!(self.parse_doc(span));
                let doc_span = self.stream.expand_span(span);
                match node {
                    Some(node) => ast::Stmt::Macro((
                        Spanned::new(node, doc_span),
                        MacroKind::Doc,
                        BTreeMap::new(),
                    )),
                    // Malformed block name: skipped, but the file keeps parsing.
                    None => ast::Stmt::Comment(Spanned::new(ast::Comment, doc_span)),
                }
            }
            #[cfg(feature = "macros")]
            "materialization" => {
                let (macro_, adapter, supported_languages) = ok!(self.parse_materialization(span));
                let mut meta = BTreeMap::from([("adapter".to_string(), Value::from(adapter))]);
                if let Some(supported_languages) = supported_languages {
                    meta.insert("supported_languages".to_string(), supported_languages);
                }
                ast::Stmt::Macro((
                    Spanned::new(macro_, self.stream.expand_span(span)),
                    MacroKind::Materialization,
                    meta,
                ))
            }
            #[cfg(feature = "macros")]
            "call" => {
                let node = ok!(self.parse_call_block(span));
                ast::Stmt::CallBlock(Spanned::new(node, self.stream.expand_span(span)))
            }
            #[cfg(feature = "loop_controls")]
            "continue" => {
                if !self.in_loop {
                    syntax_error!(
                        "'continue' must be placed inside a loop",
                        &self.filename,
                        &span
                    )
                }
                ast::Stmt::Continue(respan!(ast::Continue))
            }
            #[cfg(feature = "loop_controls")]
            "break" => {
                if !self.in_loop {
                    syntax_error!(
                        "'break' must be placed inside a loop",
                        &self.filename,
                        &span
                    )
                }
                ast::Stmt::Break(respan!(ast::Break))
            }
            "do" => ast::Stmt::Do(respan!(ok!(self.parse_do()))),
            "print" => {
                // remark, this is an error in Jinja2, but dbt swallows it...
                let ignore = ast::Stmt::Do(respan!(ok!(self.parse_do())));
                // ... so we translate it in a semantic skip! This `WithBlock`
                // is entirely synthetic — there's no real `{% with %}`/
                // `{% endwith %}` tag pair in the source, just the one
                // `{% print %}` tag standing in for it — so there's no tag
                // text to type; give both a zero-width span at the tag's
                // start so any consumer treating them as real tag ranges
                // skips them as empty rather than double-typing real bytes
                // that already belong to `ignore`'s own span.
                let zero_width_span = Span {
                    end_line: span.start_line,
                    end_col: span.start_col,
                    end_offset: span.start_offset,
                    ..span
                };
                ast::Stmt::WithBlock(Spanned::new(
                    ast::WithBlock {
                        assignments: vec![],
                        body: vec![ignore],
                        start_tag_span: zero_width_span,
                        end_tag_span: zero_width_span,
                    },
                    self.stream.expand_span(span),
                ))
            }
            name => {
                if self.ignore_unknown_stmts {
                    // If we ignore unknown statements, we just skip them.
                    self.skip_until_block_end()?;
                    ast::Stmt::Comment(Spanned::new(ast::Comment, self.stream.expand_span(span)))
                } else {
                    syntax_error!("unknown statement {}", &self.filename, &span, name)
                }
            }
        })
    }

    fn parse_assign_name(&mut self, dotted: bool) -> Result<ast::Expr<'a>, Error> {
        let (id, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
        if RESERVED_NAMES.contains(&id) {
            syntax_error!(
                "cannot assign to reserved variable name {}",
                &self.filename,
                &span,
                id
            )
        }
        let mut rv = ast::Expr::Var(Spanned::new(ast::Var { id }, span));
        if dotted {
            while skip_token!(self, Token::Dot) {
                let (attr, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
                rv = ast::Expr::GetAttr(Spanned::new(
                    ast::GetAttr {
                        expr: rv,
                        name: attr,
                    },
                    span,
                ));
            }
        }
        Ok(rv)
    }

    fn parse_assignment(&mut self) -> Result<ast::Expr<'a>, Error> {
        let span = self.stream.current_span();
        let mut items = Vec::new();
        let mut is_tuple = false;

        loop {
            if !items.is_empty() {
                expect_token!(self, Token::Comma, "`,`");
            }
            if matches_token!(
                self,
                Token::ParenClose | Token::VariableEnd | Token::BlockEnd | Token::Ident("in")
            ) {
                break;
            }
            items.push(if skip_token!(self, Token::ParenOpen) {
                let rv = ok!(self.parse_assignment());
                expect_token!(self, Token::ParenClose, "`)`");
                rv
            } else {
                ok!(self.parse_assign_name(false))
            });
            if matches_token!(self, Token::Comma) {
                is_tuple = true;
            } else {
                break;
            }
        }

        if !is_tuple && items.len() == 1 {
            Ok(items.into_iter().next().unwrap())
        } else {
            Ok(ast::Expr::List(Spanned::new(
                ast::List { items },
                self.stream.expand_span(span),
            )))
        }
    }

    fn parse_for_stmt(&mut self, start_open_span: Span) -> Result<ast::ForLoop<'a>, Error> {
        let old_in_loop = std::mem::replace(&mut self.in_loop, true);
        let target = ok!(self.parse_assignment());
        expect_token!(self, Token::Ident("in"), "in");
        let mut iter = ok!(self.parse_expr_noif());
        if skip_token!(self, Token::Comma) {
            let mut items = vec![iter];
            loop {
                if matches_token!(self, Token::BlockEnd) {
                    break;
                }
                items.push(ok!(self.parse_expr_noif()));
                if !skip_token!(self, Token::Comma) {
                    break;
                }
            }
            iter = ast::Expr::List(Spanned::new(
                ast::List { items },
                self.stream.current_span(),
            ));
        }
        let filter_expr = if skip_token!(self, Token::Ident("if")) {
            Some(ok!(self.parse_expr()))
        } else {
            None
        };
        let recursive = skip_token!(self, Token::Ident("recursive"));
        skip_token!(self, Token::Colon);

        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let body = ok!(self.subparse(
            &|tok| matches!(tok, Token::Ident("endfor" | "else")),
            Some(("for", &["endfor", "else"])),
        ));
        let next_open_span = self.stream.last_span();
        let (else_body, else_tag_span, end_open_span) = match ok!(self.stream.next()) {
            Some((Token::Ident("else"), _)) => {
                expect_token!(self, Token::BlockEnd, "end of block");
                let else_tag_span = self.stream.expand_span(next_open_span);
                let else_body = ok!(self.subparse(
                    &|tok| matches!(tok, Token::Ident("endfor")),
                    Some(("for", &["endfor"])),
                ));
                let end_open_span = self.stream.last_span();
                expect_token!(self, Token::Ident("endfor"), "endfor");
                (else_body, Some(else_tag_span), end_open_span)
            }
            Some((Token::Ident("endfor"), _)) => (Vec::new(), None, next_open_span),
            Some((token, span)) => {
                syntax_error!(
                    "unexpected end of for-loop: expected endfor or else, got {}",
                    &self.filename,
                    &span,
                    token
                );
            }
            None => {
                return Err(unexpected_eof("endfor or else"));
            }
        };
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        self.in_loop = old_in_loop;
        Ok(ast::ForLoop {
            target,
            iter,
            filter_expr,
            recursive,
            body,
            else_body,
            start_tag_span,
            else_tag_span,
            end_tag_span,
        })
    }

    fn parse_if_cond(
        &mut self,
        start_open_span: Span,
        start_tag_kind: JinjaLayoutEventKind,
    ) -> Result<ast::IfCond<'a>, Error> {
        let expr = ok!(self.parse_expr_noif_or_implied_tuple());
        skip_token!(self, Token::Colon);
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let true_body = ok!(self.subparse(
            &|tok| matches!(tok, Token::Ident("endif" | "else" | "elif")),
            Some(("if", &["elif", "else", "endif"])),
        ));
        let next_open_span = self.stream.last_span();
        let (false_body, else_tag_span, end_tag_span) = match ok!(self.stream.next()) {
            Some((Token::Ident("else"), _)) => {
                expect_token!(self, Token::BlockEnd, "end of block");
                let else_tag_span = self.stream.expand_span(next_open_span);
                let rv = ok!(self.subparse(
                    &|tok| matches!(tok, Token::Ident("endif")),
                    Some(("if", &["endif"])),
                ));
                let end_open_span = self.stream.last_span();
                expect_token!(self, Token::Ident("endif"), "endif");
                expect_token!(self, Token::BlockEnd, "end of block");
                let end_tag_span = self.stream.expand_span(end_open_span);
                (rv, Some(else_tag_span), end_tag_span)
            }
            Some((Token::Ident("elif"), span)) => {
                let nested =
                    ok!(self.parse_if_cond(next_open_span, JinjaLayoutEventKind::BlockMid));
                let end_tag_span = nested.end_tag_span;
                (
                    vec![ast::Stmt::IfCond(Spanned::new(
                        nested,
                        self.stream.expand_span(span),
                    ))],
                    None,
                    end_tag_span,
                )
            }
            Some((Token::Ident("endif"), _)) => {
                expect_token!(self, Token::BlockEnd, "end of block");
                let end_tag_span = self.stream.expand_span(next_open_span);
                (Vec::new(), None, end_tag_span)
            }
            Some((token, span)) => {
                syntax_error!(
                    "unexpected end of if-block: expected endif, else, or elif, got {}",
                    &self.filename,
                    &span,
                    token
                );
            }
            None => {
                return Err(unexpected_eof("endif, else, or elif"));
            }
        };

        Ok(ast::IfCond {
            expr,
            true_body,
            false_body,
            start_tag_kind,
            start_tag_span,
            else_tag_span,
            end_tag_span,
        })
    }

    fn parse_with_block(&mut self, start_open_span: Span) -> Result<ast::WithBlock<'a>, Error> {
        let mut assignments = Vec::new();

        while !matches_token!(self, Token::BlockEnd) {
            if !assignments.is_empty() {
                expect_token!(self, Token::Comma, "comma");
            }
            let target = if skip_token!(self, Token::ParenOpen) {
                let assign = ok!(self.parse_assignment());
                expect_token!(self, Token::ParenClose, "`)`");
                assign
            } else {
                ok!(self.parse_assign_name(false))
            };
            expect_token!(self, Token::Assign, "assignment operator");
            let expr = ok!(self.parse_expr());
            assignments.push((target, expr));
        }

        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let body = ok!(self.subparse(
            &|tok| matches!(tok, Token::Ident("endwith")),
            Some(("with", &["endwith"])),
        ));
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::WithBlock {
            assignments,
            body,
            start_tag_span,
            end_tag_span,
        })
    }

    // both the left hand side and right hand side can be a list
    fn parse_set(&mut self, start_tag_span: Span) -> Result<SetParseResult<'a>, Error> {
        let in_paren = skip_token!(self, Token::ParenOpen);
        let mut targets = Vec::new();

        if in_paren {
            let assign = ok!(self.parse_assignment());
            expect_token!(self, Token::ParenClose, "`)`");
            targets.push(assign);
        } else {
            // support {{set a, b = [1,2]}}
            loop {
                targets.push(ok!(self.parse_assign_name(true)));
                if skip_token!(self, Token::Comma) {
                    if matches_token!(self, Token::BlockEnd) {
                        break;
                    }
                    continue;
                } else {
                    break;
                }
            }
        };
        if !in_paren && matches_token!(self, Token::BlockEnd | Token::Pipe) {
            let filter = if skip_token!(self, Token::Pipe) {
                Some(ok!(self.parse_filter_chain()))
            } else {
                None
            };
            expect_token!(self, Token::BlockEnd, "end of block");
            let start_tag_span = self.stream.expand_span(start_tag_span);
            let body = ok!(self.subparse(
                &|tok| matches!(tok, Token::Ident("endset")),
                Some(("set", &["endset"])),
            ));
            let end_open_span = self.stream.last_span();
            ok!(self.stream.next());
            expect_token!(self, Token::BlockEnd, "end of block");
            let end_tag_span = self.stream.expand_span(end_open_span);
            Ok(SetParseResult::SetBlock(ast::SetBlock {
                target: targets.into_iter().next().unwrap(),
                filter,
                body,
                start_tag_span,
                end_tag_span,
            }))
        } else {
            expect_token!(self, Token::Assign, "assignment operator");
            let mut exprs: Vec<ast::Expr<'a>> = Vec::new();
            // parse multiple righthand side expressions; a comma makes this a tuple
            let mut is_tuple = false;
            loop {
                exprs.push(ok!(self.parse_expr()));
                if skip_token!(self, Token::Comma) {
                    is_tuple = true;
                    if matches_token!(self, Token::BlockEnd) {
                        break;
                    }
                    continue;
                } else {
                    break;
                }
            }
            Ok(SetParseResult::Set(ast::Set {
                target: if targets.len() == 1 {
                    targets.into_iter().next().unwrap()
                } else {
                    ast::Expr::List(Spanned::new(
                        ast::List { items: targets },
                        self.stream.current_span(),
                    ))
                },
                expr: if !is_tuple {
                    exprs.into_iter().next().unwrap()
                } else {
                    ast::Expr::Tuple(Spanned::new(
                        ast::Tuple { items: exprs },
                        self.stream.current_span(),
                    ))
                },
            }))
        }
    }

    #[cfg(feature = "multi_template")]
    fn parse_block(&mut self, start_open_span: Span) -> Result<ast::Block<'a>, Error> {
        if self.in_macro {
            syntax_error!(
                "block tags in macros are not allowed",
                &self.filename,
                &self.stream.current_span()
            )
        }
        let old_in_loop = std::mem::replace(&mut self.in_loop, false);
        let (name, _) = expect_token!(self, Token::Ident(name) => name, "identifier");
        if !self.blocks.insert(name) {
            syntax_error!(
                "block '{}' defined twice",
                &self.filename,
                &self.stream.current_span(),
                name
            )
        }

        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let body = ok!(self.subparse(
            &|tok| matches!(tok, Token::Ident("endblock")),
            Some(("block", &["endblock"])),
        ));
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());

        if let Some((Token::Ident(trailing_name), _)) = ok!(self.stream.current()) {
            if *trailing_name != name {
                syntax_error!(
                    "mismatching name on block. Got `{}`, expected `{}`",
                    &self.filename,
                    &self.stream.current_span(),
                    *trailing_name,
                    name
                );
            }
            ok!(self.stream.next());
        }
        self.in_loop = old_in_loop;

        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);

        Ok(ast::Block {
            name,
            body,
            start_tag_span,
            end_tag_span,
        })
    }
    fn parse_auto_escape(&mut self, start_open_span: Span) -> Result<ast::AutoEscape<'a>, Error> {
        let enabled = ok!(self.parse_expr());
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let body = ok!(self.subparse(
            &|tok| matches!(tok, Token::Ident("endautoescape")),
            Some(("autoescape", &["endautoescape"])),
        ));
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::AutoEscape {
            enabled,
            body,
            start_tag_span,
            end_tag_span,
        })
    }

    fn parse_filter_chain(&mut self) -> Result<ast::Expr<'a>, Error> {
        let mut filter = None;

        while !matches_token!(self, Token::BlockEnd) {
            if filter.is_some() {
                expect_token!(self, Token::Pipe, "`|`");
            }
            let (name, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
            let args = if matches_token!(self, Token::ParenOpen) {
                ok!(self.parse_args())
            } else {
                Vec::new()
            };
            filter = Some(ast::Expr::Filter(Spanned::new(
                ast::Filter {
                    name,
                    expr: filter,
                    args,
                },
                self.stream.expand_span(span),
            )));
        }

        filter.ok_or_else(|| syntax_error(Cow::Borrowed("expected a filter")))
    }

    fn parse_filter_block(&mut self, start_open_span: Span) -> Result<ast::FilterBlock<'a>, Error> {
        let filter = ok!(self.parse_filter_chain());
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let body = ok!(self.subparse(
            &|tok| matches!(tok, Token::Ident("endfilter")),
            Some(("filter", &["endfilter"])),
        ));
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::FilterBlock {
            filter,
            body,
            start_tag_span,
            end_tag_span,
        })
    }

    #[cfg(feature = "multi_template")]
    fn parse_extends(&mut self) -> Result<ast::Extends<'a>, Error> {
        let name = ok!(self.parse_expr());
        Ok(ast::Extends { name })
    }

    #[cfg(feature = "multi_template")]
    fn parse_include(&mut self) -> Result<ast::Include<'a>, Error> {
        let name = ok!(self.parse_expr());

        // with/without context is without meaning in MiniJinja, but for syntax
        // compatibility it's supported.
        if skip_token!(self, Token::Ident("without" | "with")) {
            expect_token!(self, Token::Ident("context"), "missing keyword");
        }

        let ignore_missing = if skip_token!(self, Token::Ident("ignore")) {
            expect_token!(self, Token::Ident("missing"), "missing keyword");
            if skip_token!(self, Token::Ident("without" | "with")) {
                expect_token!(self, Token::Ident("context"), "missing keyword");
            }
            true
        } else {
            false
        };
        Ok(ast::Include {
            name,
            ignore_missing,
        })
    }

    #[cfg(feature = "multi_template")]
    fn parse_import(&mut self) -> Result<ast::Import<'a>, Error> {
        let expr = ok!(self.parse_expr());
        expect_token!(self, Token::Ident("as"), "as");
        let name = ok!(self.parse_expr());
        Ok(ast::Import { expr, name })
    }

    #[cfg(feature = "multi_template")]
    fn parse_from_import(&mut self) -> Result<ast::FromImport<'a>, Error> {
        let expr = ok!(self.parse_expr());
        let mut names = Vec::new();
        expect_token!(self, Token::Ident("import"), "import");
        loop {
            if matches_token!(self, Token::BlockEnd) {
                break;
            }
            if !names.is_empty() {
                expect_token!(self, Token::Comma, "`,`");
            }
            if matches_token!(self, Token::BlockEnd) {
                break;
            }
            let name = ok!(self.parse_assign_name(false));
            let alias = if skip_token!(self, Token::Ident("as")) {
                Some(ok!(self.parse_assign_name(false)))
            } else {
                None
            };
            names.push((name, alias));
        }
        Ok(ast::FromImport { expr, names })
    }

    #[cfg(feature = "macros")]
    fn parse_macro_args_and_defaults(
        &mut self,
        args: &mut Vec<ast::Expr<'a>>,
        defaults: &mut Vec<ast::Expr<'a>>,
    ) -> Result<(), Error> {
        loop {
            if skip_token!(self, Token::ParenClose) {
                break;
            }
            if !args.is_empty() {
                expect_token!(self, Token::Comma, "`,`");
                if skip_token!(self, Token::ParenClose) {
                    break;
                }
            }
            args.push(ok!(self.parse_assign_name(false)));
            if skip_token!(self, Token::Colon) {
                // consume and discard dbt-style type annotation (e.g. `arg: str`)
                expect_token!(self, Token::Ident(name) => name, "identifier");
            }
            if skip_token!(self, Token::Assign) {
                defaults.push(ok!(self.parse_expr()));
            } else if !defaults.is_empty() {
                expect_token!(self, Token::Assign, "`=`");
            }
        }
        Ok(())
    }

    /// reference: https://docs.getdbt.com/guides/create-new-materializations?step=2
    /// syntax: {% materialization [materialization name], ["specified adapter" | default] %}
    ///
    /// Returns the adapter name parsed from the macro definition
    #[cfg(feature = "macros")]
    fn parse_materialization_adapter_languages(
        &mut self,
        supported_languages: &mut Option<ast::Expr<'a>>,
    ) -> Result<String, Error> {
        let mut ret = "default".to_string();
        loop {
            // First check if ',' is specified
            if skip_token!(self, Token::Comma) {
                // Check if default is specified
                if skip_token!(self, Token::Ident("default")) {
                    // Continue to allow additional parameters after 'default'
                    continue;
                }

                // Check if the adapter is specified
                if skip_token!(self, Token::Ident("adapter")) {
                    expect_token!(self, Token::Assign, "`=`");
                    let (adapter_name, _) = expect_token!(self, Token::Str(name) => name, "str");
                    ret = adapter_name.to_string();
                // Else, check if supported_languages is specified
                } else if skip_token!(self, Token::Ident("supported_languages")) {
                    expect_token!(self, Token::Assign, "`=`");
                    *supported_languages = Some(ok!(self.parse_expr()));
                } else {
                    break;
                };
            } else {
                break;
            }
        }
        Ok(ret)
    }

    #[cfg(feature = "macros")]
    fn parse_macro_or_call_block_body(
        &mut self,
        args: Vec<ast::Expr<'a>>,
        defaults: Vec<ast::Expr<'a>>,
        name: Option<&'a str>,
        name_span: Span,
        start_open_span: Span,
    ) -> Result<ast::Macro<'a>, Error> {
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let old_in_loop = std::mem::replace(&mut self.in_loop, false);
        let old_in_macro = std::mem::replace(&mut self.in_macro, true);
        let ctx = if name.is_some() {
            ("macro", &["endmacro"][..])
        } else {
            ("call", &["endcall"][..])
        };
        let body = ok!(self.subparse(
            &|tok| match tok {
                Token::Ident("endmacro") if name.is_some() => true,
                Token::Ident("endcall") if name.is_none() => true,
                _ => false,
            },
            Some(ctx),
        ));
        self.in_macro = old_in_macro;
        self.in_loop = old_in_loop;
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::Macro {
            name: name.unwrap_or("caller"),
            args,
            defaults,
            body,
            name_span,
            start_tag_span: Some(start_tag_span),
            end_tag_span: Some(end_tag_span),
        })
    }

    #[cfg(feature = "macros")]
    fn parse_test_or_call_block_body(
        &mut self,
        args: Vec<ast::Expr<'a>>,
        defaults: Vec<ast::Expr<'a>>,
        name: Option<&'a str>,
        name_span: Span,
        start_open_span: Span,
    ) -> Result<ast::Macro<'a>, Error> {
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let old_in_loop = std::mem::replace(&mut self.in_loop, false);
        let old_in_macro = std::mem::replace(&mut self.in_macro, true);
        let ctx = if name.is_some() {
            ("test", &["endtest"][..])
        } else {
            ("call", &["endcall"][..])
        };
        let body = ok!(self.subparse(
            &|tok| match tok {
                Token::Ident("endtest") if name.is_some() => true,
                Token::Ident("endcall") if name.is_none() => true,
                _ => false,
            },
            Some(ctx),
        ));
        self.in_macro = old_in_macro;
        self.in_loop = old_in_loop;
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::Macro {
            name: name.unwrap_or("caller"),
            args,
            defaults,
            body,
            name_span,
            start_tag_span: Some(start_tag_span),
            end_tag_span: Some(end_tag_span),
        })
    }

    #[cfg(feature = "macros")]
    fn parse_snapshot_or_call_block_body(
        &mut self,
        name: Option<&'a str>,
        name_span: Span,
        start_open_span: Span,
    ) -> Result<ast::Macro<'a>, Error> {
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let old_in_loop = std::mem::replace(&mut self.in_loop, false);
        let old_in_macro = std::mem::replace(&mut self.in_macro, true);
        let ctx = if name.is_some() {
            ("snapshot", &["endsnapshot"][..])
        } else {
            ("call", &["endcall"][..])
        };
        let body = ok!(self.subparse(
            &|tok| match tok {
                Token::Ident("endsnapshot") if name.is_some() => true,
                Token::Ident("endcall") if name.is_none() => true,
                _ => false,
            },
            Some(ctx),
        ));
        self.in_macro = old_in_macro;
        self.in_loop = old_in_loop;
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::Macro {
            name: name.unwrap_or("caller"),
            args: Vec::new(),
            defaults: Vec::new(),
            body,
            name_span,
            start_tag_span: Some(start_tag_span),
            end_tag_span: Some(end_tag_span),
        })
    }

    #[cfg(feature = "macros")]
    fn parse_doc_or_call_block_body(
        &mut self,
        args: Vec<ast::Expr<'a>>,
        defaults: Vec<ast::Expr<'a>>,
        name: Option<&'a str>,
        name_span: Span,
        start_open_span: Span,
    ) -> Result<ast::Macro<'a>, Error> {
        // Unlike its siblings, this function doesn't consume the opening
        // tag's `%}` itself — `parse_doc()` already did, via its own
        // stray-content-tolerant drain loop — so we're already positioned
        // right after it; `expand_span` still gives the exact full range.
        let start_tag_span = self.stream.expand_span(start_open_span);
        let old_in_loop = std::mem::replace(&mut self.in_loop, false);
        let old_in_macro = std::mem::replace(&mut self.in_macro, true);
        let ctx = if name.is_some() {
            ("docs", &["enddocs"][..])
        } else {
            ("call", &["endcall"][..])
        };
        let body = ok!(self.subparse(
            &|tok| match tok {
                Token::Ident("enddocs") if name.is_some() => true,
                Token::Ident("endcall") if name.is_none() => true,
                _ => false,
            },
            Some(ctx),
        ));
        self.in_macro = old_in_macro;
        self.in_loop = old_in_loop;
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        // Only the `enddocs`/`endcall` token itself, not the full closing
        // tag: unlike its siblings, this function can't consume through the
        // real trailing `%}` here — dbt-core tolerates a stray name after
        // `enddocs` (e.g. `{% enddocs my_doc %}`), and draining that is
        // `parse_doc()`'s job, after this function returns. `parse_doc()`
        // widens this span itself once it knows where the real `%}` is.
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::Macro {
            name: name.unwrap_or("caller"),
            args,
            defaults,
            body,
            name_span,
            start_tag_span: Some(start_tag_span),
            end_tag_span: Some(end_tag_span),
        })
    }

    #[cfg(feature = "macros")]
    fn parse_materialization_or_call_block_body(
        &mut self,
        name: Option<&'a str>,
        name_span: Span,
        start_open_span: Span,
    ) -> Result<ast::Macro<'a>, Error> {
        expect_token!(self, Token::BlockEnd, "end of block");
        let start_tag_span = self.stream.expand_span(start_open_span);
        let old_in_loop = std::mem::replace(&mut self.in_loop, false);
        let old_in_macro = std::mem::replace(&mut self.in_macro, true);
        let ctx = if name.is_some() {
            ("materialization", &["endmaterialization"][..])
        } else {
            ("call", &["endcall"][..])
        };
        let body = ok!(self.subparse(
            &|tok| match tok {
                Token::Ident("endmaterialization") if name.is_some() => true,
                Token::Ident("endcall") if name.is_none() => true,
                _ => false,
            },
            Some(ctx),
        ));
        self.in_macro = old_in_macro;
        self.in_loop = old_in_loop;
        let end_open_span = self.stream.last_span();
        ok!(self.stream.next());
        expect_token!(self, Token::BlockEnd, "end of block");
        let end_tag_span = self.stream.expand_span(end_open_span);
        Ok(ast::Macro {
            name: name.unwrap_or("caller"),
            args: Vec::new(),
            defaults: Vec::new(),
            body,
            name_span,
            start_tag_span: Some(start_tag_span),
            end_tag_span: Some(end_tag_span),
        })
    }

    #[cfg(feature = "macros")]
    fn parse_macro(&mut self, start_open_span: Span) -> Result<ast::Macro<'a>, Error> {
        let (name, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
        expect_token!(self, Token::ParenOpen, "`(`");
        let mut args = Vec::new();
        let mut defaults = Vec::new();
        ok!(self.parse_macro_args_and_defaults(&mut args, &mut defaults));
        self.parse_macro_or_call_block_body(args, defaults, Some(name), span, start_open_span)
    }

    #[cfg(feature = "macros")]
    fn parse_test(&mut self, start_open_span: Span) -> Result<ast::Macro<'a>, Error> {
        let (name, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
        // Assuming self has access to an arena or string interner
        let macro_name = self.intern_string(&format!("test_{name}"));
        expect_token!(self, Token::ParenOpen, "`(`");
        let mut args = Vec::new();
        let mut defaults = Vec::new();
        ok!(self.parse_macro_args_and_defaults(&mut args, &mut defaults));
        self.parse_test_or_call_block_body(args, defaults, Some(macro_name), span, start_open_span)
    }

    #[cfg(feature = "macros")]
    fn parse_snapshot(&mut self, start_open_span: Span) -> Result<ast::Macro<'a>, Error> {
        let (name, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
        let macro_name = self.intern_string(&format!("snapshot_{name}"));
        // dbt-core's regex extractor stops at the first non-identifier character and
        // silently ignores everything else in the block tag, so `{% snapshot snp.sql %}`
        // and `{% snapshot snp() %}` both become name "snp". Drain all trailing tokens
        // using the same aggressive approach as `parse_doc` — see that function for the
        // full rationale. We stop *before* BlockEnd here (rather than consuming it like
        // `parse_doc` does) because `parse_snapshot_or_call_block_body` expects to
        // consume BlockEnd itself.
        let mut drained_suffix = false;
        loop {
            match ok!(self.stream.current()) {
                Some((&Token::BlockEnd, _)) | None => break,
                Some(_) => {
                    drained_suffix = true;
                    ok!(self.stream.next());
                }
            }
        }
        if drained_suffix {
            self.stream
                .tokenizer
                .notify_malformed_block_name(BlockNameKind::Snapshot, name, &span);
        }
        self.parse_snapshot_or_call_block_body(Some(macro_name), span, start_open_span)
    }

    /// Returns `Ok(None)` for a doc block whose name is not a valid identifier —
    /// the block is consumed and skipped, but the rest of the file still parses.
    #[cfg(feature = "macros")]
    fn parse_doc(&mut self, start_open_span: Span) -> Result<Option<ast::Macro<'a>>, Error> {
        // Doc names may start with a digit (e.g., `3_months_prior_date`).
        // dbt-core allows this; see https://github.com/dbt-labs/dbt-fusion/issues/998
        let (name, span) = match ok!(self.stream.next()) {
            Some((Token::Ident(name), span)) => (Some(name), span),
            Some((Token::Int(n), span)) => {
                let mut full_name = n.to_string();
                let end_offset = span.end_offset;
                // Consume adjacent identifier tokens that continue the name
                // (e.g., Int(3) + Ident("_months_prior_date") → "3_months_prior_date")
                if let Ok(Some((&Token::Ident(ident), ident_span))) = self.stream.current() {
                    if ident_span.start_offset == end_offset {
                        let _ = self.stream.next();
                        full_name.push_str(ident);
                    }
                }
                (Some(self.intern_string(&full_name)), span)
            }
            // dbt-core extracts doc blocks with a regex that only accepts an
            // identifier as the block name, so a name it cannot match (e.g.
            // `{% docs *** end of list *** %}`) yields an unreferenceable block
            // while every other block in the file is still registered. Failing
            // here would instead drop every doc in the file, which surfaces later
            // as unresolved `doc()` calls.
            Some((_, span)) => (None, span),
            None => return Err(unexpected_eof("identifier")),
        };

        // Skip everything until BlockEnd, advancing on Token Errors.
        // This is specifically because doc macros can have random tokens, characters,
        // spaces, etc. in the names (e.g. `{% doc package.doc_name %%% $$$$ hehehe %}`
        // is valid in dbt-core). TODO(alex): One day deprecate this — it is really a
        // parsing bug in dbt that is unreported (but let's bug-for-bug repro for now).
        // See also: `parse_snapshot`, which uses the same aggressive drain pattern.
        // Unlike here, that drain stops *before* consuming BlockEnd.
        let mut drained_suffix = false;
        loop {
            match self.stream.next() {
                Ok(Some((Token::BlockEnd, _))) => break,
                Ok(_) => {
                    drained_suffix = true;
                    continue;
                }
                Err(_) => {
                    drained_suffix = true;
                    self.stream.tokenizer.force_advance(1)?;
                    continue;
                }
            }
        }
        // Only warn about a *valid* identifier followed by extra characters.
        if let Some(name) = name {
            if drained_suffix {
                self.stream
                    .tokenizer
                    .notify_malformed_block_name(BlockNameKind::Docs, name, &span);
            }
        }

        // Always consume the body up to `{% enddocs %}`, even for a skipped block:
        // leaving the closing tag in the stream would fail the whole file instead.
        let mut body = ok!(self.parse_doc_or_call_block_body(
            Vec::new(),
            Vec::new(),
            Some(name.unwrap_or_default()),
            span,
            start_open_span
        ));

        // Drain the closing tag rather than requiring `%}` right after `enddocs`.
        // dbt-core compares end tags by block type only, so it accepts a stray name
        // in `{% enddocs my_doc %}`; rejecting it here would drop the whole file.
        loop {
            match self.stream.next() {
                Ok(Some((Token::BlockEnd, _))) => break,
                Ok(Some(_)) => continue,
                Ok(None) => return Err(unexpected_eof("end of block")),
                Err(_) => {
                    self.stream.tokenizer.force_advance(1)?;
                    continue;
                }
            }
        }
        // `body.end_tag_span` currently covers only the `enddocs` token
        // itself (that's as far as `parse_doc_or_call_block_body` could see);
        // widen it from that same start point through the real closing `%}`
        // we just found, including any stray name in between.
        if let Some(end_tag_span) = body.end_tag_span {
            body.end_tag_span = Some(self.stream.expand_span(end_tag_span));
        }

        Ok(name.map(|_| body))
    }

    #[cfg(feature = "macros")]
    fn parse_materialization(
        &mut self,
        start_open_span: Span,
    ) -> Result<(ast::Macro<'a>, String, Option<Value>), Error> {
        let (name, span) = expect_token!(self, Token::Ident(name) => name, "identifier");
        let mut supported_languages = None;
        let adapter = ok!(self.parse_materialization_adapter_languages(&mut supported_languages));
        let supported_languages = supported_languages.and_then(|expr| match expr {
            ast::Expr::List(list) => list.as_const(),
            _ => None,
        });
        // TODO: This can be cleaned up to add better error messages for miss-formatted materialization macros
        let macro_name = self.intern_string(&materialization_macro_name(name, &adapter));
        Ok((
            ok!(self.parse_materialization_or_call_block_body(
                Some(macro_name),
                span,
                start_open_span
            )),
            adapter,
            supported_languages,
        ))
    }

    #[cfg(feature = "macros")]
    fn parse_call_block(&mut self, start_open_span: Span) -> Result<ast::CallBlock<'a>, Error> {
        let span = self.stream.last_span();
        let mut args = Vec::new();
        let mut defaults = Vec::new();
        if skip_token!(self, Token::ParenOpen) {
            ok!(self.parse_macro_args_and_defaults(&mut args, &mut defaults));
        }
        let call = match ok!(self.parse_expr()) {
            ast::Expr::Call(call) => call,
            expr => syntax_error!(
                "expected call expression in call block, got {}",
                &self.filename,
                &self.stream.current_span(),
                expr.description()
            ),
        };
        let macro_decl = ok!(self.parse_macro_or_call_block_body(
            args,
            defaults,
            None,
            call.span(),
            start_open_span
        ));
        Ok(ast::CallBlock {
            call,
            macro_decl: Spanned::new(macro_decl, self.stream.expand_span(span)),
        })
    }

    fn parse_do(&mut self) -> Result<ast::Do<'a>, Error> {
        let expr = ok!(self.parse_expr_or_implied_tuple());
        Ok(ast::Do { expr })
    }

    fn subparse(
        &mut self,
        end_check: &dyn Fn(&Token) -> bool,
        block_context: Option<(&'static str, &'static [&'static str])>,
    ) -> Result<Vec<ast::Stmt<'a>>, Error> {
        with_recursion_guard!(self, self.subparse_internal(end_check, block_context))
    }

    fn subparse_internal(
        &mut self,
        end_check: &dyn Fn(&Token) -> bool,
        block_context: Option<(&'static str, &'static [&'static str])>,
    ) -> Result<Vec<ast::Stmt<'a>>, Error> {
        let mut rv = Vec::new();
        while let Some((token, span)) = ok!(self.stream.next()) {
            match token {
                Token::TemplateData(raw) => {
                    rv.push(ast::Stmt::EmitRaw(Spanned::new(ast::EmitRaw { raw }, span)))
                }
                Token::VariableStart => {
                    let expr = ok!(self.parse_expr_or_implied_tuple());
                    expect_token!(self, Token::VariableEnd, "end of variable block");
                    macro_rules! expand_span {
                        ($expr:expr, $variant:path) => {{
                            let mut spanned = $expr;
                            spanned.span = self.stream.expand_span(span);
                            $variant(spanned)
                        }};
                    }

                    macro_rules! match_expr_variants {
                        ($expr:expr, $($variant:ident),*) => {
                            match $expr {
                                $(
                                    ast::Expr::$variant(spanned) => expand_span!(spanned, ast::Expr::$variant),
                                )*
                            }
                        };
                    }

                    let expr = match_expr_variants!(
                        expr, Var, Const, Slice, UnaryOp, BinOp, IfExpr, Filter, Test, GetAttr,
                        GetItem, Call, List, Map, Tuple
                    );
                    rv.push(ast::Stmt::EmitExpr(Spanned::new(
                        ast::EmitExpr { expr },
                        self.stream.expand_span(span),
                    )));
                }
                Token::BlockStart => {
                    let (tok, span) = match ok!(self.stream.current()) {
                        Some(rv) => rv,
                        None => syntax_error!(
                            "unexpected end of input, expected keyword",
                            &self.filename,
                            &self.stream.current_span()
                        ),
                    };
                    if end_check(tok) {
                        return Ok(rv);
                    }
                    // Check for a misplaced end-block tag before dispatching to parse_stmt.
                    // This happens when an inner block (e.g. {% if %}) is closed by an outer
                    // end tag (e.g. {% endmacro %}) without first closing the inner block.
                    if let (Token::Ident(name), Some((innermost_block, expected_tags))) =
                        (tok, block_context)
                    {
                        if ENDBLOCK_IDENT.contains(name) {
                            let expected_list = expected_tags
                                .iter()
                                .map(|t| format!("'{t}'"))
                                .collect::<Vec<_>>()
                                .join(" or ");
                            syntax_error!(
                                "Encountered unknown tag '{}'. You probably made a nesting \
                                 mistake. Jinja is currently looking for {}. The innermost \
                                 block that needs to be closed is '{}'.",
                                &self.filename,
                                &span,
                                name,
                                expected_list,
                                innermost_block
                            )
                        }
                    }
                    rv.push(ok!(self.parse_stmt()));
                    // WARNING: this is a dangerous change.
                    // expect_token!(self, Token::BlockEnd, "end of block");
                }
                Token::Comment => {
                    rv.push(ast::Stmt::Comment(Spanned::new(
                        Comment,
                        self.stream.expand_span(span),
                    )));
                }

                _ => unreachable!("lexer produced garbage"),
            }
        }
        Ok(rv)
    }

    fn intern_string(&mut self, s: &str) -> &'a str {
        if let Some(interned) = self.string_interner.get(s) {
            return interned;
        }

        // Create a new string in the same allocation as source
        let new_str = {
            let owned = s.to_string();
            let boxed = Box::leak(owned.into_boxed_str());
            let reference: &'a str = boxed;
            reference
        };

        self.string_interner.insert(s.to_string(), new_str);
        new_str
    }

    /// Parses only top-level statements of a specific type.
    /// Ignores all other statements and expressions between the start and end tags.
    pub fn parse_top_level_statements(
        &mut self,
        statement_type: &[&str],
    ) -> Result<ast::Stmt<'a>, Error> {
        let span = self.stream.last_span();
        self.subparse_top_level(&|_tok| false, statement_type)
            .map(|children| {
                ast::Stmt::Template(Spanned::new(
                    ast::Template { children },
                    self.stream.expand_span(span),
                ))
            })
    }

    fn subparse_top_level(
        &mut self,
        end_check: &dyn Fn(&Token) -> bool,
        statement_type: &[&str],
    ) -> Result<Vec<ast::Stmt<'a>>, Error> {
        let mut rv = Vec::new();
        while let Some((token, span)) = ok!(self.stream.next()) {
            match token {
                Token::BlockStart => {
                    let (tok, _span) = match ok!(self.stream.current()) {
                        Some(rv) => rv,
                        None => syntax_error!(
                            "unexpected end of input, expected keyword",
                            &self.filename,
                            &self.stream.current_span()
                        ),
                    };

                    if end_check(tok) {
                        return Ok(rv);
                    }

                    // Only parse statements that match our target type
                    if let Token::Ident(ident) = tok {
                        if statement_type.contains(ident) {
                            rv.push(ok!(self.parse_stmt()));
                        } else if ENDBLOCK_IDENT.contains(ident) {
                            syntax_error!(
                                "unexpected '{}' end of block identifier",
                                &self.filename,
                                &self.stream.current_span(),
                                ident
                            );
                        } else {
                            // Skip until we find the end of this block ignoring any and all errors
                            self.ignore_unknown_stmts = true;
                            let _ = self.parse_stmt();
                            self.ignore_unknown_stmts = false;
                        }
                    } else {
                        // Skip non-identifier tokens
                        self.skip_until_block_end()?;
                    }
                }
                // parse TemplateData because it can be a signature string
                Token::TemplateData(raw) => {
                    rv.push(ast::Stmt::EmitRaw(Spanned::new(ast::EmitRaw { raw }, span)));
                }
                // Skip all other token types
                _ => continue,
            }
        }
        Ok(rv)
    }

    fn skip_until_block_end(&mut self) -> Result<(), Error> {
        let mut depth = 1;
        while let Some((token, _)) = ok!(self.stream.next()) {
            match token {
                Token::BlockStart => depth += 1,
                Token::BlockEnd => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => continue,
            }
        }
        Ok(())
    }
}

/// Parses a template.
pub fn parse<'source>(
    source: &'source str,
    filename: &'source str,
    syntax_config: SyntaxConfig,
    whitespace_config: WhitespaceConfig,
) -> Result<ast::Stmt<'source>, Error> {
    Parser::new(source, filename, false, syntax_config, whitespace_config).parse()
}

/// Parses a template and notifies listeners as source tokens are emitted.
pub fn parse_with_listeners<'source>(
    source: &'source str,
    filename: &'source str,
    syntax_config: SyntaxConfig,
    whitespace_config: WhitespaceConfig,
    source_listeners: &[Rc<dyn TokenizerEventListener>],
) -> Result<ast::Stmt<'source>, Error> {
    Parser::new_with_tokenizer_listeners(
        source,
        filename,
        false,
        syntax_config,
        whitespace_config,
        source_listeners,
    )
    .parse()
}

/// Parses a standalone expression.
pub fn parse_expr(source: &str) -> Result<ast::Expr<'_>, Error> {
    Parser::new(
        source,
        "<expression>",
        true,
        Default::default(),
        Default::default(),
    )
    .parse_standalone_expr()
}

pub fn materialization_macro_name<N: fmt::Display>(name: N, adapter: &str) -> String {
    format!("materialization_{name}_{adapter}")
}

#[cfg(test)]
mod malformed_block_name_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::compiler::lexer::WhitespaceConfig;
    use crate::compiler::parser::Parser;
    use crate::compiler::tokens::{Span, Token};
    use crate::listener::{BlockNameKind, TokenizerEventListener};
    use crate::syntax::SyntaxConfig;

    #[derive(Debug, Default)]
    struct CapturingListener {
        events: RefCell<Vec<(BlockNameKind, String)>>,
    }

    impl TokenizerEventListener for CapturingListener {
        fn on_source_token(&self, _token: &Token<'_>, _span: &Span) {}
        fn on_malformed_block_name(&self, kind: BlockNameKind, name: &str, _span: &Span) {
            self.events.borrow_mut().push((kind, name.to_string()));
        }
    }

    fn events_for(src: &str) -> Vec<(BlockNameKind, String)> {
        let listener = Rc::new(CapturingListener::default());
        let dyn_listener: Rc<dyn TokenizerEventListener> = listener.clone();
        let mut parser = Parser::new_with_tokenizer_listeners(
            src,
            "test.sql",
            false,
            SyntaxConfig::builder().build().unwrap(),
            WhitespaceConfig::default(),
            &[dyn_listener],
        );
        parser
            .parse_top_level_statements(&["snapshot", "docs"])
            .unwrap();
        let events = listener.events.borrow().clone();
        events
    }

    #[test]
    fn malformed_snapshot_name_emits_event() {
        let events = events_for("{% snapshot stg_crm.sql %}select 1{% endsnapshot %}");
        assert_eq!(
            events,
            vec![(BlockNameKind::Snapshot, "stg_crm".to_string())]
        );
    }

    #[test]
    fn clean_snapshot_name_emits_nothing() {
        let events = events_for("{% snapshot my_snapshot %}select 1{% endsnapshot %}");
        assert!(events.is_empty(), "got {events:?}");
    }

    #[test]
    fn malformed_docs_name_emits_event() {
        let events = events_for("{% docs pkg.doc_name %}content{% enddocs %}");
        assert_eq!(events, vec![(BlockNameKind::Docs, "pkg".to_string())]);
    }

    #[test]
    fn lex_error_suffix_emits_event() {
        // Suffix is composed of lex-error characters the parser drains via
        // force_advance. A tokenizer-only listener would miss this entirely.
        let events = events_for("{% docs my_doc $$$ %}content{% enddocs %}");
        assert_eq!(events, vec![(BlockNameKind::Docs, "my_doc".to_string())]);
    }

    #[test]
    fn clean_docs_name_emits_nothing() {
        let events = events_for("{% docs my_doc %}content{% enddocs %}");
        assert!(events.is_empty(), "got {events:?}");
    }

    #[test]
    fn digit_leading_docs_name_emits_nothing() {
        // Regression guard for issue #998: `3_months_prior_date` is a single
        // valid doc name via digit-leading glue, not a malformed suffix.
        let events = events_for("{% docs 3_months_prior_date %}content{% enddocs %}");
        assert!(events.is_empty(), "got {events:?}");
    }
}
