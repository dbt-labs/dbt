from dbt.clients.jinja_static import statically_parse_unrendered_config
from dbt.flags import get_flags
from dbt_common.contracts.config.base import BaseConfig


def upgrade_manifest_json_dbt_version(manifest: dict) -> dict:
    upgrade_unrendered_config(manifest)

    return manifest


def upgrade_unrendered_config(manifest: dict) -> dict:
    """Patch unrendered_config entries in a loaded manifest to include raw kwarg values extracted from raw_code.

    unrendered_config from raw_code was previously set to a Python representation of the Jinja `{{ config() }}` call.
    The latest implementation of statically_parse_unrendered_config improved on this by reconstructing the raw unrendered_config
    call from the intermediate Python representation, behind state_modified_compare_more_unrendered_values.

    Only upgrade the manifest if state_modified_compare_more_unrendered_values is set to True, as the change to statically_parse_unrendered_config
    would only lead to false positives in that scenario as projects with state_modified_compare_more_unrendered_values set to false do not go through
    the statically_parse_unrendered_config call at all.

    Similarly, simple built-in generic tests (not_null, unique) are skipped because they have no raw_code config block.
    Their config is set directly on the ContextConfig without going through a Jinja config() call.
    """
    if get_flags().state_modified_compare_more_unrendered_values:
        for unique_id, node in manifest.get("nodes", {}).items():
            if _node_is_simple_builtin_generic_test(node):
                continue
            if node.get("raw_code") and node.get("unrendered_config"):
                new_unrendered_config_from_code = _without_merged_grants(
                    node["unrendered_config"],
                    statically_parse_unrendered_config(node.get("raw_code", "")) or {},
                )
                manifest["nodes"][unique_id]["unrendered_config"].update(
                    new_unrendered_config_from_code
                )

    return manifest


def _without_merged_grants(stored: dict, from_code: dict) -> dict:
    """Drop dict_key_append configs (grants) from `from_code` when `stored` has them as a dict:
    stored grants already include the code's grants merged with inherited grants."""
    return {
        key: value
        for key, value in from_code.items()
        if not (
            key in BaseConfig.mergebehavior["dict_key_append"]
            and isinstance(stored.get(key), dict)
            and isinstance(value, dict)
        )
    }


def _node_is_simple_builtin_generic_test(node: dict) -> bool:
    return node.get("resource_type") == "test" and node.get("depends_on", {}).get("macros") in (
        ["macro.dbt.test_not_null"],
        ["macro.dbt.test_unique"],
    )
