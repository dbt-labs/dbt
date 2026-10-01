use minijinja::{Environment, Error, Value};
fn eval_expr(env: &Environment, expr: &str) -> Result<Value, Error> {
    env.compile_expression(expr).unwrap().eval((), &[])
}

#[test]
fn test_validation() {
    use minijinja_contrib::modules::validation::create_validation_namespace;

    let mut env = Environment::new();
    env.add_global("validation", create_validation_namespace());

    assert!(eval_expr(
        &env,
        "validation.any['compound', 'interleaved']('compound')"
    )
    .unwrap()
    .is_true());
    assert!(eval_expr(
        &env,
        "validation.any['compound', 'interleaved']('interleaved')"
    )
    .unwrap()
    .is_true());
    assert!(eval_expr(
        &env,
        "validation.any['compound', 'interleaved']('something_else')"
    )
    .is_err());

    assert!(eval_expr(&env, "validation.any['compound']('compound')")
        .unwrap()
        .is_true());
    assert!(eval_expr(&env, "validation.any['compound']('something_else')").is_err());
    // compound here is a type, we should always return true when type is used
    assert!(
        eval_expr(&env, "validation.any[anytype, 'compound']('test')")
            .unwrap()
            .is_true()
    );
    assert!(eval_expr(&env, "validation.any[anytype](1)")
        .unwrap()
        .is_true());
    assert!(eval_expr(&env, "validation.any['test', anytype](1)")
        .unwrap()
        .is_true());
}
