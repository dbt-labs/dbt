"""Artifacts crossing the boundary as msgpack.

The JSON the engine writes to target/ is the oracle: the decoded dataclass has to
agree with it.
"""

import json

from dbt.artifacts.schemas.base import BaseArtifactMetadata, TimingInfo
from dbt.artifacts.schemas.catalog import CatalogArtifact as PyCatalog
from dbt.artifacts.schemas.manifest import Manifest as PyManifest
from dbt.artifacts.schemas.run import RunResultsArtifact as PyRunResults


def _built(built_project, invoke):
    res = invoke(built_project(), "build")
    assert res.success, res.exception
    return res


def test_msgpack_matches_run_results_json(tmp_project, invoke):
    """Decoded object and on-disk artifact describe the same run."""
    proj = tmp_project("layered")
    res = invoke(proj, "build")
    assert res.success, res.exception

    art = PyRunResults.from_msgpack(res.result.to_msgpack())
    disk = json.loads((proj / "target" / "run_results.json").read_text())

    assert sorted(art.args) == sorted(disk["args"])
    assert {r.unique_id for r in art.results} == {r["unique_id"] for r in disk["results"]}


def test_nested_types_decode_as_dataclasses(built_project, invoke):
    """Nesting is typed, not raw dicts."""
    res = _built(built_project, invoke)
    art = PyRunResults.from_msgpack(res.result.to_msgpack())

    assert isinstance(art.metadata, BaseArtifactMetadata)
    assert art.metadata.dbt_schema_version.endswith("run-results/v6.json")

    timed = [r for r in art.results if r.timing]
    assert timed, "no result carried timing info"
    assert all(isinstance(t, TimingInfo) for t in timed[0].timing)


def test_run_results_is_a_sequence(built_project, invoke):
    """A sequence over its rows, like dbt-core's ExecutionResult."""
    res = _built(built_project, invoke)
    art = PyRunResults.from_msgpack(res.result.to_msgpack())

    assert len(art) == len(art.results)
    assert [r.unique_id for r in art] == [r.unique_id for r in art.results]
    assert art[0] is art.results[0]


def test_catalog_type_alias_survives_both_directions(tmp_project, invoke):
    """Both are `type` on the wire; the alias must hold on decode and re-encode."""
    proj = tmp_project("layered")
    assert invoke(proj, "build").success
    res = invoke(proj, "compile", "--write-catalog")
    assert res.success, res.exception

    cat = PyCatalog.from_msgpack(res.catalog.to_msgpack())
    key = sorted(cat.nodes)[0]
    column = next(iter(cat.nodes[key].columns.values()))

    # Decoded into the Python-side name...
    assert column.data_type
    assert cat.nodes[key].metadata.materialization_type
    # ...and re-encodes back to the wire name.
    as_dict = cat.to_dict()
    assert "type" in as_dict["nodes"][key]["columns"][column.name]
    assert "type" in as_dict["nodes"][key]["metadata"]


def test_catalog_column_index_is_an_int(tmp_project, invoke):
    """i128 in Rust; dbt_yaml normalises it to something msgpack can carry."""
    proj = tmp_project("layered")
    assert invoke(proj, "build").success
    res = invoke(proj, "compile", "--write-catalog")
    assert res.success, res.exception

    cat = PyCatalog.from_msgpack(res.catalog.to_msgpack())
    columns = next(iter(cat.nodes.values())).columns
    assert all(isinstance(c.index, int) for c in columns.values())


def test_manifest_metadata_is_flattened(tmp_project, invoke):
    """Rust's #[serde(flatten)] puts both halves at the same level."""
    res = invoke(tmp_project("layered"), "parse")
    assert res.success, res.exception

    meta = PyManifest.from_msgpack(res.result.to_msgpack()).metadata

    # from BaseMetadata
    assert meta.dbt_schema_version.endswith("manifest/v12.json")
    # from ManifestMetadata itself
    assert meta.project_name == "layered"
    assert meta.adapter_type == "duckdb"


def test_unknown_keys_are_tolerated():
    """A reader must tolerate keys it predates."""
    art = PyRunResults.from_dict(
        {
            "metadata": {"dbt_schema_version": "x", "some_future_key": 1},
            "results": [{"status": "success", "unique_id": "model.a", "brand_new_field": True}],
            "elapsed_time": 1.0,
            "args": {"command": "build", "which": "build"},
        }
    )

    assert art.results[0].unique_id == "model.a"
    assert art.metadata.dbt_schema_version == "x"
