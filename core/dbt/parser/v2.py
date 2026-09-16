"""v2 parser integration.

Delegates parsing to an external v2 parser subprocess that produces a
manifest.json on disk. dbt-core then loads that manifest and converts it
to a runtime Manifest, bypassing its own parser entirely.

This module implements the handoff to the v2 parser and loading of the
resulting manifest artifacts.
"""

from __future__ import annotations

import json
import os
import platform
import shlex
import shutil
import subprocess
import sysconfig
import tempfile
import threading
import time
from pathlib import Path
from typing import TYPE_CHECKING, Dict, List, Optional, Tuple

from dbt.artifacts.exceptions import IncompatibleSchemaError
from dbt.artifacts.schemas.manifest import WritableManifest
from dbt.contracts.files import ParseFileType
from dbt.contracts.graph.manifest import Manifest
from dbt.events.types import V2ParserEnd, V2ParserStart
from dbt.exceptions import V2ParserError, V2ParserSchemaError, V2ParserVersionError
from dbt.flags import get_flags
from dbt_common import ui
from dbt_common.events.base_types import EventLevel
from dbt_common.events.functions import fire_event, get_invocation_id
from dbt_common.events.types import Note

if TYPE_CHECKING:
    from dbt.config import RuntimeConfig


def parse_with_v2(
    runtime_config: "RuntimeConfig",
    write: bool,
    write_json: bool,
) -> Manifest:
    """Invoke the v2 parser, load the resulting manifest.json, return runtime Manifest.

    The v2 parser is run into a temp handoff dir rather than the project's
    target dir so that (a) we can detect "parser exited 0 without writing"
    instead of silently loading a stale manifest from a prior run, and (b)
    `--no-write-json` doesn't leak a manifest.json into the user's target dir.
    """
    from dbt.parser.manifest import (
        assert_no_get_nodes_plugins,
        enrich_manifest_with_plugin_artifacts,
    )

    assert_no_get_nodes_plugins(runtime_config.project_name)

    flags = get_flags()
    project_target_path = Path(runtime_config.project_target_path)
    v2_parser_command = getattr(flags, "V2_PARSER", "dbt-core-experimental-parser parse")
    project_name = runtime_config.project_name

    fire_event(V2ParserStart(v2_parser_command=v2_parser_command, project_name=project_name))
    start_time = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix="dbt-v2-") as handoff_dir:
            handoff = Path(handoff_dir)
            argv = _build_argv(flags, target_path_override=str(handoff))

            _run_v2(argv)

            manifest_path = handoff / "manifest.json"
            if not manifest_path.exists():
                raise V2ParserError(
                    f"v2 parser exited successfully but did not produce {manifest_path.name} "
                    f"in the handoff directory."
                )

            writable_manifest = _load_writable_manifest(manifest_path)

            if write and write_json:
                # macro rediscovery below doesn't affect semantic models, so
                # semantic_manifest.json needs no correction pass.
                project_target_path.mkdir(parents=True, exist_ok=True)
                semantic_manifest_path = handoff / "semantic_manifest.json"
                if semantic_manifest_path.exists():
                    shutil.copyfile(
                        semantic_manifest_path, project_target_path / "semantic_manifest.json"
                    )
    except (
        V2ParserVersionError,
        V2ParserSchemaError,
        V2ParserError,
    ) as e:
        fire_event(
            V2ParserEnd(
                status="failure",
                execution_time=time.monotonic() - start_time,
                error_class=type(e).__name__,
                exit_code=getattr(e, "returncode", -1),
                project_name=project_name,
            ),
            level=EventLevel.ERROR,
        )
        raise

    fire_event(
        V2ParserEnd(
            status="success",
            execution_time=time.monotonic() - start_time,
            error_class="",
            exit_code=-1,
            project_name=project_name,
        ),
        level=EventLevel.INFO,
    )

    manifest = Manifest.from_writable_manifest(writable_manifest)
    rediscover_adapter_macros(manifest, runtime_config)
    # build_flat_graph is normally called by ManifestLoader.get_full_manifest;
    # the v2 path bypasses that loader, so populate flat_graph here to
    # power the `graph` context variable (graph.nodes, graph.sources, ...).
    manifest.build_flat_graph()

    _delete_stale_partial_parse(project_target_path)

    if write and write_json:
        # Written from the corrected manifest so the on-disk artifact reflects
        # rediscovered adapter macros rather than the v2 parser's bundled ones.
        # write_manifest() isn't reusable here: it no-ops under USE_V2_PARSER
        # and would also rewrite the semantic_manifest.json copied above.
        from dbt.utils.artifact_upload import add_artifact_produced

        manifest_out_path = str(project_target_path / "manifest.json")
        manifest.write(manifest_out_path)
        add_artifact_produced(manifest_out_path)
        enrich_manifest_with_plugin_artifacts(manifest, runtime_config.project_name)

    return manifest


def rediscover_adapter_macros(manifest: Manifest, runtime_config: "RuntimeConfig") -> None:
    """Evict v2-embedded adapter macros and re-parse them from the installed adapter.

    The v2 parser compiles against its own bundled adapter macros. If the user's installed
    dbt-<adapter> differs, those differences are silently lost after manifest load.
    This function replaces the embedded macros with freshly parsed ones from disk.
    """
    from dbt.adapters.factory import (
        get_adapter_package_names,
        get_include_paths,
        load_plugin,
        register_adapter,
    )
    from dbt.context.macro_resolver import MacroResolver
    from dbt.mp_context import get_mp_context
    from dbt.parser.generic_test import GenericTestParser
    from dbt.parser.macros import MacroParser
    from dbt.parser.manifest import resolve_macro_depends_on
    from dbt.parser.read_files import load_source_file
    from dbt.parser.search import FileBlock, filesystem_search

    adapter_type = runtime_config.credentials.type
    load_plugin(adapter_type)
    # resolve_macro_depends_on below needs a live adapter instance, but in the
    # v2 CLI flow the adapter isn't registered until after this function
    # returns (see requires.py's _wire_adapter_for_external_manifest). Register
    # it now; a later re-registration for the same adapter name is a no-op.
    register_adapter(runtime_config, get_mp_context())
    internal_pkg_names_list = get_adapter_package_names(adapter_type)
    internal_pkg_names = set(internal_pkg_names_list)

    stale_ids = [uid for uid, m in manifest.macros.items() if m.package_name in internal_pkg_names]
    for uid in stale_ids:
        manifest.macros.pop(uid)
    manifest._macros_by_name = None
    manifest._macros_by_package = None

    # load_dependencies() caches its result onto runtime_config.dependencies, so calling
    # it here would permanently exclude installed packages from later ref resolution
    # and macro dispatch. load_projects() has no such side effect.
    adapter_projects = dict(runtime_config.load_projects(get_include_paths(adapter_type)))
    pre_existing_ids = set(manifest.macros.keys())
    for project_name, project in adapter_projects.items():
        if project_name not in internal_pkg_names:
            continue
        macro_parser = MacroParser(project, manifest)
        for path in macro_parser.get_paths():
            source_file = load_source_file(path, ParseFileType.Macro, project.project_name, {})
            if source_file:
                macro_parser.parse_file(FileBlock(source_file))

        # Eviction above only restores via MacroParser, so the built-in generic
        # tests (parsed separately by GenericTestParser) are lost without this pass.
        generic_test_parser = GenericTestParser(project, manifest)
        for path in filesystem_search(
            project=project, relative_dirs=project.generic_test_paths, extension=".sql"
        ):
            source_file = load_source_file(
                path, ParseFileType.GenericTest, project.project_name, {}
            )
            if source_file:
                generic_test_parser.parse_file(FileBlock(source_file))

    new_macro_ids = set(manifest.macros.keys()) - pre_existing_ids
    if new_macro_ids:
        macro_resolver = MacroResolver(
            manifest.macros, runtime_config.project_name, internal_pkg_names_list
        )
        new_macros = [manifest.macros[uid] for uid in new_macro_ids]
        # resolve_macro_depends_on statically extracts adapter.dispatch() calls, which
        # requires runtime_config.dependencies to be a mapping rather than None. We
        # can't populate it via load_dependencies() (see comment above), so set it
        # temporarily to what we just loaded and restore it afterward.
        previous_dependencies = runtime_config.dependencies
        runtime_config.dependencies = adapter_projects
        try:
            resolve_macro_depends_on(runtime_config, macro_resolver, new_macros)
        finally:
            runtime_config.dependencies = previous_dependencies


def _build_argv(flags, target_path_override: Optional[str] = None) -> List[str]:
    """Translate dbt-core flags into v2 parser CLI args.

    The base command is taken from flags.V2_PARSER (default
    'dbt-core-experimental-parser parse') and split with shlex so users can
    configure subcommands or wrappers.

    Forwarded flags (must affect manifest output):
      --project-dir, --profiles-dir, --profile, --target,
      --target-path, --vars, --packages-install-path

    Also always forwards --log-format otel so the subprocess emits
    newline-delimited otel-shaped JSON (self-describing LogRecord/SpanStart/
    SpanEnd records) on stdout/stderr instead of human-formatted text. This is
    not a user-configurable passthrough (unlike the flags above) — it's how
    _run_v2 talks to the subprocess, so it's added unconditionally rather
    than gated on a dbt-core flag.

    Also always forwards --log-level-file off so the subprocess doesn't write
    its own dbt.log alongside dbt-core's.

    When target_path_override is provided, it replaces the user's --target-path
    so the v2 parser writes its handoff manifest where dbt expects it (a
    temp dir).
    """
    # posix=False on Windows so backslashes in paths (e.g. C:\path\to\parser.exe)
    # aren't stripped as shell escapes.
    base = shlex.split(
        getattr(flags, "V2_PARSER", "dbt-core-experimental-parser parse"),
        posix=(os.name != "nt"),
    )
    # Expand `~` in the parser binary so users can point --v2-parser at e.g.
    # `~/bin/my-parser`. shlex.split treats `~` as literal.
    if base:
        base[0] = _resolve_engine_command(os.path.expanduser(base[0]))
    forwarded: List[str] = []

    project_dir = getattr(flags, "PROJECT_DIR", None)
    if project_dir:
        forwarded += ["--project-dir", str(project_dir)]

    profiles_dir = getattr(flags, "PROFILES_DIR", None)
    if profiles_dir:
        forwarded += ["--profiles-dir", str(profiles_dir)]

    profile = getattr(flags, "PROFILE", None)
    if profile:
        forwarded += ["--profile", profile]

    target = getattr(flags, "TARGET", None)
    if target:
        forwarded += ["--target", target]

    if target_path_override is not None:
        forwarded += ["--target-path", target_path_override]
    else:
        target_path = getattr(flags, "TARGET_PATH", None)
        if target_path:
            forwarded += ["--target-path", str(target_path)]

    packages_install_path = getattr(flags, "PACKAGES_INSTALL_PATH", None)
    if packages_install_path:
        forwarded += ["--packages-install-path", str(packages_install_path)]

    cli_vars = getattr(flags, "VARS", None)
    if cli_vars:
        forwarded += ["--vars", _serialize_vars(cli_vars)]

    # Forward dbt-core's invocation_id so fs telemetry shares the same trace.
    invocation_id = get_invocation_id()
    if invocation_id:
        forwarded += ["--invocation-id", str(invocation_id)]

    # otel output lets _run_v2 re-level each line by its real severity and
    # relay it as a structured event instead of a single hardcoded level per
    # stream. Request full verbosity here and let dbt-core's own event system
    # filter by level, rather than also forwarding a --log-level and
    # double-filtering.
    forwarded += ["--log-format", "otel"]

    # The v2 parser defaults its file log to {--project-dir}/logs/dbt.log, which
    # is the same path dbt-core writes its own file log to when --log-path isn't
    # redirected. Both processes would then append the same relayed events to one
    # file. dbt-core is the only writer that should own that file, so turn the
    # subprocess's off: everything it emits reaches dbt-core over stdout anyway
    # and lands in dbt.log through dbt-core's own logger.
    forwarded += ["--log-level-file", "off"]

    return base + forwarded


def _resolve_engine_command(command: str) -> str:
    """Resolve a bare engine binary name to the wheel-installed path.

    If the user supplied an absolute/relative path or a multi-segment command,
    leave it alone. Otherwise, look for the binary in this Python install's
    scripts directory so we use the version pinned by dbt-core's dependency
    on dbt-core-experimental-parser rather than whatever's on PATH.
    """
    if os.sep in command or (os.altsep and os.altsep in command):
        return command
    name = f"{command}.exe" if platform.system() == "Windows" else command
    candidate = Path(sysconfig.get_path("scripts")) / name
    if candidate.exists():
        return str(candidate)
    return command


def _v2_subprocess_env() -> dict:
    """Return env for the fs subprocess, overriding DBT_INVOCATION_ENV.

    Setting DBT_INVOCATION_ENV=dbt-core-v2-parser on the child only (not the
    parent process env) tags every fs telemetry record from this run so the
    internal-analytics warehouse can attribute it to the v2-parser pathway.
    The host orchestrator's DBT_INVOCATION_ENV (set by dbt platform Orc/Sinter
    or by CI) still applies to dbt-core's own telemetry — we only relabel the
    embedded fs run.

    Also strips every DBT_ENGINE_* env var that maps to a dbt-core CLI option:
    fs hard-errors on unknown DBT_ENGINE_* vars (its prefix is reserved), and
    parsing-relevant flags are forwarded via argv by _build_argv. The
    DBT_ENGINE_STATE_* / recorder / deps vars in _ADDITIONAL_ENGINE_ENV_VARS
    are not click-bound and pass through unchanged — fs uses them natively.
    """
    from dbt.cli import params

    env = os.environ.copy()
    for engine_env_var in params.KNOWN_ENV_VARS:
        env.pop(engine_env_var.name, None)
    env["DBT_INVOCATION_ENV"] = "dbt-core-v2-parser"
    return env


# Typed-URL event_type strings (v1.public.events.fusion.log.rs / print_event.rs).
# LogMessage/UserLogMessage/ProgressMessage are "public", StdoutMessage/
# StderrMessage are "internal" -- both namespaces appear on the wire.
_EVENT_TYPE_LOG_MESSAGE = "v1.public.events.fusion.log.LogMessage"
_EVENT_TYPE_USER_LOG_MESSAGE = "v1.public.events.fusion.log.UserLogMessage"
_EVENT_TYPE_STDOUT_MESSAGE = "v1.internal.events.fusion.log.StdoutMessage"
_EVENT_TYPE_STDERR_MESSAGE = "v1.internal.events.fusion.log.StderrMessage"
_EVENT_TYPE_PROGRESS_MESSAGE = "v1.public.events.fusion.log.ProgressMessage"

# The Invocation span, whose end carries the aggregate warning/error counts.
# It is the one span record the relay reads rather than drops; see _pump.
_EVENT_TYPE_INVOCATION = "v1.public.events.fusion.invocation.Invocation"

# RESULT_LINE_OPT_OUT_COMMANDS from fusion's formatters/invocation.rs -- the
# commands whose runs print no status line, mirrored so the relay doesn't
# surface a line the v2 parser itself would have withheld.
_SUMMARY_OPT_OUT_COMMANDS = frozenset({"man", "login"})

# LogRecord event_types relayed as-is (body already holds the rendered text).
# ProgressMessage is handled separately below since it carries no body.
# Every other LogRecord (e.g. ListItemOutput, ShowDataOutput, CompiledCode,
# StateModifiedDiff -- show/list/compile concerns irrelevant to parse) is
# dropped, as is every span except the Invocation span end; see _pump.
#
# StdoutMessage/StderrMessage are retained here for forward-compatibility,
# but as of this writing they don't currently reach the wire on this path:
# their output_flags() (print_event.rs) is OUTPUT_CONSOLE only, while
# --log-format otel's JSONL layer gates on the separate EXPORT_JSONL bit
# (export.rs). This is no regression -- they were equally absent from the
# old json-compat relay.
_RELAYED_BODY_EVENT_TYPES = frozenset(
    {
        _EVENT_TYPE_LOG_MESSAGE,
        _EVENT_TYPE_USER_LOG_MESSAGE,
        _EVENT_TYPE_STDOUT_MESSAGE,
        _EVENT_TYPE_STDERR_MESSAGE,
    }
)

# ACTION_WIDTH from fusion's formatters/constants.rs, used to right-align
# ProgressMessage's action the same way format_progress_message does.
_PROGRESS_ACTION_WIDTH = 10

# OTLP severity_number subset (proto .../fusion/compat/otlp.proto), ascending.
# dbt-core's EventLevel has no TRACE tier, so TRACE floors to DEBUG.
_SEVERITY_NUMBER_BANDS: List[Tuple[int, EventLevel]] = [
    (1, EventLevel.DEBUG),
    (5, EventLevel.DEBUG),
    (9, EventLevel.INFO),
    (13, EventLevel.WARN),
    (17, EventLevel.ERROR),
]


def _severity_number_to_level(severity_number: object) -> EventLevel:
    """Map an otel severity_number to the closest EventLevel.

    Intermediate values floor to the next-lowest defined band (e.g. 10 ->
    INFO, same band as 9) rather than being treated as unknown.
    """
    if not isinstance(severity_number, (int, str)):
        return EventLevel.DEBUG
    try:
        number = int(severity_number)
    except ValueError:
        return EventLevel.DEBUG
    level = EventLevel.DEBUG
    for threshold, mapped in _SEVERITY_NUMBER_BANDS:
        if number >= threshold:
            level = mapped
    return level


def _format_progress_message(attributes: Dict) -> str:
    """Reproduce fusion's format_progress_message (formatters/progress.rs) v1-side.

    ProgressMessage carries no `body` -- dbt_emit.rs's
    emit_info_progress_message calls emit_info_event(message, None) -- so the
    display text is built entirely from `attributes` here instead.
    """
    action = str(attributes.get("action", "")).rjust(_PROGRESS_ACTION_WIDTH)
    target = attributes.get("target", "")
    description = attributes.get("description")
    if description:
        return f"{action} {target} ({description})"
    return f"{action} {target}"


def _prefix_log_message_code(attributes: Dict, body: str) -> str:
    """Reconstruct fusion's `[Name (dbt####)]` code prefix for LogMessage.

    Unlike json-compat, otel's `body` never carries the code -- fusion adds
    it in its own renderer (log_message.rs), not in the tracing event's
    message text -- so it must be composed back from LogMessage's
    `code`/`code_name` attributes here.
    """
    code = attributes.get("code")
    if code is None:
        return body
    try:
        code_number = int(code)
    except (TypeError, ValueError):
        # code is a proto u32 on the wire and should always parse; fall back
        # to the bare body rather than raising out of a daemon pump thread,
        # which would silently kill the relay for the rest of the run.
        return body
    code_name = attributes.get("code_name")
    prefix = f"[{code_name} (dbt{code_number:04d})]" if code_name else f"dbt{code_number:04d}"
    return f"{prefix}: {body}"


def _count_text(value: int, label_single: str, label_plural: str) -> str:
    return f"{value} {label_single if value == 1 else label_plural}"


def _coerce_count(value: object) -> int:
    """Read a proto uint64 off the wire, defaulting absent/garbage to 0.

    pbjson renders uint64 as a JSON *string* and omits the field entirely
    when the proto optional is unset, so both a missing key and "12" have
    to be handled.
    """
    if not isinstance(value, (int, str)):
        return 0
    try:
        return int(value)
    except ValueError:
        return 0


def _format_summary_duration(record: Dict) -> Optional[str]:
    """Render the span's elapsed time the way format_duration_for_summary
    (fusion's formatters/duration.rs) does, from the span's nanosecond
    timestamps."""
    start = _coerce_count(record.get("start_time_unix_nano"))
    end = _coerce_count(record.get("end_time_unix_nano"))
    if not start or not end or end < start:
        return None

    elapsed_nanos = end - start
    total_secs = elapsed_nanos / 1_000_000_000
    if total_secs >= 3600:
        hours = int(total_secs // 3600)
        minutes = int((total_secs % 3600) // 60)
        seconds = total_secs % 60
        if seconds >= 1:
            return f"{hours}h {minutes}m {seconds:.0f}s"
        return f"{hours}h {minutes}m" if minutes else f"{hours}h"
    if total_secs >= 60:
        minutes = int(total_secs // 60)
        seconds = total_secs % 60
        return f"{minutes}m {seconds:.0f}s" if seconds >= 1 else f"{minutes}m"
    if total_secs >= 1:
        return f"{total_secs:.1f}s"
    if elapsed_nanos >= 1_000_000:
        return f"{elapsed_nanos // 1_000_000}ms"
    if elapsed_nanos >= 1_000:
        return f"{elapsed_nanos // 1_000}us"
    return f"{elapsed_nanos}ns"


def _format_invocation_summary(record: Dict) -> Optional[str]:
    """Synthesize the v2 parser's end-of-run status line from its Invocation
    span end.

    That line has no LogRecord of its own: the v2 parser renders it in its
    console formatter (formatters/invocation.rs, format_status_line) out of
    span-end attributes, so relaying LogRecords alone loses it. The counts
    come from the parser's own metric aggregator, which means they include
    warnings whose LogRecords this relay filtered out -- tallying relayed
    lines here instead would under-report.

    Coloring uses dbt-core's own ui helpers (v1-native styling) rather than
    reproducing fusion's exact palette; ui.USE_COLOR handling is the same as
    in _style_severity.
    """
    attributes = record.get("attributes") or {}
    eval_args = attributes.get("eval_args") or {}

    command = eval_args.get("command")
    if not isinstance(command, str) or not command:
        command = "unknown"
    if command.lower() in _SUMMARY_OPT_OUT_COMMANDS:
        return None

    metrics = attributes.get("metrics") or {}
    warnings = _coerce_count(metrics.get("total_warnings"))
    errors = _coerce_count(metrics.get("total_errors"))

    if not errors and not warnings:
        status = ui.green("successfully")
    elif not errors:
        status = f"with {ui.yellow(_count_text(warnings, 'warning', 'warnings'))}"
    elif not warnings:
        status = f"with {ui.red(_count_text(errors, 'error', 'errors'))}"
    else:
        # fusion reds both counts once any error is present.
        status = (
            f"with {ui.red(_count_text(warnings, 'warning', 'warnings'))} "
            f"and {ui.red(_count_text(errors, 'error', 'errors'))}"
        )

    target = eval_args.get("target")
    for_target = f" for target '{target}'" if target else ""
    duration = _format_summary_duration(record)
    suffix = f" [{duration}]" if duration else ""

    return f"Finished '{command}' {status}{for_target}{suffix}"


def _style_severity(msg: str, level: EventLevel) -> str:
    """Apply v1-native WARN/ERROR styling via dbt_common.ui's tag helpers.

    Relies on ui.USE_COLOR already being synced from --use-colors (done at
    flag-construction time in cli/flags.py); _run_v2 only runs once flags
    are constructed. Forcing/restoring the global here would be a data race
    since _pump runs concurrently on two threads.
    """
    if level not in (EventLevel.WARN, EventLevel.ERROR):
        return msg
    return ui.warning_tag(msg) if level == EventLevel.WARN else ui.error_tag(msg)


def _run_v2(argv: List[str]) -> None:
    """Run the v2 parser subprocess, capturing stdout/stderr and re-emitting
    each line live through dbt-core's event system as it arrives.

    Piping (rather than inheriting) the child's fds means its output only
    reaches the user via fire_event — this is what makes it visible to any
    consumer of dbt-core's own event stream (e.g. dbt Studio's IDE log
    capture), which previously only saw the v2 parser's output if
    something was reading the raw inherited file descriptors directly (true
    for a CLI terminal, not true inside Studio's execution model).

    _build_argv requests --log-format otel, so each line is normally a
    self-describing JSON object: `record_type` discriminates SpanStart /
    SpanEnd / LogRecord. Spans are dropped unconditionally -- this is what
    structurally eliminates the duplicate version banner, ArtifactWritten,
    and per-node start/finish noise that the old json-compat relay used to
    show. Only an allowlist of LogRecord `event_type`s is relayed (see
    _RELAYED_BODY_EVENT_TYPES / _EVENT_TYPE_PROGRESS_MESSAGE); everything
    else is a show/list/compile concern irrelevant to parse.

    otel is not a stable, fully-supported v2 parser contract (schema/fields
    may change between v2 parser releases without notice), and the v2 parser
    emits plain text before its logger initializes (e.g. CLI-arg errors) or
    after a panic — any line that isn't a recognized otel record (invalid
    JSON, or valid JSON that isn't a LogRecord/SpanStart/SpanEnd envelope)
    falls back to being relayed verbatim. It is relayed at INFO regardless of
    stream: fire_event raises EventCompilationError for WARN-level events
    under --warn-error (dbt_common/events/event_manager.py), so a stray
    non-JSON stderr line (the old stderr->WARN fallback) could otherwise
    abort an unrelated run.

    On a nonzero exit, raises V2ParserError with just the exit code;
    the actual failure detail was already streamed live as Note events
    above, so it isn't duplicated into the exception message.
    """
    try:
        proc = subprocess.Popen(
            argv,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            env=_v2_subprocess_env(),
        )
    except FileNotFoundError as e:
        raise V2ParserError(
            f"v2 parser command not found: {argv[0]!r}. "
            f"Reinstall dbt-core-experimental-parser, or set --v2-parser to "
            f"point to an alternate engine binary."
        ) from e

    assert (
        proc.stdout is not None and proc.stderr is not None
    )  # guaranteed by stdout/stderr=PIPE above

    def _pump(stream) -> None:
        for raw_line in stream:
            line = raw_line.rstrip("\n")
            if not line:
                continue

            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                fire_event(Note(msg=line), level=EventLevel.INFO)
                continue

            record_type = record.get("record_type") if isinstance(record, dict) else None
            if record_type == "SpanEnd":
                # Spans have no body and are dropped, except the Invocation
                # span end, the only carrier of the status line's counts.
                if record.get("event_type") == _EVENT_TYPE_INVOCATION:
                    summary = _format_invocation_summary(record)
                    if summary:
                        fire_event(Note(msg=summary), level=EventLevel.INFO)
                continue
            if record_type == "SpanStart":
                continue
            if record_type != "LogRecord":
                # Valid JSON that isn't a recognized otel envelope -- relay
                # it like a non-JSON line rather than silently dropping it.
                fire_event(Note(msg=line), level=EventLevel.INFO)
                continue

            event_type = record.get("event_type", "")
            attributes = record.get("attributes") or {}
            level = _severity_number_to_level(record.get("severity_number"))

            if event_type == _EVENT_TYPE_PROGRESS_MESSAGE:
                msg = _format_progress_message(attributes)
            elif event_type in _RELAYED_BODY_EVENT_TYPES:
                msg = record.get("body", "")
                if event_type == _EVENT_TYPE_LOG_MESSAGE:
                    msg = _prefix_log_message_code(attributes, msg)
            else:
                continue

            fire_event(Note(msg=_style_severity(msg, level)), level=level)

    # Separate threads per stream avoid the deadlock a single blocking
    # readline() would hit if the other stream fills its OS pipe buffer.
    readers = [
        threading.Thread(target=_pump, args=(proc.stdout,), daemon=True),
        threading.Thread(target=_pump, args=(proc.stderr,), daemon=True),
    ]
    for reader in readers:
        reader.start()
    for reader in readers:
        reader.join()
    proc.stdout.close()
    proc.stderr.close()
    returncode = proc.wait()

    if returncode != 0:
        raise V2ParserError(
            f"v2 parser failed (exit {returncode}); see parser output above.",
            returncode=returncode,
        )


def _load_writable_manifest(path: Path) -> WritableManifest:
    try:
        return WritableManifest.read_and_check_versions(str(path))
    except IncompatibleSchemaError as e:
        raise V2ParserVersionError(
            f"v2-produced manifest at {path} has an incompatible schema "
            f"version: expected {e.expected}, found {e.found}."
        ) from e
    except Exception as e:
        raise V2ParserSchemaError(f"Could not load v2-produced manifest at {path}: {e}") from e


def _serialize_vars(cli_vars) -> str:
    """Serialize the resolved --vars dict to a YAML string for the v2 parser.

    dbt-core's --vars is parsed into a dict by click via the YAML param type
    (cli/params.py vars). Forward as a compact YAML string so the v2
    parser receives a single canonical value rather than re-resolving env
    vars or layered configs.
    """
    import yaml

    if isinstance(cli_vars, str):
        return cli_vars
    return yaml.safe_dump(cli_vars, default_flow_style=True).strip()


def _delete_stale_partial_parse(target_path: Path) -> None:
    """Remove partial_parse.msgpack written by a prior non-v2 run.

    The msgpack cache is owned by dbt-core's parser; in v2 mode it is no
    longer written, and a later non-v2 run would load a cache whose
    file_id mappings predate any v2-era source changes. Deleting on
    v2 entry is harmless if absent and unambiguous if present.
    """
    msgpack = target_path / "partial_parse.msgpack"
    if msgpack.exists():
        msgpack.unlink()
