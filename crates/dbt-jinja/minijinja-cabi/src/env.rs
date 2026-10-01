use std::ffi::{c_char, CString};
use std::ptr;

use minijinja::syntax::SyntaxConfig;
use minijinja::{Environment, Error, ErrorKind, UndefinedBehavior};

use crate::mj_value;

/// Pointer to a MiniJinja environment.
pub struct mj_env(pub(crate) Environment<'static>);

ffi_fn! {
    /// Allocates a new and empty MiniJinja environment.
    unsafe fn mj_env_new(_scope) -> *mut mj_env {
        Box::into_raw(Box::new(mj_env(Environment::new())))
    }
}

ffi_fn! {
    /// Frees a MiniJinja environment.
    unsafe fn mj_env_free(_scope, env: *mut mj_env) {
        if !env.is_null() {
            drop(Box::from_raw(env));
        }
    }
}

ffi_fn! {
    /// Registers a template with the environment.
    ///
    /// This returns `false` if the template is malformed.
    unsafe fn mj_env_add_template(
        scope,
        env: *mut mj_env,
        name: *const c_char,
        source: *const c_char
    ) -> bool {
        (*env).0.add_template_owned(
            scope.get_str(name)?.to_string(),
            scope.get_str(source)?.to_string()
        )?;
        true
    }
}

ffi_fn! {
    /// Removes a template from the environment.
    ///
    /// This returns `false` if the template is malformed.
    unsafe fn mj_env_remove_template(
        scope,
        env: *mut mj_env,
        name: *const c_char,
    ) -> bool {
        (*env).0.remove_template(scope.get_str(name)?);
        true
    }
}

ffi_fn! {
    /// Clears all templates.
    unsafe fn mj_env_clear_templates(
        _scope,
        env: *mut mj_env,
    ) -> bool {
        (*env).0.clear_templates();
        true
    }
}

ffi_fn! {
    /// Renders a template registered on the environment.
    ///
    /// Takes ownership of the given context.
    unsafe fn mj_env_render_template(
        scope,
        env: *const mj_env,
        name: *const c_char,
        ctx: mj_value
    ) -> *mut c_char {
        let t = (*env).0.get_template(scope.get_str(name)?)?;
        let rv = t.render(ctx.into_value())?;
        CString::new(rv).map_err(|_| {
            Error::new(ErrorKind::InvalidOperation, "template rendered null bytes")
        })?.into_raw()
    }
}

ffi_fn! {
    /// Renders a template from a named string.
    ///
    /// Takes ownership of the given context.
    unsafe fn mj_env_render_named_str(
        scope,
        env: *const mj_env,
        name: *const c_char,
        source: *const c_char,
        ctx: mj_value
    ) -> *mut c_char {
        let rv = (*env).0.render_named_str(
            scope.get_str(name)?,
            scope.get_str(source)?,
            ctx.into_value()
        )?;
        CString::new(rv).map_err(|_| {
            Error::new(ErrorKind::InvalidOperation, "template rendered null bytes")
        })?.into_raw()
    }
}

ffi_fn! {
    /// Evaluate an expression.
    unsafe fn mj_env_eval_expr(
        scope,
        env: *const mj_env,
        expr: *const c_char,
        ctx: mj_value
    ) -> mj_value {
        let expr = (*env).0.compile_expression(scope.get_str(expr)?)?;
        expr.eval(ctx.into_value())?.into()
    }
}

ffi_fn! {
    /// Frees an engine allocated string.
    unsafe fn mj_str_free(_scope, s: *mut c_char) {
        if !s.is_null() {
            let _ = CString::from_raw(s);
        }
    }
}

ffi_fn! {
    /// Enables or disables the `lstrip_blocks` feature.
    unsafe fn mj_env_set_lstrip_blocks(_scope, env: *mut mj_env, val: bool) {
        (*env).0.set_lstrip_blocks(val);
    }
}

ffi_fn! {
    /// Enables or disables the `trim_blocks` feature.
    unsafe fn mj_env_set_trim_blocks(_scope, env: *mut mj_env, val: bool) {
        (*env).0.set_trim_blocks(val);
    }
}

ffi_fn! {
    /// Preserve the trailing newline when rendering templates.
    unsafe fn mj_env_set_keep_trailing_newline(_scope, env: *mut mj_env, val: bool) {
        (*env).0.set_keep_trailing_newline(val);
    }
}

/// Allows one to override the syntax elements.
#[repr(C)]
pub struct mj_syntax_config {
    block_start: *const c_char,
    block_end: *const c_char,
    variable_start: *const c_char,
    variable_end: *const c_char,
    comment_start: *const c_char,
    comment_end: *const c_char,
    line_statement_prefix: *const c_char,
    line_comment_prefix: *const c_char,
}

ffi_fn! {
    /// Reconfigures the syntax.
    unsafe fn mj_env_set_syntax_config(scope, env: *mut mj_env, syntax: &mj_syntax_config) -> bool {
        let mut builder = SyntaxConfig::builder();
        builder
            .block_delimiters(
                scope.get_str(syntax.block_start)?.to_string(),
                scope.get_str(syntax.block_end)?.to_string()
            )
            .variable_delimiters(
                scope.get_str(syntax.variable_start)?.to_string(),
                scope.get_str(syntax.variable_end)?.to_string()
            )
            .comment_delimiters(
                scope.get_str(syntax.comment_start)?.to_string(),
                scope.get_str(syntax.comment_end)?.to_string()
            );
        let line_statement_prefix = scope.get_str(syntax.line_statement_prefix)?;
        if !line_statement_prefix.is_empty() {
            builder.line_statement_prefix(line_statement_prefix.to_string());
        }
        let line_comment_prefix = scope.get_str(syntax.line_comment_prefix)?;
        if !line_comment_prefix.is_empty() {
            builder.line_comment_prefix(line_comment_prefix.to_string());
        }
        (*env).0.set_syntax(builder.build()?);
        true
    }
}

ffi_fn! {
    /// Sets the syntax to defaults.
    unsafe fn mj_syntax_config_default(_scope, syntax: &mut mj_syntax_config) {
        syntax.block_start = "{%".as_ptr() as *const _;
        syntax.block_end = "%}".as_ptr() as *const _;
        syntax.variable_start = "{{".as_ptr() as *const _;
        syntax.variable_end = "}}".as_ptr() as *const _;
        syntax.comment_start = "{#".as_ptr() as *const _;
        syntax.comment_end = "#}".as_ptr() as *const _;
        syntax.line_statement_prefix = ptr::null();
        syntax.line_comment_prefix = ptr::null();
    }
}

/// Controls the undefined behavior of the engine.
#[repr(C)]
pub enum mj_undefined_behavior {
    /// The default, somewhat lenient undefined behavior.
    MJ_UNDEFINED_BEHAVIOR_LENIENT,
    /// Complains very quickly about undefined values.
    MJ_UNDEFINED_BEHAVIOR_STRICT,
    /// Like Lenient, but also allows chaining of undefined lookups.
    MJ_UNDEFINED_BEHAVIOR_CHAINABLE,
}

ffi_fn! {
    /// Reconfigures the undefined behavior.
    unsafe fn mj_env_set_undefined_behavior(_scope, env: *mut mj_env, val: mj_undefined_behavior) {
        (*env).0.set_undefined_behavior(match val {
            mj_undefined_behavior::MJ_UNDEFINED_BEHAVIOR_LENIENT => UndefinedBehavior::Lenient,
            mj_undefined_behavior::MJ_UNDEFINED_BEHAVIOR_STRICT => UndefinedBehavior::Strict,
            mj_undefined_behavior::MJ_UNDEFINED_BEHAVIOR_CHAINABLE => UndefinedBehavior::Chainable,
        })
    }
}

ffi_fn! {
    /// Enables or disables debug mode.
    unsafe fn mj_env_set_debug(_scope, env: *mut mj_env, val: bool) {
        (*env).0.set_debug(val);
    }
}

ffi_fn! {
    /// Changes the recursion limit.
    unsafe fn mj_env_set_recursion_limit(_scope, env: *mut mj_env, val: u32) {
        (*env).0.set_recursion_limit(val as _);
    }
}
