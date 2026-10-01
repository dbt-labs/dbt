use minijinja::{context, Environment};
use minijinja_contrib::filters::pluralize;
use similar_asserts::assert_eq;

#[ignore = "zhong is refactoring vm https://github.com/dbt-labs/fs/issues/4808"]
#[test]
fn test_pluralize() {
    let mut env = Environment::new();

    env.add_filter("pluralize", pluralize);
    for (num, s) in [
        (0, "You have 0 messages."),
        (1, "You have 1 message."),
        (10, "You have 10 messages."),
    ] {
        assert_eq!(
            env.render_str(
                "You have {{ num_messages }} message{{ num_messages|pluralize }}.",
                context! {
                    num_messages => num,
                },
                &[]
            )
            .unwrap(),
            s
        );
    }

    for (num, s) in [
        (0, "You have 0 walruses."),
        (1, "You have 1 walrus."),
        (10, "You have 10 walruses."),
    ] {
        assert_eq!(
            env.render_str(
                r#"You have {{ num_walruses }} walrus{{ num_walruses|pluralize(None, "es") }}."#,
                context! {
                    num_walruses => num,
                },
                &[]
            )
            .unwrap(),
            s
        );
    }

    for (num, s) in [
        (0, "You have 0 cherries."),
        (1, "You have 1 cherry."),
        (10, "You have 10 cherries."),
    ] {
        assert_eq!(
            env.render_str(
                r#"You have {{ num_cherries }} cherr{{ num_cherries|pluralize("y", "ies") }}."#,
                context! {
                    num_cherries => num,
                },
                &[]
            )
            .unwrap(),
            s
        );
    }

    assert_eq!(
        env.render_str(
            r#"You have {{ num_cherries|length }} cherr{{ num_cherries|pluralize("y", "ies") }}."#,
            context! {
                num_cherries => vec![(); 5],
            },
            &[]
        )
        .unwrap(),
        "You have 5 cherries."
    );
    assert_eq!(
        env.render_str(
            r#"You have {{ num_cherries }} cherr{{ num_cherries|pluralize("y", "ies") }}."#,
            context! {
                num_cherries => 5,
            },
            &[]
        )
        .unwrap(),
        "You have 5 cherries."
    );
    assert_eq!(
        env.render_str(
            r#"You have {{ num_cherries }} cherr{{ num_cherries|pluralize("y", "ies") }}."#,
            context! {
                num_cherries => 0.5f32,
            },
            &[]
        )
        .unwrap_err()
        .to_string(),
        "invalid operation: Pluralize argument is not an integer, or a sequence / object with \
            a length but of type number\n(in <string>:1)",
    );
}

#[test]
#[cfg(feature = "rand")]
#[cfg(target_pointer_width = "64")]
fn test_random() {
    // The small rng is pointer size specific.  Test on 64bit platforms only
    use minijinja::render;
    use minijinja_contrib::filters::random;

    let mut env = Environment::new();
    env.add_filter("random", random);

    insta::assert_snapshot!(render!(in env, r"{% set RAND_SEED = 42 %}{{ [1, 2, 3, 4]|random }}"), @"2");
    insta::assert_snapshot!(render!(in env, r"{% set RAND_SEED = 42 %}{{ 'HelloWorld'|random }}"), @"e");
}

#[test]
fn test_filesizeformat() {
    use minijinja::render;
    use minijinja_contrib::filters::filesizeformat;

    let mut env = Environment::new();
    env.add_filter("filesizeformat", filesizeformat);

    insta::assert_snapshot!(render!(in env, r"{{ 0.5|filesizeformat }}"), @"0.5 Bytes");
    insta::assert_snapshot!(render!(in env, r"{{ 1|filesizeformat }}"), @"1 Byte");
    insta::assert_snapshot!(render!(in env, r"{{ -1|filesizeformat }}"), @"-1 Bytes");
    insta::assert_snapshot!(render!(in env, r"{{ 1024|filesizeformat }}"), @"1.0 kB");
    insta::assert_snapshot!(render!(in env, r"{{ 1024|filesizeformat(true) }}"), @"1.0 KiB");
    insta::assert_snapshot!(render!(in env, r"{{ 1000|filesizeformat }}"), @"1.0 kB");
    insta::assert_snapshot!(render!(in env, r"{{ 1000|filesizeformat(true) }}"), @"1000 Bytes");
    insta::assert_snapshot!(render!(in env, r"{{ (1024 * 1024 * 1024)|filesizeformat }}"), @"1.1 GB");
    insta::assert_snapshot!(render!(in env, r"{{ (1024 * 1024 * 1024)|filesizeformat(true) }}"), @"1.0 GiB");
    insta::assert_snapshot!(render!(in env, r"{{ (1024 * 1024 * 1024 * 1024 * 1024)|filesizeformat }}"), @"1.1 PB");
    insta::assert_snapshot!(render!(in env, r"{{ (1024 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024)|filesizeformat }}"), @"1.2 YB");
    insta::assert_snapshot!(render!(in env, r"{{ (1024 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024 * 1024)|filesizeformat }}"), @"1267650.6 YB");
}

#[ignore = "zhong is refactoring vm https://github.com/dbt-labs/fs/issues/4808"]
#[test]
fn test_truncate() {
    use minijinja::render;
    use minijinja_contrib::filters::truncate;

    const LONG_TEXT: &str = "Lorem Ipsum is simply dummy text of the printing and typesetting industry. Lorem Ipsum has been the industry's standard dummy text ever since the 1500s, when an unknown printer took a galley of type and scrambled it to make a type specimen book. It has survived not only five centuries, but also the leap into electronic typesetting, remaining essentially unchanged. It was popularised in the 1960s with the release of Letraset sheets containing Lorem Ipsum passages, and more recently with desktop publishing software like Aldus PageMaker including versions of Lorem Ipsum.";
    const SHORT_TEXT: &str = "Fifteen chars !";
    const SPECIAL_TEXT: &str = "Hello 👋 World";

    let mut env = Environment::new();
    env.add_filter("truncate", truncate);

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate }}", text=>SHORT_TEXT),
        @"Fifteen chars !"
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate }}", text=>LONG_TEXT),
        @"Lorem Ipsum is simply dummy text of the printing and typesetting industry. Lorem Ipsum has been the industry's standard dummy text ever since the 1500s, when an unknown printer took a galley of type and scrambled it to make a type specimen book. It..."
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10) }}", text=>LONG_TEXT),
        @"Lorem..."
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10, killwords=true) }}", text=>LONG_TEXT),
        @"Lorem I..."
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10, end='***') }}", text=>LONG_TEXT),
        @"Lorem***"
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10, killwords=true, end='') }}", text=>LONG_TEXT),
        @"Lorem Ipsu"
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10, leeway=5) }}", text=>SHORT_TEXT),
        @"Fifteen chars !"
    );
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10, leeway=0) }}", text=>SHORT_TEXT),
        @"Fifteen..."
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=7, leeway=0, end='') }}", text=>SPECIAL_TEXT),
        @"Hello"
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=7, leeway=0, end='', killwords=true) }}", text=>SPECIAL_TEXT),
        @"Hello 👋"
    );

    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=8, leeway=0, end='') }}", text=>SPECIAL_TEXT),
        @"Hello 👋"
    );

    assert_eq!(
        env.render_str(r"{{ 'hello'|truncate(length=1) }}", context! {}, &[])
            .unwrap_err()
            .to_string(),
        "invalid operation: expected length >= 3, got 1\n(in <string>:1)"
    );
}

#[test]
fn test_truncate_positional_args() {
    use minijinja::render;
    use minijinja_contrib::filters::truncate;

    const LONG_TEXT: &str = "Lorem Ipsum is simply dummy text of the printing and typesetting industry. Lorem Ipsum has been the industry's standard dummy text ever since the 1500s, when an unknown printer took a galley of type and scrambled it to make a type specimen book. It has survived not only five centuries, but also the leap into electronic typesetting, remaining essentially unchanged. It was popularised in the 1960s with the release of Letraset sheets containing Lorem Ipsum passages, and more recently with desktop publishing software like Aldus PageMaker including versions of Lorem Ipsum.";
    const SHORT_TEXT: &str = "Fifteen chars !";

    let mut env = Environment::new();
    env.add_filter("truncate", truncate);

    // Positional argument: truncate(length)
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(10) }}", text=>LONG_TEXT),
        @"Lorem..."
    );

    // Positional arguments: truncate(length, killwords)
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(10, true) }}", text=>LONG_TEXT),
        @"Lorem I..."
    );

    // Positional arguments: truncate(length, killwords, end)
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(10, true, '') }}", text=>LONG_TEXT),
        @"Lorem Ipsu"
    );

    // Positional arguments: truncate(length, killwords, end, leeway)
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(10, false, '...', 0) }}", text=>SHORT_TEXT),
        @"Fifteen..."
    );

    // Mixed positional + keyword: truncate(length, end='')
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(64, end='') }}", text=>"this_is_a_long_column_name_that_exceeds_sixty_four_characters_and_should_be_truncated"),
        @"this_is_a_long_column_name_that_exceeds_sixty_four_characters_an"
    );

    // Keyword-only still works: truncate(length=10)
    insta::assert_snapshot!(
        render!(in env, r"{{ text|truncate(length=10) }}", text=>LONG_TEXT),
        @"Lorem..."
    );
}

#[test]
#[cfg(feature = "wordwrap")]
fn test_wordcount() {
    use minijinja_contrib::filters::wordcount;

    let mut env = Environment::new();
    env.add_filter("wordcount", wordcount);

    assert_eq!(
        env.render_str(
            "{{ text|wordcount }}",
            context! {
                text => "Hello world! How are you?"
            },
            &[]
        )
        .unwrap(),
        "5"
    );

    // Test empty string
    assert_eq!(
        env.render_str(
            "{{ text|wordcount }}",
            context! {
                text => ""
            },
            &[]
        )
        .unwrap(),
        "0"
    );

    // Test multiple whitespace
    assert_eq!(
        env.render_str(
            "{{ text|wordcount }}",
            context! {
                text => "Hello    world!   Test"
            },
            &[]
        )
        .unwrap(),
        "3"
    );

    // Test other word separators
    assert_eq!(
        env.render_str(
            "{{ text|wordcount }}",
            context! {
                text => "hello-again@world! It's_me!"
            },
            &[]
        )
        .unwrap(),
        "5"
    );

    // Test multiple other word separators
    assert_eq!(
        env.render_str(
            "{{ text|wordcount }}",
            context! {
                text => "hello--again@-!world"
            },
            &[]
        )
        .unwrap(),
        "3"
    );

    // Test unicode marks
    assert_eq!(
        env.render_str(
            "{{ text|wordcount }}",
            context! {
                text => "helloाworld"
            },
            &[]
        )
        .unwrap(),
        "2"
    );
}

#[test]
#[cfg(feature = "wordwrap")]
fn test_wordwrap() {
    use minijinja_contrib::filters::wordwrap;

    let mut env = minijinja::Environment::new();
    env.add_filter("wordwrap", wordwrap);

    // Test basic wrapping
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=20) }}",
            context! {
                text => "This is a long piece of text that should be wrapped at a specific width."
            },
            &[]
        )
        .unwrap(),
        "This is a long piece\nof text that should\nbe wrapped at a\nspecific width."
    );

    // Test custom wrap string
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=10, wrapstring=' <br> ') }}",
            context! {
                text => "This is a test of custom wrap strings."
            },
            &[]
        )
        .unwrap(),
        "This is <br> a test <br> of custom <br> wrap <br> strings."
    );

    // Test preserving newlines
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=20) }}",
            context! {
                text => "First paragraph.\n\nSecond paragraph."
            },
            &[]
        )
        .unwrap(),
        "First paragraph.\n\nSecond paragraph."
    );

    // Test breaking long words
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=10, break_long_words=true) }}",
            context! {
                text => "ThisIsAVeryLongWordThatShouldBeBroken"
            },
            &[]
        )
        .unwrap(),
        "ThisIsAVer\nyLongWordT\nhatShouldB\neBroken"
    );

    // Test not breaking long words
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=10, break_long_words=false) }}",
            context! {
                text => "ThisIsAVeryLongWordThatShouldBeBroken"
            },
            &[]
        )
        .unwrap(),
        "ThisIsAVeryLongWordThatShouldBeBroken"
    );

    // Test breaking on hyphens
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=10, break_on_hyphens=true) }}",
            context! {
                text => "This-is-a-hyphenated-word"
            },
            &[]
        )
        .unwrap(),
        "This-is-a-\nhyphenated\n-word"
    );
}

#[test]
#[cfg(feature = "wordwrap")]
fn test_wordwrap_positional_args() {
    use minijinja_contrib::filters::wordwrap;

    let mut env = minijinja::Environment::new();
    env.add_filter("wordwrap", wordwrap);

    // Positional argument: wordwrap(width)
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(20) }}",
            context! {
                text => "This is a long piece of text that should be wrapped at a specific width."
            },
            &[]
        )
        .unwrap(),
        "This is a long piece\nof text that should\nbe wrapped at a\nspecific width."
    );

    // Mixed positional + keyword: wordwrap(width, wrapstring='<br>')
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(10, wrapstring=' <br> ') }}",
            context! {
                text => "This is a test of custom wrap strings."
            },
            &[]
        )
        .unwrap(),
        "This is <br> a test <br> of custom <br> wrap <br> strings."
    );

    // Keyword-only still works: wordwrap(width=20)
    assert_eq!(
        env.render_str(
            "{{ text|wordwrap(width=20) }}",
            context! {
                text => "This is a long piece of text that should be wrapped at a specific width."
            },
            &[]
        )
        .unwrap(),
        "This is a long piece\nof text that should\nbe wrapped at a\nspecific width."
    );
}
