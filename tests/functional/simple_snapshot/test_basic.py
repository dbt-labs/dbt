import pytest
from tests.functional.simple_snapshot.fixtures import (  # noqa: F401
    models,
    seeds,
    macros,
    snapshots_pg,
)
from tests.functional.simple_snapshot.common_tests import (
    basic_snapshot_test,
    basic_ref_test,
)


@pytest.fixture
def snapshots(snapshots_pg):  # noqa: F811
    return snapshots_pg


def test_ref_snapshot(project):
    basic_ref_test(project)


def test_simple_snapshot(project):
    basic_snapshot_test(project)
