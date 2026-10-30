from argparse import Namespace
from dataclasses import dataclass
from datetime import datetime
from importlib import import_module
from typing import Optional, Type, Union
from unittest import mock
from unittest.mock import MagicMock, patch

import pytest
from dbt.adapters.contracts.connection import AdapterResponse
from dbt.adapters.planning import (
    DirectReplaceTable,
    IncrementalMaterializationPlan,
    MaterializationStatementStrategy,
    PlanProvenance,
    SnapshotMaterializationPlan,
)
from dbt.adapters.postgres import PostgresAdapter
from dbt.artifacts.resources.base import FileHash
from dbt.artifacts.resources.types import NodeType, RunHookType
from dbt.artifacts.resources.v1.components import DependsOn
from dbt.artifacts.resources.v1.config import Hook, NodeConfig
from dbt.artifacts.resources.v1.exposure import ExposureType
from dbt.artifacts.resources.v1.model import LatestVersionPointer, ModelConfig
from dbt.artifacts.resources.v1.owner import Owner
from dbt.artifacts.schemas.results import RunStatus
from dbt.artifacts.schemas.run import RunResult
from dbt.config.runtime import RuntimeConfig
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import Exposure, HookNode, ModelNode
from dbt.events.types import LogModelResult
from dbt.exceptions import DbtRuntimeError
from dbt.flags import get_flags, set_from_args
from dbt.task.run import (
    MicrobatchModelRunner,
    ModelRunner,
    NativeMaterializationExecution,
    RunTask,
    _get_adapter_info,
)
from dbt.task.runnable import _rows_affected
from dbt.tests.util import safe_set_invocation_context
from dbt.version import __version__
from dbt_common.events.base_types import EventLevel
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import add_callback_to_manager
from dbt_common.invocation import get_invocation_id
from opentelemetry import trace
from opentelemetry.trace.status import StatusCode
from psycopg2 import DatabaseError
from pytest_mock import MockerFixture

from core.dbt.task.run import MicrobatchBatchRunner


@pytest.mark.parametrize(
    "exception_to_raise, expected_cancel_connections",
    [
        (SystemExit, True),
        (KeyboardInterrupt, True),
        (Exception, False),
    ],
)
def test_run_task_cancel_connections(
    exception_to_raise, expected_cancel_connections, runtime_config: RuntimeConfig
):
    safe_set_invocation_context()

    def mock_run_queue(*args, **kwargs):
        raise exception_to_raise("Test exception")

    with (
        patch.object(RunTask, "run_queue", mock_run_queue),
        patch.object(RunTask, "_cancel_connections") as mock_cancel_connections,
    ):
        set_from_args(Namespace(write_json=False), None)
        task = RunTask(
            get_flags(),
            runtime_config,
            None,
        )
        with pytest.raises(exception_to_raise):
            task.execute_nodes()
        assert mock_cancel_connections.called == expected_cancel_connections


def test_run_task_preserve_edges():
    mock_node_selector = MagicMock()
    mock_spec = MagicMock()
    with (
        patch.object(RunTask, "get_node_selector", return_value=mock_node_selector),
        patch.object(RunTask, "get_selection_spec", return_value=mock_spec),
    ):
        task = RunTask(get_flags(), None, None)
        task.get_graph_queue()
        # when we get the graph queue, preserve_edges is True
        mock_node_selector.get_graph_queue.assert_called_with(mock_spec, True)


def test_tracking_fails_safely_for_missing_adapter():
    assert {} == _get_adapter_info(None, {})


def test_adapter_info_tracking():
    mock_run_result = MagicMock()
    mock_run_result.node = MagicMock()
    mock_run_result.node.config = {}
    assert _get_adapter_info(PostgresAdapter, mock_run_result) == {
        "model_adapter_details": {},
        "adapter_name": PostgresAdapter.__name__.split("Adapter")[0].lower(),
        "adapter_version": import_module("dbt.adapters.postgres.__version__").version,
        "base_adapter_version": import_module("dbt.adapters.__about__").version,
    }


@pytest.fixture
def model_runner(
    postgres_adapter: PostgresAdapter,
    table_model: ModelNode,
    runtime_config: RuntimeConfig,
) -> ModelRunner:
    return ModelRunner(
        config=runtime_config,
        adapter=postgres_adapter,
        node=table_model,
        node_index=1,
        num_nodes=1,
    )


@pytest.fixture
def run_result(table_model: ModelNode) -> RunResult:
    return RunResult(
        status=RunStatus.Success,
        timing=[],
        thread_id="an_id",
        execution_time=0,
        adapter_response={},
        message="It did it",
        failures=None,
        batch_results=None,
        node=table_model,
    )


class TestModelRunner:
    @pytest.fixture
    def log_model_result_catcher(self) -> EventCatcher:
        catcher = EventCatcher(event_to_catch=LogModelResult)
        add_callback_to_manager(catcher.catch)
        return catcher

    @pytest.mark.parametrize(
        "unique_id,language,uses_native",
        [
            ("macro.dbt.materialization_table_default", "sql", True),
            ("macro.dbt.materialization_view_default", "sql", True),
            ("macro.dbt.materialization_snapshot_default", "sql", True),
            ("macro.project.materialization_table_default", "sql", False),
            ("macro.project.materialization_view_default", "sql", False),
            ("macro.project.materialization_snapshot_default", "sql", False),
            ("macro.dbt_spark.materialization_snapshot_spark", "sql", False),
            ("macro.dbt.materialization_table_postgres", "sql", False),
            ("macro.dbt.materialization_table_default", "python", False),
        ],
    )
    def test_native_materialization_only_claims_exact_builtin_sql_lifecycles(
        self,
        model_runner: ModelRunner,
        unique_id: str,
        language: str,
        uses_native: bool,
    ) -> None:
        model = mock.Mock(language=language)
        materialization_macro = mock.Mock(unique_id=unique_id)

        executor = model_runner._get_native_materialization(
            model, materialization_macro
        )

        assert (executor is not None) is uses_native

    def test_native_incremental_execution_requires_adapter_opt_in(
        self, model_runner: ModelRunner
    ) -> None:
        model = mock.Mock(language="sql")
        materialization_macro = mock.Mock(
            unique_id="macro.dbt.materialization_incremental_default"
        )
        model_runner.adapter = mock.Mock(spec=[])

        assert (
            model_runner._get_native_materialization(model, materialization_macro)
            is None
        )

        incremental_plan = IncrementalMaterializationPlan(
            materialization_macro_id=materialization_macro.unique_id,
            provenance=(PlanProvenance("test.incremental", "adapter opted in"),),
        )
        model_runner.adapter = mock.Mock(spec=["plan_incremental_materialization"])
        model_runner.adapter.plan_incremental_materialization.return_value = (
            incremental_plan
        )

        execution = model_runner._get_native_materialization(
            model, materialization_macro
        )

        assert execution is not None
        assert execution.plan is incremental_plan

    def test_adapter_table_plan_selects_semantic_execution(
        self, model_runner: ModelRunner
    ) -> None:
        table_plan = DirectReplaceTable(
            statement=MaterializationStatementStrategy.NO_AUTO_BEGIN,
            provenance=(PlanProvenance("test.table", "adapter opted in"),),
        )
        model_runner.adapter = mock.Mock(
            spec=[
                "plan_table_materialization",
                "resolve_table_materialization_existing_relation",
                "resolve_table_materialization_relation",
            ]
        )
        model_runner.adapter.plan_table_materialization.return_value = table_plan
        model = mock.Mock(language="sql")
        materialization_macro = mock.Mock(
            unique_id="macro.dbt_snowflake.materialization_table_snowflake"
        )

        execution = model_runner._get_native_materialization(
            model, materialization_macro
        )

        assert execution is not None
        assert execution.plan is table_plan
        model_runner.adapter.plan_table_materialization.assert_called_once_with(
            materialization_macro.unique_id,
            "sql",
            model,
        )

    def test_adapter_incremental_plan_selects_incremental_executor(
        self, model_runner: ModelRunner
    ) -> None:
        incremental_plan = IncrementalMaterializationPlan(
            materialization_macro_id="macro.dbt_databricks.materialization_incremental_databricks",
            provenance=(PlanProvenance("test.incremental", "adapter opted in"),),
        )
        model_runner.adapter = mock.Mock(
            spec=[
                "plan_table_materialization",
                "plan_incremental_materialization",
            ]
        )
        model_runner.adapter.plan_table_materialization.return_value = None
        model_runner.adapter.plan_incremental_materialization.return_value = (
            incremental_plan
        )
        model = mock.Mock(language="sql")
        materialization_macro = mock.Mock(
            unique_id="macro.dbt_databricks.materialization_incremental_databricks"
        )

        execution = model_runner._get_native_materialization(
            model, materialization_macro
        )

        assert execution is not None
        assert execution.plan is incremental_plan

    @pytest.mark.parametrize("strategy", ["custom_strategy", "package.timestamp"])
    def test_custom_snapshot_strategy_falls_back_to_jinja(
        self,
        model_runner: ModelRunner,
        strategy: str,
    ) -> None:
        config = mock.Mock()
        config.get.side_effect = lambda name, default=None: {
            "strategy": strategy,
        }.get(name, default)
        context = {
            "config": config,
            "build_snapshot_staging_table": mock.Mock(),
        }
        plan = SnapshotMaterializationPlan(
            materialization_macro_id="macro.dbt.materialization_snapshot_default",
            provenance=(PlanProvenance("test.snapshot", "adapter opted in"),),
        )

        result = model_runner._execute_native_materialization(
            execution=NativeMaterializationExecution(plan),
            model=mock.Mock(),
            context=context,
        )

        assert result is None

    def test_snapshot_staging_builder_override_falls_back_to_jinja(
        self, model_runner: ModelRunner
    ) -> None:
        config = mock.Mock()
        config.get.side_effect = lambda name, default=None: {
            "strategy": "timestamp",
        }.get(name, default)
        helper = mock.Mock()
        helper.macro.unique_id = "macro.project.build_snapshot_staging_table"
        plan = SnapshotMaterializationPlan(
            materialization_macro_id="macro.dbt.materialization_snapshot_default",
            provenance=(PlanProvenance("test.snapshot", "adapter opted in"),),
        )

        result = model_runner._execute_native_materialization(
            execution=NativeMaterializationExecution(plan),
            model=mock.Mock(),
            context={"config": config, "build_snapshot_staging_table": helper},
        )

        assert result is None

    def test_print_result_line(
        self,
        log_model_result_catcher: EventCatcher,
        model_runner: ModelRunner,
        run_result: RunResult,
    ) -> None:
        # Check `print_result_line` with "successful" RunResult
        model_runner.print_result_line(run_result)
        assert len(log_model_result_catcher.caught_events) == 1
        assert log_model_result_catcher.caught_events[0].info.level == EventLevel.INFO
        assert (
            log_model_result_catcher.caught_events[0].data.status == run_result.message
        )

        # reset event catcher
        log_model_result_catcher.flush()

        # Check `print_result_line` with "error" RunResult
        run_result.status = RunStatus.Error
        model_runner.print_result_line(run_result)
        assert len(log_model_result_catcher.caught_events) == 1
        assert log_model_result_catcher.caught_events[0].info.level == EventLevel.ERROR
        assert log_model_result_catcher.caught_events[0].data.status == EventLevel.ERROR

    @pytest.mark.skip(
        reason="Default and adapter macros aren't being appropriately populated, leading to a runtime error"
    )
    def test_execute(
        self, table_model: ModelNode, manifest: Manifest, model_runner: ModelRunner
    ) -> None:
        model_runner.execute(model=table_model, manifest=manifest)
        # TODO: Assert that the model was executed

    def test_materialize_latest_version_pointer_for_latest_version(
        self, mocker: MockerFixture, model_runner: ModelRunner
    ) -> None:
        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str

            @property
            def name(self) -> str:
                return self.identifier

            def __str__(self) -> str:
                return f'"{self.database}"."{self.schema}"."{self.identifier}"'

        model = model_runner.node
        model.name = "versioned_model"
        model.version = 2
        model.latest_version = 2
        model.config = ModelConfig(
            materialized="table",
            latest_version_pointer=LatestVersionPointer(enabled=True),
        )

        source_relation = FakeRelation(
            database="dbt",
            schema="dbt_schema",
            identifier="versioned_model_v2",
            type="table",
        )
        pointer_relation = FakeRelation(
            database="dbt",
            schema="dbt_schema",
            identifier="versioned_model",
            type="view",
        )

        model_runner.adapter = mocker.Mock()
        manifest = mocker.Mock(spec=Manifest)
        manifest.find_macro_by_name.return_value = None  # alias macro not found
        manifest.find_materialization_macro_by_name.return_value = (
            mocker.sentinel.view_materialization
        )

        mocker.patch(
            "dbt.task.run.generate_runtime_model_context",
            return_value={"context_macro_stack": []},
        )
        mocker.patch(
            "dbt.task.run.MacroGenerator",
            return_value=mocker.Mock(return_value={"relations": []}),
        )
        mocker.patch.object(
            model_runner, "_materialization_relations", return_value=[pointer_relation]
        )

        pointer_relations = model_runner._materialize_latest_version_pointer(
            manifest=manifest,
            model=model,
            context={"context_macro_stack": []},
            relations=[source_relation],
        )

        assert pointer_relations == [pointer_relation]
        manifest.find_materialization_macro_by_name.assert_called_once_with(
            model_runner.config.project_name, "view", model_runner.adapter.type()
        )

    def test_materialize_latest_version_pointer_uses_custom_alias(
        self, mocker: MockerFixture, model_runner: ModelRunner
    ) -> None:
        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str

            @property
            def name(self) -> str:
                return self.identifier

            def __str__(self) -> str:
                return f'"{self.database}"."{self.schema}"."{self.identifier}"'

        model = model_runner.node
        model.name = "versioned_model"
        model.version = 2
        model.latest_version = 2
        model.config = ModelConfig(
            materialized="table",
            latest_version_pointer=LatestVersionPointer(
                enabled=True, alias="latest_alias"
            ),
        )

        source_relation = FakeRelation(
            database="dbt",
            schema="dbt_schema",
            identifier="versioned_model_v2",
            type="table",
        )
        pointer_relation = FakeRelation(
            database="dbt", schema="dbt_schema", identifier="latest_alias", type="view"
        )

        model_runner.adapter = mocker.Mock()
        manifest = mocker.Mock(spec=Manifest)
        manifest.find_macro_by_name.return_value = (
            None  # alias macro not found, uses custom_alias_name
        )
        manifest.find_materialization_macro_by_name.return_value = (
            mocker.sentinel.view_materialization
        )

        mock_context = {"context_macro_stack": []}
        mock_generate_context = mocker.patch(
            "dbt.task.run.generate_runtime_model_context", return_value=mock_context
        )
        mocker.patch(
            "dbt.task.run.MacroGenerator",
            return_value=mocker.Mock(return_value={"relations": []}),
        )
        mocker.patch.object(
            model_runner, "_materialization_relations", return_value=[pointer_relation]
        )

        pointer_relations = model_runner._materialize_latest_version_pointer(
            manifest=manifest,
            model=model,
            context={"context_macro_stack": []},
            relations=[source_relation],
        )

        assert pointer_relations == [pointer_relation]
        # Verify the synthetic node passed to context generation has the right alias
        call_args = mock_generate_context.call_args
        synthetic_node = call_args[0][0]
        assert synthetic_node.alias == "latest_alias"

    @pytest.mark.parametrize(
        "version,latest_version,latest_version_pointer_enabled",
        [
            (1, 2, True),
            (2, 2, False),
        ],
    )
    def test_materialize_latest_version_pointer_skips_when_not_needed(
        self,
        mocker: MockerFixture,
        model_runner: ModelRunner,
        version: int,
        latest_version: int,
        latest_version_pointer_enabled: bool,
    ) -> None:
        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str

            @property
            def name(self) -> str:
                return self.identifier

        model = model_runner.node
        model.name = "versioned_model"
        model.version = version
        model.latest_version = latest_version
        model.config = ModelConfig(
            materialized="table",
            latest_version_pointer=LatestVersionPointer(
                enabled=latest_version_pointer_enabled
            ),
        )

        model_runner.adapter = mocker.Mock()
        manifest = mocker.Mock(spec=Manifest)

        pointer_relations = model_runner._materialize_latest_version_pointer(
            manifest=manifest,
            model=model,
            context={"context_macro_stack": []},
            relations=[
                FakeRelation(
                    database="dbt",
                    schema="dbt_schema",
                    identifier="versioned_model_v2",
                    type="table",
                )
            ],
        )

        assert pointer_relations == []
        model_runner.adapter.Relation.create.assert_not_called()
        manifest.find_macro_by_name.assert_not_called()

    def test_materialize_latest_version_pointer_synthetic_node_clears_hooks_and_docs(
        self, mocker: MockerFixture, model_runner: ModelRunner
    ) -> None:
        """Synthetic node passed to view materialization has hooks cleared and
        persist_docs empty — pointer is an internal detail, not a model lifecycle event."""

        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str

            @property
            def name(self) -> str:
                return self.identifier

            def __str__(self) -> str:
                return f'"{self.database}"."{self.schema}"."{self.identifier}"'

        model = model_runner.node
        model.name = "versioned_model"
        model.version = 2
        model.latest_version = 2
        model.config = ModelConfig(
            materialized="table",
            latest_version_pointer=LatestVersionPointer(enabled=True),
            pre_hook=[Hook(sql="select 1")],
            post_hook=[Hook(sql="select 2")],
        )
        model.config.persist_docs = {"relation": True}

        source_relation = FakeRelation(
            database="dbt",
            schema="dbt_schema",
            identifier="versioned_model_v2",
            type="table",
        )

        model_runner.adapter = mocker.Mock()
        manifest = mocker.Mock(spec=Manifest)
        manifest.find_macro_by_name.return_value = None
        manifest.find_materialization_macro_by_name.return_value = (
            mocker.sentinel.view_materialization
        )

        mock_generate_context = mocker.patch(
            "dbt.task.run.generate_runtime_model_context",
            return_value={"context_macro_stack": []},
        )
        mocker.patch(
            "dbt.task.run.MacroGenerator",
            return_value=mocker.Mock(return_value={"relations": []}),
        )
        mocker.patch.object(model_runner, "_materialization_relations", return_value=[])

        model_runner._materialize_latest_version_pointer(
            manifest=manifest,
            model=model,
            context={"context_macro_stack": []},
            relations=[source_relation],
        )

        synthetic_node = mock_generate_context.call_args[0][0]
        assert synthetic_node.config.pre_hook == []
        assert synthetic_node.config.post_hook == []
        assert synthetic_node.config.persist_docs == {}
        assert synthetic_node.unique_id == f"{model.unique_id}__latest_version_pointer"

    def test_materialize_latest_version_pointer_errors_on_alias_collision(
        self, mocker: MockerFixture, model_runner: ModelRunner
    ) -> None:
        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str

            @property
            def name(self) -> str:
                return self.identifier

        model = model_runner.node
        model.name = "versioned_model"
        model.version = 2
        model.latest_version = 2
        model.config = ModelConfig(
            materialized="table",
            latest_version_pointer=LatestVersionPointer(
                enabled=True, alias="versioned_model_v2"
            ),
        )

        source_relation = FakeRelation(
            database="dbt",
            schema="dbt_schema",
            identifier="versioned_model_v2",
            type="table",
        )

        model_runner.adapter = mocker.Mock()
        manifest = mocker.Mock(spec=Manifest)
        manifest.find_macro_by_name.return_value = (
            None  # alias macro not found, falls back to custom_alias_name
        )

        with pytest.raises(DbtRuntimeError, match="already aliased"):
            model_runner._materialize_latest_version_pointer(
                manifest=manifest,
                model=model,
                context={"context_macro_stack": []},
                relations=[source_relation],
            )

    def test_materialize_latest_version_pointer_collision_is_case_insensitive_when_unquoted(
        self, mocker: MockerFixture, model_runner: ModelRunner
    ) -> None:
        # When the identifier is unquoted, the warehouse resolves it case-insensitively
        # (e.g. Snowflake folds DIM_CUSTOMERS and dim_customers to the same object), so a
        # latest-version alias differing from the pointer name only by case must still be
        # detected as a collision.
        @dataclass
        class FakePolicy:
            identifier: bool

        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str
            quote_policy: FakePolicy

            @property
            def name(self) -> str:
                return self.identifier

        model = model_runner.node
        model.name = "versioned_model"
        model.version = 2
        model.latest_version = 2
        model.config = ModelConfig(
            materialized="table",
            # pointer resolves to "versioned_model_v2"; alias differs only by case
            latest_version_pointer=LatestVersionPointer(
                enabled=True, alias="versioned_model_v2"
            ),
        )

        source_relation = FakeRelation(
            database="dbt",
            schema="dbt_schema",
            identifier="VERSIONED_MODEL_V2",
            type="table",
            quote_policy=FakePolicy(identifier=False),
        )

        model_runner.adapter = mocker.Mock()
        manifest = mocker.Mock(spec=Manifest)
        manifest.find_macro_by_name.return_value = None

        with pytest.raises(DbtRuntimeError, match="already aliased"):
            model_runner._materialize_latest_version_pointer(
                manifest=manifest,
                model=model,
                context={"context_macro_stack": []},
                relations=[source_relation],
            )

    def test_materialize_latest_version_pointer_no_collision_when_quoted_case_differs(
        self, model_runner: ModelRunner
    ) -> None:
        # When the identifier is quoted it is case-sensitive, so names differing only by
        # case are distinct relations and must NOT be treated as a collision.
        @dataclass
        class FakePolicy:
            identifier: bool

        @dataclass
        class FakeRelation:
            database: str
            schema: str
            identifier: str
            type: str
            quote_policy: FakePolicy

            @property
            def name(self) -> str:
                return self.identifier

        runner = model_runner
        assert not runner._pointer_collides_with_source(
            FakeRelation(
                database="dbt",
                schema="dbt_schema",
                identifier="VERSIONED_MODEL_V2",
                type="table",
                quote_policy=FakePolicy(identifier=True),
            ),
            "versioned_model_v2",
        )


class TestMicrobatchModelRunner:
    @pytest.fixture
    def model_runner(
        self,
        postgres_adapter: PostgresAdapter,
        table_model: ModelNode,
        runtime_config: RuntimeConfig,
    ) -> MicrobatchModelRunner:
        return MicrobatchModelRunner(
            config=runtime_config,
            adapter=postgres_adapter,
            node=table_model,
            node_index=1,
            num_nodes=1,
        )

    @pytest.fixture
    def batch_runner(
        self,
        postgres_adapter: PostgresAdapter,
        table_model: ModelNode,
        runtime_config: RuntimeConfig,
    ) -> MicrobatchBatchRunner:
        return MicrobatchBatchRunner(
            config=runtime_config,
            adapter=postgres_adapter,
            node=table_model,
            node_index=1,
            num_nodes=1,
            batch_idx=0,
            batches=[],
            relation_exists=False,
            incremental_batch=False,
        )

    @pytest.mark.parametrize(
        "has_relation,relation_type,materialized,full_refresh_config,full_refresh_flag,expectation",
        [
            (False, "table", "incremental", None, False, False),
            (True, "other", "incremental", None, False, False),
            (True, "table", "other", None, False, False),
            # model config takes precendence
            (True, "table", "incremental", True, False, False),
            # model config takes precendence
            (True, "table", "incremental", True, True, False),
            # model config takes precendence
            (True, "table", "incremental", False, False, True),
            # model config takes precendence
            (True, "table", "incremental", False, True, True),
            # model config is none, so opposite flag value
            (True, "table", "incremental", None, True, False),
            # model config is none, so opposite flag value
            (True, "table", "incremental", None, False, True),
        ],
    )
    def test__is_incremental(
        self,
        mocker: MockerFixture,
        model_runner: MicrobatchModelRunner,
        has_relation: bool,
        relation_type: str,
        materialized: str,
        full_refresh_config: Optional[bool],
        full_refresh_flag: bool,
        expectation: bool,
    ) -> None:

        # Setup adapter relation getting
        @dataclass
        class RelationInfo:
            database: str = "database"
            schema: str = "schema"
            name: str = "name"

        @dataclass
        class Relation:
            type: str

        model_runner.adapter = mocker.Mock()
        model_runner.adapter.Relation.create_from.return_value = RelationInfo()

        if has_relation:
            model_runner.adapter.get_relation.return_value = Relation(
                type=relation_type
            )
        else:
            model_runner.adapter.get_relation.return_value = None

        # Set ModelRunner configs
        model_runner.config.args = Namespace(FULL_REFRESH=full_refresh_flag)

        # Create model with configs
        model = model_runner.node
        model.config = ModelConfig(
            materialized=materialized, full_refresh=full_refresh_config
        )

        # Assert result of _is_incremental
        assert model_runner._is_incremental(model) == expectation

    @pytest.mark.parametrize(
        "adapter_microbatch_concurrency,has_relation,concurrent_batches,has_this,expectation",
        [
            (True, True, None, False, True),
            (True, True, None, True, False),
            (True, True, True, False, True),
            (True, True, True, True, True),
            (True, True, False, False, False),
            (True, True, False, True, False),
            (True, False, None, False, False),
            (True, False, None, True, False),
            (True, False, True, False, False),
            (True, False, True, True, False),
            (True, False, False, False, False),
            (True, False, False, True, False),
            (False, True, None, False, False),
            (False, True, None, True, False),
            (False, True, True, False, False),
            (False, True, True, True, False),
            (False, True, False, False, False),
            (False, True, False, True, False),
            (False, False, None, False, False),
            (False, False, None, True, False),
            (False, False, True, False, False),
            (False, False, True, True, False),
            (False, False, False, False, False),
            (False, False, False, True, False),
        ],
    )
    def test_should_run_in_parallel(
        self,
        mocker: MockerFixture,
        batch_runner: MicrobatchBatchRunner,
        adapter_microbatch_concurrency: bool,
        has_relation: bool,
        concurrent_batches: Optional[bool],
        has_this: bool,
        expectation: bool,
    ) -> None:
        batch_runner.node._has_this = has_this
        batch_runner.node.config = ModelConfig(concurrent_batches=concurrent_batches)
        batch_runner.relation_exists = has_relation

        mocked_supports = mocker.patch.object(batch_runner.adapter, "supports")
        mocked_supports.return_value = adapter_microbatch_concurrency

        # Assert result of should_run_in_parallel
        assert batch_runner.should_run_in_parallel() == expectation

    def test_get_microbatch_builder_uses_original_invocation_time_on_retry(
        self,
        mocker: MockerFixture,
        model_runner: MicrobatchModelRunner,
    ) -> None:
        """When retrying, get_microbatch_builder should use the original invocation
        time from the previous run rather than the current invocation time."""
        original_time = datetime(2025, 3, 21, 7, 55, 0)
        current_time = datetime(2025, 3, 25, 1, 4, 0)

        # Set up a mock parent task with original_invocation_started_at
        mock_parent = mocker.Mock(spec=RunTask)
        mock_parent.original_invocation_started_at = original_time
        model_runner._parent_task = mock_parent

        # Mock _is_incremental to avoid adapter calls
        mocker.patch.object(model_runner, "_is_incremental", return_value=True)

        # Mock get_invocation_started_at to return the "current" (retry) time
        mocker.patch(
            "dbt.task.run.get_invocation_started_at",
            return_value=current_time,
        )

        model = model_runner.node
        model.config.materialized = "incremental"
        model.config.incremental_strategy = "microbatch"
        model.config.batch_size = "day"
        model.config.begin = "2024-12-01"
        model.config.event_time = "_event_date"
        model_runner.config.args = Namespace(
            EVENT_TIME_START=None, EVENT_TIME_END=None, SAMPLE=None
        )

        builder = model_runner.get_microbatch_builder(model)
        assert builder.default_end_time == original_time

    def test_get_microbatch_builder_uses_current_time_without_retry(
        self,
        mocker: MockerFixture,
        model_runner: MicrobatchModelRunner,
    ) -> None:
        """When not retrying (normal run), get_microbatch_builder should use
        the current invocation time."""
        current_time = datetime(2025, 3, 25, 1, 4, 0)

        # Set up a mock parent task with no original_invocation_started_at
        mock_parent = mocker.Mock(spec=RunTask)
        mock_parent.original_invocation_started_at = None
        model_runner._parent_task = mock_parent

        # Mock _is_incremental to avoid adapter calls
        mocker.patch.object(model_runner, "_is_incremental", return_value=True)

        mocker.patch(
            "dbt.task.run.get_invocation_started_at",
            return_value=current_time,
        )

        model = model_runner.node
        model.config.materialized = "incremental"
        model.config.incremental_strategy = "microbatch"
        model.config.batch_size = "day"
        model.config.begin = "2024-12-01"
        model.config.event_time = "_event_date"
        model_runner.config.args = Namespace(
            EVENT_TIME_START=None, EVENT_TIME_END=None, SAMPLE=None
        )

        builder = model_runner.get_microbatch_builder(model)
        assert builder.default_end_time == current_time


class TestRunTask:
    @pytest.fixture(autouse=True)
    def before_each(self, monkeypatch, otel_spans):
        self.span_exporter = otel_spans
        # Instrumentation is gated behind --snowflake-projects-otel; enable it so
        # these span-emitting tests exercise the instrumented path.
        monkeypatch.setattr("dbt.task.runnable._otel_enabled", lambda: True)
        yield

    @pytest.fixture
    def hook_node(self) -> HookNode:
        return HookNode(
            package_name="test",
            path="/root/x/path.sql",
            original_file_path="/root/path.sql",
            language="sql",
            raw_code="select * from wherever",
            name="foo",
            resource_type=NodeType.Operation,
            unique_id="model.test.foo",
            fqn=["test", "models", "foo"],
            refs=[],
            sources=[],
            metrics=[],
            depends_on=DependsOn(),
            description="",
            database="test_db",
            schema="test_schema",
            alias="bar",
            tags=[],
            config=NodeConfig(),
            index=None,
            checksum=FileHash.from_contents(""),
            unrendered_config={},
        )

    @pytest.mark.parametrize(
        "error_to_raise,expected_result,expected_span_status",
        [
            (None, RunStatus.Success, StatusCode.OK),
            (DbtRuntimeError, RunStatus.Error, StatusCode.ERROR),
            (DatabaseError, RunStatus.Error, StatusCode.ERROR),
            (KeyboardInterrupt, KeyboardInterrupt, StatusCode.UNSET),
        ],
    )
    def test_safe_run_hooks(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        hook_node: HookNode,
        error_to_raise: Optional[Type[Exception]],
        expected_result: Union[RunStatus, Type[Exception]],
        expected_span_status: StatusCode,
    ):
        mocker.patch("dbt.task.run.RunTask.get_hooks_by_type").return_value = [
            hook_node
        ]
        mocker.patch(
            "dbt.task.run.RunTask.get_hook_sql"
        ).return_value = hook_node.raw_code

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None

        run_task = RunTask(
            args=flags,
            config=runtime_config,
            manifest=manifest,
        )

        adapter = mock.Mock()
        adapter_execute = mock.Mock()
        adapter_execute.return_value = (AdapterResponse(_message="Success"), None)

        if error_to_raise:
            adapter_execute.side_effect = error_to_raise("Oh no!")

        adapter.execute = adapter_execute

        try:
            result = run_task.safe_run_hooks(
                adapter=adapter,
                hook_type=RunHookType.End,
                extra_context={},
            )
            assert isinstance(expected_result, RunStatus)
            assert result == expected_result
            exported_spans = self.span_exporter.get_finished_spans()
        except BaseException as e:
            assert not isinstance(expected_result, RunStatus)
            assert issubclass(expected_result, BaseException)
            assert type(e) == expected_result
            exported_spans = self.span_exporter.get_finished_spans()

        # each hook emits a child span + an outer type span.
        assert len(exported_spans) == 2
        outer_span = next(s for s in exported_spans if s.name == "on-run-end")
        child_span = next(s for s in exported_spans if s.name == hook_node.unique_id)

        assert expected_span_status == outer_span.status.status_code
        assert outer_span.attributes.get("hook_type") == "on-run-end"
        assert "hook_outcome" not in outer_span.attributes
        assert "node.status" not in outer_span.attributes

        # Per-hook child span attributes
        if error_to_raise is not KeyboardInterrupt:
            assert child_span.attributes.get("hook_type") == "on-run-end"
            assert child_span.attributes.get("package_name") == hook_node.package_name
            assert child_span.attributes.get("name") == hook_node.name
            assert child_span.attributes.get("hook_index") == 1
            assert child_span.attributes.get("unique_id") == hook_node.unique_id
            expected_child_outcome = (
                "error" if error_to_raise is not None else "success"
            )
            assert child_span.attributes.get("hook_outcome") == expected_child_outcome
            expected_child_status = (
                StatusCode.ERROR if error_to_raise is not None else StatusCode.OK
            )
            assert child_span.status.status_code == expected_child_status
        else:
            assert "hook_outcome" not in child_span.attributes
            assert child_span.status.status_code == StatusCode.UNSET

    def test_no_run_hooks(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
    ):
        mocker.patch("dbt.task.run.RunTask.get_hooks_by_type").return_value = []

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None

        run_task = RunTask(
            args=flags,
            config=runtime_config,
            manifest=manifest,
        )

        adapter = mock.Mock()
        adapter_execute = mock.Mock()
        adapter_execute.return_value = (AdapterResponse(_message="Success"), None)
        adapter.execute = adapter_execute

        run_task.safe_run_hooks(
            adapter=adapter,
            hook_type=RunHookType.End,
            extra_context={},
        )
        exported_spans = self.span_exporter.get_finished_spans()
        assert len(exported_spans) == 0

    def test_safe_run_hooks_no_spans_when_otel_disabled(
        self,
        monkeypatch,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        hook_node: HookNode,
    ):
        # With the gate off, safe_run_hooks must emit no spans even when hooks run.
        monkeypatch.setattr("dbt.task.runnable._otel_enabled", lambda: False)
        mocker.patch("dbt.task.run.RunTask.get_hooks_by_type").return_value = [
            hook_node
        ]
        mocker.patch(
            "dbt.task.run.RunTask.get_hook_sql"
        ).return_value = hook_node.raw_code

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None

        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        adapter = mock.Mock()
        adapter.execute = mock.Mock(
            return_value=(AdapterResponse(_message="Success"), None)
        )

        result = run_task.safe_run_hooks(
            adapter=adapter,
            hook_type=RunHookType.End,
            extra_context={},
        )
        assert result == RunStatus.Success
        assert len(self.span_exporter.get_finished_spans()) == 0

    def test_call_runner(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        model_runner: ModelRunner,
        run_result: RunResult,
    ):
        mocker.patch(
            "dbt.task.run.ModelRunner.run_with_hooks"
        ).return_value = run_result
        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None

        run_task = RunTask(
            args=flags,
            config=runtime_config,
            manifest=manifest,
        )

        run_task.call_runner(runner=model_runner)
        exported_spans = self.span_exporter.get_finished_spans()
        assert len(exported_spans) == 1
        assert exported_spans[0].status.status_code == StatusCode.OK

    def test_call_runner_error_sets_span_error(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        model_runner: ModelRunner,
        run_result: RunResult,
    ):
        """error node result must set StatusCode.ERROR on the span."""
        from dbt.artifacts.schemas.results import RunStatus as RS

        run_result_error = RunResult(
            status=RS.Error,
            timing=[],
            thread_id="an_id",
            execution_time=0,
            adapter_response={},
            message="It failed",
            failures=1,
            batch_results=None,
            node=run_result.node,
        )
        mocker.patch(
            "dbt.task.run.ModelRunner.run_with_hooks"
        ).return_value = run_result_error

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        run_task.call_runner(runner=model_runner)
        exported_spans = self.span_exporter.get_finished_spans()
        assert len(exported_spans) == 1
        assert exported_spans[0].status.status_code == StatusCode.ERROR

    def test_call_runner_exception_records_on_span(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        model_runner: ModelRunner,
    ):
        """when run_with_hooks raises, the exception must be recorded on the node
        span and the span marked StatusCode.ERROR."""
        boom = ValueError("boom")
        mocker.patch("dbt.task.run.ModelRunner.run_with_hooks").side_effect = boom

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        run_task.call_runner(runner=model_runner)
        exported_spans = self.span_exporter.get_finished_spans()
        assert len(exported_spans) == 1
        span = exported_spans[0]
        assert span.status.status_code == StatusCode.ERROR
        assert len(span.events) == 1
        event = span.events[0]
        assert event.name == "exception"
        assert event.attributes["exception.type"] == "ValueError"
        assert event.attributes["exception.message"] == "boom"

    def test_call_runner_none_guard(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        run_result: RunResult,
    ):
        """non-relational nodes (e.g. Exposure) must not emit database/schema/
        identifier/materialization span attrs when those values are None; the guard
        in _set_span_attr must silently drop them so the OTel SDK never sees None."""
        exposure = Exposure(
            name="my_exposure",
            resource_type=NodeType.Exposure,
            type=ExposureType.Notebook,
            owner=Owner(email="test@example.com"),
            fqn=["pkg", "exposures", "my_exposure"],
            unique_id="exposure.pkg.my_exposure",
            package_name="pkg",
            path="schema.yml",
            original_file_path="models/schema.yml",
        )

        mock_runner = mock.Mock()
        mock_runner.node = exposure
        mock_runner.run_with_hooks.return_value = run_result

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        run_task.call_runner(runner=mock_runner)
        exported_spans = self.span_exporter.get_finished_spans()
        assert len(exported_spans) == 1
        span = exported_spans[0]

        # None-valued attrs must be absent — the guard must have dropped them
        assert "database" not in span.attributes
        assert "schema" not in span.attributes
        assert "identifier" not in span.attributes
        assert "materialization" not in span.attributes

        # no attribute value should be None
        assert all(v is not None for v in span.attributes.values())

        # Core attrs that ARE set for all nodes must still be present
        assert "node_outcome" in span.attributes
        assert "unique_id" in span.attributes
        assert "name" in span.attributes
        assert "node_type" in span.attributes
        assert "relative_path" in span.attributes

        assert span.status.status_code == StatusCode.OK

    def test_safe_run_hooks_masking_bug_fixed(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        hook_node: HookNode,
    ):
        """when on-run-start hook[0] fails and hook[1] is consequently
        skipped, per-hook child spans capture the correct per-hook outcome at full
        granularity — no aggregate to mask anything.  hook[0] child → hook_outcome
        'error', StatusCode.ERROR; hook[1] child → hook_outcome 'skipped',
        StatusCode.OK (skipped is not an error).  Outer span → StatusCode.ERROR,
        no hook_outcome."""
        import copy

        hook_node2 = copy.deepcopy(hook_node)
        hook_node2.unique_id = "model.test.foo2"
        hook_node2.name = "foo2"

        mocker.patch("dbt.task.run.RunTask.get_hooks_by_type").return_value = [
            hook_node,
            hook_node2,
        ]
        mocker.patch(
            "dbt.task.run.RunTask.get_hook_sql"
        ).return_value = hook_node.raw_code

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        adapter = mock.Mock()
        adapter_execute = mock.Mock()
        adapter_execute.side_effect = DbtRuntimeError("hook failed!")
        adapter.execute = adapter_execute

        # hook[0] errors → hook[1] is skipped
        run_task.safe_run_hooks(
            adapter=adapter,
            hook_type=RunHookType.Start,
            extra_context={},
        )

        exported_spans = self.span_exporter.get_finished_spans()
        # 1 outer span + 2 per-hook child spans
        assert len(exported_spans) == 3

        outer_span = next(s for s in exported_spans if s.name == "on-run-start")
        child_span_0 = next(s for s in exported_spans if s.name == hook_node.unique_id)
        child_span_1 = next(s for s in exported_spans if s.name == hook_node2.unique_id)

        assert outer_span.attributes.get("hook_type") == "on-run-start"
        assert "hook_outcome" not in outer_span.attributes
        assert "node.status" not in outer_span.attributes
        assert outer_span.status.status_code == StatusCode.ERROR

        # hook[0] failed
        assert child_span_0.attributes.get("hook_outcome") == "error"
        assert child_span_0.status.status_code == StatusCode.ERROR

        # hook[1] was skipped
        assert child_span_1.attributes.get("hook_outcome") == "skipped"
        assert child_span_1.status.status_code == StatusCode.OK

    def test_run_emits_invocation_span(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
    ):
        """run() must emit a root 'dbt invocation' span carrying command,
        invocation_id and version."""
        safe_set_invocation_context()

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        flags.write_json = False
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        # Take the "nothing to do" branch so run() needs no adapter/database.
        mocker.patch.object(RunTask, "_runtime_initialize")
        run_task._flattened_nodes = []
        mocker.patch.object(RunTask, "task_end_messages")

        run_task.run()

        exported_spans = self.span_exporter.get_finished_spans()
        invocation_span = next(s for s in exported_spans if s.name == "dbt.invocation")

        assert invocation_span.attributes["command"] == get_flags().WHICH
        assert invocation_span.attributes["invocation_id"] == get_invocation_id()
        assert invocation_span.attributes["version"] == __version__

    def test_run_no_invocation_span_when_otel_disabled(
        self,
        mocker: MockerFixture,
        monkeypatch,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
    ):
        """with the gate off, run() must emit no spans at all."""
        monkeypatch.setattr("dbt.task.runnable._otel_enabled", lambda: False)
        safe_set_invocation_context()

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        flags.write_json = False
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        mocker.patch.object(RunTask, "_runtime_initialize")
        run_task._flattened_nodes = []
        mocker.patch.object(RunTask, "task_end_messages")

        run_task.run()

        assert len(self.span_exporter.get_finished_spans()) == 0

    def test_run_invocation_span_is_current_during_execution(
        self,
        mocker: MockerFixture,
        runtime_config: RuntimeConfig,
        manifest: Manifest,
        model_runner: ModelRunner,
    ):
        """the invocation span must be the current span while execute_with_hooks
        runs — that is what makes _submit's context.get_current() capture parent
        node and hook spans under it."""
        safe_set_invocation_context()

        flags = mock.Mock()
        flags.state = None
        flags.defer_state = None
        flags.write_json = False
        run_task = RunTask(args=flags, config=runtime_config, manifest=manifest)

        mocker.patch.object(RunTask, "_runtime_initialize")
        run_task._flattened_nodes = [model_runner.node]
        mocker.patch.object(RunTask, "task_end_messages")

        captured = {}

        def fake_execute_with_hooks(selected_uids):
            current = trace.get_current_span()
            captured["span_id"] = current.get_span_context().span_id
            captured["trace_id"] = current.get_span_context().trace_id
            captured["name"] = getattr(current, "name", None)
            return mock.Mock(results=[])

        mocker.patch.object(
            RunTask, "execute_with_hooks", side_effect=fake_execute_with_hooks
        )

        run_task.run()

        exported_spans = self.span_exporter.get_finished_spans()
        invocation_span = next(s for s in exported_spans if s.name == "dbt.invocation")

        assert captured["name"] == "dbt.invocation"
        assert captured["span_id"] == invocation_span.context.span_id
        assert captured["trace_id"] == invocation_span.context.trace_id


class TestRowsAffected:
    """Adapters do not agree on the type or the sentinel, and instrumentation must
    never raise on either."""

    @pytest.mark.parametrize("value", [0, 1, 42, "7"])
    def test_keeps_real_counts(self, value):
        assert _rows_affected({"rows_affected": value}) == int(value)

    @pytest.mark.parametrize(
        "value",
        [
            -1,
            # materialized_view_execute_no_op stores the sentinel as a string.
            "-1",
        ],
    )
    def test_drops_the_no_row_count_sentinel(self, value):
        assert _rows_affected({"rows_affected": value}) is None

    @pytest.mark.parametrize("value", [None, "", "abc", object()])
    def test_drops_values_that_are_not_numbers(self, value):
        assert _rows_affected({"rows_affected": value}) is None

    def test_drops_a_missing_key(self):
        assert _rows_affected({}) is None
