use std::collections::HashSet;
use std::fmt::Write;

use crate::compiler::ast;
use crate::compiler::tokens::Span;

struct AssignmentTracker<'a> {
    out: HashSet<&'a str>,
    nested_out: Option<HashSet<String>>,
    assigned: Vec<HashSet<&'a str>>,
}

impl<'a> AssignmentTracker<'a> {
    fn is_assigned(&self, name: &str) -> bool {
        self.assigned.iter().any(|x| x.contains(name))
    }

    fn assign(&mut self, name: &'a str) {
        self.assigned.last_mut().unwrap().insert(name);
    }

    fn assign_nested(&mut self, name: String) {
        if let Some(ref mut nested_out) = self.nested_out {
            if !nested_out.contains(&name) {
                nested_out.insert(name);
            }
        }
    }

    fn push(&mut self) {
        self.assigned.push(Default::default());
    }

    fn pop(&mut self) {
        self.assigned.pop();
    }
}

/// Finds all variables that need to be captured as closure for a macro.
#[cfg(feature = "macros")]
pub fn find_macro_closure<'a>(m: &ast::Macro<'a>) -> HashSet<&'a str> {
    let mut state = AssignmentTracker {
        out: HashSet::new(),
        nested_out: None,
        assigned: vec![Default::default()],
    };
    tracker_visit_macro(m, &mut state);
    state.out
}

/// Finds all variables that are undeclared in a template.
pub fn find_undeclared(t: &ast::Stmt<'_>, track_nested: bool) -> HashSet<String> {
    let mut state = AssignmentTracker {
        out: HashSet::new(),
        nested_out: if track_nested {
            Some(HashSet::new())
        } else {
            None
        },
        assigned: vec![Default::default()],
    };
    track_walk(t, &mut state);
    if let Some(nested) = state.nested_out {
        nested
    } else {
        state.out.into_iter().map(|x| x.to_string()).collect()
    }
}

fn tracker_visit_expr_opt<'a>(expr: &Option<ast::Expr<'a>>, state: &mut AssignmentTracker<'a>) {
    if let Some(expr) = expr {
        tracker_visit_expr(expr, state);
    }
}

#[cfg(feature = "macros")]
fn tracker_visit_macro<'a>(m: &ast::Macro<'a>, state: &mut AssignmentTracker<'a>) {
    m.args.iter().for_each(|arg| track_assign(arg, state));
    m.defaults
        .iter()
        .for_each(|expr| tracker_visit_expr(expr, state));
    m.body.iter().for_each(|node| track_walk(node, state));
}

fn tracker_visit_callarg<'a>(callarg: &ast::CallArg<'a>, state: &mut AssignmentTracker<'a>) {
    match callarg {
        ast::CallArg::Pos(expr)
        | ast::CallArg::Kwarg(_, expr)
        | ast::CallArg::PosSplat(expr)
        | ast::CallArg::KwargSplat(expr) => tracker_visit_expr(expr, state),
    }
}

fn tracker_visit_expr<'a>(expr: &ast::Expr<'a>, state: &mut AssignmentTracker<'a>) {
    match expr {
        ast::Expr::Var(var) => {
            if !state.is_assigned(var.id) {
                state.out.insert(var.id);
                // if we are not tracking nested assignments, we can consider a variable
                // to be assigned the first time we perform a lookup.
                if state.nested_out.is_none() {
                    state.assign(var.id);
                } else {
                    state.assign_nested(var.id.to_string());
                }
            }
        }
        ast::Expr::Const(_) => {}
        ast::Expr::UnaryOp(expr) => tracker_visit_expr(&expr.expr, state),
        ast::Expr::BinOp(expr) => {
            tracker_visit_expr(&expr.left, state);
            tracker_visit_expr(&expr.right, state);
        }
        ast::Expr::IfExpr(expr) => {
            tracker_visit_expr(&expr.test_expr, state);
            tracker_visit_expr(&expr.true_expr, state);
            tracker_visit_expr_opt(&expr.false_expr, state);
        }
        ast::Expr::Filter(expr) => {
            tracker_visit_expr_opt(&expr.expr, state);
            expr.args
                .iter()
                .for_each(|x| tracker_visit_callarg(x, state));
        }
        ast::Expr::Test(expr) => {
            tracker_visit_expr(&expr.expr, state);
            expr.args
                .iter()
                .for_each(|x| tracker_visit_callarg(x, state));
        }
        ast::Expr::GetAttr(expr) => {
            // if we are tracking nested, we check if we have a chain of attribute
            // lookups that terminate in a variable lookup.  In that case we can
            // assign the nested lookup.
            if state.nested_out.is_some() {
                let mut attrs = vec![expr.name];
                let mut ptr = &expr.expr;
                loop {
                    match ptr {
                        ast::Expr::Var(var) => {
                            if !state.is_assigned(var.id) {
                                let mut rv = var.id.to_string();
                                for attr in attrs.iter().rev() {
                                    write!(rv, ".{attr}").ok();
                                }
                                state.assign_nested(rv);
                                return;
                            } else {
                                break;
                            }
                        }
                        ast::Expr::GetAttr(expr) => {
                            attrs.push(expr.name);
                            ptr = &expr.expr;
                            continue;
                        }
                        _ => break,
                    }
                }
            }
            tracker_visit_expr(&expr.expr, state)
        }
        ast::Expr::GetItem(expr) => {
            tracker_visit_expr(&expr.expr, state);
            tracker_visit_expr(&expr.subscript_expr, state);
        }
        ast::Expr::Slice(slice) => {
            tracker_visit_expr_opt(&slice.start, state);
            tracker_visit_expr_opt(&slice.stop, state);
            tracker_visit_expr_opt(&slice.step, state);
        }
        ast::Expr::Call(expr) => {
            tracker_visit_expr(&expr.expr, state);
            expr.args
                .iter()
                .for_each(|x| tracker_visit_callarg(x, state));
        }
        ast::Expr::List(expr) => expr.items.iter().for_each(|x| tracker_visit_expr(x, state)),
        ast::Expr::Map(expr) => expr.keys.iter().zip(expr.values.iter()).for_each(|(k, v)| {
            tracker_visit_expr(k, state);
            tracker_visit_expr(v, state);
        }),
        ast::Expr::Tuple(expr) => expr.items.iter().for_each(|x| tracker_visit_expr(x, state)),
    }
}

fn track_assign<'a>(expr: &ast::Expr<'a>, state: &mut AssignmentTracker<'a>) {
    match expr {
        ast::Expr::Var(var) => state.assign(var.id),
        ast::Expr::List(list) => list.items.iter().for_each(|x| track_assign(x, state)),
        ast::Expr::Tuple(tuple) => tuple.items.iter().for_each(|x| track_assign(x, state)),
        _ => {}
    }
}

fn track_walk<'a>(node: &ast::Stmt<'a>, state: &mut AssignmentTracker<'a>) {
    match node {
        ast::Stmt::Template(stmt) => {
            state.assign("self");
            stmt.children.iter().for_each(|x| track_walk(x, state));
        }
        ast::Stmt::EmitExpr(expr) => tracker_visit_expr(&expr.expr, state),
        ast::Stmt::EmitRaw(_) => {}
        ast::Stmt::ForLoop(stmt) => {
            state.push();
            state.assign("loop");
            tracker_visit_expr(&stmt.iter, state);
            track_assign(&stmt.target, state);
            tracker_visit_expr_opt(&stmt.filter_expr, state);
            stmt.body.iter().for_each(|x| track_walk(x, state));
            state.pop();
            state.push();
            stmt.else_body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        ast::Stmt::IfCond(stmt) => {
            tracker_visit_expr(&stmt.expr, state);
            state.push();
            stmt.true_body.iter().for_each(|x| track_walk(x, state));
            state.pop();
            state.push();
            stmt.false_body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        ast::Stmt::WithBlock(stmt) => {
            state.push();
            for (target, expr) in &stmt.assignments {
                track_assign(target, state);
                tracker_visit_expr(expr, state);
            }
            stmt.body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        ast::Stmt::Set(stmt) => {
            track_assign(&stmt.target, state);
            tracker_visit_expr(&stmt.expr, state);
        }
        ast::Stmt::AutoEscape(stmt) => {
            state.push();
            stmt.body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        ast::Stmt::FilterBlock(stmt) => {
            state.push();
            stmt.body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        ast::Stmt::SetBlock(stmt) => {
            track_assign(&stmt.target, state);
            state.push();
            stmt.body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        #[cfg(feature = "multi_template")]
        ast::Stmt::Block(stmt) => {
            state.push();
            state.assign("super");
            stmt.body.iter().for_each(|x| track_walk(x, state));
            state.pop();
        }
        #[cfg(feature = "multi_template")]
        ast::Stmt::Extends(_) | ast::Stmt::Include(_) => {}
        #[cfg(feature = "multi_template")]
        ast::Stmt::Import(stmt) => {
            track_assign(&stmt.name, state);
        }
        #[cfg(feature = "multi_template")]
        ast::Stmt::FromImport(stmt) => stmt.names.iter().for_each(|(arg, alias)| {
            track_assign(alias.as_ref().unwrap_or(arg), state);
        }),
        #[cfg(feature = "macros")]
        ast::Stmt::Macro(stmt) => {
            state.assign(stmt.0.name);
            tracker_visit_macro(&stmt.0, state);
        }
        #[cfg(feature = "macros")]
        ast::Stmt::CallBlock(stmt) => {
            tracker_visit_expr(&stmt.call.expr, state);
            stmt.call
                .args
                .iter()
                .for_each(|x| tracker_visit_callarg(x, state));
            tracker_visit_macro(&stmt.macro_decl, state);
        }
        #[cfg(feature = "loop_controls")]
        ast::Stmt::Continue(_) | ast::Stmt::Break(_) => {}
        ast::Stmt::Do(stmt) => {
            tracker_visit_expr(&stmt.expr, state);
        }
        ast::Stmt::Comment(_) => {}
    }
}

/// A statically-discovered function call whose positional arguments are all
/// string literals.
///
/// Produced by [`find_string_arg_calls`]. The dbt parser uses this to surface
/// `source()` dependencies that live inside a Jinja `{% if %}` branch which
/// evaluates to `false` during the `execute=false` discovery render, and would
/// therefore never be observed by render-driven dependency collection.
/// See dbt-fusion issue #1660.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFunctionCall {
    /// Name of the called function (a bare-variable callee).
    pub name: String,
    /// Positional string-literal arguments, in source order.
    pub args: Vec<String>,
    /// Span of the call expression in the template source.
    pub span: Span,
}

/// State for [`find_string_arg_calls`], mirroring [`AssignmentTracker`]:
/// a private holder carried through a hand-rolled AST walk.
struct CallTracker<'a> {
    names: &'a [&'a str],
    out: Vec<StaticFunctionCall>,
}

/// Walks `node` and returns every function call.
///
/// Unlike rendering, this visits both arms of every `{% if %}` and all loop
/// bodies, so the result does not depend on runtime branch evaluation. A call
/// with a non-constant positional argument is skipped entirely: it cannot be
/// resolved without rendering.
pub fn find_string_arg_calls(node: &ast::Stmt<'_>, names: &[&str]) -> Vec<StaticFunctionCall> {
    let mut state = CallTracker {
        names,
        out: Vec::new(),
    };
    state.visit_stmt(node);
    state.out
}

impl<'a> CallTracker<'a> {
    fn visit_stmt(&mut self, node: &ast::Stmt<'_>) {
        match node {
            ast::Stmt::Template(stmt) => self.visit_stmts(&stmt.children),
            ast::Stmt::EmitExpr(stmt) => self.visit_expr(&stmt.expr),
            ast::Stmt::EmitRaw(_) | ast::Stmt::Comment(_) => {}
            ast::Stmt::ForLoop(stmt) => {
                self.visit_expr(&stmt.iter);
                self.visit_expr_opt(&stmt.filter_expr);
                self.visit_stmts(&stmt.body);
                self.visit_stmts(&stmt.else_body);
            }
            ast::Stmt::IfCond(stmt) => {
                // Both branches are visited regardless of how `expr` evaluates
                self.visit_expr(&stmt.expr);
                self.visit_stmts(&stmt.true_body);
                self.visit_stmts(&stmt.false_body);
            }
            ast::Stmt::WithBlock(stmt) => {
                for (_, expr) in &stmt.assignments {
                    self.visit_expr(expr);
                }
                self.visit_stmts(&stmt.body);
            }
            ast::Stmt::Set(stmt) => self.visit_expr(&stmt.expr),
            ast::Stmt::SetBlock(stmt) => self.visit_stmts(&stmt.body),
            ast::Stmt::AutoEscape(stmt) => self.visit_stmts(&stmt.body),
            ast::Stmt::FilterBlock(stmt) => self.visit_stmts(&stmt.body),
            #[cfg(feature = "multi_template")]
            ast::Stmt::Block(stmt) => self.visit_stmts(&stmt.body),
            #[cfg(feature = "multi_template")]
            ast::Stmt::Extends(_) | ast::Stmt::Include(_) => {}
            #[cfg(feature = "multi_template")]
            ast::Stmt::Import(_) | ast::Stmt::FromImport(_) => {}
            #[cfg(feature = "macros")]
            ast::Stmt::Macro(stmt) => self.visit_stmts(&stmt.0.body),
            #[cfg(feature = "macros")]
            ast::Stmt::CallBlock(stmt) => {
                self.visit_call(&stmt.call);
                self.visit_stmts(&stmt.macro_decl.body);
            }
            #[cfg(feature = "loop_controls")]
            ast::Stmt::Continue(_) | ast::Stmt::Break(_) => {}
            ast::Stmt::Do(stmt) => self.visit_expr(&stmt.expr),
        }
    }

    fn visit_expr(&mut self, expr: &ast::Expr<'_>) {
        match expr {
            ast::Expr::Var(_) | ast::Expr::Const(_) => {}
            ast::Expr::UnaryOp(e) => self.visit_expr(&e.expr),
            ast::Expr::BinOp(e) => {
                self.visit_expr(&e.left);
                self.visit_expr(&e.right);
            }
            ast::Expr::IfExpr(e) => {
                self.visit_expr(&e.test_expr);
                self.visit_expr(&e.true_expr);
                self.visit_expr_opt(&e.false_expr);
            }
            ast::Expr::Filter(e) => {
                self.visit_expr_opt(&e.expr);
                self.visit_callargs(&e.args);
            }
            ast::Expr::Test(e) => {
                self.visit_expr(&e.expr);
                self.visit_callargs(&e.args);
            }
            ast::Expr::GetAttr(e) => self.visit_expr(&e.expr),
            ast::Expr::GetItem(e) => {
                self.visit_expr(&e.expr);
                self.visit_expr(&e.subscript_expr);
            }
            ast::Expr::Slice(e) => {
                self.visit_expr(&e.expr);
                self.visit_expr_opt(&e.start);
                self.visit_expr_opt(&e.stop);
                self.visit_expr_opt(&e.step);
            }
            ast::Expr::Call(e) => self.visit_call(e),
            ast::Expr::List(e) => self.visit_exprs(&e.items),
            ast::Expr::Map(e) => {
                self.visit_exprs(&e.keys);
                self.visit_exprs(&e.values);
            }
            ast::Expr::Tuple(e) => self.visit_exprs(&e.items),
        }
    }

    fn visit_callarg(&mut self, callarg: &ast::CallArg<'_>) {
        match callarg {
            ast::CallArg::Pos(expr)
            | ast::CallArg::Kwarg(_, expr)
            | ast::CallArg::PosSplat(expr)
            | ast::CallArg::KwargSplat(expr) => self.visit_expr(expr),
        }
    }

    /// Recurses into the callee and arguments first so nested calls are
    /// found, then records this call if it matches `self.names` and every
    /// positional argument is a string literal.
    fn visit_call(&mut self, call: &ast::Spanned<ast::Call<'_>>) {
        self.visit_expr(&call.expr);
        self.visit_callargs(&call.args);

        let ast::CallType::Function(fname) = call.identify_call() else {
            return;
        };
        if !self.names.contains(&fname) {
            return;
        }

        let mut args = Vec::new();
        for arg in &call.args {
            match arg {
                ast::CallArg::Pos(ast::Expr::Const(c)) => match c.value.as_str() {
                    Some(s) => args.push(s.to_string()),
                    None => return,
                },
                ast::CallArg::Pos(_) | ast::CallArg::PosSplat(_) => return,
                ast::CallArg::Kwarg(_, _) | ast::CallArg::KwargSplat(_) => {}
            }
        }
        if !args.is_empty() {
            self.out.push(StaticFunctionCall {
                name: fname.to_string(),
                args,
                span: call.span,
            });
        }
    }

    fn visit_stmts(&mut self, nodes: &[ast::Stmt<'_>]) {
        nodes.iter().for_each(|n| self.visit_stmt(n));
    }

    fn visit_exprs(&mut self, exprs: &[ast::Expr<'_>]) {
        exprs.iter().for_each(|e| self.visit_expr(e));
    }

    fn visit_callargs(&mut self, args: &[ast::CallArg<'_>]) {
        args.iter().for_each(|a| self.visit_callarg(a));
    }

    fn visit_expr_opt(&mut self, expr: &Option<ast::Expr<'_>>) {
        if let Some(e) = expr {
            self.visit_expr(e);
        }
    }
}
