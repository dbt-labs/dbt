import io
import json
import re
import threading
from dataclasses import dataclass
from typing import (
    TYPE_CHECKING,
    Any,
    Collection,
    Dict,
    List,
    Optional,
    Tuple,
    Type,
    Union,
)

import daff

from dbt.adapters.exceptions import MissingMaterializationError
from dbt.artifacts.schemas.catalog import PrimitiveDict
from dbt.artifacts.schemas.results import TestStatus
from dbt.artifacts.schemas.run import RunResult
from dbt.clients.jinja import MacroGenerator
from dbt.context.providers import generate_runtime_model_context
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import (
    GenericTestNode,
    SingularTestNode,
    TestNode,
    UnitTestDefinition,
    UnitTestNode,
)
from dbt.events.types import LogStartLine, LogTestResult
from dbt.exceptions import BooleanError, DbtInternalError
from dbt.flags import get_flags
from dbt.graph import ResourceTypeSelector
from dbt.node_types import TEST_NODE_TYPES, NodeType
from dbt.parser.unit_tests import UnitTestManifestLoader
from dbt.task import group_lookup
from dbt.task.base import BaseRunner, resource_types_from_args
from dbt.task.compile import CompileRunner
from dbt.task.run import RunTask
from dbt.utils import _coerce_decimal, strtobool
from dbt_common.dataclass_schema import dbtClassMixin
from dbt_common.events.format import pluralize
from dbt_common.events.functions import fire_event
from dbt_common.exceptions import DbtBaseException, DbtRuntimeError
from dbt_common.ui import green, red

if TYPE_CHECKING:
    import agate


@dataclass
class UnitTestDiff(dbtClassMixin):
    actual: List[Dict[str, Any]]
    expected: List[Dict[str, Any]]
    rendered: str


@dataclass
class TestResultData(dbtClassMixin):
    failures: int
    should_warn: bool
    should_error: bool
    adapter_response: Dict[str, Any]

    @classmethod
    def validate(cls, data):
        data["should_warn"] = cls.convert_bool_type(data["should_warn"])
        data["should_error"] = cls.convert_bool_type(data["should_error"])
        super().validate(data)

    def convert_bool_type(field) -> bool:
        # if it's type string let python decide if it's a valid value to convert to bool
        if isinstance(field, str):
            try:
                return bool(strtobool(field))  # type: ignore
            except ValueError:
                raise BooleanError(field, "get_test_sql")

        # need this so we catch both true bools and 0/1
        return bool(field)


@dataclass
class UnitTestResultData(dbtClassMixin):
    should_error: bool
    adapter_response: Dict[str, Any]
    diff: Optional[UnitTestDiff] = None


class TestRunner(CompileRunner):
    _ANSI_ESCAPE = re.compile(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")

    def describe_node_name(self) -> str:
        if self.node.resource_type == NodeType.Unit:
            name = f"{self.node.model}::{self.node.versioned_name}"
            return name
        else:
            return self.node.name

    def describe_node(self) -> str:
        return f"{self.node.resource_type} {self.describe_node_name()}"

    def print_result_line(self, result):
        model = result.node
        group = group_lookup.get(model.unique_id)
        attached_node = (
            result.node.attached_node if isinstance(result.node, GenericTestNode) else None
        )

        fire_event(
            LogTestResult(
                name=self.describe_node_name(),
                status=str(result.status),
                index=self.node_index,
                num_models=self.num_nodes,
                execution_time=result.execution_time,
                node_info=model.node_info,
                num_failures=result.failures,
                group=group,
                attached_node=attached_node,
            ),
            level=LogTestResult.status_to_level(str(result.status)),
        )

    def print_start_line(self):
        fire_event(
            LogStartLine(
                description=self.describe_node(),
                index=self.node_index,
                total=self.num_nodes,
                node_info=self.node.node_info,
            )
        )

    def before_execute(self) -> None:
        self.print_start_line()

    def execute_data_test(self, data_test: TestNode, manifest: Manifest) -> TestResultData:
        context = generate_runtime_model_context(data_test, self.config, manifest)

        hook_ctx = self.adapter.pre_model_hook(context["config"])

        materialization_macro = manifest.find_materialization_macro_by_name(
            self.config.project_name, data_test.get_materialization(), self.adapter.type()
        )

        if materialization_macro is None:
            raise MissingMaterializationError(
                materialization=data_test.get_materialization(), adapter_type=self.adapter.type()
            )

        if "config" not in context:
            raise DbtInternalError(
                "Invalid materialization context generated, missing config: {}".format(context)
            )

        # generate materialization macro
        macro_func = MacroGenerator(materialization_macro, context)
        try:
            # execute materialization macro
            macro_func()
        finally:
            self.adapter.post_model_hook(context, hook_ctx)

        # load results from context
        # could eventually be returned directly by materialization
        result = context["load_result"]("main")
        table = result["table"]
        num_rows = len(table.rows)
        if num_rows != 1:
            raise DbtInternalError(
                f"dbt internally failed to execute {data_test.unique_id}: "
                f"Returned {num_rows} rows, but expected "
                f"1 row"
            )
        num_cols = len(table.columns)
        if num_cols != 3:
            raise DbtInternalError(
                f"dbt internally failed to execute {data_test.unique_id}: "
                f"Returned {num_cols} columns, but expected "
                f"3 columns"
            )

        test_result_dct: PrimitiveDict = dict(
            zip(
                [column_name.lower() for column_name in table.column_names],
                map(_coerce_decimal, table.rows[0]),
            )
        )
        test_result_dct["adapter_response"] = result["response"].to_dict(omit_none=True)
        TestResultData.validate(test_result_dct)
        return TestResultData.from_dict(test_result_dct)

    def build_unit_test_manifest_from_test(
        self, unit_test_def: UnitTestDefinition, manifest: Manifest
    ) -> Manifest:
        # build a unit test manifest with only the test from this UnitTestDefinition
        loader = UnitTestManifestLoader(manifest, self.config, {unit_test_def.unique_id})
        return loader.load()

    def execute_unit_test(
        self, unit_test_def: UnitTestDefinition, manifest: Manifest
    ) -> Tuple[UnitTestNode, UnitTestResultData]:

        unit_test_manifest = self.build_unit_test_manifest_from_test(unit_test_def, manifest)

        # The unit test node and definition have the same unique_id
        unit_test_node = unit_test_manifest.nodes[unit_test_def.unique_id]
        assert isinstance(unit_test_node, UnitTestNode)

        # Compile the node
        unit_test_node = self.compiler.compile_node(unit_test_node, unit_test_manifest, {})
        assert isinstance(unit_test_node, UnitTestNode)

        # generate_runtime_unit_test_context not strictly needed - this is to run the 'unit'
        # materialization, not compile the node.compiled_code
        context = generate_runtime_model_context(unit_test_node, self.config, unit_test_manifest)

        hook_ctx = self.adapter.pre_model_hook(context["config"])

        materialization_macro = unit_test_manifest.find_materialization_macro_by_name(
            self.config.project_name, unit_test_node.get_materialization(), self.adapter.type()
        )

        if materialization_macro is None:
            raise MissingMaterializationError(
                materialization=unit_test_node.get_materialization(),
                adapter_type=self.adapter.type(),
            )

        if "config" not in context:
            raise DbtInternalError(
                "Invalid materialization context generated, missing config: {}".format(context)
            )

        # generate materialization macro
        macro_func = MacroGenerator(materialization_macro, context)
        try:
            # execute materialization macro
            macro_func()
        except DbtBaseException as e:
            raise DbtRuntimeError(
                f"An error occurred during execution of unit test '{unit_test_def.name}'. "
                f"There may be an error in the unit test definition: check the data types.\n {e}"
            )
        finally:
            self.adapter.post_model_hook(context, hook_ctx)

        # load results from context
        # could eventually be returned directly by materialization
        result = context["load_result"]("main")
        adapter_response = result["response"].to_dict(omit_none=True)
        table = result["table"]
        actual = self._get_unit_test_agate_table(table, "actual")
        expected = self._get_unit_test_agate_table(table, "expected")

        # generate diff, if exists
        should_error, diff = False, None
        daff_diff = self._get_daff_diff(expected, actual)
        if daff_diff.hasDifference():
            should_error = True
            rendered = self._render_daff_diff(daff_diff)
            rendered = f"\n\n{green('actual')} differs from {red('expected')}:\n\n{rendered}\n"

            diff = UnitTestDiff(
                actual=json_rows_from_table(actual),
                expected=json_rows_from_table(expected),
                rendered=rendered,
            )

        unit_test_result_data = UnitTestResultData(
            diff=diff,
            should_error=should_error,
            adapter_response=adapter_response,
        )

        return unit_test_node, unit_test_result_data

    def execute(self, test: Union[TestNode, UnitTestNode], manifest: Manifest):
        if isinstance(test, UnitTestDefinition):
            unit_test_node, unit_test_result = self.execute_unit_test(test, manifest)
            return self.build_unit_test_run_result(unit_test_node, unit_test_result)
        else:
            # Note: manifest here is a normal manifest
            assert isinstance(test, (SingularTestNode, GenericTestNode))
            test_result = self.execute_data_test(test, manifest)
            return self.build_test_run_result(test, test_result)

    def build_test_run_result(self, test: TestNode, result: TestResultData) -> RunResult:
        severity = test.config.severity.upper()
        thread_id = threading.current_thread().name
        num_errors = pluralize(result.failures, "result")
        failures = result.failures
        status = TestStatus.Pass
        message = None
        if severity == "ERROR" and result.should_error:
            status = TestStatus.Fail
            message = f"Got {num_errors}, configured to fail if {test.config.error_if}"
        elif result.should_warn:
            if get_flags().WARN_ERROR or get_flags().WARN_ERROR_OPTIONS.includes(
                LogTestResult.__name__
            ):
                status = TestStatus.Fail
                message = f"Got {num_errors}, configured to fail if {test.config.warn_if}"
            else:
                status = TestStatus.Warn
                message = f"Got {num_errors}, configured to warn if {test.config.warn_if}"

        run_result = RunResult(
            node=test,
            status=status,
            timing=[],
            thread_id=thread_id,
            execution_time=0,
            message=message,
            adapter_response=result.adapter_response,
            failures=failures,
            batch_results=None,
        )
        return run_result

    def build_unit_test_run_result(
        self, test: UnitTestNode, result: UnitTestResultData
    ) -> RunResult:
        thread_id = threading.current_thread().name

        status = TestStatus.Pass
        message = None
        failures = 0
        if result.should_error:
            status = TestStatus.Fail
            message = result.diff.rendered if result.diff else None
            failures = 1

        return RunResult(
            node=test,
            status=status,
            timing=[],
            thread_id=thread_id,
            execution_time=0,
            message=message,
            adapter_response=result.adapter_response,
            failures=failures,
            batch_results=None,
        )

    def after_execute(self, result) -> None:
        self.print_result_line(result)

    def _get_unit_test_agate_table(self, result_table, actual_or_expected: str):
        unit_test_table = result_table.where(
            lambda row: row["actual_or_expected"] == actual_or_expected
        )
        columns = list(unit_test_table.columns.keys())
        columns.remove("actual_or_expected")
        return unit_test_table.select(columns)

    def _get_daff_diff(
        self, expected: "agate.Table", actual: "agate.Table", ordered: bool = False
    ) -> daff.TableDiff:
        # Sort expected and actual inputs prior to creating daff diff to ensure order insensitivity
        # https://github.com/paulfitz/daff/issues/200
        expected_daff_table = daff.PythonTableView(list_rows_from_table(expected, sort=True))
        actual_daff_table = daff.PythonTableView(list_rows_from_table(actual, sort=True))

        flags = daff.CompareFlags()
        flags.ordered = ordered

        alignment = daff.Coopy.compareTables(expected_daff_table, actual_daff_table, flags).align()
        result = daff.PythonTableView([])

        diff = daff.TableDiff(alignment, flags)
        diff.hilite(result)
        return diff

    def _render_daff_diff(self, daff_diff: daff.TableDiff) -> str:
        result = daff.PythonTableView([])
        daff_diff.hilite(result)
        rendered = daff.TerminalDiffRender().render(result)
        # strip colors if necessary
        if not self.config.args.use_colors:
            rendered = self._ANSI_ESCAPE.sub("", rendered)

        return rendered


class TestTask(RunTask):
    """
    Testing:
        Read schema files + custom data tests and validate that
        constraints are satisfied.
    """

    __test__ = False

    def raise_on_first_error(self) -> bool:
        return False

    @property
    def resource_types(self) -> List[NodeType]:
        resource_types: Collection[NodeType] = resource_types_from_args(
            self.args, set(TEST_NODE_TYPES), set(TEST_NODE_TYPES)
        )

        # filter out any non-test node types
        resource_types = [rt for rt in resource_types if rt in TEST_NODE_TYPES]
        return list(resource_types)

    def get_node_selector(self) -> ResourceTypeSelector:
        if self.manifest is None or self.graph is None:
            raise DbtInternalError("manifest and graph must be set to get perform node selection")
        return ResourceTypeSelector(
            graph=self.graph,
            manifest=self.manifest,
            previous_state=self.previous_state,
            resource_types=self.resource_types,
        )

    def get_runner_type(self, _) -> Optional[Type[BaseRunner]]:
        return TestRunner


# This was originally in agate_helper, but that was moved out into dbt_common
def json_rows_from_table(table: "agate.Table") -> List[Dict[str, Any]]:
    "Convert a table to a list of row dict objects"
    output = io.StringIO()
    table.to_json(path=output)  # type: ignore

    return json.loads(output.getvalue())


# This was originally in agate_helper, but that was moved out into dbt_common
def list_rows_from_table(table: "agate.Table", sort: bool = False) -> List[Any]:
    """
    Convert given table to a list of lists, where the first element represents the header

    By default, sort is False and no sort order is applied to the non-header rows of the given table.

    If sort is True, sort the non-header rows hierarchically, treating None values as lower in order.
    Examples:
        * [['a','b','c'],[4,5,6],[1,2,3]] -> [['a','b','c'],[1,2,3],[4,5,6]]
        * [['a','b','c'],[4,5,6],[1,null,3]] -> [['a','b','c'],[1,null,3],[4,5,6]]
        * [['a','b','c'],[4,5,6],[null,2,3]] -> [['a','b','c'],[4,5,6],[null,2,3]]
    """
    header = [col.name for col in table.columns]

    rows = []
    for row in table.rows:
        rows.append(list(row.values()))

    if sort:
        rows = sorted(rows, key=lambda x: [(elem is None, elem) for elem in x])

    return [header] + rows
