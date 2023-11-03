from datetime import datetime
from typing import Tuple

import pytest

from dbt.tests.util import UtilityMethodNotImplementedError, run_dbt


class MaterializedViewAutoRefreshNoChanges:
    """
    When dbt runs on a materialized view that has no configuration changes, it can default
    to manually refresh the materialized view. In order to optimize cost and performance,
    there is no need to run a manual refresh if one is already scheduled due to
    auto refresh being turned on. Therefore, we should ensure that a manual refresh
    is only issued if the materialized view does not refresh automatically, and dbt
    otherwise does nothing.

    To implement:
    - override `seeds` and provide a seed for your materialized views
    - override `models` and provide a materialized view auto refresh turned off called "auto_refresh_off.sql"
    - override `last_refreshed` with logic that inspects the platform for the last refresh timestamp

    If your platform supports auto refresh:
    - in `models`, provide another materialized view with auto refresh turned on called "auto_refresh_on.sql"

    If your platform does not support auto refresh:
    - override `test_manual_refresh_does_not_occur_when_auto_refresh_is_on` and mark it with `@pytest.mark.skip`
    """

    @pytest.fixture(scope="class", autouse=True)
    def seeds(self):
        yield {"my_seed.csv": ""}

    @pytest.fixture(scope="class", autouse=True)
    def models(self):
        yield {"auto_refresh_on.sql": "", "auto_refresh_off.sql": ""}

    @pytest.fixture(scope="class", autouse=True)
    def setup(self, project):
        run_dbt(["seed"])
        run_dbt(["run"])
        yield

    def last_refreshed(self, project, materialized_view: str) -> datetime:
        raise UtilityMethodNotImplementedError(
            "MaterializedViewAutoRefreshNoChanges", "last_refreshed"
        )

    def run_dbt_with_no_changes_and_capture_refresh_times(
        self, project, materialized_view: str
    ) -> Tuple[datetime, datetime]:
        last_refresh = self.last_refreshed(project, materialized_view)
        run_dbt(["run", "--models", materialized_view])
        next_refresh = self.last_refreshed(project, materialized_view)
        return last_refresh, next_refresh

    def test_manual_refresh_occurs_when_auto_refresh_is_off(self, project):
        last_refresh, next_refresh = self.run_dbt_with_no_changes_and_capture_refresh_times(
            project, "auto_refresh_off"
        )
        assert next_refresh > last_refresh

    def test_manual_refresh_does_not_occur_when_auto_refresh_is_on(self, project):
        last_refresh, next_refresh = self.run_dbt_with_no_changes_and_capture_refresh_times(
            project, "auto_refresh_on"
        )
        assert next_refresh == last_refresh
