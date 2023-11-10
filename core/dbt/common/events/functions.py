from pathlib import Path

from dbt.common.helper_types import WarnErrorOptions
from dbt.common.utils import ForgivingJSONEncoder
from dbt.common.events.base_types import BaseEvent, EventLevel, EventMsg
from dbt.common.events.eventmgr import EventManager, IEventManager
from dbt.common.events.logger import LoggerConfig, LineFormat
from dbt.common.exceptions import scrub_secrets, env_secrets
from dbt.common.events.types import Note
from functools import partial
import json
import os
import sys
from typing import Callable, Dict, List, Optional, TextIO, Union
from types import ModuleType
import uuid
from google.protobuf.json_format import MessageToDict

LOG_VERSION = 3
metadata_vars: Optional[Dict[str, str]] = None
_METADATA_ENV_PREFIX = "DBT_ENV_CUSTOM_ENV_"
# These are the logging events issued by the "clean" command,
# where we can't count on having a log directory. We've removed
# the "class" flags on the events in types.py. If necessary we
# could still use class or method flags, but we'd have to get
# the type class from the msg and then get the information from the class.
nofile_codes = ["Z012", "Z013", "Z014", "Z015"]
WARN_ERROR_OPTIONS = WarnErrorOptions(include=[], exclude=[])
WARN_ERROR = False


def make_log_dir_if_missing(log_path: Union[Path, str]) -> None:
    if isinstance(log_path, str):
        log_path = Path(log_path)
    log_path.mkdir(parents=True, exist_ok=True)


# TODO: make None default
def setup_event_logger(
    flags, callbacks: List[Callable[[EventMsg], None]] = [], event_modules: List[ModuleType] = []
) -> None:
    cleanup_event_logger()
    make_log_dir_if_missing(flags.LOG_PATH)
    EVENT_MANAGER.callbacks = callbacks.copy()
    for event_module in event_modules:
        EVENT_MANAGER.add_event_protos(event_module)

    if flags.LOG_LEVEL != "none":
        line_format = _line_format_from_str(flags.LOG_FORMAT, LineFormat.PlainText)
        log_level = (
            EventLevel.ERROR
            if flags.QUIET
            else EventLevel.DEBUG
            if flags.DEBUG
            else EventLevel(flags.LOG_LEVEL)
        )
        console_config = _get_stdout_config(
            line_format,
            flags.USE_COLORS,
            log_level,
            flags.LOG_CACHE_EVENTS,
        )
        EVENT_MANAGER.add_logger(console_config)

        if _CAPTURE_STREAM:
            # Create second stdout logger to support test which want to know what's
            # being sent to stdout.
            console_config.output_stream = _CAPTURE_STREAM
            EVENT_MANAGER.add_logger(console_config)

    if flags.LOG_LEVEL_FILE != "none":
        # create and add the file logger to the event manager
        log_file = os.path.join(flags.LOG_PATH, "dbt.log")
        log_file_format = _line_format_from_str(flags.LOG_FORMAT_FILE, LineFormat.DebugText)
        log_level_file = EventLevel.DEBUG if flags.DEBUG else EventLevel(flags.LOG_LEVEL_FILE)
        EVENT_MANAGER.add_logger(
            _get_logfile_config(
                log_file,
                flags.USE_COLORS_FILE,
                log_file_format,
                log_level_file,
                flags.LOG_FILE_MAX_BYTES,
            )
        )


def _line_format_from_str(format_str: str, default: LineFormat) -> LineFormat:
    if format_str == "text":
        return LineFormat.PlainText
    elif format_str == "debug":
        return LineFormat.DebugText
    elif format_str == "json":
        return LineFormat.Json

    return default


def _get_stdout_config(
    line_format: LineFormat,
    use_colors: bool,
    level: EventLevel,
    log_cache_events: bool,
) -> LoggerConfig:
    return LoggerConfig(
        name="stdout_log",
        level=level,
        use_colors=use_colors,
        line_format=line_format,
        scrubber=env_scrubber,
        filter=partial(
            _stdout_filter,
            log_cache_events,
            line_format,
        ),
        invocation_id=EVENT_MANAGER.invocation_id,
        output_stream=sys.stdout,
    )


def _stdout_filter(
    log_cache_events: bool,
    line_format: LineFormat,
    msg: EventMsg,
) -> bool:
    return msg.info.name not in ["CacheAction", "CacheDumpGraph"] or log_cache_events


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
        invocation_id=EVENT_MANAGER.invocation_id,
        output_file_name=log_path,
        output_file_max_bytes=log_file_max_bytes,
    )


def _logfile_filter(log_cache_events: bool, line_format: LineFormat, msg: EventMsg) -> bool:
    return msg.info.code not in nofile_codes and not (
        msg.info.name in ["CacheAction", "CacheDumpGraph"] and not log_cache_events
    )


def env_scrubber(msg: str) -> str:
    return scrub_secrets(msg, env_secrets())


def cleanup_event_logger() -> None:
    # Reset to a no-op manager to release streams associated with logs. This is
    # especially important for tests, since pytest replaces the stdout stream
    # during test runs, and closes the stream after the test is over.
    EVENT_MANAGER.loggers.clear()
    EVENT_MANAGER.callbacks.clear()


# Since dbt-rpc does not do its own log setup, and since some events can
# currently fire before logs can be configured by setup_event_logger(), we
# create a default configuration with default settings and no file output.
EVENT_MANAGER: IEventManager = EventManager()
EVENT_MANAGER.add_logger(_get_stdout_config(LineFormat.PlainText, True, EventLevel.INFO, False))

# This global, and the following two functions for capturing stdout logs are
# an unpleasant hack we intend to remove as part of API-ification. The GitHub
# issue #6350 was opened for that work.
_CAPTURE_STREAM: Optional[TextIO] = None


# used for integration tests
def capture_stdout_logs(stream: TextIO) -> None:
    global _CAPTURE_STREAM
    _CAPTURE_STREAM = stream


def stop_capture_stdout_logs() -> None:
    global _CAPTURE_STREAM
    _CAPTURE_STREAM = None


# returns a dictionary representation of the event fields.
# the message may contain secrets which must be scrubbed at the usage site.
def msg_to_json(msg: EventMsg) -> str:
    msg_dict = msg_to_dict(msg)
    raw_log_line = json.dumps(msg_dict, sort_keys=True, cls=ForgivingJSONEncoder)
    return raw_log_line


def msg_to_dict(msg: EventMsg) -> dict:
    msg_dict = dict()
    try:
        msg_dict = MessageToDict(
            msg, preserving_proto_field_name=True, including_default_value_fields=True  # type: ignore
        )
    except Exception as exc:
        event_type = type(msg).__name__
        fire_event(
            Note(msg=f"type {event_type} is not serializable. {str(exc)}"), level=EventLevel.WARN
        )
    # We don't want an empty NodeInfo in output
    if (
        "data" in msg_dict
        and "node_info" in msg_dict["data"]
        and msg_dict["data"]["node_info"]["node_name"] == ""
    ):
        del msg_dict["data"]["node_info"]
    return msg_dict


def warn_or_error(event, node=None) -> None:
    if WARN_ERROR or WARN_ERROR_OPTIONS.includes(type(event).__name__):

        # TODO: resolve this circular import when at top
        from dbt.common.exceptions import EventCompilationError

        raise EventCompilationError(event.message(), node)
    else:
        fire_event(event)


# an alternative to fire_event which only creates and logs the event value
# if the condition is met. Does nothing otherwise.
def fire_event_if(
    conditional: bool, lazy_e: Callable[[], BaseEvent], level: Optional[EventLevel] = None
) -> None:
    if conditional:
        fire_event(lazy_e(), level=level)


# a special case of fire_event_if, to only fire events in our unit/functional tests
def fire_event_if_test(
    lazy_e: Callable[[], BaseEvent], level: Optional[EventLevel] = None
) -> None:
    fire_event_if(conditional=("pytest" in sys.modules), lazy_e=lazy_e, level=level)


# top-level method for accessing the new eventing system
# this is where all the side effects happen branched by event type
# (i.e. - mutating the event history, printing to stdout, logging
# to files, etc.)
def fire_event(e: BaseEvent, level: Optional[EventLevel] = None) -> None:
    EVENT_MANAGER.fire_event(e, level=level)


def get_metadata_vars() -> Dict[str, str]:
    global metadata_vars
    if not metadata_vars:
        metadata_vars = {
            k[len(_METADATA_ENV_PREFIX) :]: v
            for k, v in os.environ.items()
            if k.startswith(_METADATA_ENV_PREFIX)
        }
    return metadata_vars


def reset_metadata_vars() -> None:
    global metadata_vars
    metadata_vars = None


def get_invocation_id() -> str:
    return EVENT_MANAGER.invocation_id


def set_invocation_id() -> None:
    # This is primarily for setting the invocation_id for separate
    # commands in the dbt servers. It shouldn't be necessary for the CLI.
    EVENT_MANAGER.invocation_id = str(uuid.uuid4())


def ctx_set_event_manager(event_manager: IEventManager) -> None:
    global EVENT_MANAGER
    EVENT_MANAGER = event_manager
