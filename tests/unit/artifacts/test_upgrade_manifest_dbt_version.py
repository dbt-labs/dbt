from argparse import Namespace
from unittest import mock

import pytest

from dbt.artifacts.schemas.upgrades.upgrade_manifest_dbt_version import (
    upgrade_unrendered_config,
)


@pytest.fixture(autouse=True)
def compare_more_unrendered_values():
    with mock.patch(
        "dbt.artifacts.schemas.upgrades.upgrade_manifest_dbt_version.get_flags",
        return_value=Namespace(state_modified_compare_more_unrendered_values=True),
    ):
        yield


def _manifest(raw_code: str, unrendered_config: dict) -> dict:
    return {
        "nodes": {
            "model.pkg.m": {
                "resource_type": "model",
                "raw_code": raw_code,
                "unrendered_config": unrendered_config,
            }
        }
    }


def test_hook_keys_stored_with_hyphens_are_replaced():
    # dbt stored hooks under "pre-hook"/"post-hook" when the flag was off (the default before 1.10),
    # while the static parse of the config() call returns the kwarg names "pre_hook"/"post_hook".
    manifest = _manifest(
        '{{ config(pre_hook=["select 1"], post_hook="select 2", tags=["a"]) }}\nselect 1 as id',
        {
            "materialized": "table",
            "tags": ["a"],
            "pre-hook": ["select 1"],
            "post-hook": "select 2",
        },
    )

    upgrade_unrendered_config(manifest)

    unrendered_config = manifest["nodes"]["model.pkg.m"]["unrendered_config"]
    assert sorted(unrendered_config) == ["materialized", "post_hook", "pre_hook", "tags"]


def test_hook_key_is_kept_when_the_code_no_longer_sets_the_hook():
    # the hook was removed from the model, so the stored hook must stay to be detected as a change
    manifest = _manifest(
        '{{ config(tags=["a"]) }}\nselect 1 as id',
        {"materialized": "table", "tags": ["a"], "post-hook": "select 2"},
    )

    upgrade_unrendered_config(manifest)

    assert "post-hook" in manifest["nodes"]["model.pkg.m"]["unrendered_config"]
