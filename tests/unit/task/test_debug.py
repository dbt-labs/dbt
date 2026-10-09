from unittest import mock

import pytest

from dbt.cli.main import dbtRunner


@pytest.fixture
def debug_project(tmp_path):
    (tmp_path / "profiles.yml").write_text(
        """test:
  target: dev
  outputs:
    dev:
      type: postgres
      host: localhost
      user: root
      password: password
      port: 5432
      dbname: postgres
      schema: public
      threads: 1
"""
    )
    return tmp_path


def invoke_debug(project_dir, project_flags, connection_only=True, connection_error=None):
    (project_dir / "dbt_project.yml").write_text(
        "name: test\nversion: 1.0\nconfig-version: 2\nprofile: test\n" + project_flags
    )
    events = []
    args = [
        "debug",
        "--project-dir",
        str(project_dir),
        "--profiles-dir",
        str(project_dir),
        "--no-send-anonymous-usage-stats",
    ]
    if connection_only:
        args.append("--connection")
    with mock.patch(
        "dbt.task.debug.DebugTask.attempt_connection", return_value=connection_error
    ) as connection:
        result = dbtRunner(callbacks=[events.append]).invoke(args)
    messages = [event.info.msg for event in events]
    return result, messages, connection


@pytest.mark.parametrize(
    "project_flags,error_text,project_fails",
    [
        (
            "flags:\n  require_yaml_configuration_for_mf_time_spines = true\n",
            "is not of type 'object'",
            True,
        ),
        ("flags: [bad]\n", "is not of type 'object'", True),
        ("flags: true\n", "is not of type 'object'", True),
        ("flags: 1\n", "is not of type 'object'", True),
        ("flags: null\n", "is not of type 'object'", True),
        ("flags:\n  warn_error_options: bad\n", "warn_error_options", False),
        (
            "flags:\n  require_yaml_configuration_for_mf_time_spines: bad\n",
            "is not of type 'boolean'",
            False,
        ),
    ],
)
def test_invalid_project_flags_are_logged(debug_project, project_flags, error_text, project_fails):
    result, messages, connection = invoke_debug(debug_project, project_flags)

    assert result.success != project_fails
    assert result.exception is None
    connection.assert_called_once()
    assert any(error_text in message for message in messages)
    log_contents = (debug_project / "logs" / "dbt.log").read_text()
    assert str(debug_project / "dbt_project.yml") in log_contents
    assert error_text in log_contents


def test_full_debug_fails_for_invalid_project_flags(debug_project):
    result, messages, connection = invoke_debug(
        debug_project, "flags: [bad]\n", connection_only=False
    )

    assert not result.success
    connection.assert_called_once()
    assert any("Project loading failed" in message and "flags" in message for message in messages)


def test_connection_debug_still_fails_for_connection_error(debug_project):
    result, messages, connection = invoke_debug(
        debug_project,
        "flags:\n  require_yaml_configuration_for_mf_time_spines: true\n",
        connection_error="Connection refused",
    )

    assert not result.success
    connection.assert_called_once()
    assert not any("Project loading failed" in message for message in messages)
    assert any("Connection refused" in message for message in messages)


def test_connection_debug_accepts_valid_project_flags(debug_project):
    result, messages, connection = invoke_debug(
        debug_project, "flags:\n  require_yaml_configuration_for_mf_time_spines: true\n"
    )

    assert result.success
    connection.assert_called_once()
    assert not any("Project loading failed" in message for message in messages)
