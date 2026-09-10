from typing import Set

import pytest
from click import Option

from dbt.cli.flags import convert_config
from dbt.cli.option_types import WarnErrorOptionsType
from dbt.config.utils import build_warn_error_options_v2, partition_warn_error_options
from dbt.events import ALL_EVENT_NAMES
from dbt.events.fusion_warn_error_options import FUSION_WARN_ERROR_OPTION_NAMES
from dbt_common.dataclass_schema import ValidationError
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import add_callback_to_manager
from dbt_common.events.types import Note

# A v2-only grouping keyword and a v2-native error code, neither of which
# is a valid dbt-core event name.
V2_GROUP = "StaticAnalysis"
V2_CODE = "SelectorError"
# A name shared by the v2 engine (an ErrorCode) and dbt-core (an event) -- not v2-only.
SHARED_NAME = "DeprecatedModel"


def _v2_note_catcher() -> EventCatcher:
    catcher = EventCatcher(event_to_catch=Note)
    add_callback_to_manager(catcher.catch)
    return catcher


def _v2_messages(catcher: EventCatcher) -> Set[str]:
    suffix = "specific to the dbt v2 engine."
    return {e.data.msg for e in catcher.caught_events if e.data.msg.endswith(suffix)}


class TestPartitionWarnErrorOptions:
    def test_strips_v2_only_keeps_core_and_typos(self) -> None:
        weo = {
            "error": [SHARED_NAME, V2_CODE, V2_GROUP, "TotallyBogus"],
            "warn": [],
            "silence": ["DbtYamlValidationError"],
        }
        v2_names, core_v2_names = partition_warn_error_options(weo, ALL_EVENT_NAMES)

        assert v2_names == {V2_CODE, V2_GROUP, "DbtYamlValidationError"}
        assert core_v2_names["error"] == [SHARED_NAME, "TotallyBogus"]
        assert core_v2_names["silence"] == []
        # original dict is not mutated
        assert weo["error"] == [SHARED_NAME, V2_CODE, V2_GROUP, "TotallyBogus"]

    def test_noop_when_no_v2_names(self) -> None:
        weo = {"error": [SHARED_NAME], "warn": [], "silence": []}
        v2_names, core_v2_names = partition_warn_error_options(weo, ALL_EVENT_NAMES)
        assert v2_names == set()
        assert core_v2_names["error"] == [SHARED_NAME]

    def test_handles_missing_and_non_list_keys(self) -> None:
        weo = {"error": "all"}  # "all" is a str, not a list
        v2_names, core_v2_names = partition_warn_error_options(weo, ALL_EVENT_NAMES)
        assert v2_names == set()
        assert core_v2_names["error"] == "all"


class TestBuildWarnErrorOptionsV2:
    def test_v2_only_names_stripped_and_noted(self) -> None:
        catcher = _v2_note_catcher()
        weo = {"error": [SHARED_NAME, V2_CODE, V2_GROUP], "warn": [], "silence": []}

        result = build_warn_error_options_v2(weo, ALL_EVENT_NAMES)

        assert result.error == [SHARED_NAME]
        assert _v2_messages(catcher) == {
            f"{V2_CODE} is not being used because it's specific to the dbt v2 engine.",
            f"{V2_GROUP} is not being used because it's specific to the dbt v2 engine.",
        }

    def test_unknown_name_still_raises(self) -> None:
        with pytest.raises(ValidationError, match="not a valid dbt error name"):
            build_warn_error_options_v2(
                {"error": ["TotallyBogus"], "warn": [], "silence": []}, ALL_EVENT_NAMES
            )

    def test_valid_only_config_unchanged(self) -> None:
        result = build_warn_error_options_v2(
            {"error": [SHARED_NAME], "warn": [], "silence": []}, ALL_EVENT_NAMES
        )
        assert result.error == [SHARED_NAME]


class TestWarnErrorOptionsTypeTolerance:
    """The CLI / env-var path: ``--warn-error-options`` and ``DBT_WARN_ERROR_OPTIONS``."""

    def _convert(self, raw: str):
        return WarnErrorOptionsType().convert(raw, Option(["--warn-error-options"]), None)

    def test_v2_only_name_does_not_raise(self) -> None:
        catcher = _v2_note_catcher()
        result = self._convert(f"{{'error': ['{V2_CODE}', '{SHARED_NAME}']}}")
        assert result.error == [SHARED_NAME]
        assert (
            f"{V2_CODE} is not being used because it's specific to the dbt v2 engine."
            in _v2_messages(catcher)
        )

    def test_typo_still_raises(self) -> None:
        with pytest.raises(ValidationError, match="not a valid dbt error name"):
            self._convert("{'error': ['TotallyBogus']}")


class TestConvertConfigTolerance:
    """The ``dbt_project.yml`` / ``profiles.yml`` path via ``flags.convert_config``."""

    def test_v2_only_name_does_not_raise(self) -> None:
        catcher = _v2_note_catcher()
        result = convert_config("warn_error_options", {"error": [V2_GROUP, SHARED_NAME]})
        assert result.error == [SHARED_NAME]
        assert (
            f"{V2_GROUP} is not being used because it's specific to the dbt v2 engine."
            in _v2_messages(catcher)
        )

    def test_typo_still_raises(self) -> None:
        with pytest.raises(ValidationError, match="not a valid dbt error name"):
            convert_config("warn_error_options", {"error": ["TotallyBogus"]})


class TestVendoredFusionNames:
    """Invariants of the vendored v2 engine name set in fusion_warn_error_options.py."""

    def test_sanity_contents(self) -> None:
        # v2-only groups present; the set is disjoint from dbt-core's events,
        # so a name shared with dbt-core (handled as a core event) is absent.
        assert {"StaticAnalysis", "PackageParsingCompatibility"} <= FUSION_WARN_ERROR_OPTION_NAMES
        assert SHARED_NAME not in FUSION_WARN_ERROR_OPTION_NAMES
        assert not (FUSION_WARN_ERROR_OPTION_NAMES & ALL_EVENT_NAMES)
