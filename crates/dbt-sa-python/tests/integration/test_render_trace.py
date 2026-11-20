"""Opt-in provenance is produced by the same runtime as ordinary compilation."""

import json

import pytest

TRACE_EPOCHS = "private/metadata/compile/render_traces/v1_*.parquet"


def events(trace):
    return trace["events"]


def preview_json(result, prefix):
    assert result.success, result.exception
    body = b"".join(result.preview)
    start = body.index(prefix)
    value, _ = json.JSONDecoder().raw_decode(body[start:].decode("utf-8", errors="ignore"))
    return value


def read_trace(project, invoke, parquet_path, unique_id):
    latest = (
        " qualify row_number() over (partition by unique_id order by ingested_at desc) = 1"
        if "*" in parquet_path
        else ""
    )
    result = invoke(
        project,
        "show",
        "--inline",
        (
            "select to_json(t) as trace_json "
            f"from read_parquet('{parquet_path}') t "
            f"where unique_id = '{unique_id}'{latest}"
        ),
        "--limit",
        "1",
    )
    return preview_json(result, b'{"protocol_version":')


def read_unique_ids(project, invoke, parquet_path):
    result = invoke(
        project,
        "show",
        "--inline",
        (
            "select to_json(list(distinct unique_id order by unique_id)) as trace_ids "
            f"from read_parquet('{parquet_path}')"
        ),
        "--limit",
        "1",
    )
    return preview_json(result, b"[")


def assert_matches_golden(project, invoke, trace):
    normalized = dict(trace)
    normalized["dbt_version"] = "0.0.0-example"
    normalized["invocation_id"] = "00000000-0000-4000-8000-000000000001"
    normalized["source_path"] = normalized["filename"]
    normalized.pop("ingested_at")
    golden = read_trace(project, invoke, "goldens/render_traces.parquet", trace["unique_id"])
    assert normalized == golden, json.dumps(normalized, indent=2, ensure_ascii=False)


def test_render_trace_matches_compile_and_preserves_provenance(tmp_project, invoke):
    project = tmp_project("render_trace")
    normal = invoke(project, "compile", "--select", "probe", "--target-path", "normal")
    assert normal.success, normal.exception
    assert not list(project.glob("normal/private/metadata/compile/render_traces/*.parquet"))

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

    trace = read_trace(project, invoke, f"traced/{TRACE_EPOCHS}", "model.render_trace.probe")
    assert trace["protocol_version"] == 2
    assert trace["unique_id"] == "model.render_trace.probe"
    assert trace["render_stage"] == "jinja_before_ephemeral_injection"
    assert trace["source"] == (project / "models/probe.sql").read_bytes().decode("utf-8")
    # The parent CTE is injected after the traced Jinja render.
    assert "99 as base_value" not in trace["rendered"]
    assert "99 as base_value" in (project / "traced" / f"{relative}.sql").read_text()

    source = trace["source"].encode()
    rendered = trace["rendered"].encode()
    event_rows = events(trace)
    literals = [event for event in event_rows if event["kind"] == "literal"]
    for event in literals:
        assert (
            source[event["source_start"] : event["source_end"]]
            == rendered[event["rendered_start"] : event["rendered_end"]]
        )
    assert any(event["kind"] == "loop_iteration" for event in event_rows)
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
    refreshed = read_trace(project, invoke, f"traced/{TRACE_EPOCHS}", "model.render_trace.probe")
    assert refreshed["invocation_id"] != trace["invocation_id"]
    assert refreshed["rendered"] != trace["rendered"]
    assert "select 11 as value" in refreshed["rendered"]


def test_failed_render_does_not_export_successful_trace(tmp_project, invoke):
    project = tmp_project("render_trace")
    (project / "models/probe.sql").write_text("select {{ missing_function() }}")
    result = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    assert not result.success
    assert not list(project.glob("target/private/metadata/compile/render_traces/*.parquet"))


@pytest.mark.parametrize("partial_load", [True, False])
def test_selected_compiles_retain_unselected_traces(tmp_project, invoke, partial_load):
    project = tmp_project("render_trace")
    retained = project / "models/retained.sql"
    retained.write_text("select 1 as retained_value\n")
    flags = [
        "--feature",
        "write_render_trace",
        "--write-metadata",
        "--static-analysis",
        "off",
    ]
    if not partial_load:
        flags.append("--no-partial-load")
    parquet_path = f"target/{TRACE_EPOCHS}"
    probe_id = "model.render_trace.probe"
    retained_id = "model.render_trace.retained"
    base_id = "model.render_trace.base"
    added_id = "model.render_trace.added"

    first = invoke(project, "compile", "--select", "probe", "retained", *flags)
    assert first.success, first.exception
    assert read_unique_ids(project, invoke, parquet_path) == sorted(
        [base_id, probe_id, retained_id]
    )
    original = read_trace(project, invoke, parquet_path, retained_id)
    probe_before = read_trace(project, invoke, parquet_path, probe_id)

    selected = invoke(project, "compile", "--select", "probe", *flags)
    assert selected.success, selected.exception
    assert read_unique_ids(project, invoke, parquet_path) == sorted(
        [base_id, probe_id, retained_id]
    )
    assert read_trace(project, invoke, parquet_path, retained_id) == original

    macro = project / "macros/value.sql"
    macro.write_text(macro.read_text().replace("return(value)", "return(value + 10)"))
    updated = invoke(project, "compile", "--select", "probe", *flags)
    assert updated.success, updated.exception
    assert read_unique_ids(project, invoke, parquet_path) == sorted(
        [base_id, probe_id, retained_id]
    )
    assert read_trace(project, invoke, parquet_path, retained_id) == original
    probe_after = read_trace(project, invoke, parquet_path, probe_id)
    assert probe_after["invocation_id"] != probe_before["invocation_id"]
    assert "select 11 as value" in probe_after["rendered"]

    (project / "models/added.sql").write_text("select 2 as added_value\n")
    added = invoke(project, "compile", "--select", "added", *flags)
    assert added.success, added.exception
    assert read_unique_ids(project, invoke, parquet_path) == sorted(
        [added_id, base_id, probe_id, retained_id]
    )
    assert read_trace(project, invoke, parquet_path, retained_id) == original
    added_trace = read_trace(project, invoke, parquet_path, added_id)

    retained.unlink()
    deleted = invoke(project, "compile", "--select", "probe", *flags)
    assert deleted.success, deleted.exception
    assert read_unique_ids(project, invoke, parquet_path) == sorted([added_id, base_id, probe_id])
    assert read_trace(project, invoke, parquet_path, added_id) == added_trace

    probe = project / "models/probe.sql"
    source = probe.read_text()
    probe.write_text(
        "{% if execute %}{{ exceptions.raise_compiler_error('trace failure') }}{% endif %}\n"
        + source
    )
    failed = invoke(project, "compile", "--select", "probe", *flags)
    assert not failed.success
    assert read_unique_ids(project, invoke, parquet_path) == sorted([added_id, base_id])
    assert read_trace(project, invoke, parquet_path, added_id) == added_trace

    probe.write_text(source)
    recovered = invoke(project, "compile", "--select", "probe", *flags)
    assert recovered.success, recovered.exception
    assert read_unique_ids(project, invoke, parquet_path) == sorted([added_id, base_id, probe_id])
    assert read_trace(project, invoke, parquet_path, added_id) == added_trace


def test_failed_rerender_removes_stale_trace(tmp_project, invoke):
    project = tmp_project("render_trace")
    first = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    assert first.success, first.exception
    assert "model.render_trace.probe" in read_unique_ids(project, invoke, f"target/{TRACE_EPOCHS}")

    (project / "models/probe.sql").write_text("select {{ missing_function() }}")
    failed = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    assert not failed.success
    assert "model.render_trace.probe" not in read_unique_ids(
        project, invoke, f"target/{TRACE_EPOCHS}"
    )


def test_common_dbt_jinja_patterns_are_traced_columnarly(tmp_project, invoke):
    project = tmp_project("render_trace")
    result = invoke(
        project,
        "compile",
        "--select",
        "common_patterns",
        "--feature",
        "write_render_trace",
    )
    assert result.success, result.exception
    trace = read_trace(
        project, invoke, f"target/{TRACE_EPOCHS}", "model.render_trace.common_patterns"
    )
    assert_matches_golden(project, invoke, trace)
    event_rows = events(trace)
    source = trace["source"]
    rendered = trace["rendered"]

    assert trace["protocol_version"] == 2
    assert "source('raw', 'events')" in source
    assert "ref('base')" in source
    assert "adapter.dispatch('trace_column', 'render_trace')" in source
    assert "is_incremental()" in source
    assert '"account_id" as account_id' in rendered
    assert '"event_type" as event_type' in rendered
    assert "'dev' as dbt_target" in rendered
    assert "7 as trace_version" in rendered
    assert any(event["kind"] == "loop_iteration" for event in event_rows)
    assert any(event["kind"] == "block_start" for event in event_rows)


def test_package_star_macro_is_traced_columnarly(tmp_project, invoke):
    project = tmp_project("render_trace")
    (project / "packages.yml").write_text((project / "star_packages.yml").read_text())
    dependencies = invoke(project, "deps")
    assert dependencies.success, dependencies.exception

    result = invoke(
        project,
        "compile",
        "--select",
        "common_patterns",
        "star_usage",
        "--threads",
        "4",
        "--feature",
        "write_render_trace",
    )
    assert result.success, result.exception
    assert read_unique_ids(project, invoke, f"target/{TRACE_EPOCHS}") == [
        "model.render_trace.base",
        "model.render_trace.common_patterns",
        "model.render_trace.star_usage",
    ]
    trace = read_trace(project, invoke, f"target/{TRACE_EPOCHS}", "model.render_trace.star_usage")
    assert_matches_golden(project, invoke, trace)
    event_rows = events(trace)

    assert "dbt_utils.star(" in trace["source"]
    assert 'base_values."base_value"' in trace["rendered"]
    assert 'as "copied_base_value"' in trace["rendered"]
    assert 'base_values."created_at"' in trace["rendered"]
    assert "internal_note" not in trace["rendered"]
    assert any(event["kind"] == "macro" for event in event_rows)


def test_disabled_model_is_identified_in_manifest_without_trace(tmp_project, invoke):
    project = tmp_project("render_trace")
    (project / "models/disabled_model.sql").write_text("{{ config(enabled=false) }}\nselect 1\n")
    result = invoke(
        project,
        "compile",
        "--select",
        "probe",
        "disabled_model",
        "--feature",
        "write_render_trace",
    )
    assert result.success, result.exception
    manifest = json.loads((project / "target/manifest.json").read_text())
    disabled = manifest["disabled"]["model.render_trace.disabled_model"]
    assert len(disabled) == 1
    assert disabled[0]["config"]["enabled"] is False
    assert disabled[0]["original_file_path"].replace("\\", "/") == "models/disabled_model.sql"
    assert read_unique_ids(project, invoke, f"target/{TRACE_EPOCHS}") == [
        "model.render_trace.base",
        "model.render_trace.probe",
    ]


def test_selected_seed_leaves_existing_trace_epochs_unchanged(tmp_project, invoke):
    project = tmp_project("render_trace")
    traced = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    assert traced.success, traced.exception

    probe = project / "models/probe.sql"
    probe.write_text(probe.read_text().replace("select 2 as value", "select 3 as value"))
    retraced = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    assert retraced.success, retraced.exception
    epochs = sorted(project.glob(f"target/{TRACE_EPOCHS}"))
    before = {epoch.name: epoch.read_bytes() for epoch in epochs}
    assert len(before) == 2

    seeds = project / "seeds"
    seeds.mkdir()
    (seeds / "seed_only.csv").write_text("value\n1\n")
    selected = invoke(
        project,
        "compile",
        "--select",
        "seed_only",
        "--feature",
        "write_render_trace",
    )
    assert selected.success, selected.exception
    after = {
        epoch.name: epoch.read_bytes() for epoch in sorted(project.glob(f"target/{TRACE_EPOCHS}"))
    }
    assert after == before


def test_trace_write_error_warns_without_failing_compile(tmp_project, invoke, capfd):
    project = tmp_project("render_trace")
    trace_dir = project / "target/private/metadata/compile/render_traces"
    trace_dir.parent.mkdir(parents=True)
    trace_dir.write_text("not a directory")

    result = invoke(project, "compile", "--select", "probe", "--feature", "write_render_trace")
    captured = capfd.readouterr()

    assert result.success, result.exception
    assert "render_traces" in captured.out + captured.err
    assert trace_dir.is_file()
