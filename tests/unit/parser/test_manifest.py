from argparse import Namespace
from typing import Optional
from unittest.mock import MagicMock, patch

import jinja2
import msgpack
import pytest
from metricflow_semantic_interfaces.type_enums import AggregationType, MetricType
from pytest_mock import MockerFixture

from dbt.adapters.postgres import PostgresAdapter
from dbt.artifacts.resources import Metric, MetricAggregationParams, MetricTypeParams
from dbt.artifacts.resources.base import FileHash
from dbt.artifacts.resources.types import FunctionLanguage, FunctionType, NodeType
from dbt.artifacts.resources.v1.semantic_model import NodeRelation
from dbt.config import RuntimeConfig
from dbt.contracts.graph.manifest import Manifest, ManifestStateCheck
from dbt.events.types import InvalidConcurrentBatchesConfig, UnusedResourceConfigPath
from dbt.exceptions import ParsingError
from dbt.flags import set_from_args
from dbt.hints import HintType
from dbt.parser.manifest import (
    LONG_PARSING_THRESHOLD_SECONDS,
    ManifestLoader,
    _check_function_language_support,
    _maybe_show_long_parsing_hint,
    _process_metric_node,
    _warn_for_unused_resource_config_paths,
    extended_mashumaro_encoder,
    extended_msgpack_encoder,
    version_to_str,
)
from dbt.parser.read_files import FileDiff
from dbt.tracking import User
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import add_callback_to_manager
from tests.unit.fixtures import generic_test_node, model_node


class TestPartialParse:
    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    @patch("dbt.parser.manifest.os.path.exists")
    @patch("dbt.parser.manifest.open")
    def test_partial_parse_file_path(self, patched_open, patched_os_exist, patched_state_check):
        mock_project = MagicMock(RuntimeConfig)
        mock_project.project_target_path = "mock_target_path"
        patched_os_exist.return_value = True
        ManifestLoader(mock_project, {})
        # by default we use the project_target_path
        patched_open.assert_called_with("mock_target_path/partial_parse.msgpack", "rb")
        set_from_args(Namespace(partial_parse_file_path="specified_partial_parse_path"), {})
        ManifestLoader(mock_project, {})
        # if specified in flags, we use the specified path
        patched_open.assert_called_with("specified_partial_parse_path", "rb")

    def test_profile_hash_change(self, mock_project):
        # This test validate that the profile_hash is updated when the connection keys change
        profile_hash = "750bc99c1d64ca518536ead26b28465a224be5ffc918bf2a490102faa5a1bcf5"
        mock_project.credentials.connection_info.return_value = "test"
        manifest = ManifestLoader(mock_project, {})
        assert manifest.manifest.state_check.profile_hash.checksum == profile_hash
        mock_project.credentials.connection_info.return_value = "test1"
        manifest = ManifestLoader(mock_project, {})
        assert manifest.manifest.state_check.profile_hash.checksum != profile_hash

    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    @patch("dbt.parser.manifest.os.path.exists")
    @patch("dbt.parser.manifest.open")
    def test_partial_parse_by_version(
        self,
        patched_open,
        patched_os_exist,
        patched_state_check,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
    ):
        file_hash = FileHash.from_contents("test contests")
        manifest.state_check = ManifestStateCheck(
            vars_hash=file_hash,
            profile_hash=file_hash,
            profile_env_vars_hash=file_hash,
            project_env_vars_hash=file_hash,
        )
        # we need a loader to compare the two manifests
        loader = ManifestLoader(runtime_config, {runtime_config.project_name: runtime_config})
        loader.manifest = manifest.deepcopy()

        is_partial_parsable, _ = loader.is_partial_parsable(manifest)
        assert is_partial_parsable

        manifest.metadata.dbt_version = "0.0.1a1"
        is_partial_parsable, _ = loader.is_partial_parsable(manifest)
        assert not is_partial_parsable

        manifest.metadata.dbt_version = "99999.99.99"
        is_partial_parsable, _ = loader.is_partial_parsable(manifest)
        assert not is_partial_parsable


class TestFailedPartialParse:
    @patch("dbt.tracking.track_partial_parser")
    @patch("dbt.tracking.active_user")
    @patch("dbt.parser.manifest.PartialParsing")
    @patch("dbt.parser.manifest.ManifestLoader.read_manifest_for_partial_parse")
    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    def test_partial_parse_safe_update_project_parser_files_partially(
        self,
        patched_state_check,
        patched_read_manifest_for_partial_parse,
        patched_partial_parsing,
        patched_active_user,
        patched_track_partial_parser,
    ):
        mock_instance = MagicMock()
        mock_instance.skip_parsing.return_value = False
        mock_instance.get_parsing_files.side_effect = KeyError("Whoopsie!")
        patched_partial_parsing.return_value = mock_instance

        mock_project = MagicMock(RuntimeConfig)
        mock_project.project_target_path = "mock_target_path"

        mock_saved_manifest = MagicMock(Manifest)
        mock_saved_manifest.files = {}
        patched_read_manifest_for_partial_parse.return_value = mock_saved_manifest

        loader = ManifestLoader(mock_project, {})
        loader.safe_update_project_parser_files_partially({})

        patched_track_partial_parser.assert_called_once()
        exc_info = patched_track_partial_parser.call_args[0][0]
        assert "traceback" in exc_info
        assert "exception" in exc_info
        assert "code" in exc_info
        assert "location" in exc_info
        assert "full_reparse_reason" in exc_info
        assert "KeyError: 'Whoopsie!'" == exc_info["exception"]
        assert isinstance(exc_info["code"], str) or isinstance(exc_info["code"], type(None))


class TestGetFullManifest:
    @pytest.fixture
    def set_required_mocks(
        self, mocker: MockerFixture, manifest: Manifest, mock_adapter: MagicMock
    ):
        mocker.patch("dbt.parser.manifest.get_adapter").return_value = mock_adapter
        mocker.patch("dbt.parser.manifest.ManifestLoader.load").return_value = manifest
        mocker.patch("dbt.parser.manifest._check_manifest").return_value = None
        mocker.patch("dbt.parser.manifest.ManifestLoader.save_macros_to_adapter").return_value = (
            None
        )
        mocker.patch("dbt.tracking.active_user").return_value = User(None)

    def test_write_perf_info(
        self,
        mock_project: MagicMock,
        mocker: MockerFixture,
        set_required_mocks,
    ) -> None:
        write_perf_info = mocker.patch("dbt.parser.manifest.ManifestLoader.write_perf_info")

        ManifestLoader.get_full_manifest(
            config=mock_project,
            # write_perf_info=False let it default instead
        )
        assert not write_perf_info.called

        ManifestLoader.get_full_manifest(config=mock_project, write_perf_info=False)
        assert not write_perf_info.called

        ManifestLoader.get_full_manifest(config=mock_project, write_perf_info=True)
        assert write_perf_info.called

    def test_reset(
        self,
        mock_project: MagicMock,
        mock_adapter: MagicMock,
        set_required_mocks,
    ) -> None:

        ManifestLoader.get_full_manifest(
            config=mock_project,
            # reset=False let it default instead
        )
        assert not mock_project.clear_dependencies.called
        assert not mock_adapter.clear_macro_resolver.called

        ManifestLoader.get_full_manifest(config=mock_project, reset=False)
        assert not mock_project.clear_dependencies.called
        assert not mock_adapter.clear_macro_resolver.called

        ManifestLoader.get_full_manifest(config=mock_project, reset=True)
        assert mock_project.clear_dependencies.called
        assert mock_adapter.clear_macro_resolver.called

    def test_partial_parse_file_diff_flag(
        self,
        mock_project: MagicMock,
        mocker: MockerFixture,
        set_required_mocks,
    ) -> None:

        # FileDiff.from_dict is only called if PARTIAL_PARSE_FILE_DIFF == False
        # So we can track this function call to check if setting PARTIAL_PARSE_FILE_DIFF
        # works appropriately
        mock_file_diff = mocker.patch("dbt.parser.read_files.FileDiff.from_dict")
        mock_file_diff.return_value = FileDiff([], [], [])

        ManifestLoader.get_full_manifest(config=mock_project)
        assert not mock_file_diff.called

        set_from_args(Namespace(PARTIAL_PARSE_FILE_DIFF=True), {})
        ManifestLoader.get_full_manifest(config=mock_project)
        assert not mock_file_diff.called

        set_from_args(Namespace(PARTIAL_PARSE_FILE_DIFF=False), {})
        ManifestLoader.get_full_manifest(config=mock_project)
        assert mock_file_diff.called


class TestWarnUnusedConfigs:
    @pytest.mark.parametrize(
        "resource_type,path,expect_used",
        [
            ("data_tests", "unused_path", False),
            ("data_tests", "minimal", True),
            ("metrics", "unused_path", False),
            ("metrics", "test", True),
            ("models", "unused_path", False),
            ("models", "pkg", True),
            ("saved_queries", "unused_path", False),
            ("saved_queries", "test", True),
            ("seeds", "unused_path", False),
            ("seeds", "pkg", True),
            ("semantic_models", "unused_path", False),
            ("semantic_models", "test", True),
            ("sources", "unused_path", False),
            ("sources", "pkg", True),
            ("unit_tests", "unused_path", False),
            ("unit_tests", "pkg", True),
        ],
    )
    def test_warn_for_unused_resource_config_paths(
        self,
        resource_type: str,
        path: str,
        expect_used: bool,
        manifest: Manifest,
        runtime_config: RuntimeConfig,
    ) -> None:
        catcher = EventCatcher(UnusedResourceConfigPath)
        add_callback_to_manager(catcher.catch)

        setattr(runtime_config, resource_type, {path: {"+materialized": "table"}})

        _warn_for_unused_resource_config_paths(manifest=manifest, config=runtime_config)

        if expect_used:
            assert len(catcher.caught_events) == 0
        else:
            assert len(catcher.caught_events) == 1
            assert f"{resource_type}.{path}" in str(catcher.caught_events[0].data)


class TestCheckForcingConcurrentBatches:
    @pytest.fixture
    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    @patch("dbt.parser.manifest.os.path.exists")
    @patch("dbt.parser.manifest.open")
    def manifest_loader(
        self, patched_open, patched_os_exist, patched_state_check
    ) -> ManifestLoader:
        mock_project = MagicMock(RuntimeConfig)
        mock_project.project_target_path = "mock_target_path"
        mock_project.project_name = "mock_project_name"
        return ManifestLoader(mock_project, {})

    @pytest.fixture
    def event_catcher(self) -> EventCatcher:
        return EventCatcher(InvalidConcurrentBatchesConfig)

    @pytest.mark.parametrize(
        "adapter_support,concurrent_batches_config,expect_warning",
        [
            (False, True, True),
            (False, False, False),
            (False, None, False),
            (True, True, False),
            (True, False, False),
            (True, None, False),
        ],
    )
    def test_check_forcing_concurrent_batches(
        self,
        mocker: MockerFixture,
        manifest_loader: ManifestLoader,
        postgres_adapter: PostgresAdapter,
        event_catcher: EventCatcher,
        adapter_support: bool,
        concurrent_batches_config: Optional[bool],
        expect_warning: bool,
    ):
        add_callback_to_manager(event_catcher.catch)
        model = model_node()
        model.config.concurrent_batches = concurrent_batches_config
        mocker.patch.object(postgres_adapter, "supports").return_value = adapter_support
        mocker.patch("dbt.parser.manifest.get_adapter").return_value = postgres_adapter
        mocker.patch.object(manifest_loader.manifest, "use_microbatch_batches").return_value = True

        manifest_loader.manifest.add_node_nofile(model)
        manifest_loader.check_forcing_batch_concurrency()

        if expect_warning:
            assert len(event_catcher.caught_events) == 1
            assert "Batches will be run sequentially" in event_catcher.caught_events[0].info.msg  # type: ignore
        else:
            assert len(event_catcher.caught_events) == 0


class TestUpdateSemanticModel:
    """Tests for ManifestLoader.update_semantic_model."""

    @pytest.fixture
    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    @patch("dbt.parser.manifest.os.path.exists")
    @patch("dbt.parser.manifest.open")
    def loader(self, patched_open, patched_os_exist, patched_state_check):
        mock_project = MagicMock(RuntimeConfig)
        mock_project.project_target_path = "mock_target_path"
        return ManifestLoader(mock_project, {})

    def test_no_index_error_when_depends_on_nodes_is_empty(self, loader):
        """Regression: update_semantic_model must not raise IndexError when
        depends_on_nodes is empty (e.g. referenced model is disabled)."""
        semantic_model = MagicMock()
        semantic_model.depends_on_nodes = []

        # Before the fix this raised: IndexError: list index out of range
        loader.update_semantic_model(semantic_model)

        # node_relation must not have been assigned
        assert "node_relation" not in semantic_model.__dict__

    def test_node_relation_set_when_depends_on_nodes_has_entry(self, loader):
        """When depends_on_nodes is non-empty, node_relation is populated."""
        refd_node = MagicMock()
        refd_node.relation_name = '"db"."schema"."my_model"'
        refd_node.alias = "my_model"
        refd_node.schema = "schema"
        refd_node.database = "db"
        loader.manifest.nodes["model.pkg.my_model"] = refd_node

        semantic_model = MagicMock()
        semantic_model.depends_on_nodes = ["model.pkg.my_model"]

        loader.update_semantic_model(semantic_model)

        assert semantic_model.node_relation == NodeRelation(
            relation_name='"db"."schema"."my_model"',
            alias="my_model",
            schema_name="schema",
            database="db",
        )


class TestExtendedMsgpackEncoder:
    """
    Unit tests for extended_msgpack_encoder and extended_mashumaro_encoder.

    The key regression being guarded: jinja2.Undefined objects that end up in
    manifest node fields (e.g. meta values rendered from schema.yml with an
    undefined Jinja variable) must be serialisable to msgpack.  Without the
    isinstance(obj, jinja2.Undefined) branch in extended_msgpack_encoder the
    packer raises:
        TypeError: can not serialize 'Undefined' object
    """

    def test_undefined_returns_none(self):
        """extended_msgpack_encoder converts jinja2.Undefined to None."""
        undefined = jinja2.Undefined(name="some_undefined_var")
        result = extended_msgpack_encoder(undefined)
        assert result is None

    def test_undefined_subclass_returns_none(self):
        """extended_msgpack_encoder handles jinja2.Undefined subclasses."""

        class MyUndefined(jinja2.Undefined):
            pass

        result = extended_msgpack_encoder(MyUndefined())
        assert result is None

    def test_non_undefined_passthrough(self):
        """extended_msgpack_encoder passes through unknown types unchanged."""
        # msgpack itself will raise for truly unserializable objects; the
        # encoder just returns them so msgpack can raise its own error.
        obj = object()
        assert extended_msgpack_encoder(obj) is obj

    def test_mashumaro_encoder_with_undefined_in_dict(self):
        """
        extended_mashumaro_encoder can pack a dict containing jinja2.Undefined.

        This is the direct reproduction of the production traceback: a manifest
        node with a meta dict like {"key": <jinja2.Undefined>} must serialise
        without raising TypeError.
        """
        undefined = jinja2.Undefined(name="undefined_jinja_var")
        data = {"meta": {"key": undefined}}
        # Must not raise TypeError
        packed = extended_mashumaro_encoder(data)
        unpacked = msgpack.unpackb(packed, raw=False)
        assert unpacked == {"meta": {"key": None}}

    def test_mashumaro_encoder_without_undefined_unchanged(self):
        """extended_mashumaro_encoder round-trips plain data correctly."""
        data = {"meta": {"key": "value"}, "count": 42}
        packed = extended_mashumaro_encoder(data)
        unpacked = msgpack.unpackb(packed, raw=False)
        assert unpacked == data


class TestCheckFunctionLanguageSupport:
    def _make_function_node(self, name, language, function_type=FunctionType.Scalar):
        node = MagicMock()
        node.name = name
        node.language = language
        node.config = MagicMock()
        node.config.type = function_type
        return node

    def _make_config(self, adapter_type):
        config = MagicMock(spec=RuntimeConfig)
        config.credentials = MagicMock()
        config.credentials.type = adapter_type
        return config

    def test_js_udf_on_unsupported_adapter_raises(self):
        manifest = MagicMock(spec=Manifest)
        manifest.functions = {
            "function.test.my_func": self._make_function_node(
                "my_func", FunctionLanguage.javascript
            )
        }
        config = self._make_config("postgres")
        with pytest.raises(ParsingError) as excinfo:
            _check_function_language_support(manifest, config)
        assert "Function 'my_func' uses JavaScript, which is not supported on 'postgres'" in str(
            excinfo.value
        )

    def test_js_udf_on_bigquery_passes(self):
        manifest = MagicMock(spec=Manifest)
        manifest.functions = {
            "function.test.my_func": self._make_function_node(
                "my_func", FunctionLanguage.javascript
            )
        }
        config = self._make_config("bigquery")
        # Test passes if this function doesn't throw an error
        _check_function_language_support(manifest, config)

    def test_js_udf_on_snowflake_passes(self):
        manifest = MagicMock(spec=Manifest)
        manifest.functions = {
            "function.test.my_func": self._make_function_node(
                "my_func", FunctionLanguage.javascript
            )
        }
        config = self._make_config("snowflake")
        _check_function_language_support(manifest, config)

    def test_js_aggregate_on_snowflake_raises(self):
        manifest = MagicMock(spec=Manifest)
        manifest.functions = {
            "function.test.my_agg": self._make_function_node(
                "my_agg", FunctionLanguage.javascript, FunctionType.Aggregate
            )
        }
        config = self._make_config("snowflake")
        with pytest.raises(ParsingError) as excinfo:
            _check_function_language_support(manifest, config)
        assert (
            "Function 'my_agg' is a JavaScript aggregate function and not supported on 'snowflake'"
            in str(excinfo.value)
        )

    def test_js_aggregate_on_bigquery_passes(self):
        manifest = MagicMock(spec=Manifest)
        manifest.functions = {
            "function.test.my_agg": self._make_function_node(
                "my_agg", FunctionLanguage.javascript, FunctionType.Aggregate
            )
        }
        config = self._make_config("bigquery")
        _check_function_language_support(manifest, config)

    def test_js_scalar_on_snowflake_passes(self):
        manifest = MagicMock(spec=Manifest)
        manifest.functions = {
            "function.test.my_func": self._make_function_node(
                "my_func", FunctionLanguage.javascript, FunctionType.Scalar
            )
        }
        config = self._make_config("snowflake")
        _check_function_language_support(manifest, config)


class TestBackfillDirectParents:
    @pytest.fixture
    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    @patch("dbt.parser.manifest.os.path.exists")
    @patch("dbt.parser.manifest.open")
    def loader(self, patched_open, patched_os_exist, patched_state_check) -> ManifestLoader:
        mock_project = MagicMock(RuntimeConfig)
        mock_project.project_target_path = "mock_target_path"
        mock_project.project_name = "mock_project_name"
        return ManifestLoader(mock_project, {})

    def test_in_project_model_gets_direct_parents_from_depends_on(
        self, loader: ManifestLoader
    ) -> None:
        model = model_node()
        model.depends_on.nodes = ["model.test.upstream", "seed.test.seed"]
        model.direct_parents = []
        loader.manifest.add_node_nofile(model)

        loader._backfill_direct_parents()

        assert model.direct_parents == ["model.test.upstream", "seed.test.seed"]

    def test_external_model_direct_parents_not_overwritten(self, loader: ManifestLoader) -> None:
        # External nodes (from publications) carry the full transitive closure in
        # depends_on.nodes but already have the narrower nearest-public-ancestor
        # set in direct_parents. The backfill must not widen it back.
        model = model_node()
        model.depends_on.nodes = ["model.upstream.nearest", "model.upstream.further"]
        model.direct_parents = ["model.upstream.nearest"]
        loader.manifest.add_node_nofile(model)

        loader._backfill_direct_parents()

        assert model.direct_parents == ["model.upstream.nearest"]

    def test_non_model_nodes_skipped(self, loader: ManifestLoader) -> None:
        test = generic_test_node()
        test.depends_on.nodes = ["model.test.upstream"]
        loader.manifest.add_node_nofile(test)

        loader._backfill_direct_parents()

        assert not hasattr(test, "direct_parents")

    def test_backfill_copies_list(self, loader: ManifestLoader) -> None:
        model = model_node()
        model.depends_on.nodes = ["model.test.upstream"]
        loader.manifest.add_node_nofile(model)
        loader._backfill_direct_parents()

        model.direct_parents.append("model.test.injected")

        assert model.depends_on.nodes == ["model.test.upstream"]


class TestVersionToStr:
    # Regression test for #12947: PR #12828 widened NodeVersion to
    # Union[int, float, str], and mashumaro deserializes union members in
    # declaration order — so YAML scalars like `v: 4.5` now arrive as Python
    # floats. The float case (4.5 -> "4.5") guards against a future Union
    # reorder silently dropping float versions back to the "" sentinel.
    @pytest.mark.parametrize(
        "version,expected",
        [
            (None, ""),
            (1, "1"),
            ("1", "1"),
            ("4.5", "4.5"),
            (4.5, "4.5"),
            ("test", "test"),
        ],
    )
    def test_version_to_str(self, version, expected):
        assert version_to_str(version) == expected


class TestLongParsingHint:
    @pytest.fixture(autouse=True)
    def mock_hint_deps(self, mocker: MockerFixture):
        self.mock_show_hint = mocker.patch("dbt.parser.manifest.show_hint")
        self.mock_get_flags = mocker.patch("dbt.parser.manifest.get_flags")

    def _set_v2_parser(self, enabled: bool):
        self.mock_get_flags.return_value = MagicMock(USE_V2_PARSER=enabled)

    def test_fires_when_slow_and_legacy_parser(self):
        self._set_v2_parser(False)
        _maybe_show_long_parsing_hint(LONG_PARSING_THRESHOLD_SECONDS + 1)
        self.mock_show_hint.assert_called_once_with(HintType.LONG_PARSING_WITHOUT_V2_PARSER)

    def test_silent_when_fast(self):
        self._set_v2_parser(False)
        _maybe_show_long_parsing_hint(LONG_PARSING_THRESHOLD_SECONDS)
        self.mock_show_hint.assert_not_called()

    def test_silent_when_using_v2_parser(self):
        self._set_v2_parser(True)
        _maybe_show_long_parsing_hint(LONG_PARSING_THRESHOLD_SECONDS + 100)
        self.mock_show_hint.assert_not_called()

    def test_silent_when_elapsed_is_none(self):
        self._set_v2_parser(False)
        _maybe_show_long_parsing_hint(None)
        self.mock_show_hint.assert_not_called()


class TestProcessMetricNodeSemanticModelNamespace:
    """Regression tests for #16142: an in-model (v2) simple metric defined in an
    installed package must depend on the semantic model in the metric's own
    package, not in the root project namespace."""

    def _make_v2_simple_metric(self, package_name: str) -> Metric:
        return Metric(
            name="active_ads",
            resource_type=NodeType.Metric,
            type=MetricType.SIMPLE,
            type_params=MetricTypeParams(
                metric_aggregation_params=MetricAggregationParams(
                    semantic_model="ad_report",
                    agg=AggregationType.COUNT_DISTINCT,
                )
            ),
            fqn=[package_name, "metrics", "active_ads"],
            unique_id=f"metric.{package_name}.active_ads",
            package_name=package_name,
            path="models/ad_report.yml",
            original_file_path="models/ad_report.yml",
            description="",
            meta={},
            tags=[],
            label="active_ads",
        )

    def test_v2_simple_metric_depends_on_own_package_semantic_model(self):
        metric = self._make_v2_simple_metric("ad_reporting")
        _process_metric_node(manifest=MagicMock(), current_project="root_project", metric=metric)
        assert metric.depends_on.nodes == ["semantic_model.ad_reporting.ad_report"]

    def test_v2_simple_metric_in_root_project_unchanged(self):
        metric = self._make_v2_simple_metric("root_project")
        _process_metric_node(manifest=MagicMock(), current_project="root_project", metric=metric)
        assert metric.depends_on.nodes == ["semantic_model.root_project.ad_report"]
