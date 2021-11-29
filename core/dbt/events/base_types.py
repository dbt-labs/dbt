from abc import ABCMeta, abstractmethod, abstractproperty
from dataclasses import dataclass
from datetime import datetime
import json
import os
import threading
from typing import Any, Dict, Optional


# # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # #
# These base types define the _required structure_ for the concrete event #
# types defined in types.py                                               #
# # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # # #


# in preparation for #3977
class TestLevel():
    def level_tag(self) -> str:
        return "test"


class DebugLevel():
    def level_tag(self) -> str:
        return "debug"


class InfoLevel():
    def level_tag(self) -> str:
        return "info"


class WarnLevel():
    def level_tag(self) -> str:
        return "warn"


class ErrorLevel():
    def level_tag(self) -> str:
        return "error"


@dataclass
class ShowException():
    # N.B.:
    # As long as we stick with the current convention of setting the member vars in the
    # `message` method of subclasses, this is a safe operation.
    # If that ever changes we'll want to reassess.
    def __post_init__(self):
        self.exc_info: Any = True
        self.stack_info: Any = None
        self.extra: Any = None


# TODO add exhaustiveness checking for subclasses
# can't use ABCs with @dataclass because of https://github.com/python/mypy/issues/5374
# top-level superclass for all events
class Event(metaclass=ABCMeta):
    # fields that should be on all events with their default implementations
    log_version: int = 1
    ts: Optional[datetime] = None  # use getter for non-optional
    pid: Optional[int] = None  # use getter for non-optional

    # four digit string code that uniquely identifies this type of event
    # uniqueness and valid characters are enforced by tests
    @abstractproperty
    @staticmethod
    def code() -> str:
        raise Exception("code() not implemented for event")

    # do not define this yourself. inherit it from one of the above level types.
    @abstractmethod
    def level_tag(self) -> str:
        raise Exception("level_tag not implemented for Event")

    # Solely the human readable message. Timestamps and formatting will be added by the logger.
    # Must override yourself
    @abstractmethod
    def message(self) -> str:
        raise Exception("msg not implemented for Event")

    # override this method to convert non-json serializable fields to json.
    # for override examples, see existing concrete types.
    #
    # there is no type-level mechanism to have mypy enforce json serializability, so we just try
    # to serialize and raise an exception at runtime when that fails. This safety mechanism
    # only works if we have attempted to serialize every concrete event type in our tests.
    def fields_to_json(self, field_value: Any) -> Any:
        try:
            json.dumps(field_value, sort_keys=True)
            return field_value
        except TypeError:
            val_type = type(field_value).__name__
            event_type = type(self).__name__
            return Exception(
                f"type {val_type} is not serializable to json."
                f" First make sure that the call sites for {event_type} match the type hints"
                f" and if they do, you can override Event::fields_to_json in {event_type} in"
                " types.py to define your own serialization function to any valid json type"
            )

    # exactly one time stamp per concrete event
    def get_ts(self) -> datetime:
        if not self.ts:
            self.ts = datetime.now()
        return self.ts

    # exactly one pid per concrete event
    def get_pid(self) -> int:
        if not self.pid:
            self.pid = os.getpid()
        return self.pid
    
    def node_info(self, state: str) -> Dict:
    #          Examples
    # "node_info": 
    #     {"node_path": "models/slow.sql", 
    #     "node_name": "slow", 
    #     "resource_type": "model", 
    #     "node_materialized": "table", 
    #     "node_started_at": "2021-10-05T13:17:28.018613", 
    #     "unique_id": "model.my_new_project.slow", 
    #     "run_state": "running"}}
    # "node_info": 
    #     {"node_path": "models/slow.sql", 
    #     "node_name": "slow", 
    #     "resource_type": "model", 
    #     "node_materialized": "table", 
    #     "node_started_at": "2021-10-05T13:17:28.018613", 
    #     "unique_id": "model.my_new_project.slow", 
    #     "node_finished_at": "2021-10-05T13:17:29.134126", 
    #     "node_status": "passed", 
    #     "run_state": "running"}}
        return {
            "node_path": self.path, 
            "node_name": self.name, 
            "resource_type": self.resource_type, 
            "node_materialized": self.materialized, 
            "node_started_at": "TODO", 
            "unique_id": self.unique_id, 
            "node_finished_at": "TODO", 
            "node_status": "TODO", 
            "run_state": state
        }

    # in theory threads can change so we don't cache them.
    def get_thread_name(self) -> str:
        return threading.current_thread().getName()

    @classmethod
    def get_invocation_id(cls) -> str:
        from dbt.events.functions import get_invocation_id
        return get_invocation_id()


class File(Event, metaclass=ABCMeta):
    # Solely the human readable message. Timestamps and formatting will be added by the logger.
    def file_msg(self) -> str:
        # returns the event msg unless overriden in the concrete class
        return self.message()


class Cli(Event, metaclass=ABCMeta):
    # Solely the human readable message. Timestamps and formatting will be added by the logger.
    def cli_msg(self) -> str:
        # returns the event msg unless overriden in the concrete class
        return self.message()
