import os
from unittest import mock

import yaml

from dbt.artifacts.schemas.results import RunStatus
from dbt.constants import DBT_PROJECT_FILE_NAME, VARS_FILE_NAME
from dbt.task.debug import DebugTask


class TestDebugTaskVarsFile:
    def test_debug_task_load_project_with_vars_file(self, tmp_path):
        """DebugTask._load_project should render dbt_project.yml using vars from vars.yml."""
        vars_path = tmp_path / VARS_FILE_NAME
        vars_data = {
            "vars": {
                "structure": {"marts": {"database": "MY_DB"}},
                "my_profile": "custom_profile",
            }
        }
        with open(vars_path, "w") as f:
            yaml.dump(vars_data, f)

        project_path = tmp_path / DBT_PROJECT_FILE_NAME
        project_data = {
            "name": "my_project",
            "version": "1.0.0",
            "profile": "{{ var('my_profile') }}",
            "models": {
                "my_project": {
                    "marts": {
                        "+database": "{{ var('structure')['marts']['database'] }}",
                    }
                }
            },
        }
        with open(project_path, "w") as f:
            yaml.dump(project_data, f)

        task = DebugTask.__new__(DebugTask)
        task.project_dir = str(tmp_path)
        task.project_path = str(project_path)
        task.cli_vars = {}
        task.profile = None
        task.raw_profile_data = {"custom_profile": {}}
        task.args = mock.Mock(VERSION_CHECK=False, profile=None)

        with mock.patch("dbt.config.project.get_flags") as mock_flags:
            mock_flags.return_value = mock.Mock(TARGET_PATH=None, LOG_PATH="logs")
            status = task._load_project()

            assert status.run_status == RunStatus.Success
            assert task.project.vars.to_dict() == {
                "structure": {"marts": {"database": "MY_DB"}},
                "my_profile": "custom_profile",
            }
            assert task.project.models["my_project"]["marts"]["+database"] == "MY_DB"

    def test_debug_task_choose_profile_names_with_vars_file(self, tmp_path):
        """DebugTask._choose_profile_names should render profile name using vars from vars.yml."""
        vars_path = tmp_path / VARS_FILE_NAME
        vars_data = {"vars": {"my_profile": "custom_profile"}}
        with open(vars_path, "w") as f:
            yaml.dump(vars_data, f)

        project_path = tmp_path / DBT_PROJECT_FILE_NAME
        project_data = {
            "name": "my_project",
            "version": "1.0.0",
            "profile": "{{ var('my_profile') }}",
        }
        with open(project_path, "w") as f:
            yaml.dump(project_data, f)

        task = DebugTask.__new__(DebugTask)
        task.project_dir = str(tmp_path)
        task.project_path = str(project_path)
        task.cli_vars = {}
        task.profile = None
        task.raw_profile_data = {"custom_profile": {}}
        task.args = mock.Mock(VERSION_CHECK=False, profile=None)

        profiles, _ = task._choose_profile_names()
        assert profiles == ["custom_profile"]

    def test_debug_task_load_project_cli_vars_override_vars_file(self, tmp_path):
        """DebugTask._load_project should allow CLI vars to override vars from vars.yml."""
        vars_path = tmp_path / VARS_FILE_NAME
        vars_data = {
            "vars": {
                "structure": {"marts": {"database": "MY_DB"}},
                "my_profile": "custom_profile",
            }
        }
        with open(vars_path, "w") as f:
            yaml.dump(vars_data, f)

        project_path = tmp_path / DBT_PROJECT_FILE_NAME
        project_data = {
            "name": "my_project",
            "version": "1.0.0",
            "profile": "{{ var('my_profile') }}",
            "models": {
                "my_project": {
                    "marts": {
                        "+database": "{{ var('structure')['marts']['database'] }}",
                    }
                }
            },
        }
        with open(project_path, "w") as f:
            yaml.dump(project_data, f)

        task = DebugTask.__new__(DebugTask)
        task.project_dir = str(tmp_path)
        task.project_path = str(project_path)
        task.cli_vars = {"structure": {"marts": {"database": "OVERRIDDEN_DB"}}}
        task.profile = None
        task.raw_profile_data = {"custom_profile": {}}
        task.args = mock.Mock(VERSION_CHECK=False, profile=None)

        with mock.patch("dbt.config.project.get_flags") as mock_flags:
            mock_flags.return_value = mock.Mock(TARGET_PATH=None, LOG_PATH="logs")
            status = task._load_project()

            assert status.run_status == RunStatus.Success
            assert task.project.models["my_project"]["marts"]["+database"] == "OVERRIDDEN_DB"

    def test_debug_task_load_project_without_vars_file(self, tmp_path):
        """DebugTask._load_project should work when no vars.yml is present."""
        project_path = tmp_path / DBT_PROJECT_FILE_NAME
        project_data = {
            "name": "my_project",
            "version": "1.0.0",
            "profile": "default",
        }
        with open(project_path, "w") as f:
            yaml.dump(project_data, f)

        task = DebugTask.__new__(DebugTask)
        task.project_dir = str(tmp_path)
        task.project_path = str(project_path)
        task.cli_vars = {}
        task.profile = None
        task.raw_profile_data = {"default": {}}
        task.args = mock.Mock(VERSION_CHECK=False, profile=None)

        with mock.patch("dbt.config.project.get_flags") as mock_flags:
            mock_flags.return_value = mock.Mock(TARGET_PATH=None, LOG_PATH="logs")
            status = task._load_project()

            assert status.run_status == RunStatus.Success
            assert task.project.vars.to_dict() == {}
