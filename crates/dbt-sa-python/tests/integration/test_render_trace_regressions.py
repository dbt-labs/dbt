"""Exercise review regressions through real compilation and exported sidecars."""

import json
from collections import Counter

import pytest


@pytest.mark.parametrize(
    "case",
    [
        pytest.param(
            (
                "{% raw -%} select 1 {%- endraw %}",
                "select 1",
                {"literals": "select 1"},
            ),
            id="trimmed-raw-payload",
        ),
        pytest.param(
            (
                "{% raw -%} select 'é😀' {%- endraw %}",
                "select 'é😀'",
                {"literals": "select 'é😀'"},
            ),
            id="unicode-raw-byte-offsets",
        ),
        pytest.param(
            (
                "select 'é😀'\r\nfrom (select 1) x",
                "select 'é😀'\r\nfrom (select 1) x",
                {},
            ),
            id="unicode-and-crlf",
        ),
        pytest.param(
            (
                "{% if true %}select 1{% elif true %}select 2{% else %}select 3{% endif %}",
                "select 1",
                {"block_end": 1},
            ),
            id="if-first-branch",
        ),
        pytest.param(
            (
                "{% if false %}select 1{% elif true %}select 2{% else %}select 3{% endif %}",
                "select 2",
                {"block_end": 1},
            ),
            id="elif-branch",
        ),
        pytest.param(
            (
                "{% if false %}select 1{% elif false %}select 2{% else %}select 3{% endif %}",
                "select 3",
                {"block_end": 1},
            ),
            id="elif-falls-through-to-else",
        ),
        pytest.param(
            (
                "{% if false %}select 0{% elif true %}{% if true %}select 1{% endif %}{% endif %}",
                "select 1",
                {"block_end": 2},
            ),
            id="elif-with-nested-if",
        ),
        pytest.param(
            (
                "{% for n in [] %}select {{ n }};{% else %}select 0{% endfor %}",
                "select 0",
                {"block_end": 1, "loop_iteration": 0},
            ),
            id="empty-for-else",
        ),
        pytest.param(
            (
                "{% for n in [1] %}select {{ n }};{% else %}select 0{% endfor %}",
                "select 1;",
                {"block_end": 1, "loop_iteration": 0},
            ),
            id="single-iteration-for-else",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}select {{ n }};{% else %}select 0{% endfor %}",
                "select 1;select 2;",
                {"block_end": 2, "loop_iteration": 1},
            ),
            id="multiple-iteration-for-else",
        ),
        pytest.param(
            (
                "{% for n in [1, 2, 3] if n == 3 %}select {{ n }};{% endfor %}",
                "select 3;",
                {"block_end": 1, "loop_iteration": 0},
            ),
            id="filter-accepts-one",
        ),
        pytest.param(
            (
                "{% for n in [1, 2, 3] if n > 1 %}select {{ n }};{% endfor %}",
                "select 2;select 3;",
                {"block_end": 2, "loop_iteration": 1},
            ),
            id="filter-accepts-two",
        ),
        pytest.param(
            (
                "{% for n in [1, 2, 3] if n > 3 %}select {{ n }};{% endfor %}select 0",
                "select 0",
                {"block_end": 0, "loop_iteration": 0, "loop_skipped": 1},
            ),
            id="filter-rejects-all",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}{% if false %}{% elif true %}{% endif %}{% endfor %}select 0",
                "select 0",
                {"block_end": 4, "loop_iteration": 1},
            ),
            id="genuine-empty-iterations-are-not-deduplicated",
        ),
        pytest.param(
            (
                "select {% block b %}{% block c %}1{% endblock %}{% endblock %} as n",
                "select 1 as n",
                {"literals": "select 1 as n"},
            ),
            id="nested-buffered-blocks",
        ),
        pytest.param(
            (
                "{% set text %}{% block b %}select 1{% endblock %}{% endset %}{{ text | upper }}",
                "SELECT 1",
                {"literals": ""},
            ),
            id="captured-block-transformation",
        ),
        pytest.param(
            (
                "select {% block b %}'a'{% endblock %},{{ self.b() | upper }}",
                "select 'a','A'",
                {"literals": "select 'a',"},
            ),
            id="transformed-self-block",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}select {{ n }};{% continue %}{% endfor %}",
                "select 1;select 2;",
                {"block_end": 2, "loop_iteration": 1},
            ),
            id="continue-closes-loop-body",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}{% if n > 0 %}select {{ n }};"
                "{% continue %}{% endif %}{% endfor %}",
                "select 1;select 2;",
                {"block_end": 4, "loop_iteration": 1},
            ),
            id="continue-closes-nested-conditional",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}select {{ n }};{% break %}{% endfor %}",
                "select 1;",
                {"block_end": 1, "loop_skipped": 0},
            ),
            id="first-item-break-is-not-an-empty-loop",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}select {{ n }};{% break %}{% else %}select 0{% endfor %}",
                "select 1;",
                {"block_end": 1, "loop_skipped": 0},
            ),
            id="first-item-break-does-not-run-else",
        ),
        pytest.param(
            (
                "{% for n in [1, 2] %}select {{ n }};"
                "{% if n == 2 %}{% break %}{% endif %}{% endfor %}",
                "select 1;select 2;",
                {"block_end": 4, "loop_iteration": 1},
            ),
            id="later-break-closes-only-active-scopes",
        ),
        pytest.param(
            (
                "{% for outer in [1, 2] %}{% for n in [1, 2] %}{% if true %}"
                "{% break %}{% endif %}{% endfor %}select {{ outer }};{% endfor %}",
                "select 1;select 2;",
                {"block_end": 6, "loop_iteration": 1},
            ),
            id="inner-break-preserves-outer-loop",
        ),
        pytest.param(
            (
                "select 0 {{ return('select 1') }}",
                "select 1",
                {"literals": ""},
            ),
            id="top-level-return-replaces-prefix",
        ),
        pytest.param(
            (
                "{% if true %}discard{{ return('select 1') }}{% endif %}",
                "select 1",
                {"literals": "", "block_end": 0},
            ),
            id="return-discards-unfinished-layout",
        ),
        pytest.param(
            (
                "select {% block b %}0{{ return('1') }}{% endblock %} as n",
                "select 1 as n",
                {"literals": "select  as n"},
            ),
            id="block-return-preserves-surrounding-literals",
        ),
        pytest.param(
            (
                "select {% block b %}discard{{ return('') }}{% endblock %}1 as n",
                "select 1 as n",
                {"literals": "select 1 as n"},
            ),
            id="empty-block-return-removes-stale-events",
        ),
        pytest.param(
            (
                "select {{ probe_value(1) }} as n",
                "select 1 as n",
                {"literals": "select  as n"},
            ),
            id="ordinary-macro-return-keeps-call-site-provenance",
        ),
    ],
)
def test_review_regression_artifact(case, tmp_project, invoke):
    """Tracing must preserve compilation and export usable provenance for each case."""
    source, expected, checks = case
    project = tmp_project("render_trace")
    model = project / "models/probe.sql"
    model.write_text(source, encoding="utf-8", newline="")
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
    assert (project / "normal" / f"{relative}.sql").read_bytes() == (
        project / "traced" / f"{relative}.sql"
    ).read_bytes()
    trace = json.loads((project / "traced" / f"{relative}.render_trace.json").read_bytes())
    manifest = json.loads((project / "traced/manifest.json").read_bytes())
    assert trace["protocol_version"] == 1
    assert trace["render_stage"] == "jinja_before_ephemeral_injection"
    assert trace["unique_id"] == "model.render_trace.probe"
    assert trace["invocation_id"] == manifest["metadata"]["invocation_id"]
    assert trace["source"] == source
    assert trace["rendered"] == expected
    assert_provenance(trace)
    assert_expected_events(trace, checks)


def assert_provenance(trace):
    """Check UTF-8 ranges, exact literals, balanced source scopes and output coverage."""
    source = trace["source"].encode("utf-8")
    rendered = trace["rendered"].encode("utf-8")
    for event in trace["events"]:
        assert_event_ranges(event, source, rendered)
    assert_layout(trace["events"], source)
    covered = bytearray(len(rendered))
    for event in trace["events"]:
        if event["kind"] in {"literal", "templated", "macro"}:
            start, end = event["rendered_start"], event["rendered_end"]
            covered[start:end] = b"\x01" * (end - start)
    assert all(covered), "rendered output lacks provenance"


def assert_event_ranges(event, source, rendered):
    """Offsets must describe half-open UTF-8 slices, never partial characters."""
    start, end = event["source_start"], event["source_end"]
    out_start, out_end = event["rendered_start"], event["rendered_end"]
    assert 0 <= start <= end <= len(source), event
    assert 0 <= out_start <= out_end <= len(rendered), event
    source[:start].decode("utf-8")
    source[start:end].decode("utf-8")
    rendered[:out_start].decode("utf-8")
    rendered[out_start:out_end].decode("utf-8")
    if event["kind"] == "literal":
        assert source[start:end] == rendered[out_start:out_end], event
    elif event["kind"] not in {"templated", "macro"}:
        assert out_start == out_end, event


def assert_layout(events, source):
    """A jump must close the right source scopes in the right nesting order."""
    stack = []
    for event in events:
        kind = event["kind"]
        tag = source[event["source_start"] : event["source_end"]]
        if kind in {"block_start", "loop_iteration"}:
            stack.append(tag.decode("utf-8").removeprefix("{%").lstrip("-+ ").split()[0])
        elif kind in {"block_end", "loop_skipped"}:
            assert stack, event
            expected = f"end{stack.pop()}"
            assert tag.decode("utf-8").removeprefix("{%").lstrip("-+ ").startswith(expected)
    assert not stack, f"unclosed scopes: {stack}"


def assert_expected_events(trace, checks):
    """Pin the event multiplicities and writable literals involved in each bug."""
    counts = Counter(event["kind"] for event in trace["events"])
    source = trace["source"].encode("utf-8")
    literals = b"".join(
        source[event["source_start"] : event["source_end"]]
        for event in trace["events"]
        if event["kind"] == "literal"
    ).decode("utf-8")
    expected = {kind: count for kind, count in checks.items() if kind != "literals"}
    assert {kind: counts[kind] for kind in expected} == expected
    assert literals == checks.get("literals", literals)
