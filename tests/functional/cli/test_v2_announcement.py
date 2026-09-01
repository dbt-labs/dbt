from unittest import mock

import pytest

from dbt.cli.main import dbtRunner


class TestVersionFlagAnnouncesV2:
    @pytest.fixture(scope="class")
    def models(self):
        return {}

    def test_version_flag_announces_v2(self, project):
        # --version is an eager click option that runs before cli.invoke()/
        # preflight() -- i.e. before dbtRunner's `callbacks` get registered with
        # the event manager via setup_event_logger() -- so assert directly on
        # fire_event rather than through callbacks.
        with mock.patch("dbt.v2_announcement.fire_event") as fire_event, mock.patch(
            "dbt.cli.params.get_version_information", return_value=""
        ):
            res = dbtRunner().invoke(["--version"])

        assert res.exception is None
        fire_event.assert_called_once()
        (fired_event,), _ = fire_event.call_args
        assert type(fired_event).__name__ == "Note"

        from dbt.v2_announcement import V2_AVAILABLE_MSG

        assert fired_event.msg == V2_AVAILABLE_MSG
        assert "docs.getdbt.com/docs/local/install-dbt" in fired_event.msg
        assert "docs.getdbt.com/docs/dbt-versions/dbt-upgrade/upgrading-to-v2" in fired_event.msg

    def test_other_commands_do_not_announce_v2(self, project):
        with mock.patch("dbt.v2_announcement.fire_event") as fire_event:
            res = dbtRunner().invoke(["run"])

        assert res.success
        fire_event.assert_not_called()
