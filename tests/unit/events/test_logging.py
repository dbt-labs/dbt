from argparse import Namespace

import pytest
from pytest_mock import MockerFixture

from dbt.adapters.events.types import CacheDumpGraph
from dbt.events.logging import setup_event_logger
from dbt.flags import get_flags, set_from_args
from dbt_common.events.base_types import BaseEvent, msg_from_base_event
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

    @pytest.mark.parametrize("log_cache_events", [True, False])
    def test_file_log_respects_log_cache_events(
        self,
        mocker: MockerFixture,
        log_cache_events: bool,
    ) -> None:
        mocker.patch("dbt_common.events.logger.RotatingFileHandler")
        args = Namespace(log_cache_events=log_cache_events)
        set_from_args(args, {})
        setup_event_logger(get_flags())

        file_logger = next(
            logger for logger in get_event_manager().loggers if logger.name == "file_log"
        )
        msg = msg_from_base_event(CacheDumpGraph(before_after="before", action="adding", dump={}))
        assert file_logger.filter(msg) == log_cache_events
