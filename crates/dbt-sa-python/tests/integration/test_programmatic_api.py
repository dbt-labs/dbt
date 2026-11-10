"""Conformance with the documented programmatic API.

Mirrors every construction and invocation form on
https://docs.getdbt.com/reference/programmatic-invocations so divergences are
visible here rather than discovered by a caller. Known divergences are xfail
with strict=True, so implementing one turns this suite red until the test is
updated.
"""

from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.results import RunResultsArtifact


def test_global_flag_before_subcommand(tmp_project, invoke, unique_ids):
    """`dbt.invoke(["--fail-fast", "run", "--select", "tag:my_tag"])`.

    The documented form puts a global flag *ahead* of the subcommand, which is a
    separate clap parsing path from the trailing form.
    """
    res = invoke(tmp_project("layered"), "--fail-fast", "build", "--select", "+mart_people")
    assert res.success, res.exception
    assert "model.layered.mart_people" in unique_ids(res.result)


def test_global_flag_before_and_after_agree(tmp_project, invoke, unique_ids):
    before = invoke(tmp_project("layered"), "--fail-fast", "build", "--select", "+mart_people")
    after = invoke(tmp_project("layered"), "build", "--select", "+mart_people", "--fail-fast")

    assert before.success, before.exception
    assert after.success, after.exception
    assert unique_ids(before.result) == unique_ids(after.result)


def test_kwargs_list_and_bool_together(tmp_project, invoke, unique_ids):
    """`dbt.invoke(["run"], select=["tag:my_tag"], fail_fast=True)`.

    The documented form mixes a list-valued flag with a boolean in one call.
    """
    res = invoke(tmp_project("layered"), "build", select=["+mart_people"], fail_fast=True)
    assert res.success, res.exception
    assert "model.layered.mart_people" in unique_ids(res.result)


def test_kwargs_bool_false_becomes_negated_flag(tmp_project, invoke):
    res = invoke(tmp_project("layered"), "build", select=["+mart_people"], fail_fast=False)
    assert res.success, res.exception


# ---------------------------------------------------------------------------
# Result shape
# ---------------------------------------------------------------------------


def test_result_type_varies_by_command(tmp_project, invoke):
    """The docs say `result` "varies by command"; this is the mapping we implement."""
    proj = tmp_project("layered")
    assert isinstance(invoke(proj, "parse").result, Manifest)
    assert isinstance(invoke(proj, "list").result, list)
    assert isinstance(invoke(proj, "build").result, RunResultsArtifact)


def test_documented_result_iteration(tmp_project, invoke):
    """The documented `for r in res.result:`, reading `r.status`."""
    res = invoke(tmp_project("layered"), "build", "--select", "+mart_people")
    assert res.success, res.exception

    rows = list(res.result)
    assert rows
    for r in rows:
        assert r.status


def test_documented_result_node_attribute_has_no_v2_equivalent(tmp_project, invoke):
    """The docs also read `r.node.name`, which cannot work here.

    That example is dbt-core's in-memory RunResult. v2's on-disk RunResultOutput
    is flat — unique_id, no node. A permanent divergence, so it is pinned.
    """
    res = invoke(tmp_project("layered"), "build", "--select", "+mart_people")
    assert res.success, res.exception

    row = res.result[0]
    assert not hasattr(row, "node")
    assert row.unique_id
