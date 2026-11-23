from collections import Counter
from typing import Any

from dbt.artifacts.resources.types import NodeType
from dbt_common.events.functions import fire_event
from dbt_common.events.types import Note

KNOWN_RESOURCE_TYPES = frozenset(t.value for t in NodeType)


def _is_unknown(node: Any) -> bool:
    return isinstance(node, dict) and node.get("resource_type") not in KNOWN_RESOURCE_TYPES


def _drop_from_nodes(nodes: dict, dropped: Counter) -> None:
    for unique_id in [uid for uid, node in nodes.items() if _is_unknown(node)]:
        dropped[nodes.pop(unique_id).get("resource_type")] += 1


def _drop_from_disabled(disabled: dict, dropped: Counter) -> None:
    for unique_id, node_list in list(disabled.items()):
        unknown = [node for node in node_list if _is_unknown(node)]
        dropped.update(node.get("resource_type") for node in unknown)
        kept = [node for node in node_list if not _is_unknown(node)]
        if kept:
            disabled[unique_id] = kept
        else:
            del disabled[unique_id]


def drop_unknown_resource_types(manifest: dict) -> dict:
    """Drop nodes of unknown resource types, e.g. dbt v2 checks."""
    dropped: Counter = Counter()
    _drop_from_nodes(manifest.get("nodes") or {}, dropped)
    _drop_from_disabled(manifest.get("disabled") or {}, dropped)

    for resource_type, count in dropped.items():
        fire_event(
            Note(
                msg=f"{count} node(s) of resource type '{resource_type}' in the manifest are "
                "not being used because this version of dbt doesn't support them."
            )
        )
    return manifest
