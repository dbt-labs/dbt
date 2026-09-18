import json
import os
from pathlib import Path
from types import SimpleNamespace
from typing import Iterable, Optional
from unittest import mock

import pytest

from dbt.events.types import V2ParserEnd, V2ParserStart
from dbt.exceptions import V2ParserError, V2ParserSchemaError, V2ParserVersionError
from dbt.parser.v2 import (
    _build_argv,
    _delete_stale_partial_parse,
    _serialize_vars,
    parse_with_v2,
    rediscover_adapter_macros,
)
from dbt_common.events.base_types import EventLevel
from dbt_common.events.types import Note


def _flags(**overrides):
    base = {
        "V2_PARSER": "dbt-core-experimental-parser parse",
        "PROJECT_DIR": None,
        "PROFILES_DIR": None,
        "PROFILE": None,
        "TARGET": None,
        "TARGET_PATH": None,
        "PACKAGES_INSTALL_PATH": None,
        "VARS": None,
        "WRITE_JSON": True,
    }
    base.update(overrides)
    return SimpleNamespace(**base)


class _FakeStream:
    """Iterable text-mode stream stand-in that also supports close(), like a
    real Popen pipe."""

    def __init__(self, lines: Iterable[str]):
        self._iter = iter(lines)
        self.closed = False

    def __iter__(self):
        return self._iter

    def close(self) -> None:
        self.closed = True


class _FakePopen:
    """Stand-in for subprocess.Popen that supplies the attributes _run_v2
    reads: iterable text-mode stdout/stderr streams, wait() -> returncode."""

    def __init__(
        self,
        argv,
        returncode: int = 0,
        stdout_lines: Iterable[str] = (),
        stderr_lines: Iterable[str] = (),
    ):
        self.stdout = _FakeStream([line + "\n" for line in stdout_lines])
        self.stderr = _FakeStream([line + "\n" for line in stderr_lines])
        self.returncode = returncode

    def wait(self) -> int:
        return self.returncode


def _fake_parser(
    manifest_text: Optional[str],
    returncode: int = 0,
    stderr: str = "",
    stdout_lines: Iterable[str] = (),
    stderr_lines: Iterable[str] = (),
):
    """Build a subprocess.Popen side_effect that writes manifest.json into the
    --target-path argv slot. Pass manifest_text=None to simulate a parser run
    that exits successfully without writing a manifest."""

    def _popen(argv, *args, **kwargs):
        if manifest_text is not None and returncode == 0:
            target_path = Path(argv[argv.index("--target-path") + 1])
            target_path.mkdir(parents=True, exist_ok=True)
            (target_path / "manifest.json").write_text(manifest_text)
        lines = stderr_lines or ([stderr] if stderr else ())
        return _FakePopen(
            argv, returncode=returncode, stdout_lines=stdout_lines, stderr_lines=lines
        )

    return _popen


@pytest.fixture(autouse=True)
def _no_invocation_id():
    """Default to no invocation id so argv tests assert only the flags they set."""
    with mock.patch("dbt.parser.v2.get_invocation_id", return_value=None):
        yield


class TestBuildArgv:
    @pytest.fixture(autouse=True)
    def _identity_resolve(self):
        # Bypass wheel-binary path resolution so tests assert on the bare
        # command name regardless of whether dbt-core-experimental-parser is
        # installed in the test environment's scripts dir.
        with mock.patch("dbt.parser.v2._resolve_engine_command", side_effect=lambda c: c):
            yield

    def test_default_command_no_forwards(self):
        assert _build_argv(_flags()) == [
            "dbt-core-experimental-parser",
            "parse",
            "--log-format",
            "json",
        ]

    def test_forwards_all_known_flags(self):
        argv = _build_argv(
            _flags(
                PROJECT_DIR="/proj",
                PROFILES_DIR="/profiles",
                PROFILE="my_profile",
                TARGET="dev",
                TARGET_PATH="target",
                PACKAGES_INSTALL_PATH="dbt_packages",
                VARS={"k": "v"},
            )
        )
        assert argv[:2] == ["dbt-core-experimental-parser", "parse"]
        for pair in [
            ("--project-dir", "/proj"),
            ("--profiles-dir", "/profiles"),
            ("--profile", "my_profile"),
            ("--target", "dev"),
            ("--target-path", "target"),
            ("--packages-install-path", "dbt_packages"),
        ]:
            i = argv.index(pair[0])
            assert argv[i + 1] == pair[1]
        i = argv.index("--vars")
        assert "k" in argv[i + 1] and "v" in argv[i + 1]

    def test_forwards_log_format_json(self):
        """--log-format json is always forwarded (not user-configurable) so
        _run_v2 can parse each line's real severity instead of using a
        single hardcoded level per stream."""
        argv = _build_argv(_flags())
        i = argv.index("--log-format")
        assert argv[i + 1] == "json"

    def test_custom_command_split_with_shlex(self):
        argv = _build_argv(_flags(V2_PARSER="uv run dbt-core-experimental-parser parse"))
        assert argv == [
            "uv",
            "run",
            "dbt-core-experimental-parser",
            "parse",
            "--log-format",
            "json",
        ]

    def test_expands_tilde_in_v2_parser_path(self):
        # shlex.split leaves ~ literal; users expect shell-style expansion when
        # pointing --v2-parser at a binary under their home directory.
        home = os.path.expanduser("~")
        argv = _build_argv(_flags(V2_PARSER="~/bin/my-parser parse"))
        assert argv == [f"{home}/bin/my-parser", "parse", "--log-format", "json"]

    def test_target_path_override_replaces_user_value(self):
        argv = _build_argv(_flags(TARGET_PATH="user/target"), target_path_override="/tmp/handoff")
        i = argv.index("--target-path")
        assert argv[i + 1] == "/tmp/handoff"
        assert "user/target" not in argv

    def test_forwards_invocation_id(self):
        with mock.patch(
            "dbt.parser.v2.get_invocation_id",
            return_value="11111111-1111-1111-1111-111111111111",
        ):
            argv = _build_argv(_flags())
        i = argv.index("--invocation-id")
        assert argv[i + 1] == "11111111-1111-1111-1111-111111111111"

    def test_omits_invocation_id_when_unavailable(self):
        argv = _build_argv(_flags())
        assert "--invocation-id" not in argv


class TestSerializeVars:
    def test_dict_to_yaml(self):
        out = _serialize_vars({"a": 1, "b": "two"})
        assert "a:" in out and "b:" in out

    def test_passthrough_string(self):
        assert _serialize_vars("a: 1") == "a: 1"


class TestDeleteStalePartialParse:
    def test_deletes_when_present(self, tmp_path: Path):
        msgpack = tmp_path / "partial_parse.msgpack"
        msgpack.write_bytes(b"stale")
        _delete_stale_partial_parse(tmp_path)
        assert not msgpack.exists()

    def test_noop_when_absent(self, tmp_path: Path):
        _delete_stale_partial_parse(tmp_path)


@pytest.fixture
def _patch_v2_deps():
    """parse_with_v2 now resolves flags via get_flags() and calls
    assert_no_get_nodes_plugins / enrich_manifest_with_plugin_artifacts on the
    real plugin manager. Stub them so tests focus on parser invocation behavior.
    """
    with mock.patch("dbt.parser.v2.get_flags", return_value=_flags()), mock.patch(
        "dbt.parser.manifest.assert_no_get_nodes_plugins"
    ), mock.patch("dbt.parser.manifest.enrich_manifest_with_plugin_artifacts"), mock.patch(
        "dbt.parser.v2.rediscover_adapter_macros"
    ):
        yield


class TestParseWithV2:
    def _runtime_config(self, target_path: Path):
        return SimpleNamespace(project_target_path=str(target_path), project_name="test")

    def test_missing_binary_raises_typed_error(self, tmp_path: Path, _patch_v2_deps):
        with mock.patch(
            "dbt.parser.v2.get_flags",
            return_value=_flags(V2_PARSER="definitely-not-a-real-binary-xyz"),
        ), mock.patch("dbt.parser.v2.subprocess.Popen", side_effect=FileNotFoundError()):
            with pytest.raises(V2ParserError):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

    def test_sets_dbt_invocation_env_on_subprocess(self, tmp_path: Path, _patch_v2_deps):
        """fs must see DBT_INVOCATION_ENV=dbt-core-v2-parser regardless of the
        parent process's value, so analytics can attribute the embedded fs run
        to the v2-parser pathway without clobbering the host's own telemetry."""
        captured = {}

        def _capture(argv, *args, **kwargs):
            captured["env"] = kwargs.get("env")
            return _fake_parser(json.dumps({"metadata": {}}))(argv, *args, **kwargs)

        with mock.patch.dict(
            "os.environ", {"DBT_INVOCATION_ENV": "dbt-cloud-prod__host:cloud"}, clear=False
        ), mock.patch("dbt.parser.v2.subprocess.Popen", side_effect=_capture), mock.patch(
            "dbt.parser.v2._load_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest", return_value=mock.MagicMock()
        ):
            parse_with_v2(self._runtime_config(tmp_path), write=False, write_json=False)
            # parent process env is untouched (asserted inside the patch.dict
            # block so the original value is still in place to compare against)
            assert os.environ["DBT_INVOCATION_ENV"] == "dbt-cloud-prod__host:cloud"

        assert captured["env"] is not None
        assert captured["env"]["DBT_INVOCATION_ENV"] == "dbt-core-v2-parser"

    def test_strips_core_only_engine_env_vars_from_subprocess(
        self, tmp_path: Path, _patch_v2_deps
    ):
        """fs hard-errors on unknown DBT_ENGINE_* vars. Every DBT_ENGINE_* var
        that maps to a dbt-core CLI option must be stripped from the child env;
        non-click DBT_ENGINE_* vars (state/recorder/deps) must pass through."""
        captured = {}

        def _capture(argv, *args, **kwargs):
            captured["env"] = kwargs.get("env")
            return _fake_parser(json.dumps({"metadata": {}}))(argv, *args, **kwargs)

        parent_env = {
            # click-bound — must be stripped
            "DBT_ENGINE_USE_V2_PARSER": "true",
            "DBT_ENGINE_V2_PARSER": "dbt-core-experimental-parser parse",
            "DBT_ENGINE_SKIP_BROWSER_AUTH": "true",
            "DBT_ENGINE_SQLPARSE": '{"MAX_GROUPING_DEPTH": "10"}',
            "DBT_ENGINE_DEBUG": "true",  # alias of DBT_DEBUG, click-bound
            # not click-bound — must pass through (fs uses these natively)
            "DBT_ENGINE_STATE_OAUTH_CLIENT_ID": "client-id",
            "DBT_ENGINE_STATE_API_URL": "https://state.example.com",
        }
        with mock.patch.dict("os.environ", parent_env, clear=False), mock.patch(
            "dbt.parser.v2.subprocess.Popen", side_effect=_capture
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest", return_value=mock.MagicMock()
        ):
            parse_with_v2(self._runtime_config(tmp_path), write=False, write_json=False)

        child_env = captured["env"]
        assert "DBT_ENGINE_USE_V2_PARSER" not in child_env
        assert "DBT_ENGINE_V2_PARSER" not in child_env
        assert "DBT_ENGINE_SKIP_BROWSER_AUTH" not in child_env
        assert "DBT_ENGINE_SQLPARSE" not in child_env
        assert "DBT_ENGINE_DEBUG" not in child_env
        assert child_env["DBT_ENGINE_STATE_OAUTH_CLIENT_ID"] == "client-id"
        assert child_env["DBT_ENGINE_STATE_API_URL"] == "https://state.example.com"

    def test_nonzero_exit_raises(self, tmp_path: Path, _patch_v2_deps):
        with mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(manifest_text=None, returncode=2),
        ):
            with pytest.raises(V2ParserError, match="exit 2"):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

    def _run_and_capture_notes(self, tmp_path: Path, stdout_lines=(), stderr_lines=()):
        """Run parse_with_v2 against fake stdout/stderr lines and return
        the (msg, level) pairs of every Note event fired."""
        events: list = []
        levels: list = []

        def _capture(event, *args, **kwargs):
            events.append(event)
            levels.append(kwargs.get("level"))

        with mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(
                manifest_text=json.dumps({"metadata": {}}),
                stdout_lines=stdout_lines,
                stderr_lines=stderr_lines,
            ),
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.fire_event", side_effect=_capture
        ):
            parse_with_v2(self._runtime_config(tmp_path), write=False, write_json=False)

        return [(e.msg, lvl) for e, lvl in zip(events, levels) if isinstance(e, Note)]

    def test_output_lines_reemitted_as_note_events(self, tmp_path: Path, _patch_v2_deps):
        """Non-JSON stdout/stderr lines must still be re-emitted through
        dbt-core's event system (as Note events) at the stream's fallback
        level, so they reach any consumer of that stream (e.g. dbt Studio),
        not just a raw inherited terminal fd. This is the json-compat
        fallback path: plain-text lines (e.g. pre-JSON-logger clap/panic
        output) aren't dropped just because they aren't valid JSON."""
        notes = self._run_and_capture_notes(
            tmp_path,
            stdout_lines=["compiling model foo"],
            stderr_lines=["warning: deprecated config"],
        )
        assert ("compiling model foo", EventLevel.INFO) in notes
        assert ("warning: deprecated config", EventLevel.WARN) in notes

    @pytest.mark.parametrize(
        "v2_level, expected_level",
        [
            ("error", EventLevel.ERROR),
            ("warn", EventLevel.WARN),
            ("info", EventLevel.INFO),
            ("debug", EventLevel.DEBUG),
        ],
    )
    def test_json_line_reemitted_with_mapped_level(
        self, tmp_path: Path, _patch_v2_deps, v2_level, expected_level
    ):
        """A json-compat line's own info.level must override the stream's
        fallback level (e.g. an "error" on stdout is still fired as ERROR,
        not silently downgraded to stdout's INFO fallback)."""
        line = json.dumps({"info": {"level": v2_level, "msg": "boom"}})
        notes = self._run_and_capture_notes(tmp_path, stdout_lines=[line])
        assert ("boom", expected_level) in notes

    def test_json_line_unknown_level_falls_back(self, tmp_path: Path, _patch_v2_deps):
        """An info.level string outside the known vocabulary must not raise —
        json-compat isn't a stable contract, so an unrecognized level falls
        back to the stream's default rather than crashing the parse."""
        line = json.dumps({"info": {"level": "trace", "msg": "boom"}})
        notes = self._run_and_capture_notes(tmp_path, stdout_lines=[line])
        assert ("boom", EventLevel.INFO) in notes

    def test_non_json_line_falls_back_to_raw_note(self, tmp_path: Path, _patch_v2_deps):
        notes = self._run_and_capture_notes(tmp_path, stdout_lines=["not json at all"])
        assert ("not json at all", EventLevel.INFO) in notes

    def test_json_line_missing_info_or_msg_falls_back(self, tmp_path: Path, _patch_v2_deps):
        """json-compat isn't a guaranteed contract; a schema change that drops
        or renames fields must degrade to the raw line rather than crash."""
        line = json.dumps({"unexpected": "shape"})
        notes = self._run_and_capture_notes(tmp_path, stderr_lines=[line])
        assert (line, EventLevel.WARN) in notes

    def test_missing_manifest_after_success_raises(self, tmp_path: Path, _patch_v2_deps):
        """Parser exits 0 but writes nothing — must raise, not silently load a stale file."""
        with mock.patch(
            "dbt.parser.v2.subprocess.Popen", side_effect=_fake_parser(manifest_text=None)
        ):
            with pytest.raises(V2ParserError, match="did not produce"):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

    def test_stale_target_manifest_not_loaded(self, tmp_path: Path, _patch_v2_deps):
        """A stale manifest left in target/ from a prior run must not satisfy
        the v2 handoff — parser writes into a fresh temp dir."""
        (tmp_path / "manifest.json").write_text(json.dumps({"stale": True}))
        with mock.patch(
            "dbt.parser.v2.subprocess.Popen", side_effect=_fake_parser(manifest_text=None)
        ):
            with pytest.raises(V2ParserError, match="did not produce"):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

    def test_invalid_json_raises_schema_error(self, tmp_path: Path, _patch_v2_deps):
        with mock.patch(
            "dbt.parser.v2.subprocess.Popen", side_effect=_fake_parser("{ not valid json")
        ):
            with pytest.raises(V2ParserSchemaError):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

    def test_incompatible_schema_version_raises_version_error(
        self, tmp_path: Path, _patch_v2_deps
    ):
        bad_version = json.dumps(
            {"metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v1.json"}}
        )
        with mock.patch("dbt.parser.v2.subprocess.Popen", side_effect=_fake_parser(bad_version)):
            with pytest.raises(V2ParserVersionError):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

    def test_no_write_json_leaves_target_dir_untouched(self, tmp_path: Path, _patch_v2_deps):
        """With write_json=False, the v2 handoff manifest must not be
        copied into the user's target dir."""
        # Pre-create target dir but leave it empty.
        target = tmp_path / "target"
        target.mkdir()
        with mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(json.dumps({"metadata": {}})),
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest",
            return_value=mock.MagicMock(),
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest",
            return_value=mock.MagicMock(),
        ):
            parse_with_v2(self._runtime_config(target), write=True, write_json=False)
        assert list(target.iterdir()) == []

    def test_write_json_writes_corrected_manifest_to_target_dir(
        self, tmp_path: Path, _patch_v2_deps
    ):
        """manifest.json must be written from the corrected in-memory Manifest
        (after rediscover_adapter_macros) rather than copied from the v2 parser's raw
        handoff file, so the on-disk artifact reflects the rediscovered macros
        actually used for compilation."""
        target = tmp_path / "target"
        target.mkdir()
        corrected_manifest = mock.MagicMock()
        with mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(json.dumps({"metadata": {}})),
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest",
            return_value=mock.MagicMock(),
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest",
            return_value=corrected_manifest,
        ):
            parse_with_v2(self._runtime_config(target), write=True, write_json=True)
        corrected_manifest.write.assert_called_once_with(str(target / "manifest.json"))


class TestParseWithV2Telemetry:
    """V2ParserStart/V2ParserEnd must fire around every v2-parser handoff so
    internal analytics can attribute v2-parser invocations and measure success
    rate end-to-end. Both events must fire on success; on each typed-exception
    failure path the end event must carry status="failure" + the exception
    class name. exit_code is -1 except for V2ParserError, which surfaces
    fs's process exit code via V2ParserError.returncode."""

    def _runtime_config(self, target_path: Path):
        return SimpleNamespace(project_target_path=str(target_path), project_name="test")

    def _patch_fire_event(self):
        events: list = []

        def _capture(event, *args, **kwargs):
            events.append(event)

        return events, mock.patch("dbt.parser.v2.fire_event", side_effect=_capture)

    def test_success_fires_start_and_end_success(self, tmp_path: Path, _patch_v2_deps):
        events, patch_fire = self._patch_fire_event()
        with patch_fire, mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(json.dumps({"metadata": {}})),
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest", return_value=mock.MagicMock()
        ):
            parse_with_v2(self._runtime_config(tmp_path), write=False, write_json=False)

        types = [type(e).__name__ for e in events]
        assert types == ["V2ParserStart", "V2ParserEnd"]
        start, end = events
        assert isinstance(start, V2ParserStart)
        assert start.project_name == "test"
        assert isinstance(end, V2ParserEnd)
        assert end.status == "success"
        assert end.error_class == ""
        assert end.exit_code == -1
        assert end.execution_time >= 0

    @pytest.mark.parametrize(
        "subprocess_side_effect, expected_error_class, expected_exit_code",
        [
            (FileNotFoundError(), "V2ParserError", -1),
            # fs exits non-zero — exit code surfaced via V2ParserError.returncode
            (_fake_parser(manifest_text=None, returncode=2), "V2ParserError", 2),
            # fs exits 0 but writes no manifest — generic V2ParserError, no returncode
            (_fake_parser(manifest_text=None), "V2ParserError", -1),
            (_fake_parser("{ not valid json"), "V2ParserSchemaError", -1),
        ],
    )
    def test_failure_fires_end_failure(
        self,
        tmp_path: Path,
        _patch_v2_deps,
        subprocess_side_effect,
        expected_error_class,
        expected_exit_code,
    ):
        events, patch_fire = self._patch_fire_event()
        with patch_fire, mock.patch(
            "dbt.parser.v2.subprocess.Popen", side_effect=subprocess_side_effect
        ):
            with pytest.raises(V2ParserError):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

        types = [type(e).__name__ for e in events]
        assert types == ["V2ParserStart", "V2ParserEnd"]
        end = events[1]
        assert end.status == "failure"
        assert end.error_class == expected_error_class
        assert end.exit_code == expected_exit_code

    def test_failure_version_error_fires_end_failure(self, tmp_path: Path, _patch_v2_deps):
        events, patch_fire = self._patch_fire_event()
        bad_version = json.dumps(
            {"metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v1.json"}}
        )
        with patch_fire, mock.patch(
            "dbt.parser.v2.subprocess.Popen", side_effect=_fake_parser(bad_version)
        ):
            with pytest.raises(V2ParserVersionError):
                parse_with_v2(self._runtime_config(tmp_path), write=True, write_json=True)

        end = events[1]
        assert end.status == "failure"
        assert end.error_class == "V2ParserVersionError"
        assert end.exit_code == -1


class TestRediscoverAdapterMacros:
    def _make_macro(self, unique_id, package_name):
        m = mock.MagicMock()
        m.package_name = package_name
        return m

    def _patch_adapter_deps(self, source_file_return):
        return (
            mock.patch("dbt.adapters.factory.load_plugin"),
            mock.patch(
                "dbt.adapters.factory.get_adapter_package_names", return_value=["dbt_postgres"]
            ),
            mock.patch("dbt.adapters.factory.get_include_paths", return_value=[]),
            mock.patch("dbt.parser.macros.MacroParser"),
            mock.patch("dbt.parser.read_files.load_source_file", return_value=source_file_return),
        )

    def test_replaces_stale_macros(self):
        stale_macro = self._make_macro("macro.dbt_postgres.stale", "dbt_postgres")
        root_macro = self._make_macro("macro.my_project.custom", "my_project")

        manifest = mock.MagicMock()
        manifest.macros = {
            "macro.dbt_postgres.stale": stale_macro,
            "macro.my_project.custom": root_macro,
        }

        fake_project = mock.MagicMock()
        fake_project.project_name = "dbt_postgres"

        runtime_config = mock.MagicMock()
        runtime_config.credentials.type = "postgres"
        runtime_config.load_projects.return_value = [
            ("my_project", mock.MagicMock()),
            ("dbt_postgres", fake_project),
        ]

        p_load, p_names, p_include, MockMacroParser, p_source = self._patch_adapter_deps(
            mock.MagicMock()
        )
        with p_load, p_names, p_include, MockMacroParser as MockParser, p_source:
            mock_parser_instance = MockParser.return_value
            mock_parser_instance.get_paths.return_value = [mock.MagicMock()]

            rediscover_adapter_macros(manifest, runtime_config)

        assert "macro.dbt_postgres.stale" not in manifest.macros
        assert "macro.my_project.custom" in manifest.macros
        assert manifest._macros_by_name is None
        assert manifest._macros_by_package is None
        mock_parser_instance.parse_file.assert_called_once()

    def test_skips_none_source_file(self):
        manifest = mock.MagicMock()
        manifest.macros = {}

        fake_project = mock.MagicMock()
        fake_project.project_name = "dbt_postgres"

        runtime_config = mock.MagicMock()
        runtime_config.credentials.type = "postgres"
        runtime_config.load_projects.return_value = [("dbt_postgres", fake_project)]

        p_load, p_names, p_include, MockMacroParser, p_source = self._patch_adapter_deps(None)
        with p_load, p_names, p_include, MockMacroParser as MockParser, p_source:
            mock_parser_instance = MockParser.return_value
            mock_parser_instance.get_paths.return_value = [mock.MagicMock()]

            rediscover_adapter_macros(manifest, runtime_config)

        mock_parser_instance.parse_file.assert_not_called()

    def test_restores_evicted_generic_test_macros(self):
        """The four built-in generic tests (test_not_null, test_unique,
        test_accepted_values, test_relationships) are {% test %} blocks under
        tests/generic/, parsed by GenericTestParser over generic_test_paths --
        not by MacroParser over macro_paths. Eviction removes them (their
        package_name is "dbt"), so the GenericTestParser reparse pass must
        restore them; otherwise test compilation fails with 'test_not_null'
        is undefined (regression from issue #15914)."""
        from dbt.contracts.graph.nodes import Macro
        from dbt.node_types import NodeType

        stale = self._make_macro("macro.dbt.test_not_null", "dbt")

        manifest = mock.MagicMock()
        manifest.macros = {"macro.dbt.test_not_null": stale}

        dbt_project = mock.MagicMock()
        dbt_project.project_name = "dbt"

        runtime_config = mock.MagicMock()
        runtime_config.credentials.type = "postgres"
        runtime_config.project_name = "my_project"
        runtime_config.load_projects.return_value = [("dbt", dbt_project)]

        restored = Macro(
            name="test_not_null",
            resource_type=NodeType.Macro,
            package_name="dbt",
            path="tests/generic/builtin.sql",
            original_file_path="tests/generic/builtin.sql",
            unique_id="macro.dbt.test_not_null",
            macro_sql="{% test not_null(model, column_name) %}select 1{% endtest %}",
        )

        def _parse_file_side_effect(block):
            manifest.macros[restored.unique_id] = restored

        with mock.patch("dbt.adapters.factory.load_plugin"), mock.patch(
            "dbt.adapters.factory.get_adapter_package_names",
            return_value=["dbt_postgres", "dbt"],
        ), mock.patch("dbt.adapters.factory.get_include_paths", return_value=[]), mock.patch(
            "dbt.parser.macros.MacroParser"
        ), mock.patch(
            "dbt.parser.read_files.load_source_file", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.search.filesystem_search", return_value=[mock.MagicMock()]
        ), mock.patch(
            "dbt.parser.generic_test.GenericTestParser"
        ) as MockGenericTestParser, mock.patch(
            "dbt.parser.manifest.get_adapter", return_value=mock.MagicMock()
        ):
            MockGenericTestParser.return_value.parse_file.side_effect = _parse_file_side_effect

            rediscover_adapter_macros(manifest, runtime_config)

        assert "macro.dbt.test_not_null" in manifest.macros
        MockGenericTestParser.return_value.parse_file.assert_called_once()

    def test_populates_depends_on_for_reparsed_macros(self):
        """A re-parsed adapter macro that calls another macro should get
        depends_on.macros populated, mirroring what ManifestLoader.macro_depends_on
        does for the normal (non-v2) parse pipeline."""
        from dbt.contracts.graph.nodes import Macro
        from dbt.node_types import NodeType

        other_macro = Macro(
            name="other_macro",
            resource_type=NodeType.Macro,
            package_name="my_project",
            path="macros/other_macro.sql",
            original_file_path="macros/other_macro.sql",
            unique_id="macro.my_project.other_macro",
            macro_sql="{% macro other_macro() %}select 1{% endmacro %}",
        )
        new_macro = Macro(
            name="get_something",
            resource_type=NodeType.Macro,
            package_name="dbt_postgres",
            path="macros/get_something.sql",
            original_file_path="macros/get_something.sql",
            unique_id="macro.dbt_postgres.get_something",
            macro_sql="{% macro get_something() %}{{ other_macro() }}{% endmacro %}",
        )

        manifest = mock.MagicMock()
        manifest.macros = {other_macro.unique_id: other_macro}

        fake_project = mock.MagicMock()
        fake_project.project_name = "dbt_postgres"

        runtime_config = mock.MagicMock()
        runtime_config.credentials.type = "postgres"
        runtime_config.project_name = "my_project"
        runtime_config.load_projects.return_value = [("dbt_postgres", fake_project)]

        def _parse_file_side_effect(block):
            manifest.macros[new_macro.unique_id] = new_macro

        p_load, p_names, p_include, MockMacroParser, p_source = self._patch_adapter_deps(
            mock.MagicMock()
        )
        with p_load, p_names, p_include, MockMacroParser as MockParser, p_source:
            mock_parser_instance = MockParser.return_value
            mock_parser_instance.get_paths.return_value = [mock.MagicMock()]
            mock_parser_instance.parse_file.side_effect = _parse_file_side_effect

            with mock.patch("dbt.parser.manifest.get_adapter", return_value=mock.MagicMock()):
                rediscover_adapter_macros(manifest, runtime_config)

        assert new_macro.depends_on.macros == [other_macro.unique_id]
        # pre-existing macros not touched by the rediscovery pass are left alone
        assert other_macro.depends_on.macros == []
