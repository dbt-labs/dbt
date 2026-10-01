use minijinja::value::Value;
use minijinja::{Environment, State};

#[test]
fn test_state_lookup_global() {
    let mut env = Environment::new();
    env.add_function("lookup_global", |state: &State| -> Value {
        state.lookup("the_global", &[]).unwrap_or_default()
    });
    env.add_global("the_global", true);
    let rv = env.render_str("[{{ lookup_global() }}]", (), &[]).unwrap();
    assert_eq!(rv, "[True]");
}
