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
from typing import TYPE_CHECKING, List, Optional

from dbt.artifacts.exceptions import IncompatibleSchemaError
from dbt.artifacts.schemas.manifest import WritableManifest
from dbt.contracts.files import ParseFileType
from dbt.contracts.graph.manifest import Manifest
from dbt.events.types import V2ParserEnd, V2ParserStart
from dbt.exceptions import V2ParserError, V2ParserSchemaError, V2ParserVersionError
from dbt.flags import get_flags
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

    Also always forwards --log-format json so the subprocess emits structured
    per-line JSON on stdout/stderr instead of human-formatted text. This is not
    a user-configurable passthrough (unlike the flags above) — it's how
    _run_v2 talks to the subprocess, so it's added unconditionally rather
    than gated on a dbt-core flag.

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

    # json-compat output lets _run_v2 re-level each line by its real
    # severity instead of a single hardcoded level per stream. Request full
    # verbosity here and let dbt-core's own event system filter by level,
    # rather than also forwarding a --log-level and double-filtering.
    forwarded += ["--log-format", "json"]

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


_V2_JSON_LEVELS = {
    "error": EventLevel.ERROR,
    "warn": EventLevel.WARN,
    "info": EventLevel.INFO,
    "debug": EventLevel.DEBUG,
}


def _run_v2(argv: List[str]) -> None:
    """Run the v2 parser subprocess, capturing stdout/stderr and re-emitting
    each line live through dbt-core's event system as it arrives.

    Piping (rather than inheriting) the child's fds means its output only
    reaches the user via fire_event — this is what makes it visible to any
    consumer of dbt-core's own event stream (e.g. dbt Studio's IDE log
    capture), which previously only saw the v2 parser's output if
    something was reading the raw inherited file descriptors directly (true
    for a CLI terminal, not true inside Studio's execution model).

    _build_argv requests --log-format json, so each line is normally a JSON
    object with the v2-assigned severity at `info.level` and message at
    `info.msg`; those are re-emitted as a Note at the mapped level rather than
    the stream's fallback level. json-compat is not a stable, fully-supported
    v2 parser contract (schema/fields/level strings may change between v2
    parser releases without notice), and the v2 parser emits plain text before
    its JSON logger initializes (e.g. CLI-arg errors) or after a panic — so any
    line that isn't valid JSON, or is missing the fields above, falls back to
    today's behavior: re-emitted verbatim at the stream's fallback level
    (stdout at INFO, stderr at WARN).

    On a nonzero exit, raises V2ParserError with just the exit code;
    the actual failure detail was already streamed live as Note events
    above, so it isn't duplicated into the exception message.
    TODO: once the Python library to decode the v2 parser's native OTel log stream
    lands, replace this json-compat parsing with a real parse into properly
    leveled/structured dbt events (also preserving info.code/info.name,
    which json-compat exposes but this still discards into a flat Note).
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

    def _pump(stream, fallback_level: EventLevel) -> None:
        for raw_line in stream:
            line = raw_line.rstrip("\n")
            level = fallback_level
            msg = line
            try:
                parsed = json.loads(line)
                info = parsed["info"]
                msg = info["msg"]
                level = _V2_JSON_LEVELS.get(info.get("level"), fallback_level)
            except (json.JSONDecodeError, KeyError, TypeError):
                pass
            fire_event(Note(msg=msg), level=level)

    # Separate threads per stream avoid the deadlock a single blocking
    # readline() would hit if the other stream fills its OS pipe buffer.
    readers = [
        threading.Thread(target=_pump, args=(proc.stdout, EventLevel.INFO), daemon=True),
        threading.Thread(target=_pump, args=(proc.stderr, EventLevel.WARN), daemon=True),
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
