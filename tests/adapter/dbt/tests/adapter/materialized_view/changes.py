from typing import Optional

import pytest

from dbt.adapters.base.relation import BaseRelation
from dbt.contracts.graph.model_config import OnConfigurationChangeOption
from dbt.contracts.relation import RelationType
from dbt.tests.util import (
    UtilityMethodNotImplementedError,
    assert_message_in_logs,
    get_model_file,
    run_dbt,
    run_dbt_and_capture,
    set_model_file,
)

from dbt.tests.adapter.materialized_view.files import (
    MY_MATERIALIZED_VIEW,
    MY_SEED,
)


class MaterializedViewChanges:
    """
    Tests change management functionality:

    - apply small changes via alter
    - apply large changes via replace
    - small changes will not be made if full refresh is also happening
    - continue if changes detected and configured to continue
    - full refresh is available even if changes are detected and configured to continue
    - fail if changes are detected and configured to fail
    - full refresh is available even if changes are detected and configured to fail

    To use this class, subclass it and configure it in the adapter. Then create a test class
    that inherits both from the adapter's subclass and one of the mixins below. This avoids needing to
    configure this class three times.
    """

    """
    Configure these
    """

    @staticmethod
    def check_start_state(project, materialized_view):
        """
        Check the starting state; this should align with `files.MY_MATERIALIZED_VIEW`.
        """
        raise UtilityMethodNotImplementedError("MaterializedViewsChanges", "check_start_state")

    @staticmethod
    def change_config_via_alter(project, materialized_view):
        """
        Should be a change that can be applied without dropping the materialized view

        If there are no such changes, inherit the corresponding tests and mark them with
        `@pytest.mark.skip()`.
        """
        pass

    @staticmethod
    def check_state_alter_change_is_applied(project, materialized_view):
        """
        Verify that the changes in `change_config_via_alter` were applied.
        """
        raise UtilityMethodNotImplementedError(
            "MaterializedViewsChanges", "change_config_via_alter"
        )

    @staticmethod
    def change_config_via_replace(project, materialized_view):
        """
        Should be a change that will trigger dropping the materialized view

        If there are no such changes, inherit the corresponding tests and mark them with
        `@pytest.mark.skip()`.
        """
        pass

    @staticmethod
    def check_state_replace_change_is_applied(project, materialized_view):
        """
        Verify that the changes in `change_config_via_replace` were applied.
        This is independent of `check_state_alter_change_is_applied`.
        """
        raise UtilityMethodNotImplementedError(
            "MaterializedViewsChanges", "change_config_via_replace"
        )

    @staticmethod
    def query_relation_type(project, relation: BaseRelation) -> Optional[str]:
        raise UtilityMethodNotImplementedError("MaterializedViewsChanges", "query_relation_type")

    """
    Configure these if needed
    """

    @pytest.fixture(scope="class", autouse=True)
    def seeds(self):
        yield {"my_seed.csv": MY_SEED}

    @pytest.fixture(scope="class", autouse=True)
    def models(self):
        yield {"my_materialized_view.sql": MY_MATERIALIZED_VIEW}

    """
    Don't configure these unless absolutely necessary
    """

    @pytest.fixture(scope="class")
    def my_materialized_view(self, project) -> BaseRelation:
        return project.adapter.Relation.create(
            identifier="my_materialized_view",
            schema=project.test_schema,
            database=project.database,
            type=RelationType.MaterializedView,
        )

    @pytest.fixture(scope="function", autouse=True)
    def setup(self, project, my_materialized_view):

        # make sure the model in the data reflects the files each time
        run_dbt(["seed"])
        run_dbt(["run", "--models", my_materialized_view.identifier, "--full-refresh"])

        # the tests touch these files, store their contents in memory
        initial_model = get_model_file(project, my_materialized_view)

        yield

        # and then reset them after the test runs
        set_model_file(project, my_materialized_view, initial_model)

        # ensure clean slate each method
        project.run_sql(f"drop schema if exists {project.test_schema} cascade")

    def test_full_refresh_occurs_with_changes(self, project, my_materialized_view):
        self.change_config_via_alter(project, my_materialized_view)
        self.change_config_via_replace(project, my_materialized_view)
        _, logs = run_dbt_and_capture(
            ["--debug", "run", "--models", my_materialized_view.identifier, "--full-refresh"]
        )
        assert self.query_relation_type(project, my_materialized_view) == "materialized_view"
        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs, False)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs)

    def test_no_alter_and_no_replace_occurs_with_no_changes(self, project, my_materialized_view):
        # no changes were made to the model
        _, logs = run_dbt_and_capture(
            ["--debug", "run", "--models", my_materialized_view.identifier]
        )
        # no changes were submitted to the database
        assert self.query_relation_type(project, my_materialized_view) == "materialized_view"
        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs, False)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs, False)


class MaterializedViewChangesApplyMixin:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"models": {"on_configuration_change": OnConfigurationChangeOption.Apply.value}}

    def test_change_is_applied_via_alter(self, project, my_materialized_view):
        self.check_start_state(project, my_materialized_view)

        self.change_config_via_alter(project, my_materialized_view)
        _, logs = run_dbt_and_capture(["--debug", "run", "--models", my_materialized_view.name])

        self.check_state_alter_change_is_applied(project, my_materialized_view)

        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs, False)

    def test_change_is_applied_via_replace(self, project, my_materialized_view):
        self.check_start_state(project, my_materialized_view)

        self.change_config_via_alter(project, my_materialized_view)
        self.change_config_via_replace(project, my_materialized_view)
        _, logs = run_dbt_and_capture(["--debug", "run", "--models", my_materialized_view.name])

        self.check_state_alter_change_is_applied(project, my_materialized_view)
        self.check_state_replace_change_is_applied(project, my_materialized_view)

        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs)


class MaterializedViewChangesContinueMixin:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"models": {"on_configuration_change": OnConfigurationChangeOption.Continue.value}}

    def test_change_is_not_applied_via_alter(self, project, my_materialized_view):
        self.check_start_state(project, my_materialized_view)

        self.change_config_via_alter(project, my_materialized_view)
        _, logs = run_dbt_and_capture(["--debug", "run", "--models", my_materialized_view.name])

        self.check_start_state(project, my_materialized_view)

        assert_message_in_logs(
            f"Configuration changes were identified and `on_configuration_change` was set"
            f" to `continue` for `{my_materialized_view}`",
            logs,
        )
        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs, False)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs, False)

    def test_change_is_not_applied_via_replace(self, project, my_materialized_view):
        self.check_start_state(project, my_materialized_view)

        self.change_config_via_alter(project, my_materialized_view)
        self.change_config_via_replace(project, my_materialized_view)
        _, logs = run_dbt_and_capture(["--debug", "run", "--models", my_materialized_view.name])

        self.check_start_state(project, my_materialized_view)

        assert_message_in_logs(
            f"Configuration changes were identified and `on_configuration_change` was set"
            f" to `continue` for `{my_materialized_view}`",
            logs,
        )
        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs, False)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs, False)


class MaterializedViewChangesFailMixin:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"models": {"on_configuration_change": OnConfigurationChangeOption.Fail.value}}

    def test_change_is_not_applied_via_alter(self, project, my_materialized_view):
        self.check_start_state(project, my_materialized_view)

        self.change_config_via_alter(project, my_materialized_view)
        _, logs = run_dbt_and_capture(
            ["--debug", "run", "--models", my_materialized_view.name], expect_pass=False
        )

        self.check_start_state(project, my_materialized_view)

        assert_message_in_logs(
            f"Configuration changes were identified and `on_configuration_change` was set"
            f" to `fail` for `{my_materialized_view}`",
            logs,
        )
        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs, False)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs, False)

    def test_change_is_not_applied_via_replace(self, project, my_materialized_view):
        self.check_start_state(project, my_materialized_view)

        self.change_config_via_alter(project, my_materialized_view)
        self.change_config_via_replace(project, my_materialized_view)
        _, logs = run_dbt_and_capture(
            ["--debug", "run", "--models", my_materialized_view.name], expect_pass=False
        )

        self.check_start_state(project, my_materialized_view)

        assert_message_in_logs(
            f"Configuration changes were identified and `on_configuration_change` was set"
            f" to `fail` for `{my_materialized_view}`",
            logs,
        )
        assert_message_in_logs(f"Applying ALTER to: {my_materialized_view}", logs, False)
        assert_message_in_logs(f"Applying REPLACE to: {my_materialized_view}", logs, False)
