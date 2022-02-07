import pytest
from tests.util import run_dbt, get_manifest


my_model_sql = """
  select 1 as fun
"""


@pytest.fixture
def models():
    return {'my_model.sql': my_model_sql}


def test_basic(project_root, project):
    # Tests that a project with a single model works
    results = run_dbt(['run'])
    assert len(results) == 1
    manifest = get_manifest(project_root)
    assert 'model.test.my_model' in manifest.nodes
