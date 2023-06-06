from typing import List

import pytest
import yaml

from dbt.tests.util import read_file, write_file, relation_from_name
from dbt.contracts.graph.model_config import OnConfigurationChangeOption
from dbt.contracts.results import RunStatus

from dbt.tests.adapter.materialized_view.base import (
    Base,
    assert_message_in_logs,
)


def get_project_config(project):
    file_yaml = read_file(project.project_root, "dbt_project.yml")
    return yaml.safe_load(file_yaml)


def set_project_config(project, config):
    config_yaml = yaml.safe_dump(config)
    write_file(config_yaml, project.project_root, "dbt_project.yml")


def get_model_file(project, model: str) -> str:
    return read_file(project.project_root, "models", f"{model}.sql")


def set_model_file(project, model: str, model_sql: str):
    write_file(model_sql, project.project_root, "models", f"{model}.sql")


def assert_proper_scenario(
    on_configuration_change,
    results,
    logs,
    status: RunStatus,
    messages_in_logs: List[str] = None,
    messages_not_in_logs: List[str] = None,
):
    assert len(results.results) == 1
    result = results.results[0]

    assert result.node.config.on_configuration_change == on_configuration_change
    assert result.status == status
    for message in messages_in_logs or []:
        assert_message_in_logs(logs, message)
    for message in messages_not_in_logs or []:
        assert_message_in_logs(logs, message, expected_fail=True)


class OnConfigurationChangeBase(Base):

    on_configuration_change = OnConfigurationChangeOption.Apply
    base_materialized_view = "base_materialized_view"

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"models": {"on_configuration_change": str(self.on_configuration_change)}}

    @pytest.fixture(scope="function")
    def alter_message(self, project):
        return f"Applying ALTER to: {relation_from_name(project.adapter, self.base_materialized_view)}"

    @pytest.fixture(scope="function")
    def create_message(self, project):
        return f"Applying CREATE to: {relation_from_name(project.adapter, self.base_materialized_view)}"

    @pytest.fixture(scope="function")
    def refresh_message(self, project):
        return f"Applying REFRESH to: {relation_from_name(project.adapter, self.base_materialized_view)}"

    @pytest.fixture(scope="function")
    def replace_message(self, project):
        return f"Applying REPLACE to: {relation_from_name(project.adapter, self.base_materialized_view)}"

    @pytest.fixture(scope="function")
    def configuration_change_message(self, project):
        return (
            f"Determining configuration changes on: "
            f"{relation_from_name(project.adapter, self.base_materialized_view)}"
        )

    @pytest.fixture(scope="function")
    def configuration_change_continue_message(self, project):
        return (
            f"Configuration changes were identified and `on_configuration_change` "
            f"was set to `continue` for `{relation_from_name(project.adapter, self.base_materialized_view)}`"
        )

    @pytest.fixture(scope="function")
    def configuration_change_fail_message(self, project):
        return (
            f"Configuration changes were identified and `on_configuration_change` "
            f"was set to `fail` for `{relation_from_name(project.adapter, self.base_materialized_view)}`"
        )
