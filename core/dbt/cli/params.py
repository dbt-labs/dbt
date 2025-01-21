from pathlib import Path

import click

from dbt.cli.option_types import YAML, ChoiceTuple, Package, WarnErrorOptionsType
from dbt.cli.options import MultiOption
from dbt.cli.resolvers import default_profiles_dir, default_project_dir
from dbt.version import get_version_information

add_package = click.option(
    "--add-package",
    help="Add a package to current package spec, specify it as package-name@version. Change the source with --source flag.",
    envvar=None,
    type=Package(),
)
args = click.option(
    "--args",
    envvar=None,
    help="Supply arguments to the macro. This dictionary will be mapped to the keyword arguments defined in the selected macro. This argument should be a YAML string, eg. '{my_variable: my_value}'",
    type=YAML(),
)

browser = click.option(
    "--browser/--no-browser",
    envvar=None,
    help="Wether or not to open a local web browser after starting the server",
    default=True,
)

cache_selected_only = click.option(
    "--cache-selected-only/--no-cache-selected-only",
    envvar="DBT_CACHE_SELECTED_ONLY",
    help="At start of run, populate relational cache only for schemas containing selected nodes, or for all schemas of interest.",
)

introspect = click.option(
    "--introspect/--no-introspect",
    envvar="DBT_INTROSPECT",
    help="Whether to scaffold introspective queries as part of compilation",
    default=True,
)

compile_docs = click.option(
    "--compile/--no-compile",
    envvar=None,
    help="Whether or not to run 'dbt compile' as part of docs generation",
    default=True,
)

compile_inject_ephemeral_ctes = click.option(
    "--inject-ephemeral-ctes/--no-inject-ephemeral-ctes",
    envvar=None,
    help="Internal flag controlling injection of referenced ephemeral models' CTEs during `compile`.",
    hidden=True,
    default=True,
)

config_dir = click.option(
    "--config-dir",
    envvar=None,
    help="Print a system-specific command to access the directory that the current dbt project is searching for a profiles.yml. Then, exit. This flag renders other debug step flags no-ops.",
    is_flag=True,
)

debug = click.option(
    "--debug/--no-debug",
    "-d/ ",
    envvar="DBT_DEBUG",
    help="Display debug logging during dbt execution. Useful for debugging and making bug reports.",
)

# flag was previously named DEFER_MODE
defer = click.option(
    "--defer/--no-defer",
    envvar="DBT_DEFER",
    help="If set, resolve unselected nodes by deferring to the manifest within the --state directory.",
)

deprecated_defer = click.option(
    "--deprecated-defer",
    envvar="DBT_DEFER_TO_STATE",
    help="Internal flag for deprecating old env var.",
    default=False,
    hidden=True,
)

empty = click.option(
    "--empty/--no-empty",
    envvar="DBT_EMPTY",
    help="If specified, limit input refs and sources to zero rows.",
    is_flag=True,
)

event_time_end = click.option(
    "--event-time-end",
    envvar="DBT_EVENT_TIME_END",
    help="If specified, the end datetime dbt uses to filter microbatch model inputs (exclusive).",
    type=click.DateTime(),
    default=None,
)

event_time_start = click.option(
    "--event-time-start",
    envvar="DBT_EVENT_TIME_START",
    help="If specified, the start datetime dbt uses to filter microbatch model inputs (inclusive).",
    type=click.DateTime(),
    default=None,
)

exclude = click.option(
    "--exclude",
    envvar=None,
    type=tuple,
    cls=MultiOption,
    multiple=True,
    help="Specify the nodes to exclude.",
)

export_saved_queries = click.option(
    "--export-saved-queries/--no-export-saved-queries",
    envvar="DBT_EXPORT_SAVED_QUERIES",
    help="Export saved queries within the 'build' command, otherwise no-op",
    is_flag=True,
    hidden=True,
)

fail_fast = click.option(
    "--fail-fast/--no-fail-fast",
    "-x/ ",
    envvar="DBT_FAIL_FAST",
    help="Stop execution on first failure.",
)

favor_state = click.option(
    "--favor-state/--no-favor-state",
    envvar="DBT_FAVOR_STATE",
    help="If set, defer to the argument provided to the state flag for resolving unselected nodes, even if the node(s) exist as a database object in the current environment.",
)

deprecated_favor_state = click.option(
    "--deprecated-favor-state",
    envvar="DBT_FAVOR_STATE_MODE",
    help="Internal flag for deprecating old env var.",
)

full_refresh = click.option(
    "--full-refresh",
    "-f",
    envvar="DBT_FULL_REFRESH",
    help="If specified, dbt will drop incremental models and fully-recalculate the incremental table from the model definition.",
    is_flag=True,
)

host = click.option(
    "--host",
    envvar="DBT_HOST",
    help="host to serve dbt docs on",
    type=click.STRING,
    default="127.0.0.1",
)

indirect_selection = click.option(
    "--indirect-selection",
    envvar="DBT_INDIRECT_SELECTION",
    help="Choose which tests to select that are adjacent to selected resources. Eager is most inclusive, cautious is most exclusive, and buildable is in between. Empty includes no tests at all.",
    type=click.Choice(["eager", "cautious", "buildable", "empty"], case_sensitive=False),
    default="eager",
)

lock = click.option(
    "--lock",
    envvar=None,
    help="Generate the package-lock.yml file without install the packages.",
    is_flag=True,
)

log_cache_events = click.option(
    "--log-cache-events/--no-log-cache-events",
    help="Enable verbose logging for relational cache events to help when debugging.",
    envvar="DBT_LOG_CACHE_EVENTS",
)

log_format = click.option(
    "--log-format",
    envvar="DBT_LOG_FORMAT",
    help="Specify the format of logging to the console and the log file. Use --log-format-file to configure the format for the log file differently than the console.",
    type=click.Choice(["text", "debug", "json", "default"], case_sensitive=False),
    default="default",
)

log_format_file = click.option(
    "--log-format-file",
    envvar="DBT_LOG_FORMAT_FILE",
    help="Specify the format of logging to the log file by overriding the default value and the general --log-format setting.",
    type=click.Choice(["text", "debug", "json", "default"], case_sensitive=False),
    default="debug",
)

log_level = click.option(
    "--log-level",
    envvar="DBT_LOG_LEVEL",
    help="Specify the minimum severity of events that are logged to the console and the log file. Use --log-level-file to configure the severity for the log file differently than the console.",
    type=click.Choice(["debug", "info", "warn", "error", "none"], case_sensitive=False),
    default="info",
)

log_level_file = click.option(
    "--log-level-file",
    envvar="DBT_LOG_LEVEL_FILE",
    help="Specify the minimum severity of events that are logged to the log file by overriding the default value and the general --log-level setting.",
    type=click.Choice(["debug", "info", "warn", "error", "none"], case_sensitive=False),
    default="debug",
)

use_colors = click.option(
    "--use-colors/--no-use-colors",
    envvar="DBT_USE_COLORS",
    help="Specify whether log output is colorized in the console and the log file. Use --use-colors-file/--no-use-colors-file to colorize the log file differently than the console.",
    default=True,
)

use_colors_file = click.option(
    "--use-colors-file/--no-use-colors-file",
    envvar="DBT_USE_COLORS_FILE",
    help="Specify whether log file output is colorized by overriding the default value and the general --use-colors/--no-use-colors setting.",
    default=True,
)

log_file_max_bytes = click.option(
    "--log-file-max-bytes",
    envvar="DBT_LOG_FILE_MAX_BYTES",
    help="Configure the max file size in bytes for a single dbt.log file, before rolling over. 0 means no limit.",
    default=10 * 1024 * 1024,  # 10mb
    type=click.INT,
    hidden=True,
)

log_path = click.option(
    "--log-path",
    envvar="DBT_LOG_PATH",
    help="Configure the 'log-path'. Only applies this setting for the current run. Overrides the 'DBT_LOG_PATH' if it is set.",
    default=None,
    type=click.Path(resolve_path=True, path_type=Path),
)

macro_debugging = click.option(
    "--macro-debugging/--no-macro-debugging",
    envvar="DBT_MACRO_DEBUGGING",
    hidden=True,
)

# This less standard usage of --output where output_path below is more standard
output = click.option(
    "--output",
    envvar=None,
    help="Specify the output format: either JSON or a newline-delimited list of selectors, paths, or names",
    type=click.Choice(["json", "name", "path", "selector"], case_sensitive=False),
    default="selector",
)

show_output_format = click.option(
    "--output",
    envvar=None,
    help="Output format for dbt compile and dbt show",
    type=click.Choice(["json", "text"], case_sensitive=False),
    default="text",
)

show_limit = click.option(
    "--limit",
    envvar=None,
    help="Limit the number of results returned by dbt show",
    type=click.INT,
    default=5,
)

output_keys = click.option(
    "--output-keys",
    envvar=None,
    help=(
        "Space-delimited listing of node properties to include as custom keys for JSON output "
        "(e.g. `--output json --output-keys name resource_type description`)"
    ),
    type=tuple,
    cls=MultiOption,
    multiple=True,
    default=[],
)

output_path = click.option(
    "--output",
    "-o",
    envvar=None,
    help="Specify the output path for the JSON report. By default, outputs to 'target/sources.json'",
    type=click.Path(file_okay=True, dir_okay=False, writable=True),
    default=None,
)

partial_parse = click.option(
    "--partial-parse/--no-partial-parse",
    envvar="DBT_PARTIAL_PARSE",
    help="Allow for partial parsing by looking for and writing to a pickle file in the target directory. This overrides the user configuration file.",
    default=True,
)

partial_parse_file_path = click.option(
    "--partial-parse-file-path",
    envvar="DBT_PARTIAL_PARSE_FILE_PATH",
    help="Internal flag for path to partial_parse.manifest file.",
    default=None,
    hidden=True,
    type=click.Path(exists=True, dir_okay=False, resolve_path=True),
)

partial_parse_file_diff = click.option(
    "--partial-parse-file-diff/--no-partial-parse-file-diff",
    envvar="DBT_PARTIAL_PARSE_FILE_DIFF",
    help="Internal flag for whether to compute a file diff during partial parsing.",
    hidden=True,
    default=True,
)

populate_cache = click.option(
    "--populate-cache/--no-populate-cache",
    envvar="DBT_POPULATE_CACHE",
    help="At start of run, use `show` or `information_schema` queries to populate a relational cache, which can speed up subsequent materializations.",
    default=True,
)

port = click.option(
    "--port",
    envvar=None,
    help="Specify the port number for the docs server",
    default=8080,
    type=click.INT,
)

print = click.option(
    "--print/--no-print",
    envvar="DBT_PRINT",
    help="Output all {{ print() }} macro calls.",
    default=True,
)

deprecated_print = click.option(
    "--deprecated-print/--deprecated-no-print",
    envvar="DBT_NO_PRINT",
    help="Internal flag for deprecating old env var.",
    default=True,
    hidden=True,
    callback=lambda ctx, param, value: not value,
)

printer_width = click.option(
    "--printer-width",
    envvar="DBT_PRINTER_WIDTH",
    help="Sets the width of terminal output",
    type=click.INT,
    default=80,
)

profile = click.option(
    "--profile",
    envvar="DBT_PROFILE",
    help="Which existing profile to load. Overrides setting in dbt_project.yml.",
)

profiles_dir = click.option(
    "--profiles-dir",
    envvar="DBT_PROFILES_DIR",
    help="Which directory to look in for the profiles.yml file. If not set, dbt will look in the current working directory first, then HOME/.dbt/",
    default=default_profiles_dir,
    type=click.Path(exists=True),
)

# `dbt debug` uses this because it implements custom behaviour for non-existent profiles.yml directories
# `dbt deps` does not load a profile at all
# `dbt init` will write profiles.yml if it doesn't yet exist
profiles_dir_exists_false = click.option(
    "--profiles-dir",
    envvar="DBT_PROFILES_DIR",
    help="Which directory to look in for the profiles.yml file. If not set, dbt will look in the current working directory first, then HOME/.dbt/",
    default=default_profiles_dir,
    type=click.Path(exists=False),
)

project_dir = click.option(
    "--project-dir",
    envvar="DBT_PROJECT_DIR",
    help="Which directory to look in for the dbt_project.yml file. Default is the current working directory and its parents.",
    default=default_project_dir,
    type=click.Path(exists=True),
)

quiet = click.option(
    "--quiet/--no-quiet",
    "-q",
    envvar="DBT_QUIET",
    help="Suppress all non-error logging to stdout. Does not affect {{ print() }} macro calls.",
)

record_timing_info = click.option(
    "--record-timing-info",
    "-r",
    envvar=None,
    help="When this option is passed, dbt will output low-level timing stats to the specified file. Example: `--record-timing-info output.profile`",
    type=click.Path(exists=False),
)

resource_type = click.option(
    "--resource-types",
    "--resource-type",
    envvar="DBT_RESOURCE_TYPES",
    help="Restricts the types of resources that dbt will include",
    type=ChoiceTuple(
        [
            "metric",
            "semantic_model",
            "saved_query",
            "source",
            "analysis",
            "model",
            "test",
            "unit_test",
            "exposure",
            "snapshot",
            "seed",
            "default",
            "all",
        ],
        case_sensitive=False,
    ),
    cls=MultiOption,
    multiple=True,
    default=(),
)

exclude_resource_type = click.option(
    "--exclude-resource-types",
    "--exclude-resource-type",
    envvar="DBT_EXCLUDE_RESOURCE_TYPES",
    help="Specify the types of resources that dbt will exclude",
    type=ChoiceTuple(
        [
            "metric",
            "semantic_model",
            "saved_query",
            "source",
            "analysis",
            "model",
            "test",
            "unit_test",
            "exposure",
            "snapshot",
            "seed",
            "default",
        ],
        case_sensitive=False,
    ),
    cls=MultiOption,
    multiple=True,
    default=(),
)

# Renamed to --export-saved-queries
deprecated_include_saved_query = click.option(
    "--include-saved-query/--no-include-saved-query",
    envvar="DBT_INCLUDE_SAVED_QUERY",
    help="Include saved queries in the list of resources to be selected for build command",
    is_flag=True,
    hidden=True,
)

model_decls = ("-m", "--models", "--model")
select_decls = ("-s", "--select")
select_attrs = {
    "envvar": None,
    "help": "Specify the nodes to include.",
    "cls": MultiOption,
    "multiple": True,
    "type": tuple,
}

inline = click.option(
    "--inline",
    envvar=None,
    help="Pass SQL inline to dbt compile and show",
)

inline_direct = click.option(
    "--inline-direct",
    envvar=None,
    help="Internal flag to pass SQL inline to dbt show. Do not load the entire project or apply templating.",
    hidden=True,
)

# `--select` and `--models` are analogous for most commands except `dbt list` for legacy reasons.
# Most CLI arguments should use the combined `select` option that aliases `--models` to `--select`.
# However, if you need to split out these separators (like `dbt ls`), use the `models` and `raw_select` options instead.
# See https://github.com/dbt-labs/dbt-core/pull/6774#issuecomment-1408476095 for more info.
models = click.option(*model_decls, **select_attrs)  # type: ignore[arg-type]
raw_select = click.option(*select_decls, **select_attrs)  # type: ignore[arg-type]
select = click.option(*select_decls, *model_decls, **select_attrs)  # type: ignore[arg-type]

selector = click.option(
    "--selector",
    envvar=None,
    help="The selector name to use, as defined in selectors.yml",
)

send_anonymous_usage_stats = click.option(
    "--send-anonymous-usage-stats/--no-send-anonymous-usage-stats",
    envvar="DBT_SEND_ANONYMOUS_USAGE_STATS",
    help="Send anonymous usage stats to dbt Labs.",
    default=True,
)

clean_project_files_only = click.option(
    "--clean-project-files-only / --no-clean-project-files-only",
    envvar="DBT_CLEAN_PROJECT_FILES_ONLY",
    help="If disabled, dbt clean will delete all paths specified in clean-paths, even if they're outside the dbt project.",
    default=True,
)

show = click.option(
    "--show",
    envvar=None,
    help="Show a sample of the loaded data in the terminal",
    is_flag=True,
)

# TODO:  The env var is a correction!
# The original env var was `DBT_TEST_SINGLE_THREADED`.
# This broke the existing naming convention.
# This will need to be communicated as a change to the community!
#
# N.B. This flag is only used for testing, hence it's hidden from help text.
single_threaded = click.option(
    "--single-threaded/--no-single-threaded",
    envvar="DBT_SINGLE_THREADED",
    default=False,
    hidden=True,
)

skip_profile_setup = click.option(
    "--skip-profile-setup",
    "-s",
    envvar=None,
    help="Skip interactive profile setup.",
    is_flag=True,
)

empty_catalog = click.option(
    "--empty-catalog",
    help="If specified, generate empty catalog.json file during the `dbt docs generate` command.",
    default=False,
    is_flag=True,
)

source = click.option(
    "--source",
    envvar=None,
    help="Source to download page from, must be one of hub, git, or local. Defaults to hub.",
    type=click.Choice(["hub", "git", "local"], case_sensitive=True),
    default="hub",
)

static = click.option(
    "--static",
    help="Generate an additional static_index.html with manifest and catalog built-in.",
    default=False,
    is_flag=True,
)

state = click.option(
    "--state",
    envvar="DBT_STATE",
    help="Unless overridden, use this state directory for both state comparison and deferral.",
    type=click.Path(
        dir_okay=True,
        file_okay=False,
        readable=True,
        resolve_path=False,
        path_type=Path,
    ),
)

defer_state = click.option(
    "--defer-state",
    envvar="DBT_DEFER_STATE",
    help="Override the state directory for deferral only.",
    type=click.Path(
        dir_okay=True,
        file_okay=False,
        readable=True,
        resolve_path=False,
        path_type=Path,
    ),
)

deprecated_state = click.option(
    "--deprecated-state",
    envvar="DBT_ARTIFACT_STATE_PATH",
    help="Internal flag for deprecating old env var.",
    hidden=True,
    type=click.Path(
        dir_okay=True,
        file_okay=False,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
)

static_parser = click.option(
    "--static-parser/--no-static-parser",
    envvar="DBT_STATIC_PARSER",
    help="Use the static parser.",
    default=True,
)

store_failures = click.option(
    "--store-failures",
    envvar="DBT_STORE_FAILURES",
    help="Store test results (failing rows) in the database",
    is_flag=True,
)

target = click.option(
    "--target",
    "-t",
    envvar="DBT_TARGET",
    help="Which target to load for the given profile",
)

target_path = click.option(
    "--target-path",
    envvar="DBT_TARGET_PATH",
    help="Configure the 'target-path'. Only applies this setting for the current run. Overrides the 'DBT_TARGET_PATH' if it is set.",
    type=click.Path(),
)

upgrade = click.option(
    "--upgrade",
    envvar=None,
    help="Upgrade packages to the latest version.",
    is_flag=True,
)

debug_connection = click.option(
    "--connection",
    envvar=None,
    help="Test the connection to the target database independent of dependency checks.",
    is_flag=True,
)

threads = click.option(
    "--threads",
    envvar=None,
    help="Specify number of threads to use while executing models. Overrides settings in profiles.yml.",
    default=None,
    type=click.INT,
)

use_experimental_parser = click.option(
    "--use-experimental-parser/--no-use-experimental-parser",
    envvar="DBT_USE_EXPERIMENTAL_PARSER",
    help="Enable experimental parsing features.",
)

vars = click.option(
    "--vars",
    envvar=None,
    help="Supply variables to the project. This argument overrides variables defined in your dbt_project.yml file. This argument should be a YAML string, eg. '{my_variable: my_value}'",
    type=YAML(),
    default="{}",
)


# TODO: when legacy flags are deprecated use
# click.version_option instead of a callback
def _version_callback(ctx, _param, value):
    if not value or ctx.resilient_parsing:
        return
    click.echo(get_version_information())
    ctx.exit()


version = click.option(
    "--version",
    "-V",
    "-v",
    callback=_version_callback,
    envvar=None,
    expose_value=False,
    help="Show version information and exit",
    is_eager=True,
    is_flag=True,
)

version_check = click.option(
    "--version-check/--no-version-check",
    envvar="DBT_VERSION_CHECK",
    help="If set, ensure the installed dbt version matches the require-dbt-version specified in the dbt_project.yml file (if any). Otherwise, allow them to differ.",
    default=True,
)

warn_error = click.option(
    "--warn-error",
    envvar="DBT_WARN_ERROR",
    help="If dbt would normally warn, instead raise an exception. Examples include --select that selects nothing, deprecations, configurations with no associated models, invalid test configurations, and missing sources/refs in tests.",
    default=None,
    is_flag=True,
)

warn_error_options = click.option(
    "--warn-error-options",
    envvar="DBT_WARN_ERROR_OPTIONS",
    default="{}",
    help="""If dbt would normally warn, instead raise an exception based on include/exclude configuration. Examples include --select that selects nothing, deprecations, configurations with no associated models, invalid test configurations,
    and missing sources/refs in tests. This argument should be a YAML string, with keys 'include' or 'exclude'. eg. '{"include": "all", "exclude": ["NoNodesForSelectionCriteria"]}'""",
    type=WarnErrorOptionsType(),
)

write_json = click.option(
    "--write-json/--no-write-json",
    envvar="DBT_WRITE_JSON",
    help="Whether or not to write the manifest.json and run_results.json files to the target directory",
    default=True,
)

show_resource_report = click.option(
    "--show-resource-report/--no-show-resource-report",
    default=False,
    envvar="DBT_SHOW_RESOURCE_REPORT",
    hidden=True,
)

use_fast_test_edges = click.option(
    "--use-fast-test-edges/--no-use-fast-test-edges",
    envvar="DBT_USE_FAST_TEST_EDGES",
    default=False,
    hidden=True,
)

type_check = click.option(
    "--type-check/--no-type-check",
    envvar="DBT_TYPE_CHECK",
    default=False,
    hidden=True,
)
