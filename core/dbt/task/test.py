from distutils.util import strtobool

import agate
import daff
import io
import json
import re
from dataclasses import dataclass
from dbt.utils import _coerce_decimal
from dbt_common.events.format import pluralize
from dbt_common.dataclass_schema import dbtClassMixin
import threading
from typing import Dict, Any, Optional, Union, List

from .compile import CompileRunner
from .run import RunTask

from dbt.contracts.graph.nodes import TestNode, UnitTestDefinition, UnitTestNode
from dbt.contracts.graph.manifest import Manifest
from dbt.artifacts.results import TestStatus
from dbt.artifacts.run import RunResult
from dbt.artifacts.catalog import PrimitiveDict
from dbt.context.providers import generate_runtime_model_context
from dbt.clients.jinja import MacroGenerator
from dbt_common.events.functions import fire_event
from dbt.events.types import (
    LogTestResult,
    LogStartLine,
)
from dbt.exceptions import (
    DbtInternalError,
    BooleanError,
)
from ..adapters.exceptions import MissingMaterializationError
from dbt.graph import (
    ResourceTypeSelector,
)
from dbt.node_types import NodeType
from dbt.parser.unit_tests import UnitTestManifestLoader
from dbt.flags import get_flags
from dbt_common.ui import green, red


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

    def describe_node(self):
        return f"{self.node.resource_type} {self.node.name}"

    def print_result_line(self, result):
        model = result.node

        fire_event(
            LogTestResult(
                name=model.name,
                status=str(result.status),
                index=self.node_index,
                num_models=self.num_nodes,
                execution_time=result.execution_time,
                node_info=model.node_info,
                num_failures=result.failures,
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

    def before_execute(self):
        self.print_start_line()

    def execute_data_test(self, data_test: TestNode, manifest: Manifest) -> TestResultData:
        context = generate_runtime_model_context(data_test, self.config, manifest)

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
        # execute materialization macro
        macro_func()
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
    ) -> UnitTestResultData:
        # TODO: this should return a list?

        unit_test_manifest = self.build_unit_test_manifest_from_test(unit_test_def, manifest)

        # The unit test node and definition have the same unique_id
        unit_test_node = unit_test_manifest.nodes[unit_test_def.unique_id]
        assert isinstance(unit_test_node, UnitTestNode)

        # Compile the node
        unit_test_node = self.compiler.compile_node(unit_test_node, unit_test_manifest, {})

        # generate_runtime_unit_test_context not strictly needed - this is to run the 'unit'
        # materialization, not compile the node.compiled_code
        context = generate_runtime_model_context(unit_test_node, self.config, unit_test_manifest)

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
        # execute materialization macro
        macro_func()
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
            rendered = f"\n\n{red('expected')} differs from {green('actual')}:\n\n{rendered}\n"

            diff = UnitTestDiff(
                actual=json_rows_from_table(actual),
                expected=json_rows_from_table(expected),
                rendered=rendered,
            )

        return UnitTestResultData(
            diff=diff,
            should_error=should_error,
            adapter_response=adapter_response,
        )

    def execute(self, test: Union[TestNode, UnitTestDefinition], manifest: Manifest):
        if isinstance(test, UnitTestDefinition):
            # TODO: There is one unit test node but it may be linked to multiple versions of
            # the model denoted in the depends_on.  If we process each separatly here we end up with
            # multiple RunResults for the same unit test node but we only handle a single run result
            # upstream.  Should I override the run method to handle a list of RunResults? Seems risky.
            unit_test_result = self.execute_unit_test(test, manifest)
            # TODO: if above handles multiple unit tests then this should as well.
            return self.build_unit_test_run_result(test, unit_test_result)
        else:
            # Note: manifest here is a normal manifest
            test_result = self.execute_data_test(test, manifest)
            return self.build_test_run_result(test, test_result)

    def build_test_run_result(self, test: TestNode, result: TestResultData) -> RunResult:
        severity = test.config.severity.upper()
        thread_id = threading.current_thread().name
        num_errors = pluralize(result.failures, "result")
        status = None
        message = None
        failures = 0
        if severity == "ERROR" and result.should_error:
            status = TestStatus.Fail
            message = f"Got {num_errors}, configured to fail if {test.config.error_if}"
            failures = result.failures
        elif result.should_warn:
            if get_flags().WARN_ERROR:
                status = TestStatus.Fail
                message = f"Got {num_errors}, configured to fail if {test.config.warn_if}"
            else:
                status = TestStatus.Warn
                message = f"Got {num_errors}, configured to warn if {test.config.warn_if}"
            failures = result.failures
        else:
            status = TestStatus.Pass

        run_result = RunResult(
            node=test,
            status=status,
            timing=[],
            thread_id=thread_id,
            execution_time=0,
            message=message,
            adapter_response=result.adapter_response,
            failures=failures,
        )
        return run_result

    def build_unit_test_run_result(
        self, test: UnitTestDefinition, result: UnitTestResultData
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
            node=test,  # type: ignore
            status=status,
            timing=[],
            thread_id=thread_id,
            execution_time=0,
            message=message,
            adapter_response=result.adapter_response,
            failures=failures,
        )

    def after_execute(self, result):
        self.print_result_line(result)

    def _get_unit_test_agate_table(self, result_table, actual_or_expected: str):
        unit_test_table = result_table.where(
            lambda row: row["actual_or_expected"] == actual_or_expected
        )
        columns = list(unit_test_table.columns.keys())
        columns.remove("actual_or_expected")
        return unit_test_table.select(columns)

    def _get_daff_diff(
        self, expected: agate.Table, actual: agate.Table, ordered: bool = False
    ) -> daff.TableDiff:

        expected_daff_table = daff.PythonTableView(list_rows_from_table(expected))
        actual_daff_table = daff.PythonTableView(list_rows_from_table(actual))

        alignment = daff.Coopy.compareTables(expected_daff_table, actual_daff_table).align()
        result = daff.PythonTableView([])

        flags = daff.CompareFlags()
        flags.ordered = ordered

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


class TestSelector(ResourceTypeSelector):
    def __init__(self, graph, manifest, previous_state) -> None:
        super().__init__(
            graph=graph,
            manifest=manifest,
            previous_state=previous_state,
            resource_types=[NodeType.Test, NodeType.Unit],
        )


class TestTask(RunTask):
    """
    Testing:
        Read schema files + custom data tests and validate that
        constraints are satisfied.
    """

    __test__ = False

    def raise_on_first_error(self):
        return False

    def get_node_selector(self) -> TestSelector:
        if self.manifest is None or self.graph is None:
            raise DbtInternalError("manifest and graph must be set to get perform node selection")
        return TestSelector(
            graph=self.graph,
            manifest=self.manifest,
            previous_state=self.previous_state,
        )

    def get_runner_type(self, _):
        return TestRunner


# This was originally in agate_helper, but that was moved out into dbt_common
def json_rows_from_table(table: agate.Table) -> List[Dict[str, Any]]:
    "Convert a table to a list of row dict objects"
    output = io.StringIO()
    table.to_json(path=output)  # type: ignore

    return json.loads(output.getvalue())


# This was originally in agate_helper, but that was moved out into dbt_common
def list_rows_from_table(table: agate.Table) -> List[Any]:
    "Convert a table to a list of lists, where the first element represents the header"
    rows = [[col.name for col in table.columns]]
    for row in table.rows:
        rows.append(list(row.values()))

    return rows
