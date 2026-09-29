"""The `result` contract: which artifact each command returns, and how a handled
failure is distinguished from a real error.

Asserts on the returned objects, never on stdout.
"""

from dbt.artifacts.schemas.sources import FreshnessStatus
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.results import CatalogArtifact, FreshnessResultsArtifact, RunResultsArtifact


def test_parse_returns_manifest(tmp_project, invoke):
    res = invoke(tmp_project("layered"), "parse")

    assert res.success, res.exception
    assert isinstance(res.result, Manifest), type(res.result)
    assert any(uid.startswith("model.") for uid in res.result.nodes)


def test_list_returns_list_of_str(tmp_project, invoke):
    res = invoke(tmp_project("layered"), "list")

    assert res.success, res.exception
    # Was None before the engine captured list output.
    assert isinstance(res.result, list), type(res.result)
    assert all(isinstance(item, str) for item in res.result)
    assert "layered.mart_people" in res.result, res.result


def test_build_returns_run_results(tmp_project, invoke, unique_ids):
    res = invoke(tmp_project("layered"), "build")

    assert res.success, res.exception
    assert res.exit_code == 0
    assert isinstance(res.result, RunResultsArtifact), type(res.result)
    # seed + 2 models + 4 tests. Models report "success", tests report "pass".
    assert len(res.result.results) == 7, unique_ids(res.result)
    assert {r.status for r in res.result.results} == {"success", "pass"}


def test_compile_write_catalog_populates_catalog(tmp_project, invoke):
    proj = tmp_project("layered")
    assert invoke(proj, "build").success

    res = invoke(proj, "compile", "--write-catalog")

    assert res.success, res.exception
    # `result` stays dbt-core-compatible; the catalog rides alongside it.
    assert isinstance(res.result, RunResultsArtifact), type(res.result)
    assert isinstance(res.catalog, CatalogArtifact), type(res.catalog)
    assert res.catalog.nodes, "catalog has no nodes"


def test_catalog_is_none_without_the_flag(tmp_project, invoke):
    res = invoke(tmp_project("layered"), "compile")

    assert res.success, res.exception
    assert res.catalog is None


def test_failing_test_is_a_handled_failure(tmp_project, invoke):
    """A failing test returns its artifact and sets no exception."""
    res = invoke(tmp_project("failing_test"), "build")

    assert res.success is False
    assert res.exception is None, res.exception
    assert isinstance(res.result, RunResultsArtifact), type(res.result)

    assert res.exit_code == 1
    failed = [r for r in res.result.results if r.status == "fail"]
    assert len(failed) == 1, res.result.results
    assert failed[0].unique_id.startswith("test.failing_test.unique_dupes_id")
    assert failed[0].failures == 1


def test_source_freshness_returns_freshness_results(built_project, invoke):
    """`source freshness` reports sources.json, not run_results.json."""
    proj = built_project("freshness")

    res = invoke(proj, "source", "freshness", "--select", "source:raw.fresh_events")

    assert res.success, res.exception
    assert res.exit_code == 0
    assert isinstance(res.result, FreshnessResultsArtifact), type(res.result)
    assert [r.unique_id for r in res.result] == ["source.freshness.raw.fresh_events"]
    # Capitalised, unlike run_results' statuses.
    assert res.result[0].status == FreshnessStatus.PASS
    assert res.result[0].criteria.warn_after.count == 1


def test_stale_source_is_a_handled_failure(built_project, invoke):
    """A stale source is accounted for by the artifact, so it sets no exception."""
    proj = built_project("freshness")

    res = invoke(proj, "source", "freshness")

    assert res.success is False
    assert res.exception is None, res.exception
    assert res.exit_code == 1
    statuses = {r.unique_id: r.status for r in res.result}
    assert statuses == {
        "source.freshness.raw.fresh_events": FreshnessStatus.PASS,
        "source.freshness.raw.stale_events": FreshnessStatus.ERROR,
    }


def test_unresolvable_ref_returns_partial_manifest_with_error(tmp_project, invoke):
    """A parse error returns both its exception and the manifest built before failure."""
    res = invoke(tmp_project("broken_ref"), "parse")

    assert res.success is False
    assert res.exception is not None
    assert isinstance(res.exception, BaseException)
    # Not the bare "exit code 1" this used to report.
    assert "exit code" not in str(res.exception), res.exception
    assert isinstance(res.result, Manifest), type(res.result)
    assert "model.broken_ref.broken" in res.result.nodes
    # Docs say 2 for an unhandled error; fusion's CLI exits 1 for a compilation
    # error and this field reports the engine's real code.
    assert res.exit_code == 1
