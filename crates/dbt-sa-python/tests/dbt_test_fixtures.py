"""Shared pytest fixtures, loaded as a plugin (`-p dbt_test_fixtures`).

A module rather than a conftest, so every distribution's test directory can use
these — pytest merges conftests down a single path, and only honors
`pytest_plugins` in the rootdir conftest.

Every fixture project targets duckdb, so the hermetic tier runs a real adapter
with no credentials and tests can assert on artifacts instead of on stdout.
Profiles use `:memory:` unless a test needs warehouse state to survive from one
invocation to the next (see the `layered` fixture).
"""

import contextlib
import os
import shutil
from pathlib import Path

import pytest
from dbt.cli.main import dbtRunner, dbtRunnerResult

TESTS_DIR = Path(__file__).parent
FIXTURES_DIR = TESTS_DIR / "integration" / "fixtures"


@pytest.fixture
def tmp_project(tmp_path):
    """Copy a fixture project into a fresh tmp dir so tests don't share target/ state.

    Copying the same fixture twice in one test yields two independent projects, so
    a test can compare two invocations without them sharing target/ or the duckdb
    file.
    """
    copies: dict[str, int] = {}

    def _copy(name: str) -> Path:
        seq = copies[name] = copies.get(name, 0) + 1
        dst = tmp_path / (name if seq == 1 else f"{name}_{seq}")
        shutil.copytree(FIXTURES_DIR / name, dst)
        return dst

    return _copy


@contextlib.contextmanager
def _chdir(path: Path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


@pytest.fixture
def invoke():
    """Run a command against a project, reusing one runner per test.

    Runs with the cwd set to the project, mirroring the Rust harness's
    CurrentWorkingDirGuard. A duckdb `path:` is resolved relative to the cwd, not
    to --project-dir, so without this a relative path would land in the crate dir
    and every project would share one database.
    """
    runner = dbtRunner()

    def run(project: Path, *args: str, **kwargs) -> dbtRunnerResult:
        argv = [
            *args,
            "--project-dir",
            str(project),
            "--profiles-dir",
            str(project),
        ]
        with _chdir(project):
            return runner.invoke(argv, **kwargs)

    return run


@pytest.fixture
def built_project(tmp_project, invoke):
    """A project whose nodes are all already materialized.

    Selecting a subset makes dbt rebuild only that subset, so upstream relations
    must exist beforehand or the model's SQL hits a missing table. Call twice to
    get two independent built copies.
    """

    def build(name: str = "layered") -> Path:
        project = tmp_project(name)
        result = invoke(project, "build")
        assert result.success, result.exception
        return project

    return build


@pytest.fixture
def unique_ids():
    """The set of unique_ids in a RunResultsArtifact."""

    def get(result) -> set[str]:
        return {r.unique_id for r in result.results}

    return get
