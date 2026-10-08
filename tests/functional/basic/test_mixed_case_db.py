import os

import pytest

from dbt.tests.util import get_manifest
from tests.functional.v2_parser_parity.v2_self_parser import run_dbt_for_mode

model_sql = """
  select 1 as id
"""


@pytest.fixture(scope="class")
def models():
    return {"model.sql": model_sql}


@pytest.fixture(scope="class")
def dbt_profile_data(unique_schema):

    return {
        "test": {
            "outputs": {
                "default": {
                    "type": "postgres",
                    "threads": 4,
                    "host": "localhost",
                    "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                    "user": "root",
                    "pass": "password",
                    "dbname": "dbtMixedCase",
                    "schema": unique_schema,
                },
            },
            "target": "default",
        },
    }


@pytest.mark.v2_parser_parity
def test_basic(project_root, project, parser_mode):

    assert project.database == "dbtMixedCase"

    # Tests that a project with a single model works
    results = run_dbt_for_mode(parser_mode, ["run"])
    assert len(results) == 1
    manifest = get_manifest(project_root)
    assert "model.test.model" in manifest.nodes
    # Running a second time works
    results = run_dbt_for_mode(parser_mode, ["run"])
