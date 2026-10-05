from argparse import Namespace

import pytest
from pytest_mock import MockerFixture

from dbt.events.logging import setup_event_logger
from dbt.exceptions import DbtRuntimeError
from dbt.flags import get_flags, set_from_args
from dbt_common.events.base_types import BaseEvent
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import get_event_manager
from dbt_common.events.logger import LoggerConfig


class TestSetupEventLogger:
    def test_clears_preexisting_event_manager_state(self) -> None:
        manager = get_event_manager()
        manager.add_logger(LoggerConfig(name="test_logger"))
        manager.callbacks.append(EventCatcher(BaseEvent).catch)
        assert len(manager.loggers) == 1
        assert len(manager.callbacks) == 1

        args = Namespace(log_level="none", log_level_file="none")
        set_from_args(args, {})

        setup_event_logger(get_flags())
        assert len(manager.loggers) == 0
        assert len(manager.callbacks) == 1  # snowplow tracker for behavior flags

    def test_specify_max_bytes(
        self,
        mocker: MockerFixture,
    ) -> None:
        patched_file_handler = mocker.patch("dbt_common.events.logger.RotatingFileHandler")
        args = Namespace(log_file_max_bytes=1234567)
        set_from_args(args, {})
        setup_event_logger(get_flags())
        patched_file_handler.assert_called_once_with(
            filename="logs/dbt.log", encoding="utf8", maxBytes=1234567, backupCount=5
        )

    def test_unwritable_log_dir_raises_clear_error(self, mocker: MockerFixture) -> None:
        mocker.patch(
            "dbt.events.logging.make_log_dir_if_missing",
            side_effect=PermissionError(13, "Permission denied"),
        )
        args = Namespace(log_path="/read/only/logs")
        set_from_args(args, {})

        with pytest.raises(DbtRuntimeError, match="Could not create the log directory"):
            setup_event_logger(get_flags())

        # the console logger is already set up, so the error can be shown to the user
        assert len(get_event_manager().loggers) == 1

    def test_log_dir_not_created_when_file_logging_is_off(self, mocker: MockerFixture) -> None:
        make_dir = mocker.patch("dbt.events.logging.make_log_dir_if_missing")
        args = Namespace(log_level_file="none")
        set_from_args(args, {})

        setup_event_logger(get_flags())
        make_dir.assert_not_called()
