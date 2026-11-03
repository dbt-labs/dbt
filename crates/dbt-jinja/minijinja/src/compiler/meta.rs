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
    /// Lenient mode for collecting runtime values (`var`, `env_var`): a call
    /// matches once its first argument is a string literal, and the walk also
    /// covers macro/call-block defaults and set/filter block filters.
    runtime_values: bool,
    out: Vec<StaticFunctionCall>,
    dynamic_calls: HashSet<String>,
}

/// Runtime calls with literal names, and functions whose names require evaluation.
pub struct RuntimeValueCalls {
    pub calls: Vec<StaticFunctionCall>,
    pub dynamic_calls: HashSet<String>,
}

fn context_field<'e>(expr: &'e ast::Expr<'_>) -> Option<Option<&'e str>> {
    match expr {
        ast::Expr::GetAttr(field) if matches!(&field.expr, ast::Expr::Var(var) if var.id == "context") => {
            Some(match field.name {
                "get" | "values" | "items" | "copy" => None,
                name => Some(name),
            })
        }
        ast::Expr::GetItem(field) if matches!(&field.expr, ast::Expr::Var(var) if var.id == "context") => {
            Some(match &field.subscript_expr {
                ast::Expr::Const(value) => value.value.as_str(),
                _ => None,
            })
        }
        ast::Expr::Call(call) if matches!(call.identify_call(), ast::CallType::Method(ast::Expr::Var(var), "get") if var.id == "context") => {
            Some(match call.args.first() {
                Some(ast::CallArg::Pos(ast::Expr::Const(value))) => value.value.as_str(),
                _ => None,
            })
        }
        _ => None,
    }
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
        runtime_values: false,
        out: Vec::new(),
        dynamic_calls: HashSet::new(),
    };
    state.visit_stmt(node);
    state.out
}

/// Like [`find_string_arg_calls`], but a call is recorded as soon as its first
/// positional argument is a string literal; the other arguments (e.g. a
/// default value) may be arbitrary expressions and are ignored. `args` of the
/// returned calls holds only that first argument. Also scans macro and
/// call-block defaults and set/filter block filters.
pub fn find_first_string_arg_calls(
    node: &ast::Stmt<'_>,
    names: &[&str],
) -> Vec<StaticFunctionCall> {
    find_runtime_value_calls(node, names).calls
}

/// Includes unresolved calls so callers can account for the full input namespace.
pub fn find_runtime_value_calls(node: &ast::Stmt<'_>, names: &[&str]) -> RuntimeValueCalls {
    let mut state = CallTracker {
        names,
        runtime_values: true,
        out: Vec::new(),
        dynamic_calls: HashSet::new(),
    };
    state.visit_stmt(node);
    RuntimeValueCalls {
        calls: state.out,
        dynamic_calls: state.dynamic_calls,
    }
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
            ast::Stmt::SetBlock(stmt) => {
                if self.runtime_values {
                    self.visit_expr_opt(&stmt.filter);
                }
                self.visit_stmts(&stmt.body);
            }
            ast::Stmt::AutoEscape(stmt) => {
                if self.runtime_values {
                    self.visit_expr(&stmt.enabled);
                }
                self.visit_stmts(&stmt.body);
            }
            ast::Stmt::FilterBlock(stmt) => {
                if self.runtime_values {
                    self.visit_expr(&stmt.filter);
                }
                self.visit_stmts(&stmt.body);
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Block(stmt) => self.visit_stmts(&stmt.body),
            #[cfg(feature = "multi_template")]
            ast::Stmt::Extends(stmt) => {
                if self.runtime_values {
                    self.visit_expr(&stmt.name);
                }
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Include(stmt) => {
                if self.runtime_values {
                    self.visit_expr(&stmt.name);
                }
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::Import(stmt) => {
                if self.runtime_values {
                    self.visit_expr(&stmt.expr);
                }
            }
            #[cfg(feature = "multi_template")]
            ast::Stmt::FromImport(stmt) => {
                if self.runtime_values {
                    self.visit_expr(&stmt.expr);
                }
            }
            #[cfg(feature = "macros")]
            ast::Stmt::Macro(stmt) => {
                if self.runtime_values {
                    self.visit_exprs(&stmt.0.defaults);
                }
                self.visit_stmts(&stmt.0.body);
            }
            #[cfg(feature = "macros")]
            ast::Stmt::CallBlock(stmt) => {
                self.visit_call(&stmt.call);
                if self.runtime_values {
                    self.visit_exprs(&stmt.macro_decl.defaults);
                }
                self.visit_stmts(&stmt.macro_decl.body);
            }
            #[cfg(feature = "loop_controls")]
            ast::Stmt::Continue(_) | ast::Stmt::Break(_) => {}
            ast::Stmt::Do(stmt) => self.visit_expr(&stmt.expr),
        }
    }

    fn visit_expr(&mut self, expr: &ast::Expr<'_>) {
        if self.runtime_values {
            if let Some(field) = context_field(expr) {
                match field {
                    Some(name) if self.names.contains(&name) => {
                        self.dynamic_calls.insert(name.to_string());
                    }
                    Some("context") | None => self
                        .dynamic_calls
                        .extend(self.names.iter().map(|name| name.to_string())),
                    _ => {}
                }
                if let ast::Expr::GetItem(field) = expr {
                    self.visit_expr(&field.subscript_expr);
                } else if let ast::Expr::Call(call) = expr {
                    self.visit_callargs(&call.args);
                }
                return;
            }
        }
        match expr {
            ast::Expr::Var(var) => {
                if self.runtime_values {
                    if self.names.contains(&var.id) {
                        self.dynamic_calls.insert(var.id.to_string());
                    } else if var.id == "context" {
                        self.dynamic_calls
                            .extend(self.names.iter().map(|name| name.to_string()));
                    }
                }
            }
            ast::Expr::Const(value) => {
                // Runtime call results and defaults can contain nested templates.
                if self.runtime_values {
                    if let Some(source) = value.value.as_str() {
                        if ["{{", "{%", "{#"]
                            .iter()
                            .any(|token| source.contains(token))
                        {
                            if let Ok(template) = crate::compiler::parser::parse(
                                source,
                                "<runtime-string>",
                                crate::syntax::SyntaxConfig::default(),
                                crate::compiler::lexer::WhitespaceConfig::default(),
                            ) {
                                self.visit_stmt(&template);
                            }
                        }
                    }
                }
            }
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
        let fname = match call.identify_call() {
            ast::CallType::Function(name) if self.names.contains(&name) => Some(name),
            ast::CallType::Method(ast::Expr::Var(var), "has_var")
                if self.runtime_values && var.id == "var" =>
            {
                Some("var")
            }
            _ if self.runtime_values => context_field(&call.expr)
                .flatten()
                .filter(|name| self.names.contains(name)),
            _ => None,
        };
        // A callable used as a value can escape through aliases or context lookups.
        // Direct calls already retain their precise literal-name dependency below.
        if fname.is_none() {
            self.visit_expr(&call.expr);
        }
        self.visit_callargs(&call.args);
        let Some(fname) = fname else {
            return;
        };
        if !self.names.contains(&fname) {
            return;
        }

        let mut args = Vec::new();
        if self.runtime_values {
            // The runtime accepts the name positionally or as a keyword
            // (`var(name=..)`, `env_var(var=..)`).
            let kw = if fname == "env_var" { "var" } else { "name" };
            let first = match call.args.first() {
                Some(ast::CallArg::Pos(e)) => Some(e),
                _ => call.args.iter().find_map(|a| match a {
                    ast::CallArg::Kwarg(k, e) if *k == kw => Some(e),
                    _ => None,
                }),
            };
            if let Some(ast::Expr::Const(c)) = first {
                if let Some(s) = c.value.as_str() {
                    self.out.push(StaticFunctionCall {
                        name: fname.to_string(),
                        args: vec![s.to_string()],
                        span: call.span,
                    });
                    return;
                }
            }
            self.dynamic_calls.insert(fname.to_string());
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::lexer::WhitespaceConfig;
    use crate::compiler::parser::parse;
    use crate::syntax::SyntaxConfig;

    fn first_args(src: &str) -> Vec<(String, String)> {
        let ast = parse(
            src,
            "t",
            SyntaxConfig::default(),
            WhitespaceConfig::default(),
        )
        .expect("valid jinja");
        find_first_string_arg_calls(&ast, &["var", "env_var"])
            .into_iter()
            .map(|c| (c.name, c.args.into_iter().next().unwrap()))
            .collect()
    }

    #[test]
    fn find_first_string_arg_calls_captures_call_with_default() {
        assert_eq!(
            first_args("{{ var('a', false) }}"),
            vec![("var".to_string(), "a".to_string())]
        );
    }

    #[test]
    fn find_first_string_arg_calls_skips_dynamic_name() {
        assert!(first_args("{{ var(name) }}{{ var(name, 'x') }}").is_empty());
    }

    #[test]
    fn find_first_string_arg_calls_finds_nested_calls() {
        let src = "{% if x %}{{ var('a') }}{% endif %}\
                   {% for i in y %}{{ var('b', 1) }}{% endfor %}\
                   {% macro m() %}{{ env_var('C') }}{% endmacro %}";
        let mut found = first_args(src);
        found.sort();
        assert_eq!(
            found,
            vec![
                ("env_var".to_string(), "C".to_string()),
                ("var".to_string(), "a".to_string()),
                ("var".to_string(), "b".to_string()),
            ]
        );
    }

    #[test]
    fn find_first_string_arg_calls_captures_env_var_with_kwarg_default() {
        assert_eq!(
            first_args("{{ env_var(\"X\", default=\"y\") }}"),
            vec![("env_var".to_string(), "X".to_string())]
        );
    }

    #[test]
    fn find_first_string_arg_calls_captures_keyword_name() {
        assert_eq!(
            first_args("{{ var(name='a') }}{{ env_var(var='B', default='y') }}"),
            vec![
                ("var".to_string(), "a".to_string()),
                ("env_var".to_string(), "B".to_string())
            ]
        );
    }

    #[test]
    fn find_first_string_arg_calls_scans_macro_default() {
        assert_eq!(
            first_args("{% macro m(mode=env_var('MODE')) %}{{ mode }}{% endmacro %}"),
            vec![("env_var".to_string(), "MODE".to_string())]
        );
    }

    #[test]
    fn find_first_string_arg_calls_scans_call_block_default() {
        assert_eq!(
            first_args("{% call(x=var('v')) m() %}{{ x }}{% endcall %}"),
            vec![("var".to_string(), "v".to_string())]
        );
    }

    #[test]
    fn find_first_string_arg_calls_scans_set_block_filter() {
        assert_eq!(
            first_args("{% set x | default(var('v')) %}{% endset %}"),
            vec![("var".to_string(), "v".to_string())]
        );
    }

    #[test]
    fn find_first_string_arg_calls_scans_filter_block_filter() {
        assert_eq!(
            first_args("{% filter replace('a', env_var('B')) %}a{% endfilter %}"),
            vec![("env_var".to_string(), "B".to_string())]
        );
    }

    #[test]
    fn runtime_calls_in_nested_default_strings() {
        for source in [
            r#"{{ var('query', "{{ env_var('MODE') }}") }}"#,
            r#"{{ env_var('QUERY', default="{{ env_var('MODE') }}") }}"#,
            r#"{% macro m(query="{{ env_var('MODE') }}") %}{{ var('query', query) }}{% endmacro %}"#,
        ] {
            assert!(
                first_args(source).contains(&("env_var".into(), "MODE".into())),
                "{source}"
            );
        }
    }

    #[test]
    fn runtime_calls_in_autoescape_condition() {
        assert_eq!(
            first_args("{% autoescape var('escape', false) %}x{% endautoescape %}"),
            vec![("var".into(), "escape".into())],
        );
    }

    #[test]
    fn runtime_callables_keep_precise_calls_and_mark_escaping_values() {
        for (source, dynamic) in [
            ("{{ var('x') }}{{ env_var('Y') }}", vec![]),
            ("{% set lookup=var %}{{ lookup('x') }}", vec!["var"]),
            ("{{ var(key) }}", vec!["var"]),
            ("{{ env_var(key) }}", vec!["env_var"]),
            ("{{ context['env_var']('Y') }}", vec![]),
            ("{{ context.get('var')('x') }}", vec![]),
            ("{{ context.get('target').name }}", vec![]),
            ("{{ context.target.name }}", vec![]),
            (
                "{% set lookup=context['var'] %}{{ lookup('x') }}",
                vec!["var"],
            ),
            (
                "{% set lookup=context.get('var') %}{{ lookup('x') }}",
                vec!["var"],
            ),
            ("{% include var('template') %}", vec![]),
        ] {
            let ast = parse(
                source,
                "t",
                SyntaxConfig::default(),
                WhitespaceConfig::default(),
            )
            .unwrap();
            let calls = find_runtime_value_calls(&ast, &["var", "env_var"]);
            assert_eq!(
                calls.dynamic_calls,
                dynamic.into_iter().map(str::to_owned).collect(),
                "{source}"
            );
        }
    }

    #[test]
    fn find_string_arg_calls_ignores_macro_default() {
        let ast = parse(
            "{% macro m(s=source('a','b')) %}{% endmacro %}",
            "t",
            SyntaxConfig::default(),
            WhitespaceConfig::default(),
        )
        .expect("valid jinja");
        assert!(find_string_arg_calls(&ast, &["source"]).is_empty());
    }
}
