import pytest
from click.testing import CliRunner

from dbt.cli.main import cli


@pytest.mark.parametrize("args", [["login"], ["login", "status"]])
def test_unwritable_log_dir_reports_clean_error(args, mocker, tmp_path):
    mocker.patch(
        "dbt.events.logging.make_log_dir_if_missing",
        side_effect=PermissionError(13, "Permission denied"),
    )
    result = CliRunner().invoke(cli, [*args, "--log-path", str(tmp_path / "logs")])

    assert result.exit_code == 2
    assert "Could not create the log directory" in result.output
    assert "Traceback" not in result.output
