from types import SimpleNamespace
from unittest.mock import MagicMock, call, patch

import pytest

from dbt.task.run_operation import RunOperationTask


@pytest.fixture
def task() -> RunOperationTask:
    task = RunOperationTask.__new__(RunOperationTask)
    task.args = SimpleNamespace(sql="select 1")
    task.config = SimpleNamespace()
    return task


def test_inline_sql_commits_before_releasing_connection(task: RunOperationTask) -> None:
    adapter = MagicMock()
    response = object()
    adapter.execute.return_value = (response, None)

    with patch("dbt.task.run_operation.get_adapter", return_value=adapter):
        assert task._run_unsafe_sql() is response

    adapter.assert_has_calls(
        [
            call.connection_named("inline_query"),
            call.connection_named().__enter__(),
            call.clear_transaction(),
            call.execute("select 1", auto_begin=True, fetch=False),
            call.commit_if_has_connection(),
            call.connection_named().__exit__(None, None, None),
        ]
    )


def test_inline_sql_execution_failure_does_not_commit(task: RunOperationTask) -> None:
    adapter = MagicMock()
    adapter.execute.side_effect = RuntimeError("execution failed")

    with patch("dbt.task.run_operation.get_adapter", return_value=adapter):
        with pytest.raises(RuntimeError, match="execution failed"):
            task._run_unsafe_sql()

    adapter.commit_if_has_connection.assert_not_called()
    adapter.connection_named.return_value.__exit__.assert_called_once()


def test_inline_sql_commit_failure_is_not_silenced(task: RunOperationTask) -> None:
    adapter = MagicMock()
    adapter.execute.return_value = (object(), None)
    adapter.commit_if_has_connection.side_effect = RuntimeError("commit failed")

    with patch("dbt.task.run_operation.get_adapter", return_value=adapter):
        with pytest.raises(RuntimeError, match="commit failed"):
            task._run_unsafe_sql()

    adapter.connection_named.return_value.__exit__.assert_called_once()
