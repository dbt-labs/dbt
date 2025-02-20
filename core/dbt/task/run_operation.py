import os
import threading
import traceback
from datetime import datetime
from typing import TYPE_CHECKING, List

import dbt_common.exceptions
from dbt.adapters.factory import get_adapter
from dbt.artifacts.schemas.results import RunStatus, TimingInfo, collect_timing_info
from dbt.artifacts.schemas.run import RunResult, RunResultsArtifact
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
from dbt_common.events.functions import fire_event

RESULT_FILE_NAME = "run_results.json"


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

        if isinstance(res, str):
            return None

        return res

    def run(self) -> RunResultsArtifact:
        timing: List[TimingInfo] = []

        with collect_timing_info("compile", timing.append):
            self.compile_manifest()

        start = timing[0].started_at

        success = True
        package_name, macro_name = self._get_macro_parts()
        execute_macro_result = None

        with collect_timing_info("execute", timing.append):
            try:
                execute_macro_result = self._run_unsafe(package_name, macro_name)
            except dbt_common.exceptions.DbtBaseException as exc:
                fire_event(RunningOperationCaughtError(exc=str(exc)))
                fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))
                success = False
            except Exception as exc:
                fire_event(RunningOperationUncaughtError(exc=str(exc)))
                fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))
                success = False

        end = timing[1].completed_at

        macro = (
            self.manifest.find_macro_by_name(macro_name, self.config.project_name, package_name)
            if self.manifest
            else None
        )

        if macro:
            unique_id = macro.unique_id
            fqn = unique_id.split(".")
        else:
            raise dbt_common.exceptions.UndefinedMacroError(
                f"dbt could not find a macro with the name '{macro_name}' in any package"
            )

        execution_time = (end - start).total_seconds() if start and end else 0.0

        run_result = RunResult(
            adapter_response={},
            status=RunStatus.Success if success else RunStatus.Error,
            execution_time=execution_time,
            failures=0 if success else 1,
            message=None,
            node=HookNode(
                alias=macro_name,
                checksum=FileHash.from_contents(unique_id),
                database=self.config.credentials.database,
                schema=self.config.credentials.schema,
                resource_type=NodeType.Operation,
                fqn=fqn,
                name=macro_name,
                unique_id=unique_id,
                package_name=package_name,
                path="",
                original_file_path="",
            ),
            thread_id=threading.current_thread().name,
            timing=timing,
            agate_table=execute_macro_result,
            batch_results=None,
        )

        results = RunResultsArtifact.from_execution_results(
            generated_at=end or datetime.utcnow(),
            elapsed_time=execution_time,
            args={
                k: v
                for k, v in self.args.__dict__.items()
                if k.islower() and type(v) in (str, int, float, bool, list, dict)
            },
            results=[run_result],
        )

        result_path = os.path.join(self.config.project_target_path, RESULT_FILE_NAME)

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
