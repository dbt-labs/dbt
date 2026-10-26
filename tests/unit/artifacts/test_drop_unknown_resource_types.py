from unittest import mock

from dbt.artifacts.schemas.manifest import WritableManifest
from dbt.contracts.graph.manifest import Manifest
from tests.unit.utils.manifest import make_manifest, make_model


def _check_node(unique_id: str) -> dict:
    return {
        "resource_type": "check",
        "unique_id": unique_id,
        "name": unique_id.split(".")[-1],
        "package_name": "pkg",
        "raw_code": "select unique_id from {{ info_schema('models') }}",
    }


def _manifest_dict_with_checks() -> dict:
    model = make_model("pkg", "a", "select 1 as id")
    data = make_manifest(nodes=[model]).writable_manifest().to_dict()
    data["nodes"]["check.pkg.models_have_desc"] = _check_node("check.pkg.models_have_desc")
    data["disabled"]["check.pkg.disabled_check"] = [_check_node("check.pkg.disabled_check")]
    return data


def test_manifest_with_unknown_resource_types_loads():
    with mock.patch(
        "dbt.artifacts.schemas.upgrades.unknown_resource_types.fire_event"
    ) as fire_event:
        writable = WritableManifest.upgrade_schema_version(_manifest_dict_with_checks())

    assert list(writable.nodes) == ["model.pkg.a"]
    assert writable.disabled == {}
    assert "model.pkg.a" in Manifest.from_writable_manifest(writable).nodes
    fire_event.assert_called_once()
    assert "2 node(s) of resource type 'check'" in fire_event.call_args[0][0].msg


def test_manifest_without_unknown_resource_types_fires_no_note():
    data = make_manifest(nodes=[make_model("pkg", "a", "select 1 as id")])
    with mock.patch(
        "dbt.artifacts.schemas.upgrades.unknown_resource_types.fire_event"
    ) as fire_event:
        writable = WritableManifest.upgrade_schema_version(data.writable_manifest().to_dict())

    assert list(writable.nodes) == ["model.pkg.a"]
    fire_event.assert_not_called()
