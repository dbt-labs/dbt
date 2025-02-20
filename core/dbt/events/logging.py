import os
from functools import partial
from typing import Callable, List

from dbt.tracking import track_behavior_change_warn
from dbt_common.events.base_types import EventLevel, EventMsg
from dbt_common.events.event_manager_client import (
    add_callback_to_manager,
    add_logger_to_manager,
    cleanup_event_logger,
    get_event_manager,
)
from dbt_common.events.functions import (
    env_scrubber,
    get_capture_stream,
    get_stdout_config,
    make_log_dir_if_missing,
)
from dbt_common.events.logger import LineFormat, LoggerConfig
from dbt_common.invocation import get_invocation_id

# These are the logging events issued by the "clean" command,
# where we can't count on having a log directory. We've removed
# the "class" flags on the events in types.py. If necessary we
# could still use class or method flags, but we'd have to get
# the type class from the msg and then get the information from the class.
_NOFILE_CODES = ["Z012", "Z013", "Z014", "Z015"]


def _line_format_from_str(format_str: str, default: LineFormat) -> LineFormat:
    if format_str == "text":
        return LineFormat.PlainText
    elif format_str == "debug":
        return LineFormat.DebugText
    elif format_str == "json":
        return LineFormat.Json

    return default


def _get_logfile_config(
    log_path: str,
    use_colors: bool,
    line_format: LineFormat,
    level: EventLevel,
    log_file_max_bytes: int,
    log_cache_events: bool = False,
) -> LoggerConfig:
    return LoggerConfig(
        name="file_log",
        line_format=line_format,
        use_colors=use_colors,
        level=level,  # File log is *always* debug level
        scrubber=env_scrubber,
        filter=partial(_logfile_filter, log_cache_events, line_format),
        invocation_id=get_invocation_id(),
        output_file_name=log_path,
        output_file_max_bytes=log_file_max_bytes,
    )


def _logfile_filter(log_cache_events: bool, line_format: LineFormat, msg: EventMsg) -> bool:
    return msg.info.code not in _NOFILE_CODES and not (
        msg.info.name in ["CacheAction", "CacheDumpGraph"] and not log_cache_events
    )


def setup_event_logger(flags, callbacks: List[Callable[[EventMsg], None]] = []) -> None:
    cleanup_event_logger()
    event_manager = get_event_manager()
    event_manager.callbacks = callbacks.copy()
    add_callback_to_manager(track_behavior_change_warn)

    if flags.LOG_LEVEL != "none":
        line_format = _line_format_from_str(flags.LOG_FORMAT, LineFormat.PlainText)
        log_level = (
            EventLevel.ERROR
            if flags.QUIET
            else EventLevel.DEBUG if flags.DEBUG else EventLevel(flags.LOG_LEVEL)
        )
        console_config = get_stdout_config(
            line_format,
            flags.USE_COLORS,
            log_level,
            flags.LOG_CACHE_EVENTS,
        )

        if get_capture_stream():
            # Create second stdout logger to support test which want to know what's
            # being sent to stdout.
            console_config.output_stream = get_capture_stream()
        add_logger_to_manager(console_config)

    if flags.LOG_LEVEL_FILE != "none":
        make_log_dir_if_missing(flags.LOG_PATH)

        # create and add the file logger to the event manager
        log_file = os.path.join(flags.LOG_PATH, "dbt.log")
        log_file_format = _line_format_from_str(flags.LOG_FORMAT_FILE, LineFormat.DebugText)
        log_level_file = EventLevel.DEBUG if flags.DEBUG else EventLevel(flags.LOG_LEVEL_FILE)
        add_logger_to_manager(
            _get_logfile_config(
                log_file,
                flags.USE_COLORS_FILE,
                log_file_format,
                log_level_file,
                flags.LOG_FILE_MAX_BYTES,
            )
        )
