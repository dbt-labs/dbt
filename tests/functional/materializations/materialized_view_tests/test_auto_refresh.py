from datetime import datetime

import pytest

from tests.adapter.dbt.tests.adapter.materialized_view.auto_refresh import (
    MaterializedViewAutoRefreshNoChanges,
)

from tests.functional.materializations.materialized_view_tests import files


class TestMaterializedViewAutoRefreshNoChanges(MaterializedViewAutoRefreshNoChanges):
    @pytest.fixture(scope="class", autouse=True)
    def seeds(self):
        yield {"my_seed.csv": files.SEED__MY_SEED}

    @pytest.fixture(scope="class", autouse=True)
    def models(self):
        yield {"auto_refresh_off.sql": files.MODEL__MY_MATERIALIZED_VIEW}

    @pytest.fixture(scope="class", autouse=True)
    def macros(self):
        yield {"postgres__test__last_refresh.sql": files.MACRO__LAST_REFRESH}

    def last_refreshed(self, project, materialized_view: str) -> datetime:
        with project.adapter.connection_named("__test"):
            kwargs = {"schema": project.test_schema, "identifier": materialized_view}
            last_refresh_results = project.adapter.execute_macro(
                "postgres__test__last_refresh", kwargs=kwargs
            )
        last_refresh = last_refresh_results[0].get("last_refresh")
        return last_refresh

    @pytest.mark.skip("Postgres does not support auto refresh.")
    def test_manual_refresh_does_not_occur_when_auto_refresh_is_on(self, project):
        pass
