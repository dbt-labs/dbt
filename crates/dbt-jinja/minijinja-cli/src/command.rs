/// This module defines the command-line interface for the CLI.
/// It is separated into its own file because it is used both by the main
/// application and by build.rs to generate shell completions.
use std::path::PathBuf;

use clap::builder::ArgPredicate;
use clap::{arg, command, value_parser, ArgAction, Command};

const ADVANCED: &str = "Advanced";
const BEHAVIOR: &str = "Template Behavior";
const SECURITY: &str = "Security";

/// Supported formats
pub static SUPPORTED_FORMATS: &[(&str, &str, &[&str])] = &[
    #[cfg(feature = "cbor")]
    ("cbor", "CBOR", &["cbor"]),
    #[cfg(feature = "ini")]
    (
        "ini",
        "INI / Config",
        &["ini", "conf", "config", "properties"],
    ),
    #[cfg(not(feature = "json5"))]
    ("json", "JSON", &["json"]),
    #[cfg(feature = "json5")]
    ("json", "JSON / JSON5", &["json", "json5"]),
    #[cfg(feature = "querystring")]
    ("querystring", "Query String / Form Encoded", &["qs"]),
    #[cfg(feature = "toml")]
    ("toml", "TOML", &["toml"]),
    #[cfg(feature = "yaml")]
    ("yaml", "YAML 1.2", &["yaml", "yml"]),
];

fn format_formats(s: &str) -> String {
    use std::fmt::Write;
    let mut formats = String::new();

    for (fmt, title, exts) in SUPPORTED_FORMATS.iter() {
        write!(formats, "- {} ({}): ", fmt, title).ok();
        for (idx, ext) in exts.iter().enumerate() {
            if idx > 0 {
                formats.push_str(", ");
            }
            formats.push_str("*.");
            formats.push_str(ext);
        }
        formats.push('\n');
    }

    s.replace("###FORMATS###", &formats)
}

pub(super) fn make_command() -> Command {
    command!()
        .disable_help_flag(true)
        .max_term_width(120)
        .args([
            #[cfg(feature = "toml")]
            arg!(--"config-file" <PATH> "Alternative path to the config file.")
                .value_parser(value_parser!(PathBuf))
                .long_help("\
                    Sets an alternative path to the config file.  By default the config file \
                    is loaded from $HOME/.minijinja.toml.\n\n\
                    \
                    To see the possible config values use --print-config which will print the \
                    current state of the config.\n\n\
                    [env var: MINIJINJA_CONFIG_FILE]
                    "),
            arg!(-f --format <FORMAT> "The format of the input data")
                .long_help(format_formats("\
                    Sets the format of the input data.\n\n\
                    \
                    The following formats are supported (and the default detected file extensions):\n\n\
                    - auto\n\
                    ###FORMATS###\n\
                    Auto detection (auto) is unavailable when stdin is used as input format.\n\n\
                    \
                    For most formats the mapping is pretty straight forward as you expect.  The \
                    only format worth calling out is INI where the unnamed section is always \
                    called 'default' instead (in contrast to TOML which leaves it toplevel).\n\n\
                    \
                    [env var: MINIJINJA_FORMAT]"))
                .value_parser([
                    "auto",
                    #[cfg(feature = "cbor")]
                    "cbor",
                    #[cfg(feature = "ini")]
                    "ini",
                    "json",
                    #[cfg(feature = "querystring")]
                    "querystring",
                    #[cfg(feature = "toml")]
                    "toml",
                    #[cfg(feature = "yaml")]
                    "yaml",
                ]),
            arg!(-a --autoescape <MODE> "Reconfigures autoescape behavior")
                .long_help("\
                    Reconfigures autoescape behavior.  The default is 'auto' which means that \
                    the file extension sets the auto escaping mode.\n\n\
                    \
                    html means that variables are escaped to HTML5 and XML rules.  json means \
                    that output is safe for both JSON and YAML rules (eg: strings are formatted \
                    as JSON strings etc.).  none disables escaping entirely.\n\n\
                    \
                    [env var: MINIJINJA_AUTOESCAPE]")
                .value_parser(["auto", "html", "json", "none"])
                .help_heading(BEHAVIOR),
            arg!(-D --define <EXPR> "Defines an input variable (key=value / key:=json_value)")
                .long_help("\
                    This defines an input variable for the template.  This is used in addition \
                    to the input data file.  It supports three forms: key defines a single bool, \
                    key=value defines a string value, key:=json_value defines a JSON/YAML value.  \
                    The latter is useful to define strings, integers or simple array literals.  \
                    It can be supplied multiple times to set more than one value.\n\n\
                    \
                    Examples:\n\
                    -D name=Peter       defines a basic string\n\
                    -D user_id:=42      defines an integer\n\
                    -D is_active:=true  defines a boolean\n\
                    -D is_true          shortform to define true boolean")
                .action(ArgAction::Append),
            arg!(--strict "Disallow undefined variables in templates")
                .long_help("\
                    Disallow undefined variables in templates instead of rendering empty strings.\n\n\
                    \
                    By default a template will allow a singular undefined access.  This means that \
                    for instance an unknown attribute to an object will render an empty string.  To \
                    disable that you can use the strict mode in which case all undefined attributes \
                    will error instead.\n\n\
                    \
                    [env var: MINIJINJA_STRICT]")
                .help_heading(BEHAVIOR),
            arg!(--"no-include" "Disallow includes and extending")
                .long_help("\
                    Disallow includes and extending for security reasons.\n\n\
                    \
                    When this is enabled all inclusions and template extension features are disabled \
                    entirely.  An alternative to disabling includes is to use the --safe-path feature \
                    which allows white listing individual folders instead.\n\n\
                    \
                    [env var: MINIJINJA_INCLUDE]")
                .help_heading(SECURITY),
            arg!(--"safe-path" <PATH>... "Only allow includes from this path")
                .long_help("\
                    Only allow includes from this path.\n\n\
                    \
                    This can be used to better control where includes and layout extensions can load \
                    templates from.  This can be supplied multiple times.\n\n\
                    \
                    When the environment variable is used to control this, use ':' to split multiple \
                    paths on Unix and ';' on Windows (analog to the PATH environment variable).\n\n\
                    \
                    [env var: MINIJINJA_SAFE_PATH]
                    ")
                .conflicts_with("no-include")
                .value_parser(value_parser!(PathBuf))
                .help_heading(SECURITY),
            arg!(--fuel <AMOUNT> "Configures the maximum fuel")
                .long_help("\
                    Sets the maximum fuel a template can consume.\n\n\
                    \
                    When fuel is set, every instruction consumes a certain amount of fuel. Usually 1, \
                    some will consume no fuel. By default the engine has the fuel feature disabled (0). \
                    To turn on fuel set something like 50000 which will allow 50.000 instructions to \
                    execute before running out of fuel.\n\n\
                    \
                    This is useful as a basic security feature in CI pipelines or similar.\n\n\
                    \
                    [env var: MINIJINJA_FUEL]")
                .value_parser(value_parser!(u64))
                .help_heading(SECURITY),
            arg!(-n --"no-newline" "Do not output a trailing newline")
                .long_help("\
                    Do not output a trailing newline after template evaluation.\n\n\
                    \
                    By default minijinja-cli will render a trailing newline when rendering.  This \
                    flag can be used to disable that.\n\n\
                    \
                    [env var: MINIJINJA_NEWLINE]")
                .help_heading(BEHAVIOR),
            arg!(--"trim-blocks" "Enable the trim-blocks flag")
                .long_help("\
                    Enable the trim-blocks flag.\n\n\
                    \
                    This flag controls the trim-blocks template syntax feature.  When enabled trailing \
                    whitespace including one newline is removed after a block tag.\n\n\
                    \
                    [env var: MINIJINJA_TRIM_BLOCKS]")
                .help_heading(BEHAVIOR),
            arg!(--"lstrip-blocks" "Enable the lstrip-blocks flag")
                .long_help("\
                    Enable the lstrip-blocks flag.\n\n\
                    \
                    This flag controls the lstrip-blocks template syntax feature.  When enabled leading \
                    whitespace is removed before a block tag.\n\n\
                    \
                    [env var: MINIJINJA_LSTRIP_BLOCKS]")
                .help_heading(BEHAVIOR),
            #[cfg(feature = "contrib")]
            arg!(--"py-compat" "Enables improved Python compatibility")
                .long_help("\
                    Enables improved Python compatibility for templates.\n\n\
                    \
                    Enabling this adds methods such as dict.keys and some common others.  This is useful \
                    when rendering templates that should be shared with Jinja2.\n\n\
                    \
                    [env var: MINIJINJA_PY_COMPAT]")
                .help_heading(BEHAVIOR),
            arg!(-s --syntax <PAIR>... "Changes a syntax feature (feature=value) \
                [possible features: block-start, block-end, variable-start, variable-end, \
                comment-start, comment-end, line-statement-prefix, \
                line-statement-comment]")
                .long_help("\
                    Changes a syntax feature.\n\n\
                    \
                    This allows reconfiguring syntax delimiters.  The flag can be provided multiple \
                    times.  Each time it's feature=value where feature is the name of the syntax \
                    delimiter to change.  The following list is the full list of syntax features \
                    that can be reconfigured and the default value:\n\n\
                    \
                    block-start={%\n\
                    block-end=%}\n\
                    variable-start={{\n\
                    variable-end=}}\n\
                    comment-start={#\n\
                    comment-end=%}\n\
                    line-statement-prefix=\n\
                    line-statement-comment=\n\n\
                    \
                    Example: minijinja-cli -svariable-start='${' -svariable-end='}'\n\n\
                    \
                    For environment variable usage split multiple config strings with whitespace.\n\n\
                    \
                    [env var: MINIJINJA_SYNTAX]")
                .help_heading(BEHAVIOR),
            arg!(--env "Pass environment variables as ENV to the template")
                .long_help("\
                    Pass environment variables to the template and make them available under the ENV \
                    variable within the template.\n\n\
                    \
                    [env var: MINIJINJA_ENV]")
                .help_heading(BEHAVIOR),
            arg!(-t --template <TEMPLATE_STRING> "Render a string template")
                .long_help("\
                    Renders a template from a string instead of the file given.\n\n\
                    \
                    This can be used as an alternative to the template file that is normally passed. \
                    Note that this is different to --expr which evaluates expressions instead.\n\n\
                    \
                    Example: minijinja-cli --template='Hello {{ name }}' -Dname=World"),
            arg!(-E --expr <EXPR> "Evaluates an template expression")
                .long_help("\
                    Evalues a template expression instead of rendering a template.\n\n\
                    \
                    The value to the parameter is a template expression that is evaluated with the \
                    context of the template and the result is emitted according to --expr-out.  The \
                    default output mode is to print the result of the expression to stdout.\n\n\
                    \
                    Example: minijinja-cli --expr='1 < 10'")
                .help_heading(ADVANCED),
            arg!(--"expr-out" <MODE> "The expression output mode")
                .long_help("\
                    Sets the expression output mode for --expr.\n\n\
                    \
                    This defaults to 'print' which means that the expression's result is written to \
                    stdout.  'json' (and 'json-pretty') does mostly the same but writes the result as \
                    JSON result instead with one as a one-liner, the second in prett printing. 'status' \
                    exits the program with the result as a status code.  If the result is not a number it \
                    will first convert the result into a bool and then exits as 0 if it was true, 1 \
                    otherwise.\n\n\
                    \
                    [env var: MINIJINJA_EXPR_OUT]")
                .value_parser(["print", "json", "json-pretty", "status"])
                .requires("expr")
                .help_heading(ADVANCED),
            arg!(--dump <KIND> "Dump internals of a template")
                .long_help("\
                    Dump internals of a template to stdout.\n\n\
                    \
                    This feature is primarily useful to debug what is going on in a MiniJinja template. \
                    'instructions' will dump out the bytecode that the engine generated, 'ast' dumps out \
                    the AST in a text only format and 'tokens' will print a line per token of the template \
                    after lexing.")
                .value_parser(["instructions", "ast", "tokens"])
                .help_heading(ADVANCED),
            #[cfg(feature = "repl")]
            arg!(--repl "Starts the repl with the given data")
                .long_help("\
                    Starts the read-eval loop with the given input data.\n\n\
                    \
                    This allows basic experimentation of MiniJinja expressions with some input data.")
                .conflicts_with_all(["expr", "template", "template_file"])
                .help_heading(ADVANCED),
            arg!(-o --output <FILENAME> "Path to the output file")
                .long_help("\
                    Path to the output file instead of stdout.\n\n\
                    \
                    By default templates will be rendered to stdout, but this can be used to directly write \
                    into a target file instead.  The --no-newline flag can be used to disable the printing \
                    of the trailing newline.  Files will be written atomically.  This means that if template \
                    evaluation fails the original file remains.")
                .default_value("-")
                .value_parser(value_parser!(PathBuf)),
            arg!(--select <SELECTOR> "Select a subset of the input data")
                .long_help("\
                    Select a subset of the input data with a path expression.\n\n\
                    \
                    By default the input file is fed directly as context.  You can however also select a \
                    sub-section of this file.  For instance if you have a TOML file where all variables \
                    are placed in the values section you normally need to reference the values like so:\n\n\
                    \
                    {{ values.key }}\n\n\
                    \
                    If you however invoke minijinja-cli with --select=values you can directly reference \
                    the keys:\n\n\
                    \
                    {{ key }}\n\n\
                    \
                    You can use dotted paths to select into sub sections (eg: --select=values.0.box)."),
            arg!(--"print-config" "Print out the loaded config"),
            arg!(-h --help "Print short help (short texts)")
                .action(ArgAction::HelpShort),
            arg!(--"long-help" "Print long help (extended, long explanation texts)")
                .action(ArgAction::HelpLong),
            arg!(--"syntax-help" "Print syntax help (primer on Jinja2/MiniJinja syntax)")
                .action(ArgAction::SetTrue),
            arg!(template_file: [TEMPLATE_FILE] "Path to the input template")
                .long_help("\
                    This is the path to the input template in MiniJinja/Jinja2 syntax.  \
                    If not provided this defaults to '-' which means the template is \
                    loaded from stdin.  When the format is set to 'auto' which is the \
                    default, the extension of the filename is used to detect the format.\n\n\
                    \
                    This argument can be set to an empty string when --template is provided \
                    to allow a data file to be supplied.")
                .default_value("-")
                .default_value_if("template", ArgPredicate::IsPresent, None),
            arg!(data_file: [DATA_FILE] "Path to the data file")
                .long_help("\
                    Path to the data file in the given format.\n\n\
                    \
                    The data file is used to supply the context (variables) to the template. \
                    Various file formats are supported.  When data is read from stdin (by using '-' \
                    as file name), --format must be specified as auto detection is based on \
                    file extensions.")
                .value_parser(value_parser!(PathBuf)),
            #[cfg(feature = "completions")]
            arg!(--"generate-completion" <SH> "Generate a completion script for the given shell")
                .long_help("\
                    Generate a completion script for the given shell and print it to stdout.\n\n\
                    \
                    This completion script can be added to your shell startup to provide completions \
                    for the minijinja-cli command.")
                .value_parser([
                    "bash",
                    "elvish",
                    "fig",
                    "fish",
                    "nushell",
                    "powershell",
                    "zsh",
                ]).help_heading("Shell Support"),
        ])
        .before_help("minijinja-cli is a command line tool to render or evaluate jinja2 templates.")
        .after_help("For a short help use --help, for extended help --long-help, and for help on syntax --syntax-help.")
        .about("Pass a template and optionally a file with template variables to render it to stdout.")
        .long_about(include_str!("long_help.txt"))
}
