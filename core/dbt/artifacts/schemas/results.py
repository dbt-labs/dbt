from dbt.contracts.graph.nodes import ResultNode
from dbt_common.events.helpers import datetime_to_json_string
from dbt_common.utils import cast_to_str, cast_to_int
from dbt_common.dataclass_schema import dbtClassMixin, StrEnum

from dataclasses import dataclass
from datetime import timezone, datetime
from typing import Any, Callable, Dict, List, Optional, Sequence, Union


@dataclass
class TimingInfo(dbtClassMixin):
    name: str
    started_at: Optional[datetime] = None
    completed_at: Optional[datetime] = None

    def begin(self):
        self.started_at = datetime.now(timezone.utc).replace(tzinfo=None)

    def end(self):
        self.completed_at = datetime.now(timezone.utc).replace(tzinfo=None)

    def to_msg_dict(self):
        msg_dict = {"name": self.name}
        if self.started_at:
            msg_dict["started_at"] = datetime_to_json_string(self.started_at)
        if self.completed_at:
            msg_dict["completed_at"] = datetime_to_json_string(self.completed_at)
        return msg_dict


# This is a context manager
class collect_timing_info:
    def __init__(self, name: str, callback: Callable[[TimingInfo], None]) -> None:
        self.timing_info = TimingInfo(name=name)
        self.callback = callback

    def __enter__(self):
        self.timing_info.begin()

    def __exit__(self, exc_type, exc_value, traceback):
        self.timing_info.end()
        self.callback(self.timing_info)


class RunningStatus(StrEnum):
    Started = "started"
    Compiling = "compiling"
    Executing = "executing"


class NodeStatus(StrEnum):
    Success = "success"
    Error = "error"
    Fail = "fail"
    Warn = "warn"
    Skipped = "skipped"
    Pass = "pass"
    RuntimeErr = "runtime error"


class RunStatus(StrEnum):
    Success = NodeStatus.Success
    Error = NodeStatus.Error
    Skipped = NodeStatus.Skipped


class TestStatus(StrEnum):
    __test__ = False
    Pass = NodeStatus.Pass
    Error = NodeStatus.Error
    Fail = NodeStatus.Fail
    Warn = NodeStatus.Warn
    Skipped = NodeStatus.Skipped


class FreshnessStatus(StrEnum):
    Pass = NodeStatus.Pass
    Warn = NodeStatus.Warn
    Error = NodeStatus.Error
    RuntimeErr = NodeStatus.RuntimeErr


@dataclass
class BaseResult(dbtClassMixin):
    status: Union[RunStatus, TestStatus, FreshnessStatus]
    timing: List[TimingInfo]
    thread_id: str
    execution_time: float
    adapter_response: Dict[str, Any]
    message: Optional[str]
    failures: Optional[int]

    @classmethod
    def __pre_deserialize__(cls, data):
        data = super().__pre_deserialize__(data)
        if "message" not in data:
            data["message"] = None
        if "failures" not in data:
            data["failures"] = None
        return data

    def to_msg_dict(self):
        msg_dict = {
            "status": str(self.status),
            "message": cast_to_str(self.message),
            "thread": self.thread_id,
            "execution_time": self.execution_time,
            "num_failures": cast_to_int(self.failures),
            "timing_info": [ti.to_msg_dict() for ti in self.timing],
            "adapter_response": self.adapter_response,
        }
        return msg_dict


@dataclass
class NodeResult(BaseResult):
    node: ResultNode


@dataclass
class ExecutionResult(dbtClassMixin):
    results: Sequence[BaseResult]
    elapsed_time: float

    def __len__(self):
        return len(self.results)

    def __iter__(self):
        return iter(self.results)

    def __getitem__(self, idx):
        return self.results[idx]


# due to issues with typing.Union collapsing subclasses, this can't subclass
# PartialResult
