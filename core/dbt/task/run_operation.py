import os
import threading
import traceback
from datetime import datetime, timezone
from typing import TYPE_CHECKING, List

import dbt_common.exceptions
from dbt.adapters.factory import get_adapter
from dbt.artifacts.schemas.results import RunStatus, TimingInfo, collect_timing_info
from dbt.artifacts.schemas.run import RunResult, RunResultsArtifact
from dbt.clients.jinja_static import statically_check_has_jinja
from dbt.constants import RUN_RESULTS_FILE_NAME
from dbt.contracts.files import FileHash
from dbt.contracts.graph.nodes import HookNode
from dbt.events.types import (
    ArtifactWritten,
    LogDebugStackTrace,
    RunningOperationCaughtError,
    RunningOperationUncaughtError,
)
from dbt.node_types import NodeType
from dbt.task.base import ConfiguredTask
from dbt_common.events.base_types import EventLevel
from dbt_common.events.format import format_fancy_output_line
from dbt_common.events.functions import fire_event
from dbt_common.events.types import Note
from dbt_common.ui import green, red

if TYPE_CHECKING:
    import agate


class RunOperationTask(ConfiguredTask):
    def _get_macro_parts(self):
        macro_name = self.args.macro
        if "." in macro_name:
            package_name, macro_name = macro_name.split(".", 1)
        else:
            package_name = None

        return package_name, macro_name

    def _run_unsafe(self, package_name, macro_name) -> "agate.Table":
        adapter = get_adapter(self.config)

        macro_kwargs = self.args.args

        with adapter.connection_named("macro_{}".format(macro_name)):
            adapter.clear_transaction()
            res = adapter.execute_macro(
                macro_name, project=package_name, kwargs=macro_kwargs, macro_resolver=self.manifest
            )

        return res

    def _run_unsafe_sql(self):
        adapter = get_adapter(self.config)

        if statically_check_has_jinja(self.args.sql):
            from dbt.parser.manifest import process_node
            from dbt.parser.sql import SqlBlockParser

            block_parser = SqlBlockParser(
                project=self.config, manifest=self.manifest, root_project=self.config
            )
            sql_node = block_parser.parse_remote(self.args.sql, "inline_query")
            process_node(self.config, self.manifest, sql_node)

            compiled_node = self.compiler.compile_node(
                sql_node, self.manifest, extra_context=None, write=False
            )
            sql = compiled_node.compiled_code
        else:
            sql = self.args.sql

        fire_event(
            Note(
                msg=format_fancy_output_line(
                    msg="START executing inline_query",
                    status="RUN",
                    index=1,
                    total=1,
                )
            )
        )

        with adapter.connection_named("inline_query"):
            adapter.clear_transaction()
            response, _ = adapter.execute(sql, auto_begin=True, fetch=False)
            adapter.commit_if_has_connection()

        return response

    def _is_sql(self):
        return bool(getattr(self.args, "sql", None))

    def run(self) -> RunResultsArtifact:
        sql_mode = self._is_sql()
        macro_name = getattr(self.args, "macro", None)

        if not sql_mode and not macro_name:
            raise dbt_common.exceptions.DbtRuntimeError(
                "Either a macro name or --sql must be passed to run-operation"
            )
        if sql_mode and macro_name:
            raise dbt_common.exceptions.DbtRuntimeError(
                "Cannot specify both a macro name and --sql for run-operation"
            )
        if sql_mode and self.args.args:
            raise dbt_common.exceptions.DbtRuntimeError(
                "--args cannot be used with --sql; pass arguments directly in the SQL"
            )

        timing: List[TimingInfo] = []

        # Skip manifest graph compilation for Jinja-free SQL — the
        # manifest is not needed to execute plain SQL directly.
        needs_manifest = not sql_mode or statically_check_has_jinja(self.args.sql)

        with collect_timing_info("compile", timing.append):
            if needs_manifest:
                self.compile_manifest()

        start = timing[0].started_at

        success = True
        error_message = None
        adapter_response = {}

        if sql_mode:
            package_name = self.config.project_name
            operation_name = "inline_query"
        else:
            package_name, operation_name = self._get_macro_parts()

        with collect_timing_info("execute", timing.append):
            try:
                if sql_mode:
                    response = self._run_unsafe_sql()
                    adapter_response = response.to_dict() if response else {}
                else:
                    self._run_unsafe(package_name, operation_name)
            except dbt_common.exceptions.DbtBaseException as exc:
                fire_event(RunningOperationCaughtError(exc=str(exc)))
                fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))
                success = False
                error_message = str(exc)
            except Exception as exc:
                fire_event(RunningOperationUncaughtError(exc=str(exc)))
                fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))
                success = False
                error_message = str(exc)

        end = timing[1].completed_at
        execution_time = (end - start).total_seconds() if start and end else 0.0

        if sql_mode:
            if success:
                status_msg = adapter_response.get("_message") or "OK"
                fire_event(
                    Note(
                        msg=format_fancy_output_line(
                            msg="OK executed inline_query",
                            status=green(status_msg),
                            index=1,
                            total=1,
                            execution_time=execution_time,
                        )
                    ),
                    level=EventLevel.INFO,
                )
            else:
                fire_event(
                    Note(
                        msg=format_fancy_output_line(
                            msg="ERROR executing inline_query",
                            status=red("ERROR"),
                            index=1,
                            total=1,
                            execution_time=execution_time,
                        )
                    ),
                    level=EventLevel.ERROR,
                )

        if sql_mode:
            unique_id = f"{NodeType.SqlOperation}.{package_name}.{operation_name}"
            fqn = unique_id.split(".")
        else:
            macro = (
                self.manifest.find_macro_by_name(
                    operation_name, self.config.project_name, package_name
                )
                if self.manifest
                else None
            )

            if macro:
                unique_id = macro.unique_id
                fqn = unique_id.split(".")
            else:
                raise dbt_common.exceptions.UndefinedMacroError(
                    f"dbt could not find a macro with the name '{operation_name}' in any package"
                )

        run_result = RunResult(
            adapter_response=adapter_response,
            status=RunStatus.Success if success else RunStatus.Error,
            execution_time=execution_time,
            failures=0 if success else 1,
            message=error_message,
            node=HookNode(
                alias=operation_name,
                checksum=FileHash.from_contents(unique_id),
                database=self.config.credentials.database,
                schema=self.config.credentials.schema,
                resource_type=NodeType.Operation,
                fqn=fqn,
                name=operation_name,
                unique_id=unique_id,
                package_name=package_name,
                path="",
                original_file_path="",
            ),
            thread_id=threading.current_thread().name,
            timing=timing,
            batch_results=None,
        )

        results = RunResultsArtifact.from_execution_results(
            generated_at=end or datetime.now(timezone.utc).replace(tzinfo=None),
            elapsed_time=execution_time,
            args={
                k: v
                for k, v in self.args.__dict__.items()
                if k.islower() and type(v) in (str, int, float, bool, list, dict)
            },
            results=[run_result],
        )

        result_path = os.path.join(self.config.project_target_path, RUN_RESULTS_FILE_NAME)

        if self.args.write_json:
            results.write(result_path)
            fire_event(
                ArtifactWritten(
                    artifact_type=results.__class__.__name__, artifact_path=result_path
                )
            )

        return results

    @classmethod
    def interpret_results(cls, results):
        return results.results[0].status == RunStatus.Success
