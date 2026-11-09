import os

import pytest

from dbt.contracts.files import ParseFileType
from dbt.parser.read_files import get_source_files, normalize_file_contents


@pytest.mark.parametrize(
    "file_contents,expected_normalized_file_contents",
    [
        ("", ""),
        (" ", ""),
        ("  ", ""),
        ("\n", ""),
        ("a b", "a b"),
        ("a  b", "a b"),
        ("a\nb", "a b"),
        ("a\n b", "a b"),
        ("a b ", "a b"),
        ("  a b  ", "a b"),
        ("\na b\n", "a b"),
        ("\n\na b\n\n", "a b"),
    ],
)
def test_normalize_file_contents(file_contents: str, expected_normalized_file_contents: str):
    assert normalize_file_contents(file_contents) == expected_normalized_file_contents


class _FakeProject:
    def __init__(self, project_root, test_paths):
        self.project_root = str(project_root)
        self.project_name = "repro"
        self.test_paths = test_paths
        self.generic_test_paths = [os.path.join(p, "generic") for p in test_paths]
        self.fixture_paths = [os.path.join(p, "fixtures") for p in test_paths]


def _write(path, contents="select 1 as id"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents)


@pytest.mark.parametrize(
    "test_paths",
    [
        ["tests", "tests/dbt"],
        ["tests/dbt", "tests"],
        ["tests/", "./tests/dbt"],
    ],
)
def test_singular_tests_skip_fixtures_and_generic_tests_in_nested_test_paths(tmp_path, test_paths):
    _write(tmp_path / "tests" / "real_singular_test.sql")
    _write(tmp_path / "tests" / "dbt" / "nested_singular_test.sql")
    _write(tmp_path / "tests" / "fixtures" / "outer_fixture.sql")
    _write(tmp_path / "tests" / "dbt" / "fixtures" / "foo" / "my_model__expected.sql")
    _write(tmp_path / "tests" / "dbt" / "generic" / "my_generic_test.sql")
    project = _FakeProject(tmp_path, test_paths)

    source_files = get_source_files(
        project, project.test_paths, ".sql", ParseFileType.SingularTest, {}, None
    )

    original_file_paths = {os.path.normpath(sf.path.original_file_path) for sf in source_files}
    assert original_file_paths == {
        os.path.join("tests", "real_singular_test.sql"),
        os.path.join("tests", "dbt", "nested_singular_test.sql"),
    }
