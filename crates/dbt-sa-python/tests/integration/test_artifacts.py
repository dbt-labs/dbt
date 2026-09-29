"""The JSON artifacts written to target/ agree with the objects returned in memory."""

import json

from dbt.contracts.results import CatalogArtifact, FreshnessResultsArtifact, RunResultsArtifact


def _read(path):
    return json.loads(path.read_text())


def test_run_results_json_matches_returned_artifact(tmp_project, invoke, unique_ids):
    proj = tmp_project("layered")
    res = invoke(proj, "build")
    assert res.success, res.exception

    on_disk = proj / "target" / "run_results.json"
    assert on_disk.is_file()

    parsed = _read(on_disk)
    assert {r["unique_id"] for r in parsed["results"]} == unique_ids(res.result)
    assert len(parsed["results"]) == len(res.result.results)


def test_run_results_read_round_trips(tmp_project, invoke, unique_ids):
    """RunResultsArtifact.read() gives the same node set as the in-memory object."""
    proj = tmp_project("layered")
    res = invoke(proj, "build")
    assert res.success, res.exception

    from_disk = RunResultsArtifact.read(str(proj / "target" / "run_results.json"))
    assert unique_ids(from_disk) == unique_ids(res.result)


def test_manifest_json_matches_returned_manifest(tmp_project, invoke):
    proj = tmp_project("layered")
    res = invoke(proj, "parse")
    assert res.success, res.exception

    on_disk = proj / "target" / "manifest.json"
    assert on_disk.is_file()

    parsed = _read(on_disk)
    assert set(parsed["nodes"]) == set(res.result.nodes)
    assert set(parsed["sources"]) == set(res.result.sources)


def test_catalog_json_matches_returned_catalog(tmp_project, invoke):
    proj = tmp_project("layered")
    assert invoke(proj, "build").success

    res = invoke(proj, "compile", "--write-catalog")
    assert res.success, res.exception

    on_disk = proj / "target" / "catalog.json"
    assert on_disk.is_file()

    parsed = _read(on_disk)
    assert set(parsed["nodes"]) == set(res.catalog.nodes)

    from_disk = CatalogArtifact.read(str(on_disk))
    assert set(from_disk.nodes) == set(res.catalog.nodes)


def test_sources_json_matches_returned_artifact(built_project, invoke, unique_ids):
    proj = built_project("freshness")
    # Exits 1 on the stale source; the artifact is written and returned regardless.
    res = invoke(proj, "source", "freshness")

    on_disk = proj / "target" / "sources.json"
    assert on_disk.is_file()

    parsed = _read(on_disk)
    assert {r["unique_id"] for r in parsed["results"]} == unique_ids(res.result)
    assert [r["status"] for r in parsed["results"]] == [r.status for r in res.result]

    from_disk = FreshnessResultsArtifact.read(str(on_disk))
    assert unique_ids(from_disk) == unique_ids(res.result)


def test_failing_run_still_writes_run_results(tmp_project, invoke, unique_ids):
    """A handled failure persists its artifact, and it matches what was returned."""
    proj = tmp_project("failing_test")
    res = invoke(proj, "build")
    assert res.success is False

    parsed = _read(proj / "target" / "run_results.json")
    assert {r["unique_id"] for r in parsed["results"]} == unique_ids(res.result)
    assert any(r["status"] == "fail" for r in parsed["results"])
