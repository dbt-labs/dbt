import dbt.flags
import dbt_common.ui

import json
import logging
import sys
import time
import warnings
from dataclasses import dataclass
from datetime import timezone, datetime
from typing import Optional, List, ContextManager, Callable, Dict, Any, Set

import logbook

from dbt_common.context import get_invocation_context
from dbt_common.dataclass_schema import dbtClassMixin

STDOUT_LOG_FORMAT = "{record.message}"
DEBUG_LOG_FORMAT = "{record.time:%Y-%m-%d %H:%M:%S.%f%z} ({record.thread_name}): {record.message}"


def get_secret_env() -> List[str]:
    return get_invocation_context().env_secrets


ExceptionInformation = str


@dataclass
class LogMessage(dbtClassMixin):
    timestamp: datetime
    message: str
    channel: str
    level: int
    levelname: str
    thread_name: str
    process: int
    extra: Optional[Dict[str, Any]] = None
    exc_info: Optional[ExceptionInformation] = None

    @classmethod
    def from_record_formatted(cls, record: logbook.LogRecord, message: str):
        extra = dict(record.extra)
        log_message = LogMessage(
            timestamp=record.time,
            message=message,
            channel=record.channel,
            level=record.level,
            levelname=logbook.get_level_name(record.level),
            extra=extra,
            thread_name=record.thread_name,
            process=record.process,
            exc_info=record.formatted_exception,
        )
        return log_message


class LogMessageFormatter(logbook.StringFormatter):
    def __call__(self, record, handler):
        data = self.format_record(record, handler)
        exc = self.format_exception(record)
        if exc:
            data.exc_info = exc
        return data

    def format_record(self, record, handler):
        message = super().format_record(record, handler)
        return LogMessage.from_record_formatted(record, message)


class JsonFormatter(LogMessageFormatter):
    def __call__(self, record, handler):
        """Return a the record converted to LogMessage's JSON form"""
        # utils imports exceptions which imports logger...
        import dbt.utils

        log_message = super().__call__(record, handler)
        dct = log_message.to_dict(omit_none=True)
        return json.dumps(dct, cls=dbt.utils.JSONEncoder)


class FormatterMixin:
    def __init__(self, format_string) -> None:
        self._text_format_string = format_string
        self.formatter_class = logbook.StringFormatter
        # triggers a formatter update via logbook.StreamHandler
        self.format_string = self._text_format_string

    def format_json(self):
        # set our formatter to the json formatter
        self.formatter_class = JsonFormatter
        self.format_string = STDOUT_LOG_FORMAT

    def format_text(self):
        # set our formatter to the regular stdout/stderr handler
        self.formatter_class = logbook.StringFormatter
        self.format_string = self._text_format_string

    def reset(self):
        raise NotImplementedError("reset() not implemented in FormatterMixin subclass")


class OutputHandler(logbook.StreamHandler, FormatterMixin):
    """Output handler.

    The `format_string` parameter only changes the default text output, not
      debug mode or json.
    """

    def __init__(
        self,
        stream,
        level=logbook.INFO,
        format_string=STDOUT_LOG_FORMAT,
        bubble=True,
    ) -> None:
        self._default_format = format_string
        logbook.StreamHandler.__init__(
            self,
            stream=stream,
            level=level,
            format_string=format_string,
            bubble=bubble,
        )
        FormatterMixin.__init__(self, format_string)

    def set_text_format(self, format_string: str):
        """Set the text format to format_string. In JSON output mode, this is
        a noop.
        """
        if self.formatter_class is logbook.StringFormatter:
            # reset text format
            self._text_format_string = format_string
            self.format_text()

    def reset(self):
        self.level = logbook.INFO
        self._text_format_string = self._default_format
        self.format_text()

    def should_handle(self, record):
        if record.level < self.level:
            return False
        text_mode = self.formatter_class is logbook.StringFormatter
        if text_mode and record.extra.get("json_only", False):
            return False
        elif not text_mode and record.extra.get("text_only", False):
            return False
        else:
            return True


def _root_channel(record: logbook.LogRecord) -> str:
    return record.channel.split(".")[0]


class Relevel(logbook.Processor):
    def __init__(
        self,
        allowed: List[str],
        min_level=logbook.WARNING,
        target_level=logbook.DEBUG,
    ) -> None:
        self.allowed: Set[str] = set(allowed)
        self.min_level = min_level
        self.target_level = target_level
        super().__init__()

    def process(self, record):
        if _root_channel(record) in self.allowed:
            return
        record.extra["old_level"] = record.level
        # suppress logs at/below our min level by lowering them to NOTSET
        if record.level < self.min_level:
            record.level = logbook.NOTSET
        # if we didn't mess with it, then lower all logs above our level to
        # our target level.
        else:
            record.level = self.target_level


class TextOnly(logbook.Processor):
    def process(self, record):
        record.extra["text_only"] = True


class TimingProcessor(logbook.Processor):
    def __init__(self, timing_info: Optional[dbtClassMixin] = None) -> None:
        self.timing_info = timing_info
        super().__init__()

    def process(self, record):
        if self.timing_info is not None:
            record.extra["timing_info"] = self.timing_info.to_dict(omit_none=True)


class DbtProcessState(logbook.Processor):
    def __init__(self, value: str) -> None:
        self.value = value
        super().__init__()

    def process(self, record):
        overwrite = "run_state" not in record.extra or record.extra["run_state"] == "internal"
        if overwrite:
            record.extra["run_state"] = self.value


class DbtModelState(logbook.Processor):
    def __init__(self, state: Dict[str, str]) -> None:
        self.state = state
        super().__init__()

    def process(self, record):
        record.extra.update(self.state)


class DbtStatusMessage(logbook.Processor):
    def process(self, record):
        record.extra["is_status_message"] = True


class UniqueID(logbook.Processor):
    def __init__(self, unique_id: str) -> None:
        self.unique_id = unique_id
        super().__init__()

    def process(self, record):
        record.extra["unique_id"] = self.unique_id


class NodeCount(logbook.Processor):
    def __init__(self, node_count: int) -> None:
        self.node_count = node_count
        super().__init__()

    def process(self, record):
        record.extra["node_count"] = self.node_count


class NodeMetadata(logbook.Processor):
    def __init__(self, node, index) -> None:
        self.node = node
        self.index = index
        super().__init__()

    def mapping_keys(self):
        return []

    def process_keys(self, record):
        for attr, key in self.mapping_keys():
            value = getattr(self.node, attr, None)
            if value is not None:
                record.extra[key] = value

    def process(self, record):
        self.process_keys(record)
        record.extra["node_index"] = self.index


class ModelMetadata(NodeMetadata):
    def mapping_keys(self):
        return [
            ("alias", "node_alias"),
            ("schema", "node_schema"),
            ("database", "node_database"),
            ("original_file_path", "node_path"),
            ("name", "node_name"),
            ("resource_type", "resource_type"),
            ("depends_on_nodes", "depends_on"),
        ]

    def process_config(self, record):
        if hasattr(self.node, "config"):
            materialized = getattr(self.node.config, "materialized", None)
            if materialized is not None:
                record.extra["node_materialized"] = materialized

    def process(self, record):
        super().process(record)
        self.process_config(record)


class HookMetadata(NodeMetadata):
    def mapping_keys(self):
        return [
            ("name", "node_name"),
            ("resource_type", "resource_type"),
        ]


class TimestampNamed(logbook.Processor):
    def __init__(self, name: str) -> None:
        self.name = name
        super().__init__()

    def process(self, record):
        super().process(record)
        record.extra[self.name] = datetime.now(timezone.utc).replace(tzinfo=None).isoformat()


class ScrubSecrets(logbook.Processor):
    def process(self, record):
        for secret in get_secret_env():
            record.message = str(record.message).replace(secret, "*****")


logger = logbook.Logger("dbt")
# provide this for the cache, disabled by default
CACHE_LOGGER = logbook.Logger("dbt.cache")
CACHE_LOGGER.disable()

warnings.filterwarnings("ignore", category=ResourceWarning, message="unclosed.*<socket.socket.*>")

initialized = False


def make_log_dir_if_missing(log_dir):
    import dbt_common.clients.system

    dbt_common.clients.system.make_directory(log_dir)


class DebugWarnings(logbook.compat.redirected_warnings):
    """Log warnings, except send them to 'debug' instead of 'warning' level."""

    def make_record(self, message, exception, filename, lineno):
        rv = super().make_record(message, exception, filename, lineno)
        rv.level = logbook.DEBUG
        rv.extra["from_warnings"] = True
        return rv


# push Python warnings to debug level logs. This will suppress all import-time
# warnings.
DebugWarnings().__enter__()


class LogManager(logbook.NestedSetup):
    def __init__(self, stdout=sys.stdout, stderr=sys.stderr) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self._null_handler = logbook.NullHandler()
        self._output_handler = OutputHandler(self.stdout)
        self._relevel_processor = Relevel(allowed=["dbt", "werkzeug"])
        self._state_processor = DbtProcessState("internal")
        self._scrub_processor = ScrubSecrets()
        # keep track of whether we've already entered to decide if we should
        # be actually pushing. This allows us to log in main() and also
        # support entering dbt execution via handle_and_check.
        self._stack_depth = 0
        super().__init__(
            [
                self._null_handler,
                self._output_handler,
                self._relevel_processor,
                self._state_processor,
                self._scrub_processor,
            ]
        )

    def push_application(self):
        self._stack_depth += 1
        if self._stack_depth == 1:
            super().push_application()

    def pop_application(self):
        self._stack_depth -= 1
        if self._stack_depth == 0:
            super().pop_application()

    def disable(self):
        self.add_handler(logbook.NullHandler())

    def add_handler(self, handler):
        """add an handler to the log manager that runs before the file handler."""
        self.objects.append(handler)

    def set_path(self, _):
        """No-op that allows dbt-rpc to not break. See GH #7661"""
        pass

    @property
    def initialized(self):
        """Dummy return value for dbt-rpc. See GH#7661"""
        return True

    # this is used by `dbt ls` to allow piping stdout to jq, etc
    def stderr_console(self):
        """Output to stderr at WARNING level instead of stdout"""
        self._output_handler.stream = self.stderr
        self._output_handler.level = logbook.WARNING

    def stdout_console(self):
        """enable stdout and disable stderr"""
        self._output_handler.stream = self.stdout
        self._output_handler.level = logbook.INFO

    def set_debug(self):
        self._output_handler.set_text_format(DEBUG_LOG_FORMAT)
        self._output_handler.level = logbook.DEBUG

    def format_json(self):
        for handler in self.objects:
            if isinstance(handler, FormatterMixin):
                handler.format_json()

    def format_text(self):
        for handler in self.objects:
            if isinstance(handler, FormatterMixin):
                handler.format_text()

    def reset_handlers(self):
        """Reset the handlers to their defaults. This is nice in testing!"""
        self.stdout_console()
        for handler in self.objects:
            if isinstance(handler, FormatterMixin):
                handler.reset()

    def set_output_stream(self, stream, error=None):
        if error is None:
            error = stream

        if self._output_handler.stream is self.stdout:
            self._output_handler.stream = stream
        elif self._output_handler.stream is self.stderr:
            self._output_handler.stream = error

        self.stdout = stream
        self.stderr = error


log_manager = LogManager()


def log_cache_events(flag):
    """Set the cache logger to propagate its messages based on the given flag."""
    # the flag is True if we should log, and False if we shouldn't, so disabled
    # is the inverse.
    CACHE_LOGGER.disabled = not flag


if not dbt.flags.ENABLE_LEGACY_LOGGER:
    logger.disable()
GLOBAL_LOGGER = logger


class LogMessageHandler(logbook.Handler):
    formatter_class = LogMessageFormatter

    def format_logmessage(self, record):
        """Format a LogRecord into a LogMessage"""
        message = self.format(record)
        return LogMessage.from_record_formatted(record, message)


class ListLogHandler(LogMessageHandler):
    def __init__(
        self,
        level: int = logbook.NOTSET,
        filter: Optional[Callable] = None,
        bubble: bool = False,
        lst: Optional[List[LogMessage]] = None,
    ) -> None:
        super().__init__(level, filter, bubble)
        if lst is None:
            lst = []
        self.records: List[LogMessage] = lst

    def should_handle(self, record):
        """Only ever emit dbt-sourced log messages to the ListHandler."""
        if _root_channel(record) != "dbt":
            return False
        return super().should_handle(record)

    def emit(self, record: logbook.LogRecord):
        as_dict = self.format_logmessage(record)
        self.records.append(as_dict)


def _env_log_level(var_name: str) -> int:
    # convert debugging environment variable name to a log level
    if dbt.flags.env_set_truthy(var_name):
        return logging.DEBUG
    else:
        return logging.ERROR


LOG_LEVEL_GOOGLE = _env_log_level("DBT_GOOGLE_DEBUG_LOGGING")
LOG_LEVEL_SNOWFLAKE = _env_log_level("DBT_SNOWFLAKE_CONNECTOR_DEBUG_LOGGING")
LOG_LEVEL_BOTOCORE = _env_log_level("DBT_BOTOCORE_DEBUG_LOGGING")
LOG_LEVEL_HTTP = _env_log_level("DBT_HTTP_DEBUG_LOGGING")
LOG_LEVEL_WERKZEUG = _env_log_level("DBT_WERKZEUG_DEBUG_LOGGING")

logging.getLogger("botocore").setLevel(LOG_LEVEL_BOTOCORE)
logging.getLogger("requests").setLevel(LOG_LEVEL_HTTP)
logging.getLogger("urllib3").setLevel(LOG_LEVEL_HTTP)
logging.getLogger("google").setLevel(LOG_LEVEL_GOOGLE)
logging.getLogger("snowflake.connector").setLevel(LOG_LEVEL_SNOWFLAKE)

logging.getLogger("parsedatetime").setLevel(logging.ERROR)
logging.getLogger("werkzeug").setLevel(LOG_LEVEL_WERKZEUG)


def list_handler(
    lst: Optional[List[LogMessage]],
    level=logbook.NOTSET,
) -> ContextManager:
    """Return a context manager that temporarily attaches a list to the logger."""
    return ListLogHandler(lst=lst, level=level, bubble=True)


def get_timestamp():
    return time.strftime("%H:%M:%S")


def timestamped_line(msg: str) -> str:
    return "{} | {}".format(get_timestamp(), msg)


def print_timestamped_line(msg: str, use_color: Optional[str] = None):
    if use_color is not None:
        msg = dbt_common.ui.color(msg, use_color)

    GLOBAL_LOGGER.info(timestamped_line(msg))
