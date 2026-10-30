import time
from copy import deepcopy
from typing import Dict, List
from unittest import mock

import pytest

from dbt.contracts.files import (
    BaseSourceFile,
    FileHash,
    FilePath,
    ParseFileType,
    SchemaSourceFile,
    SourceFile,
)
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import SnapshotNode, SourceDefinition
from dbt.node_types import NodeType
from dbt.parser.partial import PartialParsing
from dbt.tests.util import safe_set_invocation_context
from tests.unit.utils import normalize
from tests.unit.utils.manifest import (
    make_generic_test,
    make_manifest,
    make_model,
    make_singular_test,
    make_source,
    make_source_snapshot,
)

PROJECT_NAME = "my_test"


@pytest.fixture
def singular_test():
    st = make_singular_test(
        PROJECT_NAME, "my_singular_test", "select 1 where false", path="tests/my_singular_test.sql"
    )
    st.patch_path = "my_test://tests/tests.yml"
    return st


@pytest.fixture
def source() -> SourceDefinition:
    return make_source(PROJECT_NAME, "my_source", "my_source_table", path="models/schema.yml")


@pytest.fixture
def source_snapshot(source) -> SnapshotNode:
    return make_source_snapshot(
        PROJECT_NAME, "my_test_source_snapshot", source, path="models/schema.yml"
    )


@pytest.fixture
def files(singular_test, source, source_snapshot) -> Dict[str, BaseSourceFile]:
    project_root = "/users/root"
    sql_model_file = SourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="models",
            relative_path="my_model.sql",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("abcdef"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.Model,
        nodes=["model.my_test.my_model"],
        env_vars=[],
    )
    sql_model_file_untouched = SourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="models",
            relative_path="my_model_untouched.sql",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("abcdef"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.Model,
        nodes=["model.my_test.my_model_untouched"],
        env_vars=[],
    )

    python_model_file = SourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="models",
            relative_path="python_model.py",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("lalala"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.Model,
        nodes=["model.my_test.python_model"],
        env_vars=[],
    )
    python_model_file_untouched = SourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="models",
            relative_path="python_model_untouched.py",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("lalala"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.Model,
        nodes=["model.my_test.python_model_untouched"],
        env_vars=[],
    )
    schema_file = SchemaSourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="models",
            relative_path="schema.yml",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("ghijkl"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.Schema,
        dfy={
            "version": 2,
            "models": [
                {"name": "my_model", "description": "Test model"},
                {"name": "python_model", "description": "python"},
                {"name": "not_null", "model": "test.my_test.test_my_model"},
            ],
            "sources": [
                {"name": source.source_name, "tables": [{"name": source.name}]},
            ],
            "snapshots": [
                {
                    "name": source_snapshot.name,
                    "relation": f"source('{source.source_name}', '{source.name}')",
                },
            ],
        },
        ndp=["model.my_test.my_model"],
        env_vars={},
        data_tests={"models": {"not_null": {"test.my_test.test_my_model": []}}},
        snapshots=[f"snapshot.{PROJECT_NAME}.{source_snapshot.name}"],
        sources=[f"source.{PROJECT_NAME}.{source.source_name}.{source.name}"],
    )
    singular_test_sql_file = SourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="tests",
            relative_path=f"{singular_test.name}.sql",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("a test"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.SingularTest,
        nodes=[f"test.{PROJECT_NAME}.{singular_test.name}"],
        env_vars=[],
    )
    singular_test_yml_file = SourceFile(
        path=FilePath(
            project_root=project_root,
            searched_path="tests",
            relative_path="tests.yml",
            modification_time=time.time(),
        ),
        checksum=FileHash.from_contents("a test"),
        project_name=PROJECT_NAME,
        parse_file_type=ParseFileType.Schema,
        env_vars=[],
    )
    return {
        schema_file.file_id: schema_file,
        sql_model_file.file_id: sql_model_file,
        sql_model_file_untouched.file_id: sql_model_file_untouched,
        python_model_file.file_id: python_model_file,
        python_model_file_untouched.file_id: python_model_file_untouched,
        singular_test_sql_file.file_id: singular_test_sql_file,
        singular_test_yml_file.file_id: singular_test_yml_file,
    }


@pytest.fixture
def nodes(singular_test, source, source_snapshot) -> List[NodeType]:
    patch_path = "my_test://" + normalize("models/schema.yml")
    my_model = make_model(PROJECT_NAME, "my_model", "", patch_path=patch_path)
    return [
        my_model,
        make_model(PROJECT_NAME, "my_model_untouched", "", patch_path=patch_path),
        make_model(PROJECT_NAME, "python_model", "", language="python", patch_path=patch_path),
        make_model(
            PROJECT_NAME, "python_model_untouched", "", language="python", patch_path=patch_path
        ),
        make_generic_test(PROJECT_NAME, "test", my_model, {}),
        singular_test,
        source,
        source_snapshot,
    ]


@pytest.fixture
def manifest(files, nodes, source) -> Manifest:
    return make_manifest(files=files, nodes=nodes, sources=[source])


@pytest.fixture
def partial_parsing(manifest, files):
    safe_set_invocation_context()
    return PartialParsing(manifest, deepcopy(files))


def test_simple(partial_parsing, files, nodes):
    # Nothing has changed
    assert partial_parsing is not None
    assert partial_parsing.skip_parsing() is True

    # Change a model file
    sql_model_file_id = "my_test://" + normalize("models/my_model.sql")
    partial_parsing.new_files[sql_model_file_id].checksum = FileHash.from_contents("xyzabc")

    python_model_file_id = "my_test://" + normalize("models/python_model.py")
    partial_parsing.new_files[python_model_file_id].checksum = FileHash.from_contents("ohohoh")

    partial_parsing.build_file_diff()
    assert partial_parsing.skip_parsing() is False
    pp_files = partial_parsing.get_parsing_files()
    pp_files["my_test"]["ModelParser"] = set(pp_files["my_test"]["ModelParser"])
    # models has 'patch_path' so we expect to see a SchemaParser file listed
    schema_file_id = "my_test://" + normalize("models/schema.yml")
    expected_pp_files = {
        "my_test": {
            "ModelParser": set([sql_model_file_id, python_model_file_id]),
            "SchemaParser": [schema_file_id],
        }
    }
    assert pp_files == expected_pp_files
    schema_file = files[schema_file_id]
    schema_file_model_names = set([model["name"] for model in schema_file.pp_dict["models"]])
    expected_model_names = set(["python_model", "my_model"])
    assert schema_file_model_names == expected_model_names
    schema_file_model_descriptions = set(
        [model["description"] for model in schema_file.pp_dict["models"]]
    )
    expected_model_descriptions = set(["Test model", "python"])
    assert schema_file_model_descriptions == expected_model_descriptions


def test_schedule_nodes_for_parsing_basic(partial_parsing, nodes):
    assert partial_parsing.file_diff["deleted"] == []
    assert partial_parsing.project_parser_files == {}
    partial_parsing.schedule_nodes_for_parsing([nodes[0].unique_id])
    assert partial_parsing.project_parser_files == {
        "my_test": {
            "ModelParser": ["my_test://models/my_model.sql"],
            "SchemaParser": ["my_test://models/schema.yml"],
        }
    }


def test_schedule_macro_nodes_for_parsing_basic(partial_parsing):
    # XXX it seems kind of confusing what exactly this function does.
    # Whoever Changes this function please add more comment.

    # this rely on the dfy and data_tests fields in schema node to add schema file to reparse
    partial_parsing.schedule_macro_nodes_for_parsing(["test.my_test.test_my_model"])
    assert partial_parsing.project_parser_files == {
        "my_test": {"SchemaParser": ["my_test://models/schema.yml"]}
    }


def test_schedule_nodes_for_parsing_versioning(partial_parsing, source) -> None:
    # Modify schema file to add versioning
    schema_file_id = "my_test://" + normalize("models/schema.yml")
    partial_parsing.new_files[schema_file_id].checksum = FileHash.from_contents("changed")
    partial_parsing.new_files[schema_file_id].dfy = {
        "version": 2,
        "models": [
            {
                "name": "my_model",
                "description": "Test model",
                "latest_version": 1,
                "versions": [{"v": 1}],
            },
            {"name": "python_model", "description": "python"},
            {"name": "not_null", "model": "test.my_test.test_my_model"},
        ],
        "sources": [
            {"name": source.source_name, "tables": [{"name": source.name}]},
        ],
    }
    with mock.patch.object(
        partial_parsing, "schedule_referencing_nodes_for_parsing"
    ) as mock_schedule_referencing_nodes_for_parsing:
        partial_parsing.build_file_diff()
        partial_parsing.get_parsing_files()

        mock_schedule_referencing_nodes_for_parsing.assert_called_once_with(
            "model.my_test.my_model"
        )


class TestFileDiff:
    @pytest.fixture
    def partial_parsing(self, manifest, files):
        safe_set_invocation_context()
        saved_files = deepcopy(files)
        saved_files["my_test://models/python_model_untouched.py"].checksum = (
            FileHash.from_contents("something new")
        )
        saved_files["my_test://models/schema.yml"].dfy["sources"][0]["tables"][0][
            "description"
        ] = "test"
        saved_files["my_test://models/schema.yml"].checksum = FileHash.from_contents(
            "something new"
        )
        saved_files["my_test://tests/my_singular_test.sql"].checksum = FileHash.from_contents(
            "something new"
        )
        return PartialParsing(manifest, saved_files)

    def test_build_file_diff_basic(self, partial_parsing):
        partial_parsing.build_file_diff()
        assert set(partial_parsing.file_diff["unchanged"]) == {
            "my_test://models/my_model_untouched.sql",
            "my_test://models/my_model.sql",
            "my_test://models/python_model.py",
            "my_test://tests/tests.yml",
        }
        assert set(partial_parsing.file_diff["changed"]) == {
            "my_test://models/python_model_untouched.py",
            "my_test://tests/my_singular_test.sql",
        }
        assert partial_parsing.file_diff["changed_schema_files"] == [
            "my_test://models/schema.yml",
        ]

    def test_get_parsing_files(self, partial_parsing):
        project_parser_files = partial_parsing.get_parsing_files()
        assert project_parser_files == {
            PROJECT_NAME: {
                "ModelParser": ["my_test://models/python_model_untouched.py"],
                "SchemaParser": ["my_test://models/schema.yml"],
                "SingularTestParser": ["my_test://tests/my_singular_test.sql"],
            },
        }


def test_remove_tests_removes_disabled_generic_test(partial_parsing, nodes):
    # CORE-725: a GenericTestNode that is currently disabled (config.enabled:
    # false) lives in saved_manifest.disabled rather than saved_manifest.nodes,
    # but is still tracked in schema_file.data_tests. remove_tests must clean
    # up the disabled copy too, otherwise ghost copies accumulate across
    # partial parses.
    schema_file_id = "my_test://" + normalize("models/schema.yml")
    schema_file = partial_parsing.saved_manifest.files[schema_file_id]

    my_model = nodes[0]
    disabled_test = make_generic_test(PROJECT_NAME, "unique", my_model, {}, column_name="id")
    disabled_test.file_key_name = "models.my_model"
    disabled_test.original_file_path = normalize("models/schema.yml")

    schema_file.add_test(disabled_test.unique_id, {"key": "models", "name": "my_model"})
    partial_parsing.saved_manifest.disabled[disabled_test.unique_id] = [disabled_test]

    partial_parsing.remove_tests(schema_file, "models", "my_model")

    assert disabled_test.unique_id not in partial_parsing.saved_manifest.disabled


def test_delete_schema_data_test_patch_removes_disabled_singular_test(partial_parsing):
    # CORE-725: a disabled singular data test lives in saved_manifest.disabled
    # rather than saved_manifest.nodes. delete_schema_data_test_patch must find
    # it via schema_file.node_patches and remove the stale disabled copy so it
    # does not accumulate across partial parses.
    schema_file_id = "my_test://" + normalize("models/schema.yml")
    schema_file = partial_parsing.saved_manifest.files[schema_file_id]

    disabled_test = make_singular_test(
        PROJECT_NAME,
        "my_disabled_singular_test",
        "select 1 where false",
        path="my_disabled_singular_test.sql",
    )

    schema_file.node_patches.append(disabled_test.unique_id)
    partial_parsing.saved_manifest.disabled[disabled_test.unique_id] = [disabled_test]
    assert disabled_test.unique_id not in partial_parsing.saved_manifest.nodes

    partial_parsing.delete_schema_data_test_patch(
        schema_file, {"name": "my_disabled_singular_test"}
    )

    assert disabled_test.unique_id not in partial_parsing.saved_manifest.disabled
