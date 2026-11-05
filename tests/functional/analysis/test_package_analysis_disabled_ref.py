"""
Regression test for https://github.com/dbt-labs/dbt-core/issues/15415

Package analyses that ref a disabled model must raise during parse. They must
not inherit `models: <package>: +enabled: false` and silently skip ref checks.
"""

import pytest

from dbt.exceptions import CompilationError
from dbt.tests.fixtures.project import write_project_files
from dbt.tests.util import run_dbt

# Minimal package project: one model + one analysis that refs it.
my_package_dbt_project_yml = """
name: 'my_package'
version: '1.0'
config-version: 2

profile: 'default'

model-paths: ["models"]
analysis-paths: ["analyses"]
"""

my_package_model_a_sql = """
select 1 as x
"""

my_package_analysis_sql = """
select x + 1 as y from {{ ref('A') }}
"""


class TestPackageAnalysisRefsDisabledModel:
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project_root):
        # Write a local package beside the root test project (same pattern as
        # other package functional tests, e.g. test_duplicate_model.py).
        local_package_files = {
            "dbt_project.yml": my_package_dbt_project_yml,
            "models": {"A.sql": my_package_model_a_sql},
            "analyses": {"my_analysis.sql": my_package_analysis_sql},
        }
        write_project_files(project_root, "my_package", local_package_files)

    @pytest.fixture(scope="class")
    def packages(self):
        # Root project depends on the local package; `dbt deps` installs it.
        return {"packages": [{"local": "my_package"}]}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        # Disable only package *models*. Package analyses should stay enabled
        # so parse can report the disabled ref.
        return {"models": {"my_package": {"+enabled": False}}}

    def test_package_analysis_reports_disabled_ref(self, project):
        run_dbt(["deps"])

        with pytest.raises(CompilationError) as exc:
            run_dbt(["parse"])

        assert "which is disabled" in str(exc.value)
