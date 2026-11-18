use std::collections::BTreeMap;
use std::ops::Deref;

#[cfg(feature = "internal_debug")]
use std::fmt;

use crate::compiler::tokens::Span;
use crate::layout::JinjaLayoutEventKind;
use crate::value::{value_map_with_capacity, Value};

/// Container for nodes with location info.
///
/// This container fulfills two purposes: it adds location information
/// to nodes, but it also ensures the nodes is heap allocated.  The
/// latter is useful to ensure that enum variants do not cause the enum
/// to become too large.
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Spanned<T> {
    node: Box<T>,
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Creates a new spanned node.
    pub fn new(node: T, span: Span) -> Spanned<T> {
        Spanned {
            node: Box::new(node),
            span,
        }
    }

    /// Accesses the span.
    pub fn span(&self) -> Span {
        self.span.to_owned()
    }
}

impl<T> Deref for Spanned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

#[cfg(feature = "internal_debug")]
impl<T: fmt::Debug> fmt::Debug for Spanned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        ok!(fmt::Debug::fmt(&self.node, f));
        write!(f, "{:?}", self.span)
    }
}

/// A statement node.
#[cfg_attr(
    feature = "unstable_machinery_serde",
    derive(serde::Serialize),
    serde(tag = "stmt")
)]
pub enum Stmt<'a> {
    Template(Spanned<Template<'a>>),
    EmitExpr(Spanned<EmitExpr<'a>>),
    EmitRaw(Spanned<EmitRaw<'a>>),
    ForLoop(Spanned<ForLoop<'a>>),
    IfCond(Spanned<IfCond<'a>>),
    WithBlock(Spanned<WithBlock<'a>>),
    Set(Spanned<Set<'a>>),
    SetBlock(Spanned<SetBlock<'a>>),
    AutoEscape(Spanned<AutoEscape<'a>>),
    FilterBlock(Spanned<FilterBlock<'a>>),
    #[cfg(feature = "multi_template")]
    Block(Spanned<Block<'a>>),
    #[cfg(feature = "multi_template")]
    Import(Spanned<Import<'a>>),
    #[cfg(feature = "multi_template")]
    FromImport(Spanned<FromImport<'a>>),
    #[cfg(feature = "multi_template")]
    Extends(Spanned<Extends<'a>>),
    #[cfg(feature = "multi_template")]
    Include(Spanned<Include<'a>>),
    #[cfg(feature = "macros")]
    Macro((Spanned<Macro<'a>>, MacroKind, BTreeMap<String, Value>)), // bool is whether it's a test macro
    #[cfg(feature = "macros")]
    CallBlock(Spanned<CallBlock<'a>>),
    #[cfg(feature = "loop_controls")]
    Continue(Spanned<Continue>),
    #[cfg(feature = "loop_controls")]
    Break(Spanned<Break>),
    Do(Spanned<Do<'a>>),
    Comment(Spanned<Comment>),
}

#[cfg(feature = "internal_debug")]
impl fmt::Debug for Stmt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stmt::Template(s) => fmt::Debug::fmt(s, f),
            Stmt::EmitExpr(s) => fmt::Debug::fmt(s, f),
            Stmt::EmitRaw(s) => fmt::Debug::fmt(s, f),
            Stmt::ForLoop(s) => fmt::Debug::fmt(s, f),
            Stmt::IfCond(s) => fmt::Debug::fmt(s, f),
            Stmt::WithBlock(s) => fmt::Debug::fmt(s, f),
            Stmt::Set(s) => fmt::Debug::fmt(s, f),
            Stmt::SetBlock(s) => fmt::Debug::fmt(s, f),
            Stmt::AutoEscape(s) => fmt::Debug::fmt(s, f),
            Stmt::FilterBlock(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "multi_template")]
            Stmt::Block(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "multi_template")]
            Stmt::Extends(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "multi_template")]
            Stmt::Include(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "multi_template")]
            Stmt::Import(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "multi_template")]
            Stmt::FromImport(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "macros")]
            Stmt::Macro(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "macros")]
            Stmt::CallBlock(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "loop_controls")]
            Stmt::Continue(s) => fmt::Debug::fmt(s, f),
            #[cfg(feature = "loop_controls")]
            Stmt::Break(s) => fmt::Debug::fmt(s, f),
            Stmt::Do(s) => fmt::Debug::fmt(s, f),
            Stmt::Comment(s) => fmt::Debug::fmt(s, f),
        }
    }
}

/// An expression node.
#[allow(clippy::enum_variant_names)]
#[cfg_attr(
    feature = "unstable_machinery_serde",
    derive(serde::Serialize),
    serde(tag = "expr")
)]
pub enum Expr<'a> {
    Var(Spanned<Var<'a>>),
    Const(Spanned<Const>),
    Slice(Spanned<Slice<'a>>),
    UnaryOp(Spanned<UnaryOp<'a>>),
    BinOp(Spanned<BinOp<'a>>),
    IfExpr(Spanned<IfExpr<'a>>),
    Filter(Spanned<Filter<'a>>),
    Test(Spanned<Test<'a>>),
    GetAttr(Spanned<GetAttr<'a>>),
    GetItem(Spanned<GetItem<'a>>),
    Call(Spanned<Call<'a>>),
    List(Spanned<List<'a>>),
    Map(Spanned<Map<'a>>),
    Tuple(Spanned<Tuple<'a>>),
}

#[cfg(feature = "internal_debug")]
impl fmt::Debug for Expr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Var(s) => fmt::Debug::fmt(s, f),
            Expr::Const(s) => fmt::Debug::fmt(s, f),
            Expr::Slice(s) => fmt::Debug::fmt(s, f),
            Expr::UnaryOp(s) => fmt::Debug::fmt(s, f),
            Expr::BinOp(s) => fmt::Debug::fmt(s, f),
            Expr::IfExpr(s) => fmt::Debug::fmt(s, f),
            Expr::Filter(s) => fmt::Debug::fmt(s, f),
            Expr::Test(s) => fmt::Debug::fmt(s, f),
            Expr::GetAttr(s) => fmt::Debug::fmt(s, f),
            Expr::GetItem(s) => fmt::Debug::fmt(s, f),
            Expr::Call(s) => fmt::Debug::fmt(s, f),
            Expr::List(s) => fmt::Debug::fmt(s, f),
            Expr::Map(s) => fmt::Debug::fmt(s, f),
            Expr::Tuple(s) => fmt::Debug::fmt(s, f),
        }
    }
}

impl Expr<'_> {
    pub fn description(&self) -> &'static str {
        match self {
            Expr::Var(_) => "variable",
            Expr::Const(_) => "constant",
            Expr::Slice(_)
            | Expr::UnaryOp(_)
            | Expr::BinOp(_)
            | Expr::IfExpr(_)
            | Expr::GetAttr(_)
            | Expr::GetItem(_) => "expression",
            Expr::Call(_) => "call",
            Expr::List(_) => "list literal",
            Expr::Map(_) => "map literal",
            Expr::Test(_) => "test expression",
            Expr::Filter(_) => "filter expression",
            Expr::Tuple(_) => "tuple literal",
        }
    }
}

/// Root template node.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Template<'a> {
    pub children: Vec<Stmt<'a>>,
}

/// A for loop.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct ForLoop<'a> {
    pub target: Expr<'a>,
    pub iter: Expr<'a>,
    pub filter_expr: Option<Expr<'a>>,
    pub recursive: bool,
    pub body: Vec<Stmt<'a>>,
    pub else_body: Vec<Stmt<'a>>,
    pub start_tag_span: Span,
    pub else_tag_span: Option<Span>,
    pub end_tag_span: Span,
}

/// An if/else condition.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct IfCond<'a> {
    pub expr: Expr<'a>,
    pub true_body: Vec<Stmt<'a>>,
    pub false_body: Vec<Stmt<'a>>,
    pub start_tag_kind: JinjaLayoutEventKind,
    pub start_tag_span: Span,
    pub else_tag_span: Option<Span>,
    pub end_tag_span: Span,
}

/// A with block.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct WithBlock<'a> {
    pub assignments: Vec<(Expr<'a>, Expr<'a>)>,
    pub body: Vec<Stmt<'a>>,
    pub start_tag_span: Span,
    pub end_tag_span: Span,
}

/// A set statement.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Set<'a> {
    pub target: Expr<'a>,
    pub expr: Expr<'a>,
}

/// A set capture statement.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct SetBlock<'a> {
    pub target: Expr<'a>,
    pub filter: Option<Expr<'a>>,
    pub body: Vec<Stmt<'a>>,
    pub start_tag_span: Span,
    pub end_tag_span: Span,
}

/// A block for inheritance elements.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "multi_template")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Block<'a> {
    pub name: &'a str,
    pub body: Vec<Stmt<'a>>,
    pub start_tag_span: Span,
    pub end_tag_span: Span,
}

/// An extends block.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "multi_template")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Extends<'a> {
    pub name: Expr<'a>,
}

/// An include block.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "multi_template")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Include<'a> {
    pub name: Expr<'a>,
    pub ignore_missing: bool,
}

/// An auto escape control block.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct AutoEscape<'a> {
    pub enabled: Expr<'a>,
    pub body: Vec<Stmt<'a>>,
    pub start_tag_span: Span,
    pub end_tag_span: Span,
}

/// Applies filters to a block.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct FilterBlock<'a> {
    pub filter: Expr<'a>,
    pub body: Vec<Stmt<'a>>,
    pub start_tag_span: Span,
    pub end_tag_span: Span,
}

/// Declares a macro.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "macros")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Macro<'a> {
    pub name: &'a str,
    pub name_span: Span,
    pub args: Vec<Expr<'a>>,
    pub defaults: Vec<Expr<'a>>,
    pub body: Vec<Stmt<'a>>,
    /// The full opening/closing tag spans (e.g. `{% macro foo(x) %}` /
    /// `{% endmacro %}`), covering the whole tag rather than just the name
    /// (see `name_span`). Only populated for a plain `{% macro %}` def or a
    /// `{% call %}` block's implicit `caller()` macro — `None` for
    /// `test`/`snapshot`/`materialization`/`docs`, dbt-specific extensions
    /// this parser doesn't bother widening these spans for.
    pub start_tag_span: Option<Span>,
    pub end_tag_span: Option<Span>,
}

#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "macros")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub enum MacroKind {
    Macro,
    Test,
    Materialization,
    Doc,
    Snapshot,
}

/// A call block
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "macros")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct CallBlock<'a> {
    pub call: Spanned<Call<'a>>,
    pub macro_decl: Spanned<Macro<'a>>,
}

/// Continue
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "loop_controls")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Continue;

/// Break
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "loop_controls")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Break;

/// A call block
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Do<'a> {
    pub expr: Expr<'a>,
}

/// A "from" import
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "multi_template")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct FromImport<'a> {
    pub expr: Expr<'a>,
    pub names: Vec<(Expr<'a>, Option<Expr<'a>>)>,
}

/// A full module import
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg(feature = "multi_template")]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Import<'a> {
    pub expr: Expr<'a>,
    pub name: Expr<'a>,
}

/// Outputs the expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct EmitExpr<'a> {
    pub expr: Expr<'a>,
}

/// Outputs raw template code.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct EmitRaw<'a> {
    pub raw: &'a str,
}

/// Looks up a variable.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Var<'a> {
    pub id: &'a str,
}

/// Loads a constant
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Const {
    pub value: Value,
}

/// Represents a slice.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Slice<'a> {
    pub expr: Expr<'a>,
    pub start: Option<Expr<'a>>,
    pub stop: Option<Expr<'a>>,
    pub step: Option<Expr<'a>>,
}

/// A kind of unary operator.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub enum UnaryOpKind {
    Not,
    Neg,
    Pos,
}

/// An unary operator expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct UnaryOp<'a> {
    pub op: UnaryOpKind,
    pub expr: Expr<'a>,
}

/// A kind of binary operator.
#[derive(Clone)]
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub enum BinOpKind {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    ScAnd,
    ScOr,
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Rem,
    Pow,
    Concat,
    In,
}

/// A binary operator expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct BinOp<'a> {
    pub op: BinOpKind,
    pub left: Expr<'a>,
    pub right: Expr<'a>,
}

/// An if expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct IfExpr<'a> {
    pub test_expr: Expr<'a>,
    pub true_expr: Expr<'a>,
    pub false_expr: Option<Expr<'a>>,
}

/// A filter expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Filter<'a> {
    pub name: &'a str,
    pub expr: Option<Expr<'a>>,
    pub args: Vec<CallArg<'a>>,
}

/// A test expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Test<'a> {
    pub name: &'a str,
    pub expr: Expr<'a>,
    pub args: Vec<CallArg<'a>>,
}

/// An attribute lookup expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct GetAttr<'a> {
    pub expr: Expr<'a>,
    pub name: &'a str,
}

/// An item lookup expression.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct GetItem<'a> {
    pub expr: Expr<'a>,
    pub subscript_expr: Expr<'a>,
}

/// Calls something.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Call<'a> {
    pub expr: Expr<'a>,
    pub args: Vec<CallArg<'a>>,
}

/// A call argument helper
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub enum CallArg<'a> {
    Pos(Expr<'a>),
    Kwarg(&'a str, Expr<'a>),
    PosSplat(Expr<'a>),
    KwargSplat(Expr<'a>),
}

/// Creates a list of values.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct List<'a> {
    pub items: Vec<Expr<'a>>,
}

impl List<'_> {
    pub fn as_const(&self) -> Option<Value> {
        if !self.items.iter().all(|x| matches!(x, Expr::Const(_))) {
            return None;
        }

        let items = self.items.iter();
        let sequence = items.filter_map(|expr| match expr {
            Expr::Const(v) => Some(v.value.clone()),
            _ => None,
        });

        Some(Value::from(sequence.collect::<Vec<_>>()))
    }
}

/// Creates a map of values.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Map<'a> {
    pub keys: Vec<Expr<'a>>,
    pub values: Vec<Expr<'a>>,
}

impl Map<'_> {
    pub fn as_const(&self) -> Option<Value> {
        if !self.keys.iter().all(|x| matches!(x, Expr::Const(_)))
            || !self.values.iter().all(|x| matches!(x, Expr::Const(_)))
        {
            return None;
        }

        let mut rv = value_map_with_capacity(self.keys.len());
        for (key, value) in self.keys.iter().zip(self.values.iter()) {
            if let (Expr::Const(maybe_key), Expr::Const(value)) = (key, value) {
                rv.insert(maybe_key.value.clone(), value.value.clone());
            }
        }

        Some(Value::from_object(rv))
    }
}

/// Defines the specific type of call.
#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub enum CallType<'ast, 'source> {
    Function(&'source str),
    Method(&'ast Expr<'source>, &'source str),
    #[cfg(feature = "multi_template")]
    Block(&'source str),
    Object(&'ast Expr<'source>),
}

impl<'a> Call<'a> {
    /// Try to isolate a method call.
    ///
    /// name + call and attribute lookup + call are really method
    /// calls which are easier to handle for the compiler as a separate
    /// thing.
    pub fn identify_call(&self) -> CallType<'_, 'a> {
        match self.expr {
            Expr::Var(ref var) => CallType::Function(var.id),
            Expr::GetAttr(ref attr) => {
                #[cfg(feature = "multi_template")]
                {
                    if let Expr::Var(ref var) = attr.expr {
                        if var.id == "self" {
                            return CallType::Block(attr.name);
                        }
                    }
                }
                CallType::Method(&attr.expr, attr.name)
            }
            _ => CallType::Object(&self.expr),
        }
    }
}

#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Comment;

#[cfg_attr(feature = "internal_debug", derive(Debug))]
#[cfg_attr(feature = "unstable_machinery_serde", derive(serde::Serialize))]
pub struct Tuple<'a> {
    pub items: Vec<Expr<'a>>,
}

impl Tuple<'_> {
    pub fn as_const(&self) -> Option<Value> {
        let mut rv = Vec::with_capacity(self.items.len());
        for item in &self.items {
            match item {
                Expr::Const(Spanned { node, .. }) => rv.push(node.value.clone()),
                Expr::List(l) => rv.push(some!(l.as_const())),
                Expr::Map(m) => rv.push(some!(m.as_const())),
                Expr::Tuple(t) => rv.push(some!(t.as_const())),
                _ => return None,
            }
        }
        Some(Value::from(rv))
    }
}
