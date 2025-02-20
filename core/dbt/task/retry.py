from pathlib import Path

from click import get_current_context
from click.core import ParameterSource

from dbt.artifacts.schemas.results import NodeStatus
from dbt.cli.flags import Flags
from dbt.cli.types import Command as CliCommand
from dbt.config import RuntimeConfig
from dbt.contracts.state import load_result_state
from dbt.flags import get_flags, set_flags
from dbt.graph import GraphQueue
from dbt.parser.manifest import parse_manifest
from dbt.task.base import ConfiguredTask
from dbt.task.build import BuildTask
from dbt.task.clone import CloneTask
from dbt.task.compile import CompileTask
from dbt.task.docs.generate import GenerateTask
from dbt.task.run import RunTask
from dbt.task.run_operation import RunOperationTask
from dbt.task.seed import SeedTask
from dbt.task.snapshot import SnapshotTask
from dbt.task.test import TestTask
from dbt_common.exceptions import DbtRuntimeError

RETRYABLE_STATUSES = {
    NodeStatus.Error,
    NodeStatus.Fail,
    NodeStatus.Skipped,
    NodeStatus.RuntimeErr,
    NodeStatus.PartialSuccess,
}
IGNORE_PARENT_FLAGS = {
    "log_path",
    "output_path",
    "profiles_dir",
    "profiles_dir_exists_false",
    "project_dir",
    "defer_state",
    "deprecated_state",
    "target_path",
    "warn_error",
}

ALLOW_CLI_OVERRIDE_FLAGS = {"vars", "threads"}

TASK_DICT = {
    "build": BuildTask,
    "compile": CompileTask,
    "clone": CloneTask,
    "generate": GenerateTask,
    "seed": SeedTask,
    "snapshot": SnapshotTask,
    "test": TestTask,
    "run": RunTask,
    "run-operation": RunOperationTask,
}

CMD_DICT = {
    "build": CliCommand.BUILD,
    "compile": CliCommand.COMPILE,
    "clone": CliCommand.CLONE,
    "generate": CliCommand.DOCS_GENERATE,
    "seed": CliCommand.SEED,
    "snapshot": CliCommand.SNAPSHOT,
    "test": CliCommand.TEST,
    "run": CliCommand.RUN,
    "run-operation": CliCommand.RUN_OPERATION,
}


class RetryTask(ConfiguredTask):
    def __init__(self, args: Flags, config: RuntimeConfig) -> None:
        # load previous run results
        state_path = args.state or config.target_path
        self.previous_results = load_result_state(
            Path(config.project_root) / Path(state_path) / "run_results.json"
        )
        if not self.previous_results:
            raise DbtRuntimeError(
                f"Could not find previous run in '{state_path}' target directory"
            )
        self.previous_args = self.previous_results.args
        self.previous_command_name = self.previous_args.get("which")

        # Reslove flags and config
        if args.warn_error:
            RETRYABLE_STATUSES.add(NodeStatus.Warn)

        cli_command = CMD_DICT.get(self.previous_command_name)  # type: ignore
        # Remove these args when their default values are present, otherwise they'll raise an exception
        args_to_remove = {
            "resource_types": lambda x: x == [],
            "warn_error_options": lambda x: x == {"exclude": [], "include": []},
        }
        for k, v in args_to_remove.items():
            if k in self.previous_args and v(self.previous_args[k]):
                del self.previous_args[k]
        previous_args = {
            k: v for k, v in self.previous_args.items() if k not in IGNORE_PARENT_FLAGS
        }
        click_context = get_current_context()
        current_args = {
            k: v
            for k, v in args.__dict__.items()
            if k in IGNORE_PARENT_FLAGS
            or (
                click_context.get_parameter_source(k) == ParameterSource.COMMANDLINE
                and k in ALLOW_CLI_OVERRIDE_FLAGS
            )
        }
        combined_args = {**previous_args, **current_args}
        retry_flags = Flags.from_dict(cli_command, combined_args)  # type: ignore
        set_flags(retry_flags)
        retry_config = RuntimeConfig.from_args(args=retry_flags)

        # Parse manifest using resolved config/flags
        manifest = parse_manifest(retry_config, False, True, retry_flags.write_json)  # type: ignore
        super().__init__(args, retry_config, manifest)
        self.task_class = TASK_DICT.get(self.previous_command_name)  # type: ignore

    def run(self):
        unique_ids = {
            result.unique_id
            for result in self.previous_results.results
            if result.status in RETRYABLE_STATUSES
            and not (
                self.previous_command_name != "run-operation"
                and result.unique_id.startswith("operation.")
            )
        }

        # We need this so that re-running of a microbatch model will only rerun
        # batches that previously failed. Note _explicitly_ do no pass the
        # batch info if there were _no_ successful batches previously. This is
        # because passing the batch info _forces_ the microbatch process into
        # _incremental_ model, and it may be that we need to be in full refresh
        # mode which is only handled if previous_batch_results _isn't_ passed for a node
        batch_map = {
            result.unique_id: result.batch_results
            for result in self.previous_results.results
            if result.batch_results is not None
            and len(result.batch_results.successful) != 0
            and len(result.batch_results.failed) > 0
            and not (
                self.previous_command_name != "run-operation"
                and result.unique_id.startswith("operation.")
            )
        }

        class TaskWrapper(self.task_class):
            def get_graph_queue(self):
                new_graph = self.graph.get_subset_graph(unique_ids)
                return GraphQueue(
                    new_graph.graph,
                    self.manifest,
                    unique_ids,
                )

        task = TaskWrapper(
            get_flags(),
            self.config,
            self.manifest,
        )

        if self.task_class == RunTask:
            task.batch_map = batch_map

        return_value = task.run()
        return return_value

    def interpret_results(self, *args, **kwargs):
        return self.task_class.interpret_results(*args, **kwargs)
