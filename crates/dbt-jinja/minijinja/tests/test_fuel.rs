#![cfg(feature = "fuel")]

use minijinja::{context, Environment, ErrorKind};

#[test]
fn test_basic() {
    let mut env = Environment::new();
    assert_eq!(env.fuel(), None);
    env.set_fuel(Some(100));
    assert_eq!(env.fuel(), Some(100));
    env.add_template("test", "{% for x in seq %}{{ x }}\n{% endfor %}")
        .unwrap();
    let t = env.get_template("test").unwrap();

    // this will still manage to run with 100 fuel
    let rv = t
        .render(context!(seq => (0..15).collect::<Vec<_>>()), &[])
        .unwrap();
    assert_eq!(rv.lines().count(), 15);

    // this is above the limit
    let rv = t
        .render(context!(seq => (0..20).collect::<Vec<_>>()), &[])
        .unwrap_err();
    assert_eq!(rv.kind(), ErrorKind::OutOfFuel);
}

#[cfg(feature = "macros")]
#[test]
fn test_macro_fuel() {
    let mut env = Environment::new();
    assert_eq!(env.fuel(), None);
    env.set_fuel(Some(100));
    assert_eq!(env.fuel(), Some(100));
    env.add_template(
        "test",
        "
        {% macro x() %}{% for item in range(5) %}...{% endfor %}{% endmacro %}
        {% for count in range(macros) %}{{ x() }}{% endfor %}
    ",
    )
    .unwrap();
    let t = env.get_template("test").unwrap();

    // this should succeed
    t.render(context!(macros => 3), &[]).unwrap();

    // but running more macros should not
    let err = t.render(context!(macros => 5), &[]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::OutOfFuel);
}
