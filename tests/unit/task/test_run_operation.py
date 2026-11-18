from argparse import Namespace
from contextlib import contextmanager
from typing import List, Optional
from unittest.mock import MagicMock

import pytest

from dbt.task.run_operation import RunOperationTask


class FakeAdapter:
    """Records the order of commit and connection release around a run-operation."""

    def __init__(self, transaction_open: Optional[bool]) -> None:
        self.calls: List[str] = []
        self.connections = MagicMock()
        if transaction_open is None:
            self.connections.get_if_exists.return_value = None
        else:
            self.connections.get_if_exists.return_value = MagicMock(
                transaction_open=transaction_open
            )
        self.connections.commit.side_effect = lambda: self.calls.append("commit")
        self.execute_macro = MagicMock(return_value="macro result")
        self.execute = MagicMock(return_value=("sql response", None))

    @contextmanager
    def connection_named(self, name):
        self.calls.append(f"acquire {name}")
        try:
            yield
        finally:
            self.calls.append("release")

    def clear_transaction(self) -> None:
        self.calls.append("clear_transaction")


def make_task(mocker, adapter: FakeAdapter, **args) -> RunOperationTask:
    mocker.patch("dbt.task.run_operation.get_adapter", return_value=adapter)
    task = RunOperationTask.__new__(RunOperationTask)
    task.args = Namespace(**args)
    task.config = MagicMock()
    task.manifest = MagicMock()
    return task


class TestRunOperationCommit:
    def test_macro_commits_open_transaction_before_release(self, mocker) -> None:
        adapter = FakeAdapter(transaction_open=True)
        task = make_task(mocker, adapter, args={})

        assert task._run_unsafe(None, "probe_insert") == "macro result"

        assert adapter.calls == [
            "acquire macro_probe_insert",
            "clear_transaction",
            "commit",
            "release",
        ]

    def test_sql_commits_open_transaction_before_release(self, mocker) -> None:
        adapter = FakeAdapter(transaction_open=True)
        task = make_task(mocker, adapter, sql="insert into t values (1)")

        assert task._run_unsafe_sql() == "sql response"

        adapter.execute.assert_called_once_with(
            "insert into t values (1)", auto_begin=True, fetch=False
        )
        assert adapter.calls == [
            "acquire inline_query",
            "clear_transaction",
            "commit",
            "release",
        ]

    @pytest.mark.parametrize("transaction_open", [False, None])
    def test_macro_without_open_transaction_does_not_commit(
        self, mocker, transaction_open
    ) -> None:
        adapter = FakeAdapter(transaction_open=transaction_open)
        task = make_task(mocker, adapter, args={})

        task._run_unsafe(None, "log_only")

        adapter.connections.commit.assert_not_called()

    def test_sql_without_open_transaction_does_not_commit(self, mocker) -> None:
        adapter = FakeAdapter(transaction_open=False)
        task = make_task(mocker, adapter, sql="select 1")

        task._run_unsafe_sql()

        adapter.connections.commit.assert_not_called()

    def test_macro_that_raises_does_not_commit(self, mocker) -> None:
        adapter = FakeAdapter(transaction_open=True)
        adapter.execute_macro.side_effect = RuntimeError("boom")
        task = make_task(mocker, adapter, args={})

        with pytest.raises(RuntimeError, match="boom"):
            task._run_unsafe(None, "probe_insert_then_fail")

        adapter.connections.commit.assert_not_called()
        assert adapter.calls[-1] == "release"

    def test_sql_that_raises_does_not_commit(self, mocker) -> None:
        adapter = FakeAdapter(transaction_open=True)
        adapter.execute.side_effect = RuntimeError("boom")
        task = make_task(mocker, adapter, sql="insert into t values (1); select nope")

        with pytest.raises(RuntimeError, match="boom"):
            task._run_unsafe_sql()

        adapter.connections.commit.assert_not_called()
        assert adapter.calls[-1] == "release"
