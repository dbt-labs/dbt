use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use dashmap::DashMap;
use serde::Serialize;

use crate::compiler::ast;
use crate::compiler::cfg::build_cfg;
use crate::compiler::instructions::Instructions;
use crate::compiler::meta::find_undeclared;
use crate::compiler::parser::parse_expr;
use crate::compiler::typecheck::FunctionRegistry;
use crate::environment::Environment;
use crate::error::Error;
use crate::listener::RenderingEventListener;
use crate::value::Value;
use crate::vm::listeners::TypecheckingEventListener;
use crate::vm::typemeta::TypeChecker;
use crate::vm::Vm;
use crate::Type;

/// A handle to a compiled expression.
///
/// An expression is created via the
/// [`compile_expression`](Environment::compile_expression) method.  It provides
/// a method to evaluate the expression and return the result as value object.
/// This for instance can be used to evaluate simple expressions from user
/// provided input to implement features such as dynamic filtering.
///
/// This is usually best paired with [`context`](crate::context!) to pass
/// a single value to it.
///
/// # Example
///
/// ```rust
/// # use minijinja::{Environment, context, listener::DefaultRenderingEventListener};
/// # use std::rc::Rc;
/// let env = Environment::new();
/// let expr = env.compile_expression("number > 10 and number < 20").unwrap();
/// let rv = expr.eval(context!(number => 15), &[Rc::new(DefaultRenderingEventListener::default())]).unwrap();
/// assert!(rv.is_true());
/// ```
pub struct Expression<'env, 'source> {
    env: &'env Environment<'source>,
    instr: ExpressionBacking<'source>,
}

enum ExpressionBacking<'source> {
    Borrowed(Instructions<'source>),
    #[cfg(feature = "loader")]
    Owned(crate::loader::OwnedInstructions),
}

impl fmt::Debug for Expression<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Expression")
            .field("env", &self.env)
            .finish()
    }
}

impl<'env, 'source> Expression<'env, 'source> {
    pub(crate) fn new(
        env: &'env Environment<'source>,
        instructions: Instructions<'source>,
    ) -> Expression<'env, 'source> {
        Expression {
            env,
            instr: ExpressionBacking::Borrowed(instructions),
        }
    }

    #[cfg(feature = "loader")]
    pub(crate) fn new_owned(
        env: &'env Environment<'source>,
        instructions: crate::loader::OwnedInstructions,
    ) -> Expression<'env, 'source> {
        Expression {
            env,
            instr: ExpressionBacking::Owned(instructions),
        }
    }

    fn instructions(&self) -> &Instructions<'_> {
        match self.instr {
            ExpressionBacking::Borrowed(ref x) => x,
            #[cfg(feature = "loader")]
            ExpressionBacking::Owned(ref x) => x.borrow_dependent(),
        }
    }

    /// Evaluates the expression with some context.
    ///
    /// The result of the expression is returned as [`Value`].
    pub fn eval<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        // reduce total amount of code faling under mono morphization into
        // this function, and share the rest in _eval.
        self._eval(Value::from_serialize(&ctx), listeners)
    }

    /// Returns a set of all undeclared variables in the expression.
    ///
    /// This works the same as
    /// [`Template::undeclared_variables`](crate::Template::undeclared_variables).
    pub fn undeclared_variables(&self, nested: bool) -> HashSet<String> {
        match parse_expr(self.instructions().source()) {
            Ok(expr) => find_undeclared(
                &ast::Stmt::EmitExpr(ast::Spanned::new(
                    ast::EmitExpr { expr },
                    Default::default(),
                )),
                nested,
            ),
            Err(_) => HashSet::new(),
        }
    }

    /// Typechecks the expression with the given context.
    ///
    /// This performs static type analysis on the compiled expression to catch potential
    /// type errors before runtime execution.
    pub fn typecheck(
        &self,
        funcsigns: Arc<FunctionRegistry>,
        builtins: Arc<DashMap<String, Type>>,
        warning_printer: Rc<dyn TypecheckingEventListener>,
        typecheck_resolved_context: BTreeMap<String, Value>,
    ) -> Result<(), Error> {
        let instructions = self.instructions();
        // build CFG
        let cfg = build_cfg(&instructions.instructions);
        // create a typechecker
        let mut typechecker =
            TypeChecker::new(&instructions.instructions, cfg, funcsigns, builtins);

        typechecker
            .check(warning_printer, typecheck_resolved_context)
            .map_err(|err| {
                Error::new(
                    crate::error::ErrorKind::InvalidOperation,
                    format!("Type checking failed: {err}"),
                )
            })
    }

    fn _eval(
        &self,
        root: Value,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        Ok(ok!(Vm::new(self.env).eval(
            self.instructions(),
            root,
            &BTreeMap::new(),
            crate::AutoEscape::None,
            listeners
        ))
        .0)
    }
}
