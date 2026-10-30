from copy import deepcopy
from itertools import product
from types import SimpleNamespace
from unittest.mock import Mock, patch

import pytest

from dbt.artifacts.resources import NodeConfig
from dbt.context.context_config import UnrenderedConfigGenerator
from dbt_common.contracts.config.base import BaseConfig


@pytest.fixture
def generator():
    project = Mock()
    project.credentials.translate_aliases.side_effect = lambda value: dict(value)
    return UnrenderedConfigGenerator(project)


@pytest.mark.parametrize(
    "scopes, expected",
    [
        ([{"select": "reader"}], {"select": ["reader"]}),
        ([{"+select": "reader"}], {"select": ["reader"]}),
        (
            [{"select": ["outer"]}, {"+select": "middle"}, {"+select": ["inner"]}],
            {"select": ["outer", "middle", "inner"]},
        ),
        (
            [{"select": ["outer"]}, {"+select": ["middle"]}, {"select": "inner"}],
            {"select": ["inner"]},
        ),
        (
            [{"select": ["outer"], "insert": "writer"}, {"select": []}],
            {"select": [], "insert": ["writer"]},
        ),
        ([{"select": ["outer"]}, {}], {"select": ["outer"]}),
        (
            [{"select": "{{ var('reader') }}"}, {"+select": ["inner"]}],
            {"select": ["{{ var('reader') }}", "inner"]},
        ),
    ],
)
def test_grants_follow_privilege_merge_semantics(generator, scopes, expected):
    original = deepcopy(scopes)
    with patch(
        "dbt.context.context_config.get_flags",
        return_value=SimpleNamespace(state_modified_compare_more_unrendered_values=True),
    ):
        result = {}
        for grants in scopes:
            result = generator._update_from_config(result, {"grants": grants})
    assert result == {"grants": expected}
    assert scopes == original


def test_flag_off_keeps_legacy_grants(generator):
    with patch(
        "dbt.context.context_config.get_flags",
        return_value=SimpleNamespace(state_modified_compare_more_unrendered_values=False),
    ):
        result = generator._update_from_config({}, {"grants": {"select": ["outer"]}})
        result = generator._update_from_config(result, {"grants": {"+select": "inner"}})
    assert result == {"grants": {"+select": "inner"}}


@pytest.mark.parametrize(
    "parent, child",
    [
        ({"select": ["outer"]}, "{{ var('grants') }}"),
        ("{{ var('grants') }}", {"+select": "inner"}),
    ],
)
def test_opaque_grants_keep_legacy_precedence(generator, parent, child):
    with patch(
        "dbt.context.context_config.get_flags",
        return_value=SimpleNamespace(state_modified_compare_more_unrendered_values=True),
    ):
        result = generator._update_from_config({}, {"grants": parent})
        result = generator._update_from_config(result, {"grants": child})
    assert result == {"grants": child}


def test_other_configs_keep_legacy_precedence(generator):
    parent = {
        "materialized": "table",
        "meta": {"owner": "outer"},
        "tags": ["outer"],
        "post-hook": ["select 1"],
    }
    child = {
        "materialized": "view",
        "meta": "{{ var('metadata') }}",
        "tags": ["inner"],
        "post-hook": "select 2",
    }
    with patch(
        "dbt.context.context_config.get_flags",
        return_value=SimpleNamespace(state_modified_compare_more_unrendered_values=True),
    ):
        result = generator._update_from_config({}, parent)
        result = generator._update_from_config(result, child)
    assert result == child


def test_structured_grants_after_an_opaque_value(generator):
    with patch(
        "dbt.context.context_config.get_flags",
        return_value=SimpleNamespace(state_modified_compare_more_unrendered_values=True),
    ):
        result = generator._update_from_config({}, {"grants": "{{ var('grants') }}"})
        result = generator._update_from_config(result, {"grants": {"+select": "middle"}})
        result = generator._update_from_config(result, {"grants": {"+select": "inner"}})
    assert result == {"grants": {"select": ["middle", "inner"]}}


def test_literal_grants_match_rendered_merge(generator):
    scopes = [
        {},
        {"select": "reader"},
        {"+select": ["reader"]},
        {"select": []},
        {"insert": ["writer"]},
        {"select": ["replacement"], "+select": "addition"},
    ]
    with patch(
        "dbt.context.context_config.get_flags",
        return_value=SimpleNamespace(state_modified_compare_more_unrendered_values=True),
    ):
        for hierarchy in product(scopes, repeat=3):
            raw = {}
            rendered = NodeConfig()
            for grants in hierarchy:
                raw = generator._update_from_config(raw, {"grants": grants})
                rendered = rendered.update_from(
                    {"grants": deepcopy(grants)}, BaseConfig, validate=False
                )
            assert raw["grants"] == rendered.grants, hierarchy
