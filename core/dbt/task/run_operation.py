import os
import threading
import traceback
from datetime import datetime

import agate

import dbt.common.exceptions
from dbt.clients.adapter import get_adapter
from dbt.contracts.files import FileHash
from dbt.contracts.graph.nodes import HookNode
from dbt.contracts.results import RunResultsArtifact, RunResult, RunStatus, TimingInfo
from dbt.common.events.functions import fire_event
from dbt.common.events.types import (
    RunningOperationCaughtError,
    RunningOperationUncaughtError,
    LogDebugStackTrace,
)
from dbt.common.exceptions import DbtInternalError
from dbt.node_types import NodeType
from dbt.task.base import ConfiguredTask

RESULT_FILE_NAME = "run_results.json"


class RunOperationTask(ConfiguredTask):
    def _get_macro_parts(self):
        macro_name = self.args.macro
        if "." in macro_name:
            package_name, macro_name = macro_name.split(".", 1)
        else:
            package_name = None

        return package_name, macro_name

    def _run_unsafe(self, package_name, macro_name) -> agate.Table:
        adapter = get_adapter(self.config)

        macro_kwargs = self.args.args

        with adapter.connection_named("macro_{}".format(macro_name)):
            adapter.clear_transaction()
            res = adapter.execute_macro(
                macro_name, project=package_name, kwargs=macro_kwargs, macro_resolver=self.manifest
            )

        return res

    def run(self) -> RunResultsArtifact:
        start = datetime.utcnow()
        self.compile_manifest()

        success = True

        package_name, macro_name = self._get_macro_parts()

        try:
            self._run_unsafe(package_name, macro_name)
        except dbt.common.exceptions.DbtBaseException as exc:
            fire_event(RunningOperationCaughtError(exc=str(exc)))
            fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))
            success = False
        except Exception as exc:
            fire_event(RunningOperationUncaughtError(exc=str(exc)))
            fire_event(LogDebugStackTrace(exc_info=traceback.format_exc()))
            success = False

        end = datetime.utcnow()

        macro = (
            self.manifest.find_macro_by_name(macro_name, self.config.project_name, package_name)
            if self.manifest
            else None
        )

        if macro:
            unique_id = macro.unique_id
            fqn = unique_id.split(".")
        else:
            raise DbtInternalError(
                f"dbt could not find a macro with the name '{macro_name}' in any package"
            )

        run_result = RunResult(
            adapter_response={},
            status=RunStatus.Success if success else RunStatus.Error,
            execution_time=(end - start).total_seconds(),
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
            timing=[TimingInfo(name=macro_name, started_at=start, completed_at=end)],
        )

        results = RunResultsArtifact.from_execution_results(
            generated_at=end,
            elapsed_time=(end - start).total_seconds(),
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

        return results

    @classmethod
    def interpret_results(cls, results):
        return results.results[0].status == RunStatus.Success
