import pytest
from hypothesis import given
from hypothesis import strategies as st

from dbt.contracts.files import FileHash, FilePath, ParseFileType, SourceFile
from dbt.parser.read_files import (
    FileDiff,
    InputFile,
    ReadFilesFromDiff,
    get_source_file_checksum,
    load_source_file,
)
from dbt.parser.sql_whitespace import normalize_sql_whitespace


@pytest.mark.parametrize(
    "contents,expected",
    [
        ("select  1\t as value", "select 1 as value"),
        ("SELECT  1;", "SELECT 1;"),
        (
            "with  cte as (select  1) select  * from cte",
            "with cte as (select 1) select * from cte",
        ),
        ("select  'a  b'  as value", "select 'a  b' as value"),
        ("select  'a\tb'  as value", "select 'a\tb' as value"),
        ("select  'a\nb'  as value", "select 'a\nb' as value"),
        ("select  'a''  b'  as value", "select 'a''  b' as value"),
        ("select  'a//  b'  as value", "select 'a//  b' as value"),
        ("select  $$a//  b$$  as value", "select $$a//  b$$ as value"),
        ('select  "a  b"  from t', 'select "a  b" from t'),
        ('select  "a""  b"  from t', 'select "a""  b" from t'),
        ("select  `a  b`  from t", "select `a  b` from t"),
        ("select  $$a  b$$  as value", "select $$a  b$$ as value"),
        ("select  $tag$a  b$tag$  as value", "select $tag$a  b$tag$ as value"),
        ("select  $$'a'\n  b$$  as value", "select $$'a'\n  b$$ as value"),
        ("select  'a'\n  'b'", "select 'a'\n 'b'"),
        ("select  'a'\r\n  'b'", "select 'a'\r\n 'b'"),
        (
            "select  ts at  time zone  'Pacific  Standard Time'",
            "select ts at time zone 'Pacific  Standard Time'",
        ),
        ("select  'caf\u00e9  value'  as value", "select 'caf\u00e9  value' as value"),
    ],
)
def test_normalize_safe_sql_spacing(contents, expected):
    assert normalize_sql_whitespace(contents) == expected


@pytest.mark.parametrize(
    "contents",
    [
        "",
        "create  table t (a int)",
        "copy  t from stdin;\na  b\n\\.",
        "select  {{ ref('t') }}",
        "select  {{- 1 -}}",
        "{% set q %}a  b{% endset %}select '{{ q }}'",
        "select  1 {# comment #}",
        "select  1 -- comment\n + 2",
        "select  1 // comment\n + 2",
        "select 1 // '\n, 'a  b ' as value // '",
        "select  1 /* nested /* comment */ */ + 2",
        "select  1 # comment\n + 2",
        "select  '''a' b  c 'd''' as value",
        'select  """a" b  c "d""" as value',
        "select  r'''a' b  c 'd''' as value",
        "select  E'a  b' as value",
        "select  N'a  b' as value",
        "select  q'[a' b  c 'd]' as value",
        "select  U&'a  b' as value",
        "select  'a\\' b  c' as value",
        "select  [a  b] from t",
        "select  $1 as value",
        "select  foo$tag$a  b$tag$",
        "select  'a  b",
        'select  "a  b',
        "select  `a  b",
        "select  $tag$a  b",
        "select  1; select 2",
        "select  caf\u00e9 from t",
        "select  1\v as value",
        "select  1\f as value",
    ],
)
def test_unsupported_sql_uses_original_contents(contents):
    assert normalize_sql_whitespace(contents) == contents


@pytest.mark.parametrize(
    "before,after",
    [
        ("select 'a b'", "select 'a  b'"),
        ("select 'a b'", "select 'a\tb'"),
        ("select 'a b'", "select 'a\nb'"),
        ("select $$a b$$", "select $$a  b$$"),
        ('select "a b" from t', 'select "a  b" from t'),
        ("select `a b` from t", "select `a  b` from t"),
        ("select 'a'\n'b'", "select 'a' 'b'"),
        ("select a b", "select ab"),
        ("select - -1", "select --1"),
        (
            "select 1 // '\n, 'a b ' as value // '",
            "select 1 // '\n, 'a  b ' as value // '",
        ),
        (
            "select ts at time zone 'Pacific Standard Time'",
            "select ts at time zone 'Pacific  Standard Time'",
        ),
        ("select '''a' b c 'd'''", "select '''a' b  c 'd'''"),
        (
            "{% set q %}a b{% endset %}select '{{ q }}'",
            "{% set q %}a  b{% endset %}select '{{ q }}'",
        ),
    ],
)
def test_meaningful_whitespace_changes_checksum_input(before, after):
    assert normalize_sql_whitespace(before) != normalize_sql_whitespace(after)


@given(
    spaces=st.lists(st.text(alphabet=" \t", min_size=1, max_size=12), min_size=3, max_size=3),
    literal=st.sampled_from(["'a  b'", "'a\tb'", "'a\nb'", "'a''  b'", "$$a  b$$"]),
)
def test_horizontal_spacing_does_not_change_literal_contents(spaces, literal):
    sql = f"select{spaces[0]}{literal}{spaces[1]}as{spaces[2]}value"
    assert normalize_sql_whitespace(sql) == f"select {literal} as value"


@pytest.mark.parametrize(
    "parse_file_type",
    [
        ParseFileType.Model,
        ParseFileType.Macro,
        ParseFileType.Snapshot,
        ParseFileType.Analysis,
        ParseFileType.SingularTest,
        ParseFileType.GenericTest,
        ParseFileType.Fixture,
        ParseFileType.Function,
    ],
)
@pytest.mark.parametrize("suffix", [".sql", ".sql.j2", ".sql.jinja", ".sql.jinja2"])
def test_sql_file_checksum_scope(parse_file_type, suffix):
    checksum = get_source_file_checksum(
        "\nselect  'a  b' as value\n", f"probe{suffix}", parse_file_type
    )
    assert checksum == FileHash.from_contents("select 'a  b' as value")


@pytest.mark.parametrize(
    "parse_file_type,filename",
    [
        (ParseFileType.Model, "probe.py"),
        (ParseFileType.Function, "probe.py"),
        (ParseFileType.Function, "probe.js"),
        (ParseFileType.Fixture, "probe.csv"),
        (ParseFileType.Seed, "probe.csv"),
        (ParseFileType.Documentation, "probe.md"),
        (ParseFileType.Documentation, "probe.sql"),
        (ParseFileType.Schema, "schema.yml"),
        (ParseFileType.OSI, "probe.json"),
        (None, "probe.sql"),
    ],
)
@pytest.mark.parametrize(
    "contents",
    ["select  'a  b' as value", "\nselect  'a  b' as value\n", " \tselect  'a  b' as value\t "],
)
def test_non_sql_checksum_remains_unnormalized(parse_file_type, filename, contents):
    assert get_source_file_checksum(contents, filename, parse_file_type) == FileHash.from_contents(
        contents
    )


@pytest.mark.parametrize("action", ["added", "changed"])
@pytest.mark.parametrize(
    "filename,contents",
    [
        ("probe.sql", "select  'a  b'  as value"),
        ("probe.sql", "\nselect  'a  b'  as value\n"),
        ("probe.sql", "select  {{ 1 }}  as value"),
        ("probe.version1.sql", "select  'a  b'  as value"),
        ("probe.sql.j2", "select  'a  b'  as value"),
        ("probe.sql.jinja", "select  'a  b'  as value"),
        ("probe.sql.jinja2", "select  'a  b'  as value"),
        ("probe.py", "def model(dbt, session):\n    return 'a  b'"),
    ],
)
def test_diff_and_filesystem_use_same_checksum(
    tmp_path, project, mocker, action, filename, contents
):
    mocker.patch("dbt.parser.read_files.get_flags").return_value.ALLOW_JINJA_FILE_EXTENSIONS = True
    project.project_root = str(tmp_path)
    models_dir = tmp_path / "models"
    models_dir.mkdir()
    (models_dir / filename).write_bytes(contents.encode("utf-8"))
    path = FilePath(
        searched_path="models",
        relative_path=filename,
        modification_time=0.0,
        project_root=str(tmp_path),
    )
    from_filesystem = load_source_file(path, ParseFileType.Model, project.project_name, {})
    old_source = SourceFile(
        path=path,
        checksum=FileHash.from_contents("previous"),
        contents="previous",
        parse_file_type=ParseFileType.Model,
        project_name=project.project_name,
    )
    input_file = InputFile(path=f"models/{filename}", content=contents)
    diff = FileDiff(
        deleted=[],
        changed=[input_file] if action == "changed" else [],
        added=[input_file] if action == "added" else [],
    )
    reader = ReadFilesFromDiff(
        root_project_name=project.project_name,
        all_projects={project.project_name: project},
        file_diff=diff,
        saved_files={old_source.file_id: old_source} if action == "changed" else {},
    )

    reader.read_files()

    from_diff = reader.files[from_filesystem.file_id]
    # Hash parity preserves each reader's existing stored-source contract.
    assert from_diff.contents == contents
    assert from_filesystem.contents == contents.strip()
    assert from_diff.checksum == from_filesystem.checksum


def test_added_sql_alias_requires_extension_flag(project, mocker):
    mocker.patch("dbt.parser.read_files.get_flags").return_value.ALLOW_JINJA_FILE_EXTENSIONS = (
        False
    )
    reader = ReadFilesFromDiff(
        root_project_name=project.project_name,
        all_projects={project.project_name: project},
        file_diff=FileDiff(
            deleted=[],
            changed=[],
            added=[InputFile(path="models/probe.sql.j2", content="select  1")],
        ),
        saved_files={},
    )

    reader.read_files()

    assert reader.files == {}
