"""`show` returns its previewed rows as a chunked Arrow IPC stream.

Column types are covered by the engine's own round-trip tests; this suite has no
Arrow library, so it checks the stream's framing.
"""

from dbt.contracts.results import RunResultsArtifact

# Every message in an Arrow IPC stream, the leading schema message included, opens
# with this continuation marker; the stream ends with a zero-length one.
IPC_CONTINUATION = b"\xff\xff\xff\xff"
IPC_END = IPC_CONTINUATION + b"\x00\x00\x00\x00"


def assert_ipc_stream(preview):
    assert isinstance(preview, list), type(preview)
    for chunk in preview:
        assert isinstance(chunk, bytes), type(chunk)
        assert chunk.startswith(IPC_CONTINUATION), chunk[:16]
    assert preview[-1] == IPC_END


def test_inline_show_returns_arrow_ipc_chunks_within_limit(tmp_project, invoke):
    sql = "select * from (values ('ada'), ('grace'), ('alan')) as t(name) order by name"
    res = invoke(tmp_project("hello_world"), "show", "--inline", sql, "--limit", "2")

    assert res.success, res.exception
    assert isinstance(res.result, RunResultsArtifact), type(res.result)
    assert_ipc_stream(res.preview)
    # Schema, one batch, end marker.
    assert len(res.preview) == 3, len(res.preview)
    # Strings sit uncompressed in the record batch body.
    body = res.preview[1]
    assert b"ada" in body and b"alan" in body
    assert b"grace" not in body


def test_show_select_returns_the_model_rows(built_project, invoke):
    res = invoke(built_project("layered"), "show", "--select", "mart_people")

    assert res.success, res.exception
    assert_ipc_stream(res.preview)
    stream = b"".join(res.preview)
    for name in (b"ADA", b"GRACE", b"ALAN"):
        assert name in stream


def test_failed_show_has_no_preview(tmp_project, invoke):
    res = invoke(tmp_project("hello_world"), "show", "--inline", "select * from no_such_table")

    assert res.success is False
    assert res.preview is None
    assert isinstance(res.result, RunResultsArtifact), type(res.result)
