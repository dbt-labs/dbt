import pytest

from dataclasses import dataclass, field
from dbt.cli.main import dbtRunner
from dbt_common.events.base_types import BaseEvent, EventLevel, EventMsg
from dbt_common.events.types import Note
from dbt.events.types import SpacesInModelNameDeprecation
from typing import Dict, List


@dataclass
class EventCatcher:
    event_to_catch: BaseEvent
    caught_events: List[EventMsg] = field(default_factory=list)

    def catch(self, event: EventMsg):
        if event.info.name == self.event_to_catch.__name__:
            self.caught_events.append(event)


class TestSpacesInModelNamesHappyPath:
    def test_no_warnings_when_no_spaces_in_name(self, project) -> None:
        event_catcher = EventCatcher(SpacesInModelNameDeprecation)
        runner = dbtRunner(callbacks=[event_catcher.catch])
        runner.invoke(["parse"])
        assert len(event_catcher.caught_events) == 0


class TestSpacesInModelNamesSadPath:
    @pytest.fixture(scope="class")
    def models(self) -> Dict[str, str]:
        return {
            "my model.sql": "select 1 as id",
        }

    def tests_warning_when_spaces_in_name(self, project) -> None:
        event_catcher = EventCatcher(SpacesInModelNameDeprecation)
        runner = dbtRunner(callbacks=[event_catcher.catch])
        runner.invoke(["parse"])

        assert len(event_catcher.caught_events) == 1
        event = event_catcher.caught_events[0]
        assert "Model `my model` has spaces in its name. This is deprecated" in event.info.msg
        assert event.info.level == EventLevel.WARN


class TestSpaceInModelNamesWithDebug:
    @pytest.fixture(scope="class")
    def models(self) -> Dict[str, str]:
        return {
            "my model.sql": "select 1 as id",
            "my model2.sql": "select 1 as id",
        }

    def tests_debug_when_spaces_in_name(self, project) -> None:
        spaces_check_catcher = EventCatcher(SpacesInModelNameDeprecation)
        note_catcher = EventCatcher(Note)
        runner = dbtRunner(callbacks=[spaces_check_catcher.catch, note_catcher.catch])
        runner.invoke(["parse"])
        assert len(spaces_check_catcher.caught_events) == 1
        assert len(note_catcher.caught_events) == 1

        spaces_check_catcher = EventCatcher(SpacesInModelNameDeprecation)
        note_catcher = EventCatcher(Note)
        runner = dbtRunner(callbacks=[spaces_check_catcher.catch, note_catcher.catch])
        runner.invoke(["parse", "--debug"])
        assert len(spaces_check_catcher.caught_events) == 2
        assert len(note_catcher.caught_events) == 0
