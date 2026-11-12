import json

import pytest

from dbt.cli.main import dbtRunner
from dbt.contracts.files import FileHash, FilePath, ParseFileType
from dbt.parser.read_files import ReadFilesFromFileSystem, load_source_file
from dbt.tests.util import safe_set_invocation_context


@pytest.mark.parametrize(
    "parse_file_type,filename,contents",
    [
        (ParseFileType.Model, "probe.sql", "select 'a  b' as value"),
        (ParseFileType.Model, "probe.sql", "select 'a\tb' as value"),
        (ParseFileType.Model, "probe.sql", "select 'a\nb' as value"),
        (ParseFileType.Model, "probe.sql", "select $$a  b$$ as value"),
        (ParseFileType.Model, "probe.sql", 'select "a  b" from my_table'),
        (ParseFileType.Model, "probe.sql", "select 1 -- comment\n + 2 as value"),
        (ParseFileType.Model, "probe.sql", "select\r\n'a  b' as value"),
        (ParseFileType.Model, "probe.py", "def model(dbt, session):\n    return 'a  b'"),
        (ParseFileType.Macro, "probe.sql", "{% macro probe() %}'a  b'{% endmacro %}"),
        (
            ParseFileType.Schema,
            "schema.yml",
            "version: 2\nmodels:\n  - name: probe\n    description: a  b",
        ),
        (ParseFileType.Fixture, "probe.csv", "value\na  b"),
        (ParseFileType.Documentation, "probe.md", "{% docs probe %}a  b{% enddocs %}"),
    ],
)
def test_load_source_file_preserves_internal_whitespace(
    tmp_path, parse_file_type, filename, contents
):
    safe_set_invocation_context()
    (tmp_path / filename).write_bytes(f"\n{contents}\n".encode("utf-8"))
    path = FilePath(
        searched_path="",
        relative_path=filename,
        modification_time=0.0,
        project_root=str(tmp_path),
    )

    source_file = load_source_file(path, parse_file_type, "test_project", {})

    assert source_file.contents == contents
    assert source_file.checksum == FileHash.from_contents(contents)


def test_read_osi_file_preserves_internal_whitespace(tmp_path, project):
    project.project_root = str(tmp_path)
    project.osi_paths = ["osi"]
    osi_dir = tmp_path / "osi"
    osi_dir.mkdir()
    contents = '{"description": "a  b"}'
    (osi_dir / "probe.json").write_text(contents, encoding="utf-8")
    reader = ReadFilesFromFileSystem(all_projects={project.project_name: project})

    reader._read_osi_files_for_project(project)

    source_file = reader.files[f"{project.project_name}://osi/probe.json"]
    assert source_file.contents == contents
    assert source_file.checksum == FileHash.from_contents(contents)


@pytest.mark.parametrize(
    "original_sql,updated_sql,changed",
    [
        ("select 'a b' as value", "select 'a  b' as value", True),
        ("select 'a b' as value", "select 'a\tb' as value", True),
        ("select 'a b' as value", "select 'a\nb' as value", True),
        ("select $$a b$$ as value", "select $$a  b$$ as value", True),
        ('select "a b" from t', 'select "a  b" from t', True),
        ("select '''a' b c 'd''' as value", "select '''a' b  c 'd''' as value", True),
        (
            "select ts at time zone 'Pacific Standard Time'",
            "select ts at time zone 'Pacific  Standard Time'",
            True,
        ),
        ("select 'a b' as value", "select  'a b'  as value", False),
        ("select 'a b' as value", "select\t'a b'\tas value", False),
        ("select 'a b' as value", "select\n'a b' as value", True),
        (
            "{% set q %}a b{% endset %}select '{{ q }}' as value",
            "{% set q %}a  b{% endset %}select '{{ q }}' as value",
            True,
        ),
        ("select {{ 1 }} as value", "select  {{ 1 }} as value", True),
        ("select 1 -- comment\n as value", "select  1 -- comment\n as value", True),
        (
            "select 1 // '\n, 'a b ' as value // '",
            "select 1 // '\n, 'a  b ' as value // '",
            True,
        ),
    ],
    ids=[
        "literal-spaces",
        "literal-tab",
        "literal-newline",
        "dollar-quoted-literal",
        "quoted-identifier",
        "triple-quote-fallback",
        "timezone-literal",
        "formatting-spaces",
        "formatting-tabs",
        "formatting-newline",
        "jinja-capture",
        "jinja-fallback",
        "comment-fallback",
        "snowflake-comment-literal",
    ],
)
def test_partial_parse_sql_whitespace(tmp_path, original_sql, updated_sql, changed):
    # Parsing needs adapter credentials, but never connects to a database.
    profiles_dir = tmp_path / "profiles"
    profiles_dir.mkdir()
    (profiles_dir / "profiles.yml").write_text(
        "test:\n"
        "  target: dev\n"
        "  outputs:\n"
        "    dev:\n"
        "      type: postgres\n"
        "      host: localhost\n"
        "      user: test\n"
        "      password: test\n"
        "      port: 5432\n"
        "      dbname: test\n"
        "      schema: test\n"
        "      threads: 1\n",
        encoding="utf-8",
    )
    (tmp_path / "dbt_project.yml").write_text(
        "name: test\nversion: '1.0'\nconfig-version: 2\nprofile: test\n",
        encoding="utf-8",
    )
    models_dir = tmp_path / "models"
    models_dir.mkdir()
    model_path = models_dir / "probe.sql"
    model_path.write_bytes(original_sql.encode("utf-8"))
    target_dir = tmp_path / "target"
    args = [
        "parse",
        "--project-dir",
        str(tmp_path),
        "--profiles-dir",
        str(profiles_dir),
        "--target-path",
        str(target_dir),
        "--log-path",
        str(tmp_path / "logs"),
        "--no-use-v2-parser",
        "--no-send-anonymous-usage-stats",
    ]
    events = []
    runner = dbtRunner(callbacks=[events.append])

    first = runner.invoke([*args, "--partial-parse"])
    assert first.success, first.exception
    original_node = first.result.nodes["model.test.probe"]
    assert original_node.raw_code == original_sql
    assert (target_dir / "partial_parse.msgpack").exists()
    state_dir = tmp_path / "state"
    state_dir.mkdir()
    (state_dir / "manifest.json").write_bytes((target_dir / "manifest.json").read_bytes())

    model_path.write_bytes(updated_sql.encode("utf-8"))
    events.clear()
    updated = runner.invoke([*args, "--partial-parse"])
    assert updated.success, updated.exception
    assert not any(
        event.info.name in ("UnableToPartialParse", "ParsedFileLoadFailed") for event in events
    )
    partial_parse_events = [
        event for event in events if event.info.name == "PartialParsingEnabled"
    ]
    assert len(partial_parse_events) == 1
    partial_parse_data = partial_parse_events[0].data
    assert (partial_parse_data.deleted, partial_parse_data.added, partial_parse_data.changed) == (
        0,
        0,
        int(changed),
    )
    updated_node = updated.result.nodes["model.test.probe"]
    cached_sql = updated_sql if changed else original_sql
    assert updated_node.raw_code == cached_sql
    assert (updated_node.checksum != original_node.checksum) == changed
    assert updated_node.same_body(original_node) != changed
    manifest = json.loads((target_dir / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["nodes"]["model.test.probe"]["raw_code"] == cached_sql

    selected = runner.invoke(
        [
            "ls",
            *args[1:],
            "--partial-parse",
            "--state",
            str(state_dir),
            "--select",
            "state:modified",
            "--output",
            "name",
        ]
    )
    assert selected.success, selected.exception
    assert selected.result == (["probe"] if changed else [])

    # Reused nodes keep their prior formatting; a full parse reads current text.
    # Both paths must agree on the content-sensitive checksum.
    for flag in ("--partial-parse", "--no-partial-parse"):
        result = runner.invoke([*args, flag])
        assert result.success, result.exception
        node = result.result.nodes["model.test.probe"]
        assert node.raw_code == (cached_sql if flag == "--partial-parse" else updated_sql)
        assert node.checksum == updated_node.checksum
