use std::io::Write;
use std::process::Command;

use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
use tempfile::NamedTempFile;

fn cli() -> Command {
    Command::new(get_cargo_bin("minijinja-cli"))
}

fn file_with_contents(contents: &str) -> NamedTempFile {
    file_with_contents_and_ext(contents, "")
}

fn file_with_contents_and_ext<X: AsRef<[u8]>>(contents: X, ext: &str) -> NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix("minijinja-testfile--")
        .suffix(ext)
        .tempfile()
        .unwrap();
    f.write_all(contents.as_ref()).unwrap();
    f
}

macro_rules! bind_common_filters {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.add_filter(r"(?m)(non-existing template).*$", "$1 [FILENAME HERE]");
        settings.add_filter(r"(?mis)(^Referenced variables: \{).*\z", "$1 ... }");
        settings.add_filter(
            r"(?m)^-+ (minijinja-testfile--\S+) -+$",
            "--- [TEMPLATE] ---",
        );
        settings.add_filter(
            r"\(in .*minijinja-testfile--.*?:(\d+)\)",
            "(in [TEMPLATE]:$1)",
        );
        settings.add_filter(r"\bminijinja-cli\.exe\b", "minijinja-cli");
        let _guard = settings.bind_to_scope();
    };
}

#[test]
fn test_explicit_format() {
    let input = file_with_contents(r#"{"foo": "bar"}"#);
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg("--format=json")
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
fn test_no_newline() {
    let input = file_with_contents(r#"{"foo": "bar"}"#);
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg("--format=json")
            .arg("--no-newline")
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!
    ----- stderr -----
    "###);
}

#[test]
fn test_json() {
    let input = file_with_contents_and_ext(r#"{"foo": "bar"}"#, ".json");
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "json5")]
fn test_json5() {
    let input = file_with_contents_and_ext(r#"/* foo */{"foo": "bar"}"#, ".json");
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "yaml")]
fn test_yaml() {
    let input = file_with_contents_and_ext(r#"foo: bar"#, ".yaml");
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "yaml")]
fn test_yaml_aliases() {
    let input = file_with_contents_and_ext(
        r#"
a: &a
  key1: value1

b: &b
  key2: value2

c:
  <<: *a
  key2: from-c

d:
  <<: [*a, *b]
  key3: value3
"#,
        ".yaml",
    );
    let tmpl = file_with_contents(r#"{{ [c.key1, c.key2] }}\n{{ [d.key1, d.key2, d.key3] }}"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    ["value1", "from-c"]\n["value1", "value2", "value3"]

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "toml")]
fn test_toml() {
    let input = file_with_contents_and_ext("[section]\nfoo = \"bar\"", ".toml");
    let tmpl = file_with_contents(r#"Hello {{ section.foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);

    let mut tmpl = NamedTempFile::new().unwrap();
    tmpl.write_all(br#"Hello {{ foo }}!"#).unwrap();

    assert_cmd_snapshot!(
        cli()
            .arg("--select=section")
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "ini")]
fn test_ini_casing() {
    let input = file_with_contents_and_ext("[section]\nFOO = bar", ".ini");
    let tmpl = file_with_contents(r#"Hello {{ section.FOO }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);

    let mut tmpl = NamedTempFile::new().unwrap();
    tmpl.write_all(br#"Hello {{ FOO }}!"#).unwrap();

    assert_cmd_snapshot!(
        cli()
            .arg("--select=section")
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "querystring")]
fn test_querystring() {
    let input = file_with_contents_and_ext("foo=blub+blah%2fx", ".qs");
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello blub blah/x!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "cbor")]
fn test_cbor() {
    let input = file_with_contents_and_ext(
        [0xa1, 0x63, 0x66, 0x6f, 0x6f, 0x63, 0x62, 0x61, 0x72],
        ".cbor",
    );
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "ini")]
fn test_ini() {
    let input = file_with_contents_and_ext("[section]\nfoo = bar", ".ini");
    let tmpl = file_with_contents(r#"Hello {{ section.foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);

    let input = file_with_contents_and_ext("foo = bar", ".ini");
    let mut tmpl = NamedTempFile::new().unwrap();
    tmpl.write_all(br#"Hello {{ foo }}!"#).unwrap();

    assert_cmd_snapshot!(
        cli()
            .arg("--select=default")
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(feature = "preserve_order")]
fn test_preserve_order_json() {
    let input = file_with_contents_and_ext(r#"{"x": {"c": 3, "a": 1, "b": 2}}"#, ".json");
    let tmpl =
        file_with_contents("{% for key, value in x|items %}{{ key }}: {{ value }}\n{% endfor %}");

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    c: 3
    a: 1
    b: 2


    ----- stderr -----
    "###);
}

#[test]
#[cfg(all(feature = "preserve_order", feature = "yaml"))]
fn test_preserve_order_yaml() {
    let input = file_with_contents_and_ext(
        r#"
x:
  c: 3
  a: 1
  b: 2
"#,
        ".yaml",
    );
    let tmpl =
        file_with_contents("{% for key, value in x|items %}{{ key }}: {{ value }}\n{% endfor %}");

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    c: 3
    a: 1
    b: 2


    ----- stderr -----
    "###);
}

#[test]
#[cfg(all(feature = "preserve_order", feature = "toml"))]
fn test_preserve_order_toml() {
    let input = file_with_contents_and_ext(
        r#"
[x]
c = 3
a = 1
b = 2
"#,
        ".toml",
    );
    let tmpl =
        file_with_contents("{% for key, value in x|items %}{{ key }}: {{ value }}\n{% endfor %}");

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    c: 3
    a: 1
    b: 2


    ----- stderr -----
    "###);
}

#[test]
#[cfg(all(feature = "preserve_order", feature = "cbor"))]
fn test_preserve_order_cbor() {
    let input = file_with_contents_and_ext(
        [
            0xa1, // map(1)
            0x61, 0x78, // "x"
            0xa3, // map(3)
            0x61, 0x63, 0x03, // "c": 3
            0x61, 0x61, 0x01, // "a": 1
            0x61, 0x62, 0x02, // "b": 2
        ],
        ".cbor",
    );
    let tmpl =
        file_with_contents("{% for key, value in x|items %}{{ key }}: {{ value }}\n{% endfor %}");

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    c: 3
    a: 1
    b: 2


    ----- stderr -----
    "###);
}

#[test]
fn test_context_stdin() {
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg("-")
            .arg("--format=json")
            .pass_stdin(r#"{"foo": "bar"}"#),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg("-")
            .pass_stdin(r#""#),
        @r###"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: auto detection does not work with data from stdin
    "###);
}

#[test]
fn test_dump() {
    let tmpl = file_with_contents(r#"Hello {{ foo }}!"#);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg("--dump=tokens"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    TemplateData("Hello ")
    VariableStart
    Ident("foo")
    VariableEnd
    TemplateData("!")

    ----- stderr -----
    "###);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg("--dump=ast"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Template {
        children: [
            EmitRaw {
                raw: "Hello ",
            } @ 1:0-1:6,
            EmitExpr {
                expr: Var {
                    id: "foo",
                } @ 1:9-1:12,
            } @ 1:6-1:12,
            EmitRaw {
                raw: "!",
            } @ 1:15-1:16,
        ],
    } @ 0:0-1:16

    ----- stderr -----
    "###);

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg("--dump=instructions"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Block: "<root>"
         0: EmitRaw("Hello ")
         1: Lookup("foo")
         2: Emit
         3: EmitRaw("!")

    ----- stderr -----
    "###);
}

#[test]
fn test_include() {
    let tmpl = file_with_contents(r#"{% include ENV.OTHER_TEMPLATE %}"#);
    let other_tmpl = file_with_contents(r#"Hello!"#);
    let input = file_with_contents_and_ext(r#"{}"#, ".json");

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path())
            .arg("--env")
            .env("OTHER_TEMPLATE", other_tmpl.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello!

    ----- stderr -----
    "###);
}

#[test]
fn test_no_include() {
    let tmpl = file_with_contents(r#"{% include ENV.OTHER_TEMPLATE %}"#);
    let other_tmpl = file_with_contents(r#"Hello!"#);
    let input = file_with_contents_and_ext(r#"{}"#, ".json");

    bind_common_filters!();

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path())
            .arg("--env")
            .env("OTHER_TEMPLATE", other_tmpl.path())
            .arg("--no-include"),
        @r###"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: template not found: tried to include non-existing template [FILENAME HERE]

    --- [TEMPLATE] ---
       1 > {% include ENV.OTHER_TEMPLATE %}
         i    ^^^^^^^^^^^^^^^^^^^^^^^^^^ template not found
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    Referenced variables: { ... }
    "###);
}

#[test]
fn test_syntax_error() {
    let tmpl = file_with_contents("{{ all_good }}\n{% for item in seq");
    let input = file_with_contents_and_ext(r#"{}"#, ".json");

    bind_common_filters!();

    assert_cmd_snapshot!(
        cli()
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: syntax error: unexpected end of input, expected end of block (in [TEMPLATE]:2)

    --- [TEMPLATE] ---
       1 | {{ all_good }}
       2 > {% for item in seq
         i                ^^^ syntax error
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    No referenced variables
    -------------------------------------------------------------------------------
    "###);
}

#[test]
fn test_stdin_template() {
    let input = file_with_contents_and_ext(r#"{"foo": "bar"}"#, ".json");

    assert_cmd_snapshot!(
        cli()
            .arg("-")
            .arg(input.path())
            .pass_stdin("Hello {{ foo }}!"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello bar!

    ----- stderr -----
    "###);
}

#[test]
fn test_line_statement() {
    let tmpl = file_with_contents("# for item in seq\n  {{ item }}\n# endfor");
    let input = file_with_contents_and_ext(r#"{"seq": [1, 2, 3]}"#, ".json");

    assert_cmd_snapshot!(
        cli()
            .arg("-sline-statement-prefix=#")
            .arg("--no-newline")
            .arg(tmpl.path())
            .arg(input.path()),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
      1
      2
      3

    ----- stderr -----
    "###);
}

#[test]
#[allow(clippy::suspicious_command_arg_space)]
fn test_template_string() {
    assert_cmd_snapshot!(
        cli()
            .arg("-tHello {{ name }}")
            .arg("-Dname=Peter")
            .arg("--no-newline"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello Peter
    ----- stderr -----
    "###);
}

#[test]
#[allow(clippy::suspicious_command_arg_space)]
fn test_empty_template_name_with_string_template() {
    let input = file_with_contents_and_ext(r#"{"name": "Peter"}"#, ".json");
    assert_cmd_snapshot!(
        cli()
            .arg("-tHello {{ name }}")
            .arg("")
            .arg(input.path())
            .arg("--no-newline"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello Peter
    ----- stderr -----
    "###);
}

#[test]
#[allow(clippy::suspicious_command_arg_space)]
fn test_template_name_with_string_template_fails() {
    let input = file_with_contents_and_ext(r#"{"name": "Peter"}"#, ".json");
    assert_cmd_snapshot!(
        cli()
            .arg("-tHello {{ name }}")
            .arg("invalid.tmpl")
            .arg(input.path())
            .arg("--no-newline"),
        @r###"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: When --template is used, a template cannot be passed as argument (only an empty argument is allowed).
    "###);
}

#[test]
fn test_empty_template_name_errors() {
    let input = file_with_contents_and_ext(r#"{"name": "Peter"}"#, ".json");
    assert_cmd_snapshot!(
        cli()
            .arg("")
            .arg(input.path())
            .arg("--no-newline"),
        @r###"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: Empty template names are only valid with --template.
    "###);
}

#[test]
fn test_print_config_fully_loaded() {
    assert_cmd_snapshot!(
        cli()
            .arg("--strict")
            .arg("--trim-blocks")
            .arg("-Dvar1=value1")
            .arg("-Dvar2=value2")
            .arg("-Dvar3:=42")
            .arg("-Dvar4:=true")
            .arg("-Dvar5:=[1,2,true]")
            .arg("--print-config"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    format = "auto"
    autoescape = "auto"
    include = true
    newline = true
    trim-blocks = true
    lstrip-blocks = false
    py-compat = false
    env = false
    strict = true
    safe-paths = []
    expr-out = "print"
    fuel = 0

    [syntax]
    block-start = "{%"
    block-end = "%}"
    variable-start = "{{"
    variable-end = "}}"
    comment-start = "{#"
    comment-end = "#}"
    line-statement-prefix = ""
    line-comment-prefix = ""

    [defines]
    var1 = "value1"
    var2 = "value2"
    var3 = 42
    var4 = true
    var5 = [
        1,
        2,
        true,
    ]


    ----- stderr -----
    "###);
}

#[test]
fn test_load_config() {
    let config = file_with_contents_and_ext(
        r#"
    [defines]
    greeting = "Hello"
    punctuation = "!"
    "#,
        ".toml",
    );

    let input = file_with_contents_and_ext(r#"{"name": "World"}"#, ".json");

    assert_cmd_snapshot!(
        cli()
            .arg("--config-file")
            .arg(config.path())
            .arg("-")
            .arg(input.path())
            .pass_stdin("{{ greeting }} {{ name }}{{ punctuation }}"),
        @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World!

    ----- stderr -----
    "###);
}

#[test]
#[cfg(all(
    feature = "cbor",
    feature = "ini",
    feature = "json5",
    feature = "querystring",
    feature = "toml",
    feature = "yaml",
))]
fn test_help() {
    bind_common_filters!();
    assert_cmd_snapshot!("short_help", cli().arg("--help"));
    assert_cmd_snapshot!("long_help", cli().arg("--long-help"));
}
