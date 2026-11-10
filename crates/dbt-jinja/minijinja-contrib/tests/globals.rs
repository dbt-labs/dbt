use insta::assert_snapshot;
use minijinja::{render, Environment};
use minijinja_contrib::globals::{create_dict_namespace, cycler, joiner};

#[test]
fn test_cycler() {
    let mut env = Environment::new();
    env.add_function("cycler", cycler);

    assert_snapshot!(render!(in env, r"{% set c = cycler(1, 2) -%}
next(): {{ c.next() }}
next(): {{ c.next() }}
next(): {{ c.next() }}
cycler: {{ c }}"), @r###"
    next(): 1
    next(): 2
    next(): 1
    cycler: Cycler { items: [1, 2], pos: 1 }
    "###);
}

#[test]
fn test_joiner() {
    let mut env = Environment::new();
    env.add_function("joiner", joiner);

    assert_snapshot!(render!(in env, r"{% set j = joiner() -%}
first: [{{ j() }}]
second: [{{ j() }}]
joiner: {{ j }}"), @r"
    first: []
    second: [, ]
    joiner: Joiner { sep: ', ', used: true }
    ");

    assert_snapshot!(render!(in env, r"{% set j = joiner('|') -%}
first: [{{ j() }}]
second: [{{ j() }}]
joiner: {{ j }}"), @r"
    first: []
    second: [|]
    joiner: Joiner { sep: '|', used: true }
    ");
}

fn dict_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.add_global("dict", create_dict_namespace());
    env
}

#[test]
fn test_dict_fromkeys_default_none() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict.fromkeys(['a', 'b']) }}"),
        @"{'a': None, 'b': None}"
    );
}

#[test]
fn test_dict_fromkeys_custom_default() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict.fromkeys(['a', 'b'], 0) }}"),
        @"{'a': 0, 'b': 0}"
    );
}

#[test]
fn test_dict_fromkeys_string_iterable() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict.fromkeys('abc', '!') }}"),
        @"{'a': '!', 'b': '!', 'c': '!'}"
    );
}

#[test]
fn test_dict_fromkeys_dedupes() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict.fromkeys(['a', 'a', 'b']) }}"),
        @"{'a': None, 'b': None}"
    );
}

#[test]
fn test_dict_unknown_method_falls_back_to_constructor() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict.from_keys([('a', 1), ('b', 2)]) }}"),
        @"{'a': 1, 'b': 2}"
    );
    assert_snapshot!(
        render!(in env, r"{{ dict.anything([('a', 1)]) }}"),
        @"{'a': 1}"
    );
}

#[test]
fn test_dict_call_empty() {
    let env = dict_env();
    assert_snapshot!(render!(in env, r"{{ dict() }}"), @"{}");
}

#[test]
fn test_dict_call_kwargs() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict(a=1, b=2) }}"),
        @"{'a': 1, 'b': 2}"
    );
}

#[test]
fn test_dict_call_pairs() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict([('a', 1), ('b', 2)]) }}"),
        @"{'a': 1, 'b': 2}"
    );
}

#[test]
fn test_dict_call_mapping() {
    let env = dict_env();
    assert_snapshot!(
        render!(in env, r"{{ dict({'a': 1, 'b': 2}) }}"),
        @"{'a': 1, 'b': 2}"
    );
}

#[test]
fn test_dict_call_pairs_with_kwargs_overrides() {
    let env = dict_env();
    // kwargs come after positional and override matching keys, like Python.
    assert_snapshot!(
        render!(in env, r"{{ dict([('a', 1), ('b', 2)], b=99) }}"),
        @"{'a': 1, 'b': 99}"
    );
}

#[test]
fn test_dict_call_bad_pair_length_errors() {
    let env = dict_env();
    let err = env
        .render_str(r"{{ dict([('a',), ('b', 2)]) }}", (), &[])
        .unwrap_err();
    let detail = err.detail().unwrap_or_default();
    assert!(
        detail.contains("dictionary update sequence element #0 has length 1"),
        "unexpected error: {detail}"
    );
}

// Error-path parity with CPython: dict.fromkeys(<non-iterable>) raises a
// TypeError of the form "'<typename>' object is not iterable". This is
// covered here (rather than in the SLT) because the SLT's dbt-jinja
// executor wraps the same error in a different envelope string, making
// a single shared SLT golden unworkable.
#[test]
fn test_dict_fromkeys_int_errors_like_python() {
    let env = dict_env();
    let err = env
        .render_str(r"{{ dict.fromkeys(42) }}", (), &[])
        .unwrap_err();
    let detail = err.detail().unwrap_or_default();
    assert_eq!(detail, "'int' object is not iterable");
}

#[test]
#[cfg(feature = "rand")]
#[cfg(target_pointer_width = "64")]
fn test_lispum() {
    // The small rng is pointer size specific.  Test on 64bit platforms only
    use minijinja_contrib::globals::lipsum;

    let mut env = Environment::new();
    env.add_function("lipsum", lipsum);

    assert_snapshot!(render!(in env, r"{% set RAND_SEED = 42 %}{{ lipsum(5) }}"), @r###"
    Facilisi accumsan class rutrum integer euismod gravida cras vsociis arcu lobortis sociosqu elementum lacus nulla. Leo imperdiet penatibus id quam malesuada pretium sociosqu scelerisque diam sociosqu penatibus imperdiet et nisl. Ante s vulputate nulla porta ssociis per gravida primis porta penatibus nostra congue dui.

    Ipsum cras integer magna ssociis etiam eu rutrum ac praesent ssociis primis nisl malesuada sociosqu. Senectus sem neque ridiculus aliquet duis nisl facilisis quam diam nibh ad eget. Rutrum mauris aliquam faucibus magna eu phasellus ssociis libero neque convallis magna. Ante aliquet proin montes nibh sociosqu vulputate auctor.

    Lacinia aliquam dictumst pellentesque nibh sociosqu sagittis leo ad dictum elementum sapien mi sociosqu. Et ssociis laoreet dolor egestas scelerisque potenti duis natoque ssociis feugiat. Proin luctus porta rhoncus quis phasellus netus non proin sociosqu nonummy ornare lacinia. Leo sociis inceptos cum leo non elit class sed sapien dictum diam mattis dapibus netus facilisis. Hendrerit montes aliquam ssociis ridiculus a cras sociosqu nisi ssociis curabitur.

    Justo nonummy pulvinar potenti in potenti at facilisi platea sagittis scelerisque quis sapien semper dictum in ipsum. Nunc nonummy ornare etiam elementum nullam curae eu nullam ad nascetur ssociis nullam mus. Nisi ssociis gravida dapibus non sociosqu laoreet adipiscing potenti ipsum parturient potenti mollis odio. Leo eget felis pretium libero consectetuer hymenaeos sociosqu ssociis in posuere.

    S commodo fames ridiculus luctus proin non aptent nullam mi eleifend consectetuer aliquam ad. Scelerisque nisl blandit sociis euismod curae semper nunc nec litora condimentum fames habitasse. Inceptos augue sociosqu hendrerit justo montes orci proin mus molestie id iaculis nostra lacus. Cum facilisis potenti facilisis nonummy sem.

    "###);

    assert_snapshot!(render!(in env, r"{% set RAND_SEED = 42 %}{{ lipsum(2, html=true) }}"), @r###"
    <p>Facilisi accumsan class rutrum integer euismod gravida cras vsociis arcu lobortis sociosqu elementum lacus nulla. Leo imperdiet penatibus id quam malesuada pretium sociosqu scelerisque diam sociosqu penatibus imperdiet et nisl. Ante s vulputate nulla porta ssociis per gravida primis porta penatibus nostra congue dui.</p>

    <p>Ipsum cras integer magna ssociis etiam eu rutrum ac praesent ssociis primis nisl malesuada sociosqu. Senectus sem neque ridiculus aliquet duis nisl facilisis quam diam nibh ad eget. Rutrum mauris aliquam faucibus magna eu phasellus ssociis libero neque convallis magna. Ante aliquet proin montes nibh sociosqu vulputate auctor.</p>

    "###);
}

#[test]
#[cfg(feature = "rand")]
fn test_randrange() {
    use minijinja_contrib::globals::randrange;

    let mut env = Environment::new();
    env.add_function("randrange", randrange);

    assert_snapshot!(render!(in env, r"{% set RAND_SEED = 42 %}{{ randrange(10) }}"), @"1");
    assert_snapshot!(render!(in env, r"{% set RAND_SEED = 42 %}{{ randrange(-50, 50) }}"), @"-20");
}
