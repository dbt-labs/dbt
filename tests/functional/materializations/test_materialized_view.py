import pytest

from dbt.tests.adapter.materialized_views import (
    test_basic,
    test_on_configuration_change,
)


def update_indexes(project, relation):
    """TODO: get the model config file and update it to change the index"""
    pass


class TestBasic(test_basic.BasicTestsBase):
    @pytest.mark.skip("This fails because we are mocking with a traditional view")
    def test_updated_base_table_data_only_shows_in_materialized_view_after_rerun(self, project):
        pass


class TestOnConfigurationChangeApply(
    test_on_configuration_change.OnConfigurationChangeApplyTestsBase
):
    def apply_configuration_change_triggering_apply(self, project):
        update_indexes(project, self.materialized_view)

    def apply_configuration_change_triggering_full_refresh(self, project):
        """There are no monitored changes that trigger a full refresh"""
        pass

    @pytest.mark.skip(
        "This fails because there are no monitored changes that trigger a full refresh"
    )
    def test_full_refresh_configuration_changes_will_not_attempt_apply_configuration_changes(
        self, project
    ):
        pass


class TestOnConfigurationChangeSkip(
    test_on_configuration_change.OnConfigurationChangeSkipTestsBase
):
    def apply_configuration_change_triggering_apply(self, project):
        update_indexes(project, self.materialized_view)

    def apply_configuration_change_triggering_full_refresh(self, project):
        """There are no monitored changes that trigger a full refresh"""
        pass


class TestOnConfigurationChangeFail(
    test_on_configuration_change.OnConfigurationChangeFailTestsBase
):
    def apply_configuration_change_triggering_apply(self, project):
        update_indexes(project, self.materialized_view)

    def apply_configuration_change_triggering_full_refresh(self, project):
        """There are no monitored changes that trigger a full refresh"""
        pass
