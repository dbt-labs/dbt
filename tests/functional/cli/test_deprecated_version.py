from typing import List
from unittest import mock

import pytest

import dbt_common.semver as semver
from dbt.cli.main import dbtRunner
from dbt_common.events.base_types import EventLevel, EventMsg

DEPRECATED_VERSION = "1.9.5"


def _deprecated_version() -> semver.VersionSpecifier:
    return semver.VersionSpecifier.from_version_string(DEPRECATED_VERSION)


def _split_deprecated_version_events(events: List[EventMsg]):
    from dbt.deprecated_version import INFO_MSG, WARN_MSG

    warn = [
        e
        for e in events
        if e.info.name == "Note" and WARN_MSG in e.info.msg and e.info.level == "warn"
    ]
    info = [
        e
        for e in events
        if e.info.name == "Note" and INFO_MSG in e.info.msg and e.info.level == "info"
    ]
    return warn, info


class TestVersionFlagWarnsNotInfo:
    @pytest.fixture(scope="class")
    def models(self):
        return {}

    def test_version_flag_fires_warn_not_info(self, project):
        # --version is an eager click option that runs before cli.invoke()/
        # preflight() -- i.e. before dbtRunner's `callbacks` get registered
        # with the event manager via setup_event_logger(). So this path can't
        # be observed through callbacks like the other two tests; assert
        # directly on fire_event instead.
        with mock.patch(
            "dbt.deprecated_version.get_installed_version", side_effect=_deprecated_version
        ), mock.patch("dbt.deprecated_version.fire_event") as fire_event, mock.patch(
            "dbt.cli.params.get_version_information", return_value=""
        ):
            res = dbtRunner().invoke(["--version"])

        assert res.exception is None
        fire_event.assert_called_once()
        (fired_event,), kwargs = fire_event.call_args
        assert type(fired_event).__name__ == "Note"
        assert kwargs.get("level") == EventLevel.WARN


class TestInitWarnsNotInfo:
    @pytest.fixture(scope="class")
    def models(self):
        return {}

    def test_init_fires_warn_not_info(self, project):
        events: List[EventMsg] = []

        with mock.patch(
            "dbt.deprecated_version.get_installed_version", side_effect=_deprecated_version
        ):
            res = dbtRunner(callbacks=[events.append]).invoke(["init", "--skip-profile-setup"])

        assert res.exception is None

        warn, info = _split_deprecated_version_events(events)
        assert len(warn) == 1
        assert len(info) == 0


class TestOtherCommandsInfoNotWarn:
    @pytest.fixture(scope="class")
    def models(self):
        return {}

    def test_other_command_fires_info_not_warn(self, project):
        events: List[EventMsg] = []

        with mock.patch(
            "dbt.deprecated_version.get_installed_version", side_effect=_deprecated_version
        ):
            res = dbtRunner(callbacks=[events.append]).invoke(["run"])

        assert res.exception is None

        warn, info = _split_deprecated_version_events(events)
        assert len(warn) == 0
        assert len(info) == 1
