"""Opt-in provenance is produced by the same runtime as ordinary compilation."""

import json


def test_render_trace_matches_compile_and_preserves_provenance(tmp_project, invoke):
    project = tmp_project("render_trace")
    normal = invoke(project, "compile", "--select", "probe", "--target-path", "normal")
    assert normal.success, normal.exception
    assert not list((project / "normal").rglob("*.render_trace.json"))

    traced = invoke(
        project,
        "compile",
        "--select",
        "probe",
        "--target-path",
        "traced",
        "--feature",
        "write_render_trace",
    )
    assert traced.success, traced.exception
    relative = "compiled/render_trace/models/probe"
    assert (project / "normal" / f"{relative}.sql").read_text() == (
        project / "traced" / f"{relative}.sql"
    ).read_text()

    artifact = project / "traced" / f"{relative}.render_trace.json"
    trace = json.loads(artifact.read_text())
    assert trace["protocol_version"] == 1
    assert trace["unique_id"] == "model.render_trace.probe"
    assert trace["render_stage"] == "jinja_before_ephemeral_injection"
    assert trace["source"] == (project / "models/probe.sql").read_bytes().decode("utf-8")
    # The parent CTE is injected after the traced Jinja render.
    assert "99 as base_value" not in trace["rendered"]
    assert "99 as base_value" in (project / "traced" / f"{relative}.sql").read_text()

    source = trace["source"].encode()
    rendered = trace["rendered"].encode()
    literals = [event for event in trace["events"] if event["kind"] == "literal"]
    for event in literals:
        assert (
            source[event["source_start"] : event["source_end"]]
            == rendered[event["rendered_start"] : event["rendered_end"]]
        )
    assert any(event["kind"] == "loop_iteration" for event in trace["events"])
    assert len({event["source_start"] for event in literals}) < len(literals)

    # Reusing the target after a dependency changes must export a new render.
    macro = project / "macros/value.sql"
    macro.write_text(macro.read_text().replace("return(value)", "return(value + 10)"))
    repeated = invoke(
        project,
        "compile",
        "--select",
        "probe",
        "--target-path",
        "traced",
        "--feature",
        "write_render_trace",
    )
    assert repeated.success, repeated.exception
    refreshed = json.loads(artifact.read_text())
    assert refreshed["invocation_id"] != trace["invocation_id"]
    assert refreshed["rendered"] != trace["rendered"]
    assert "select 11 as value" in refreshed["rendered"]


def test_failed_render_does_not_export_successful_trace(tmp_project, invoke):
    project = tmp_project("render_trace")
    (project / "models/probe.sql").write_text("select {{ missing_function() }}")
    result = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    assert not result.success
    assert not list((project / "target").rglob("probe.render_trace.json"))
