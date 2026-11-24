from argparse import Namespace

import pytest

from dbt.artifacts.schemas.upgrades.upgrade_manifest_dbt_version import (
    upgrade_manifest_json_dbt_version,
)


@pytest.fixture
def args_for_flags() -> Namespace:
    return Namespace(state_modified_compare_more_unrendered_values=True)


def _manifest(raw_code: str, unrendered_config: dict) -> dict:
    return {
        "nodes": {
            "model.test.my_model": {
                "resource_type": "model",
                "raw_code": raw_code,
                "unrendered_config": unrendered_config,
            }
        }
    }


@pytest.mark.parametrize("hook", ["pre_hook", "post_hook"])
def test_upgrade_replaces_hyphenated_hook_key_from_older_manifest(hook):
    # Older versions stored inline hook configs under the hyphenated name
    hyphenated = hook.replace("_", "-")
    manifest = _manifest(
        raw_code=f"{{{{ config(materialized='table', {hook}='select 1') }}}}\nselect 1 as id",
        unrendered_config={"materialized": "table", hyphenated: "select 1"},
    )

    upgraded = upgrade_manifest_json_dbt_version(manifest)

    assert upgraded["nodes"]["model.test.my_model"]["unrendered_config"] == {
        "materialized": "table",
        hook: "select 1",
    }


def test_upgrade_keeps_hyphenated_hook_key_not_set_in_raw_code():
    manifest = _manifest(
        raw_code="{{ config(materialized='table') }}\nselect 1 as id",
        unrendered_config={"materialized": "table", "post-hook": "select 1"},
    )

    upgraded = upgrade_manifest_json_dbt_version(manifest)

    assert upgraded["nodes"]["model.test.my_model"]["unrendered_config"] == {
        "materialized": "table",
        "post-hook": "select 1",
    }
