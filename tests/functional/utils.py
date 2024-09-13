import os
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import List, Optional
from unittest import mock

from dbt.context.providers import BaseResolver
from dbt_common.events.base_types import BaseEvent, EventMsg


@contextmanager
def up_one(return_path: Optional[Path] = None):
    current_path = Path.cwd()
    os.chdir("../")
    try:
        yield
    finally:
        os.chdir(return_path or current_path)


def is_aware(dt: datetime) -> bool:
    return dt.tzinfo is not None and dt.tzinfo.utcoffset(dt) is not None


def patch_microbatch_end_time(dt_str: str):
    dt = datetime.strptime(dt_str, "%Y-%m-%d %H:%M:%S")
    return mock.patch.object(BaseResolver, "_build_end_time", return_value=dt)


@dataclass
class EventCatcher:
    event_to_catch: BaseEvent
    caught_events: List[EventMsg] = field(default_factory=list)

    def catch(self, event: EventMsg):
        if event.info.name == self.event_to_catch.__name__:
            self.caught_events.append(event)

    def flush(self) -> None:
        self.caught_events = []
