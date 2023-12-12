import os
import unittest
from copy import deepcopy
from unittest import mock

import yaml

import dbt.flags
import dbt.parser
from dbt import tracking
from dbt.context.context_config import ContextConfig
from dbt.contracts.files import SourceFile, FileHash, FilePath, SchemaSourceFile
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.model_config import NodeConfig, TestConfig, SnapshotConfig, ModelConfig
from dbt.contracts.graph.nodes import (
    ModelNode,
    Macro,
    DependsOn,
    SingularTestNode,
    SnapshotNode,
    AnalysisNode,
    UnpatchedSourceDefinition,
    RefArgs,
)
from dbt.exceptions import CompilationError, ParsingError
from dbt.node_types import NodeType
from dbt.parser import (
    ModelParser,
    MacroParser,
    SingularTestParser,
    GenericTestParser,
    SchemaParser,
    SnapshotParser,
    AnalysisParser,
)
from dbt.parser.common import YamlBlock
from dbt.parser.models import (
    _get_config_call_dict,
    _shift_sources,
    _get_exp_sample_result,
    _get_stable_sample_result,
    _get_sample_result,
)
from dbt.parser.schemas import (
    TestablePatchParser,
    ModelPatchParser,
    SourceParser,
    AnalysisPatchParser,
    MacroPatchParser,
    yaml_from_file,
)
from dbt.parser.search import FileBlock
from dbt.parser.sources import SourcePatcher
from .utils import config_from_parts_or_dicts, normalize, generate_name_macros, MockNode
from dbt.flags import set_from_args
from argparse import Namespace

set_from_args(Namespace(WARN_ERROR=False), None)

dbt.flags.set_from_args(Namespace(SEND_ANONYMOUS_USAGE_STATS=False, MACRO_DEBUGGING=False), None)


def get_abs_os_path(unix_path):
    return normalize(os.path.abspath(unix_path))


class BaseParserTest(unittest.TestCase):
    maxDiff = None

    def _generate_macros(self):
        name_sql = {}
        for component in ("database", "schema", "alias"):
            if component == "alias":
                source = "node.name"
            else:
                source = f"target.{component}"
            name = f"generate_{component}_name"
            sql = f"{{% macro {name}(value, node) %}} {{% if value %}} {{{{ value }}}} {{% else %}} {{{{ {source} }}}} {{% endif %}} {{% endmacro %}}"
            name_sql[name] = sql

        for name, sql in name_sql.items():
            pm = Macro(
                name=name,
                resource_type=NodeType.Macro,
                unique_id=f"macro.root.{name}",
                package_name="root",
                original_file_path=normalize("macros/macro.sql"),
                path=normalize("macros/macro.sql"),
                macro_sql=sql,
            )
            yield pm

    def setUp(self):
        dbt.flags.WARN_ERROR = True
        # HACK: this is needed since tracking events can
        # be sent when using the model parser
        tracking.do_not_track()

        self.maxDiff = None

        profile_data = {
            "target": "test",
            "quoting": {},
            "outputs": {
                "test": {
                    "type": "postgres",
                    "host": "localhost",
                    "schema": "analytics",
                    "user": "test",
                    "pass": "test",
                    "dbname": "test",
                    "port": 1,
                }
            },
        }

        root_project = {
            "name": "root",
            "version": "0.1",
            "profile": "test",
            "project-root": normalize("/usr/src/app"),
            "config-version": 2,
        }

        self.root_project_config = config_from_parts_or_dicts(
            project=root_project, profile=profile_data, cli_vars={"test_schema_name": "foo"}
        )

        snowplow_project = {
            "name": "snowplow",
            "version": "0.1",
            "profile": "test",
            "project-root": get_abs_os_path("./dbt_packages/snowplow"),
            "config-version": 2,
        }

        self.snowplow_project_config = config_from_parts_or_dicts(
            project=snowplow_project, profile=profile_data
        )

        self.all_projects = {
            "root": self.root_project_config,
            "snowplow": self.snowplow_project_config,
        }

        self.root_project_config.dependencies = self.all_projects
        self.snowplow_project_config.dependencies = self.all_projects
        self.patcher = mock.patch("dbt.context.providers.get_adapter")
        self.factory = self.patcher.start()

        self.parser_patcher = mock.patch("dbt.parser.base.get_adapter")
        self.factory_parser = self.parser_patcher.start()

        self.manifest = Manifest(
            macros={m.unique_id: m for m in generate_name_macros("root")},
        )

    def tearDown(self):
        self.parser_patcher.stop()
        self.patcher.stop()

    def file_block_for(self, data: str, filename: str, searched: str):
        root_dir = get_abs_os_path("./dbt_packages/snowplow")
        filename = normalize(filename)
        path = FilePath(
            searched_path=searched,
            relative_path=filename,
            project_root=root_dir,
            modification_time=0.0,
        )
        sf_cls = SchemaSourceFile if filename.endswith(".yml") else SourceFile
        source_file = sf_cls(
            path=path,
            checksum=FileHash.from_contents(data),
            project_name="snowplow",
        )
        source_file.contents = data
        return FileBlock(file=source_file)

    def assert_has_manifest_lengths(
        self, manifest, macros=3, nodes=0, sources=0, docs=0, disabled=0
    ):
        self.assertEqual(len(manifest.macros), macros)
        self.assertEqual(len(manifest.nodes), nodes)
        self.assertEqual(len(manifest.sources), sources)
        self.assertEqual(len(manifest.docs), docs)
        self.assertEqual(len(manifest.disabled), disabled)


def assertEqualNodes(node_one, node_two):
    node_one_dict = node_one.to_dict()
    if "created_at" in node_one_dict:
        del node_one_dict["created_at"]
    if "relation_name" in node_one_dict:
        del node_one_dict["relation_name"]
    node_two_dict = node_two.to_dict()
    if "created_at" in node_two_dict:
        del node_two_dict["created_at"]
    if "relation_name" in node_two_dict:
        del node_two_dict["relation_name"]
    # we don't reall care the order of packages, doing this because it is hard to
    # make config.packages a set instead of a list
    if "config" in node_one_dict and "packages" in node_one_dict["config"]:
        if "config" not in node_two_dict and "packages" in node_two_dict["config"]:
            return False
        node_one_dict["config"]["packages"] = set(node_one_dict["config"]["packages"])
        node_two_dict["config"]["packages"] = set(node_two_dict["config"]["packages"])
        node_one_dict["unrendered_config"]["packages"] = set(node_one_dict["config"]["packages"])
        node_two_dict["unrendered_config"]["packages"] = set(node_two_dict["config"]["packages"])
        if "packages" in node_one_dict["config_call_dict"]:
            node_one_dict["config_call_dict"]["packages"] = set(
                node_one_dict["config_call_dict"]["packages"]
            )
            node_two_dict["config_call_dict"]["packages"] = set(
                node_two_dict["config_call_dict"]["packages"]
            )

    assert node_one_dict == node_two_dict


SINGLE_TABLE_SOURCE = """
sources:
    - name: my_source
      tables:
        - name: my_table
"""

SINGLE_TABLE_SOURCE_TESTS = """
sources:
    - name: my_source
      tables:
        - name: my_table
          description: A description of my table
          columns:
            - name: color
              tests:
                - not_null:
                    severity: WARN
                - accepted_values:
                    values: ['red', 'blue', 'green']
"""

SINGLE_TABLE_MODEL_TESTS = """
models:
    - name: my_model
      description: A description of my model
      columns:
        - name: color
          description: The color value
          tests:
            - not_null:
                severity: WARN
            - accepted_values:
                values: ['red', 'blue', 'green']
            - foreign_package.test_case:
                arg: 100
"""


MULTIPLE_TABLE_VERSIONED_MODEL_TESTS = """
models:
    - name: my_model
      description: A description of my model
      tests:
        - unique:
            column_name: color
      columns:
        - name: color
          description: The color value
          tests:
            - not_null:
                severity: WARN
        - name: location_id
          data_type: int
      versions:
        - v: 1
          defined_in: arbitrary_file_name
          tests: []
          columns:
            - include: '*'
            - name: extra
        - v: 2
          columns:
            - include: '*'
              exclude: ['location_id']
            - name: extra
"""

MULTIPLE_TABLE_VERSIONED_MODEL = """
models:
    - name: my_model
      description: A description of my model
      config:
        materialized: table
        sql_header: test_sql_header
      columns:
        - name: color
          description: The color value
        - name: location_id
          data_type: int
      versions:
        - v: 1
          defined_in: arbitrary_file_name
          columns:
            - include: '*'
            - name: extra
        - v: 2
          config:
            materialized: view
          columns:
            - include: '*'
              exclude: ['location_id']
            - name: extra
"""

MULTIPLE_TABLE_VERSIONED_MODEL_CONTRACT_ENFORCED = """
models:
    - name: my_model
      config:
        contract:
            enforced: true
      versions:
        - v: 0
          defined_in: arbitrary_file_name
        - v: 2
"""

MULTIPLE_TABLE_VERSIONED_MODEL_V0 = """
models:
    - name: my_model
      versions:
        - v: 0
          defined_in: arbitrary_file_name
        - v: 2
"""


MULTIPLE_TABLE_VERSIONED_MODEL_V0_LATEST_VERSION = """
models:
    - name: my_model
      latest_version: 0
      versions:
        - v: 0
          defined_in: arbitrary_file_name
        - v: 2
"""


SINGLE_TABLE_SOURCE_PATCH = """
sources:
  - name: my_source
    overrides: snowplow
    tables:
      - name: my_table
        columns:
          - name: id
            tests:
              - not_null
              - unique
"""


class SchemaParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = SchemaParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )
        self.source_patcher = SourcePatcher(
            root_project=self.root_project_config,
            manifest=self.manifest,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "models")

    def yaml_block_for(self, test_yml: str, filename: str):
        file_block = self.file_block_for(data=test_yml, filename=filename)
        return YamlBlock.from_file_block(
            src=file_block,
            data=yaml.safe_load(test_yml),
        )


class SchemaParserSourceTest(SchemaParserTest):
    def test__read_basic_source(self):
        block = self.yaml_block_for(SINGLE_TABLE_SOURCE, "test_one.yml")
        analysis_blocks = AnalysisPatchParser(self.parser, block, "analyses").parse().test_blocks
        model_blocks = ModelPatchParser(self.parser, block, "models").parse().test_blocks
        source_blocks = SourceParser(self.parser, block, "sources").parse().test_blocks
        macro_blocks = MacroPatchParser(self.parser, block, "macros").parse().test_blocks
        self.assertEqual(len(analysis_blocks), 0)
        self.assertEqual(len(model_blocks), 0)
        self.assertEqual(len(source_blocks), 0)
        self.assertEqual(len(macro_blocks), 0)
        self.assertEqual(len(list(self.parser.manifest.nodes)), 0)
        source_values = list(self.parser.manifest.sources.values())
        self.assertEqual(len(source_values), 1)
        self.assertEqual(source_values[0].source.name, "my_source")
        self.assertEqual(source_values[0].table.name, "my_table")
        self.assertEqual(source_values[0].table.description, "")
        self.assertEqual(len(source_values[0].table.columns), 0)

    def test__parse_basic_source(self):
        block = self.file_block_for(SINGLE_TABLE_SOURCE, "test_one.yml")
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, sources=1)
        src = list(self.parser.manifest.sources.values())[0]
        assert isinstance(src, UnpatchedSourceDefinition)
        assert src.package_name == "snowplow"
        assert src.source.name == "my_source"
        assert src.table.name == "my_table"
        assert src.resource_type == NodeType.Source
        assert src.fqn == ["snowplow", "my_source", "my_table"]

    def test__read_basic_source_tests(self):
        block = self.yaml_block_for(SINGLE_TABLE_SOURCE_TESTS, "test_one.yml")
        analysis_tests = AnalysisPatchParser(self.parser, block, "analyses").parse().test_blocks
        model_tests = ModelPatchParser(self.parser, block, "models").parse().test_blocks
        source_tests = SourceParser(self.parser, block, "sources").parse().test_blocks
        macro_tests = MacroPatchParser(self.parser, block, "macros").parse().test_blocks
        self.assertEqual(len(analysis_tests), 0)
        self.assertEqual(len(model_tests), 0)
        self.assertEqual(len(source_tests), 0)
        self.assertEqual(len(macro_tests), 0)
        self.assertEqual(len(list(self.parser.manifest.nodes)), 0)
        self.assertEqual(len(list(self.parser.manifest.source_patches)), 0)
        source_values = list(self.parser.manifest.sources.values())
        self.assertEqual(len(source_values), 1)
        self.assertEqual(source_values[0].source.name, "my_source")
        self.assertEqual(source_values[0].table.name, "my_table")
        self.assertEqual(source_values[0].table.description, "A description of my table")
        self.assertEqual(len(source_values[0].table.columns), 1)

    def test__parse_basic_source_tests(self):
        block = self.file_block_for(SINGLE_TABLE_SOURCE_TESTS, "test_one.yml")
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assertEqual(len(self.parser.manifest.nodes), 0)
        self.assertEqual(len(self.parser.manifest.sources), 1)
        src = list(self.parser.manifest.sources.values())[0]
        self.assertEqual(src.source.name, "my_source")
        self.assertEqual(src.source.schema, None)
        self.assertEqual(src.table.name, "my_table")
        self.assertEqual(src.table.description, "A description of my table")

        tests = [
            self.source_patcher.parse_source_test(src, test, col) for test, col in src.get_tests()
        ]
        tests.sort(key=lambda n: n.unique_id)

        self.assertEqual(tests[0].config.severity, "ERROR")
        self.assertEqual(tests[0].tags, [])
        self.assertEqual(tests[0].sources, [["my_source", "my_table"]])
        self.assertEqual(tests[0].column_name, "color")
        self.assertEqual(tests[0].fqn, ["snowplow", tests[0].name])
        self.assertEqual(tests[1].config.severity, "WARN")
        self.assertEqual(tests[1].tags, [])
        self.assertEqual(tests[1].sources, [["my_source", "my_table"]])
        self.assertEqual(tests[1].column_name, "color")
        self.assertEqual(tests[1].fqn, ["snowplow", tests[1].name])

        file_id = "snowplow://" + normalize("models/test_one.yml")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(self.parser.manifest.files[file_id].tests, {})
        self.assertEqual(
            self.parser.manifest.files[file_id].sources, ["source.snowplow.my_source.my_table"]
        )
        self.assertEqual(self.parser.manifest.files[file_id].source_patches, [])

    def test__read_source_patch(self):
        block = self.yaml_block_for(SINGLE_TABLE_SOURCE_PATCH, "test_one.yml")
        analysis_tests = AnalysisPatchParser(self.parser, block, "analyses").parse().test_blocks
        model_tests = TestablePatchParser(self.parser, block, "models").parse().test_blocks
        source_tests = SourceParser(self.parser, block, "sources").parse().test_blocks
        macro_tests = MacroPatchParser(self.parser, block, "macros").parse().test_blocks
        self.assertEqual(len(analysis_tests), 0)
        self.assertEqual(len(model_tests), 0)
        self.assertEqual(len(source_tests), 0)
        self.assertEqual(len(macro_tests), 0)
        self.assertEqual(len(list(self.parser.manifest.nodes)), 0)
        self.assertEqual(len(list(self.parser.manifest.sources)), 0)
        source_patches = list(self.parser.manifest.source_patches.values())
        self.assertEqual(len(source_patches), 1)
        self.assertEqual(source_patches[0].name, "my_source")
        self.assertEqual(source_patches[0].overrides, "snowplow")
        self.assertIsNone(source_patches[0].description)
        self.assertEqual(len(source_patches[0].tables), 1)
        table = source_patches[0].tables[0]
        self.assertEqual(table.name, "my_table")
        self.assertIsNone(table.description)
        self.assertEqual(len(table.columns), 1)
        self.assertEqual(len(table.columns[0].tests), 2)


class SchemaParserModelsTest(SchemaParserTest):
    def setUp(self):
        super().setUp()
        my_model_node = MockNode(
            package="root",
            name="my_model",
            config=mock.MagicMock(enabled=True),
            refs=[],
            sources=[],
            patch_path=None,
            yaml_config_dict={},
        )
        nodes = {my_model_node.unique_id: my_model_node}
        macros = {m.unique_id: m for m in generate_name_macros("root")}
        self.manifest = Manifest(nodes=nodes, macros=macros)
        self.manifest.ref_lookup
        self.parser = SchemaParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def test__read_basic_model_tests(self):
        block = self.yaml_block_for(SINGLE_TABLE_MODEL_TESTS, "test_one.yml")
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assertEqual(len(list(self.parser.manifest.sources)), 0)
        self.assertEqual(len(list(self.parser.manifest.nodes)), 4)

    def test__parse_basic_model_tests(self):
        block = self.file_block_for(SINGLE_TABLE_MODEL_TESTS, "test_one.yml")
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=4)

        all_nodes = sorted(self.parser.manifest.nodes.values(), key=lambda n: n.unique_id)
        tests = []
        for node in all_nodes:
            if node.resource_type != NodeType.Test:
                continue
            tests.append(node)
        self.assertEqual(tests[0].config.severity, "ERROR")
        self.assertEqual(tests[0].tags, [])
        self.assertEqual(tests[0].refs, [RefArgs(name="my_model")])
        self.assertEqual(tests[0].column_name, "color")
        self.assertEqual(tests[0].package_name, "snowplow")
        self.assertTrue(tests[0].name.startswith("accepted_values_"))
        self.assertEqual(tests[0].fqn, ["snowplow", tests[0].name])
        self.assertEqual(
            tests[0].unique_id.split("."), ["test", "snowplow", tests[0].name, "9d4814efde"]
        )
        self.assertEqual(tests[0].test_metadata.name, "accepted_values")
        self.assertIsNone(tests[0].test_metadata.namespace)
        self.assertEqual(
            tests[0].test_metadata.kwargs,
            {
                "column_name": "color",
                "model": "{{ get_where_subquery(ref('my_model')) }}",
                "values": ["red", "blue", "green"],
            },
        )

        # foreign packages are a bit weird, they include the macro package
        # name in the test name
        self.assertEqual(tests[1].config.severity, "ERROR")
        self.assertEqual(tests[1].tags, [])
        self.assertEqual(tests[1].refs, [RefArgs(name="my_model")])
        self.assertEqual(tests[1].column_name, "color")
        self.assertEqual(tests[1].column_name, "color")
        self.assertEqual(tests[1].fqn, ["snowplow", tests[1].name])
        self.assertTrue(tests[1].name.startswith("foreign_package_test_case_"))
        self.assertEqual(tests[1].package_name, "snowplow")
        self.assertEqual(
            tests[1].unique_id.split("."), ["test", "snowplow", tests[1].name, "13958f62f7"]
        )
        self.assertEqual(tests[1].test_metadata.name, "test_case")
        self.assertEqual(tests[1].test_metadata.namespace, "foreign_package")
        self.assertEqual(
            tests[1].test_metadata.kwargs,
            {
                "column_name": "color",
                "model": "{{ get_where_subquery(ref('my_model')) }}",
                "arg": 100,
            },
        )

        self.assertEqual(tests[2].config.severity, "WARN")
        self.assertEqual(tests[2].tags, [])
        self.assertEqual(tests[2].refs, [RefArgs(name="my_model")])
        self.assertEqual(tests[2].column_name, "color")
        self.assertEqual(tests[2].package_name, "snowplow")
        self.assertTrue(tests[2].name.startswith("not_null_"))
        self.assertEqual(tests[2].fqn, ["snowplow", tests[2].name])
        self.assertEqual(
            tests[2].unique_id.split("."), ["test", "snowplow", tests[2].name, "2f61818750"]
        )
        self.assertEqual(tests[2].test_metadata.name, "not_null")
        self.assertIsNone(tests[2].test_metadata.namespace)
        self.assertEqual(
            tests[2].test_metadata.kwargs,
            {
                "column_name": "color",
                "model": "{{ get_where_subquery(ref('my_model')) }}",
            },
        )

        file_id = "snowplow://" + normalize("models/test_one.yml")
        self.assertIn(file_id, self.parser.manifest.files)
        schema_file_test_ids = self.parser.manifest.files[file_id].get_all_test_ids()
        self.assertEqual(sorted(schema_file_test_ids), [t.unique_id for t in tests])
        self.assertEqual(self.parser.manifest.files[file_id].node_patches, ["model.root.my_model"])


class SchemaParserVersionedModels(SchemaParserTest):
    def setUp(self):
        super().setUp()
        my_model_v1_node = MockNode(
            package="snowplow",
            name="arbitrary_file_name",
            config=mock.MagicMock(enabled=True),
            refs=[],
            sources=[],
            patch_path=None,
            file_id="snowplow://models/arbitrary_file_name.sql",
        )
        my_model_v2_node = MockNode(
            package="snowplow",
            name="my_model_v2",
            config=mock.MagicMock(enabled=True),
            refs=[],
            sources=[],
            patch_path=None,
            file_id="snowplow://models/my_model_v2.sql",
        )
        nodes = {
            my_model_v1_node.unique_id: my_model_v1_node,
            my_model_v2_node.unique_id: my_model_v2_node,
        }
        macros = {m.unique_id: m for m in generate_name_macros("root")}
        files = {node.file_id: mock.MagicMock(nodes=[node.unique_id]) for node in nodes.values()}
        self.manifest = Manifest(nodes=nodes, macros=macros, files=files)
        self.manifest.ref_lookup
        self.parser = SchemaParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def test__read_versioned_model_tests(self):
        block = self.yaml_block_for(MULTIPLE_TABLE_VERSIONED_MODEL_TESTS, "test_one.yml")
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assertEqual(len(list(self.parser.manifest.sources)), 0)
        self.assertEqual(len(list(self.parser.manifest.nodes)), 5)

    def test__parse_versioned_model_tests(self):
        block = self.file_block_for(MULTIPLE_TABLE_VERSIONED_MODEL_TESTS, "test_one.yml")
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=5)

        all_nodes = sorted(self.parser.manifest.nodes.values(), key=lambda n: n.unique_id)
        tests = [node for node in all_nodes if node.resource_type == NodeType.Test]

        # test on color column on my_model v1
        self.assertEqual(tests[0].config.severity, "WARN")
        self.assertEqual(tests[0].tags, [])
        self.assertEqual(tests[0].refs, [RefArgs(name="my_model", version="1")])
        self.assertEqual(tests[0].column_name, "color")
        self.assertEqual(tests[0].package_name, "snowplow")
        self.assertTrue(tests[0].name.startswith("not_null"))
        self.assertEqual(tests[0].fqn, ["snowplow", tests[0].name])
        self.assertEqual(
            tests[0].unique_id.split("."), ["test", "snowplow", tests[0].name, "b704420587"]
        )
        self.assertEqual(tests[0].test_metadata.name, "not_null")
        self.assertIsNone(tests[0].test_metadata.namespace)
        self.assertEqual(
            tests[0].test_metadata.kwargs,
            {
                "column_name": "color",
                "model": "{{ get_where_subquery(ref('my_model', version='1')) }}",
            },
        )

        # test on color column on my_model v2
        self.assertEqual(tests[1].config.severity, "WARN")
        self.assertEqual(tests[1].tags, [])
        self.assertEqual(tests[1].refs, [RefArgs(name="my_model", version="2")])
        self.assertEqual(tests[1].column_name, "color")
        self.assertEqual(tests[1].fqn, ["snowplow", tests[1].name])
        self.assertTrue(tests[1].name.startswith("not_null"))
        self.assertEqual(tests[1].package_name, "snowplow")
        self.assertEqual(
            tests[1].unique_id.split("."), ["test", "snowplow", tests[1].name, "3375708d04"]
        )
        self.assertEqual(tests[1].test_metadata.name, "not_null")
        self.assertIsNone(tests[0].test_metadata.namespace)
        self.assertEqual(
            tests[1].test_metadata.kwargs,
            {
                "column_name": "color",
                "model": "{{ get_where_subquery(ref('my_model', version='2')) }}",
            },
        )

        # model uniqueness test on column on my_model v2
        self.assertEqual(tests[2].config.severity, "ERROR")
        self.assertEqual(tests[2].tags, [])
        self.assertEqual(tests[2].refs, [RefArgs(name="my_model", version="2")])
        self.assertIsNone(tests[2].column_name)
        self.assertEqual(tests[2].package_name, "snowplow")
        self.assertTrue(tests[2].name.startswith("unique"))
        self.assertEqual(tests[2].fqn, ["snowplow", tests[2].name])
        self.assertEqual(
            tests[2].unique_id.split("."), ["test", "snowplow", tests[2].name, "29b09359d1"]
        )
        self.assertEqual(tests[2].test_metadata.name, "unique")
        self.assertIsNone(tests[2].test_metadata.namespace)
        self.assertEqual(
            tests[2].test_metadata.kwargs,
            {
                "column_name": "color",
                "model": "{{ get_where_subquery(ref('my_model', version='2')) }}",
            },
        )

        file_id = "snowplow://" + normalize("models/test_one.yml")
        self.assertIn(file_id, self.parser.manifest.files)
        schema_file_test_ids = self.parser.manifest.files[file_id].get_all_test_ids()
        self.assertEqual(sorted(schema_file_test_ids), [t.unique_id for t in tests])
        self.assertEqual(
            self.parser.manifest.files[file_id].node_patches,
            ["model.snowplow.my_model.v1", "model.snowplow.my_model.v2"],
        )

    def test__parsed_versioned_models(self):
        block = self.file_block_for(MULTIPLE_TABLE_VERSIONED_MODEL, "test_one.yml")
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=2)

    def test__parsed_versioned_models_contract_enforced(self):
        block = self.file_block_for(
            MULTIPLE_TABLE_VERSIONED_MODEL_CONTRACT_ENFORCED, "test_one.yml"
        )
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=2)
        for node in self.parser.manifest.nodes.values():
            assert node.contract.enforced
            node.build_contract_checksum.assert_called()

    def test__parsed_versioned_models_v0(self):
        block = self.file_block_for(MULTIPLE_TABLE_VERSIONED_MODEL_V0, "test_one.yml")
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=2)

    def test__parsed_versioned_models_v0_latest_version(self):
        block = self.file_block_for(
            MULTIPLE_TABLE_VERSIONED_MODEL_V0_LATEST_VERSION, "test_one.yml"
        )
        self.parser.manifest.files[block.file.file_id] = block.file
        dct = yaml_from_file(block.file)
        self.parser.parse_file(block, dct)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=2)


sql_model = """
{{ config(materialized="table") }}
select 1 as id
"""

sql_model_parse_error = "{{ SYNTAX ERROR }}"

python_model = """
import textblob
import text as a
from torch import b
import textblob.text
import sklearn

def model(dbt, session):
    dbt.config(
        materialized='table',
        packages=['sklearn==0.1.0']
    )
    df0 = dbt.ref("a_model").to_pandas()
    df1 = dbt.ref("my_sql_model").task.limit(2)
    df2 = dbt.ref("my_sql_model_1")
    df3 = dbt.ref("my_sql_model_2")
    df4 = dbt.source("test", 'table1').limit(max=[max(dbt.ref('something'))])
    df5 = [dbt.ref('test1')]

    a_dict = {'test2': dbt.ref('test2')}
    df5 = {'test2': dbt.ref('test3')}
    df6 = [dbt.ref("test4")]

    df = df0.limit(2)
    return df
"""

python_model_config = """
def model(dbt, session):
    dbt.config.get("param_1")
    dbt.config.get("param_2")
    return dbt.ref("some_model")
"""

python_model_config_with_defaults = """
def model(dbt, session):
    dbt.config.get("param_None", None)
    dbt.config.get("param_Str", "default")
    dbt.config.get("param_List", [1, 2])
    return dbt.ref("some_model")
"""

python_model_single_argument = """
def model(dbt):
     dbt.config(materialized="table")
     return dbt.ref("some_model")
"""

python_model_no_argument = """
import pandas as pd

def model():
    return pd.dataframe([1, 2])
"""

python_model_incorrect_argument_name = """
def model(tbd, session):
    tbd.config(materialized="table")
    return tbd.ref("some_model")
"""

python_model_multiple_models = """
def model(dbt, session):
    dbt.config(materialized='table')
    return dbt.ref("some_model")

def model(dbt, session):
    dbt.config(materialized='table')
    return dbt.ref("some_model")
"""

python_model_incorrect_function_name = """
def model1(dbt, session):
    dbt.config(materialized='table')
    return dbt.ref("some_model")
"""

python_model_empty_file = """    """

python_model_multiple_returns = """
def model(dbt, session):
    dbt.config(materialized='table')
    return dbt.ref("some_model"), dbt.ref("some_other_model")
"""

python_model_no_return = """
def model(dbt, session):
    dbt.config(materialized='table')
"""

python_model_single_return = """
import pandas as pd

def model(dbt, session):
    dbt.config(materialized='table')
    return pd.dataframe([1, 2])
"""

python_model_incorrect_ref = """
def model(dbt, session):
    model_names = ["orders", "customers"]
    models = []

    for model_name in model_names:
        models.extend(dbt.ref(model_name))

    return models[0]
"""

python_model_default_materialization = """
import pandas as pd

def model(dbt, session):
    return pd.dataframe([1, 2])
"""

python_model_custom_materialization = """
import pandas as pd

def model(dbt, session):
    dbt.config(materialized="view")
    return pd.dataframe([1, 2])
"""


class ModelParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = ModelParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "models")

    def test_basic(self):
        block = self.file_block_for(sql_model, "nested/model_1.sql")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1)
        node = list(self.parser.manifest.nodes.values())[0]
        expected = ModelNode(
            alias="model_1",
            name="model_1",
            database="test",
            schema="analytics",
            resource_type=NodeType.Model,
            unique_id="model.snowplow.model_1",
            fqn=["snowplow", "nested", "model_1"],
            package_name="snowplow",
            original_file_path=normalize("models/nested/model_1.sql"),
            config=ModelConfig(materialized="table"),
            path=normalize("nested/model_1.sql"),
            language="sql",
            raw_code=sql_model,
            checksum=block.file.checksum,
            unrendered_config={"materialized": "table"},
            config_call_dict={
                "materialized": "table",
            },
        )
        assertEqualNodes(node, expected)
        file_id = "snowplow://" + normalize("models/nested/model_1.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(self.parser.manifest.files[file_id].nodes, ["model.snowplow.model_1"])

    def test_sql_model_parse_error(self):
        block = self.file_block_for(sql_model_parse_error, "nested/model_1.sql")
        with self.assertRaises(CompilationError):
            self.parser.parse_file(block)

    def test_python_model_parse(self):
        block = self.file_block_for(python_model, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1)
        node = list(self.parser.manifest.nodes.values())[0]
        # we decided to not detect and auto supply for now since import name doesn't always match library name
        python_packages = ["sklearn==0.1.0"]
        expected = ModelNode(
            alias="py_model",
            name="py_model",
            database="test",
            schema="analytics",
            resource_type=NodeType.Model,
            unique_id="model.snowplow.py_model",
            fqn=["snowplow", "nested", "py_model"],
            package_name="snowplow",
            original_file_path=normalize("models/nested/py_model.py"),
            config=ModelConfig(materialized="table", packages=python_packages),
            # config.packages = ['textblob']
            path=normalize("nested/py_model.py"),
            language="python",
            raw_code=python_model,
            checksum=block.file.checksum,
            unrendered_config={"materialized": "table", "packages": python_packages},
            config_call_dict={"materialized": "table", "packages": python_packages},
            refs=[
                RefArgs(name="a_model"),
                RefArgs("my_sql_model"),
                RefArgs("my_sql_model_1"),
                RefArgs("my_sql_model_2"),
                RefArgs("something"),
                RefArgs("test1"),
                RefArgs("test2"),
                RefArgs("test3"),
                RefArgs("test4"),
            ],
            sources=[["test", "table1"]],
        )
        assertEqualNodes(node, expected)
        file_id = "snowplow://" + normalize("models/nested/py_model.py")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(self.parser.manifest.files[file_id].nodes, ["model.snowplow.py_model"])

    def test_python_model_config(self):
        block = self.file_block_for(python_model_config, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file

        self.parser.parse_file(block)
        node = list(self.parser.manifest.nodes.values())[0]
        self.assertEqual(node.config.to_dict()["config_keys_used"], ["param_1", "param_2"])

    def test_python_model_config_with_defaults(self):
        block = self.file_block_for(python_model_config_with_defaults, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file

        self.parser.parse_file(block)
        node = list(self.parser.manifest.nodes.values())[0]
        default_values = node.config.to_dict()["config_keys_defaults"]
        self.assertIsNone(default_values[0])
        self.assertEqual(default_values[1], "default")
        self.assertEqual(default_values[2], [1, 2])

    def test_python_model_single_argument(self):
        block = self.file_block_for(python_model_single_argument, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_no_argument(self):
        block = self.file_block_for(python_model_no_argument, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_incorrect_argument_name(self):
        block = self.file_block_for(python_model_incorrect_argument_name, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_multiple_models(self):
        block = self.file_block_for(python_model_multiple_models, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_incorrect_function_name(self):
        block = self.file_block_for(python_model_incorrect_function_name, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_empty_file(self):
        block = self.file_block_for(python_model_empty_file, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.assertIsNone(self.parser.parse_file(block))

    def test_python_model_multiple_returns(self):
        block = self.file_block_for(python_model_multiple_returns, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_no_return(self):
        block = self.file_block_for(python_model_no_return, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_single_return(self):
        block = self.file_block_for(python_model_single_return, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.assertIsNone(self.parser.parse_file(block))

    def test_python_model_incorrect_ref(self):
        block = self.file_block_for(python_model_incorrect_ref, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        with self.assertRaises(ParsingError):
            self.parser.parse_file(block)

    def test_python_model_default_materialization(self):
        block = self.file_block_for(python_model_default_materialization, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        node = list(self.parser.manifest.nodes.values())[0]
        self.assertEqual(node.get_materialization(), "table")

    def test_python_model_custom_materialization(self):
        block = self.file_block_for(python_model_custom_materialization, "nested/py_model.py")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        node = list(self.parser.manifest.nodes.values())[0]
        self.assertEqual(node.get_materialization(), "view")


class StaticModelParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = ModelParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "models")

    # tests that when the ref built-in is overriden with a macro definition
    # that the ModelParser can detect it. This does not test that the static
    # parser does not run in this case. That test is in integration test suite 072
    def test_built_in_macro_override_detection(self):
        macro_unique_id = "macro.root.ref"
        self.parser.manifest.macros[macro_unique_id] = Macro(
            name="ref",
            resource_type=NodeType.Macro,
            unique_id=macro_unique_id,
            package_name="root",
            original_file_path=normalize("macros/macro.sql"),
            path=normalize("macros/macro.sql"),
            macro_sql='{% macro ref(model_name) %}{% set x = raise("boom") %}{% endmacro %}',
        )

        raw_code = '{{ config(materialized="table") }}select 1 as id'
        block = self.file_block_for(raw_code, "nested/model_1.sql")
        node = ModelNode(
            alias="model_1",
            name="model_1",
            database="test",
            schema="analytics",
            resource_type=NodeType.Model,
            unique_id="model.snowplow.model_1",
            fqn=["snowplow", "nested", "model_1"],
            package_name="snowplow",
            original_file_path=normalize("models/nested/model_1.sql"),
            config=ModelConfig(materialized="table"),
            path=normalize("nested/model_1.sql"),
            language="sql",
            raw_code=raw_code,
            checksum=block.file.checksum,
            unrendered_config={"materialized": "table"},
        )

        assert self.parser._has_banned_macro(node)


# TODO
class StaticModelParserUnitTest(BaseParserTest):
    # _get_config_call_dict
    # _shift_sources
    # _get_exp_sample_result
    # _get_stable_sample_result
    # _get_sample_result

    def setUp(self):
        super().setUp()
        self.parser = ModelParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )
        self.example_node = ModelNode(
            alias="model_1",
            name="model_1",
            database="test",
            schema="analytics",
            resource_type=NodeType.Model,
            unique_id="model.snowplow.model_1",
            fqn=["snowplow", "nested", "model_1"],
            package_name="snowplow",
            original_file_path=normalize("models/nested/model_1.sql"),
            config=ModelConfig(materialized="table"),
            path=normalize("nested/model_1.sql"),
            language="sql",
            raw_code='{{ config(materialized="table") }}select 1 as id',
            checksum=None,
            unrendered_config={"materialized": "table"},
        )
        self.example_config = ContextConfig(
            self.root_project_config,
            self.example_node.fqn,
            self.example_node.resource_type,
            self.snowplow_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "models")

    # tests that configs get extracted properly. the function should respect merge behavior,
    # but becuase it's only reading from one dictionary it won't matter except in edge cases
    # like this example with tags changing type to a list.
    def test_config_shifting(self):
        static_parser_result = {
            "configs": [("hello", "world"), ("flag", True), ("tags", "tag1"), ("tags", "tag2")]
        }
        expected = {"hello": "world", "flag": True, "tags": ["tag1", "tag2"]}
        got = _get_config_call_dict(static_parser_result)
        self.assertEqual(expected, got)

    def test_source_shifting(self):
        static_parser_result = {"sources": [("abc", "def"), ("x", "y")]}
        expected = {"sources": [["abc", "def"], ["x", "y"]]}
        got = _shift_sources(static_parser_result)
        self.assertEqual(expected, got)

    def test_sample_results(self):
        # --- missed ref --- #
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)

        sample_node.refs = []
        node.refs = ["myref"]

        result = _get_sample_result(sample_node, sample_config, node, config)
        self.assertEqual([(7, "missed_ref_value")], result)

        # --- false positive ref --- #
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)

        sample_node.refs = ["myref"]
        node.refs = []

        result = _get_sample_result(sample_node, sample_config, node, config)
        self.assertEqual([(6, "false_positive_ref_value")], result)

        # --- missed source --- #
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)

        sample_node.sources = []
        node.sources = [["abc", "def"]]

        result = _get_sample_result(sample_node, sample_config, node, config)
        self.assertEqual([(5, "missed_source_value")], result)

        # --- false positive source --- #
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)

        sample_node.sources = [["abc", "def"]]
        node.sources = []

        result = _get_sample_result(sample_node, sample_config, node, config)
        self.assertEqual([(4, "false_positive_source_value")], result)

        # --- missed config --- #
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)

        sample_config._config_call_dict = {}
        config._config_call_dict = {"key": "value"}

        result = _get_sample_result(sample_node, sample_config, node, config)
        self.assertEqual([(3, "missed_config_value")], result)

        # --- false positive config --- #
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)

        sample_config._config_call_dict = {"key": "value"}
        config._config_call_dict = {}

        result = _get_sample_result(sample_node, sample_config, node, config)
        self.assertEqual([(2, "false_positive_config_value")], result)

    def test_exp_sample_results(self):
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)
        result = _get_exp_sample_result(sample_node, sample_config, node, config)
        self.assertEqual(["00_experimental_exact_match"], result)

    def test_stable_sample_results(self):
        node = deepcopy(self.example_node)
        config = deepcopy(self.example_config)
        sample_node = deepcopy(self.example_node)
        sample_config = deepcopy(self.example_config)
        result = _get_stable_sample_result(sample_node, sample_config, node, config)
        self.assertEqual(["80_stable_exact_match"], result)


class SnapshotParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = SnapshotParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "snapshots")

    def test_parse_error(self):
        block = self.file_block_for(
            "{% snapshot foo %}select 1 as id{%snapshot bar %}{% endsnapshot %}",
            "nested/snap_1.sql",
        )
        with self.assertRaises(CompilationError):
            self.parser.parse_file(block)

    def test_single_block(self):
        raw_code = """{{
                config(unique_key="id", target_schema="analytics",
                       target_database="dbt", strategy="timestamp",
                       updated_at="last_update")
            }}
            select 1 as id, now() as last_update"""
        full_file = """
        {{% snapshot foo %}}{}{{% endsnapshot %}}
        """.format(
            raw_code
        )
        block = self.file_block_for(full_file, "nested/snap_1.sql")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1)
        node = list(self.parser.manifest.nodes.values())[0]
        expected = SnapshotNode(
            alias="foo",
            name="foo",
            # the `database` entry is overrridden by the target_database config
            database="dbt",
            schema="analytics",
            resource_type=NodeType.Snapshot,
            unique_id="snapshot.snowplow.foo",
            fqn=["snowplow", "nested", "snap_1", "foo"],
            package_name="snowplow",
            original_file_path=normalize("snapshots/nested/snap_1.sql"),
            config=SnapshotConfig(
                strategy="timestamp",
                updated_at="last_update",
                target_database="dbt",
                target_schema="analytics",
                unique_key="id",
                materialized="snapshot",
            ),
            path=normalize("nested/snap_1.sql"),
            language="sql",
            raw_code=raw_code,
            checksum=block.file.checksum,
            unrendered_config={
                "unique_key": "id",
                "target_schema": "analytics",
                "target_database": "dbt",
                "strategy": "timestamp",
                "updated_at": "last_update",
            },
            config_call_dict={
                "strategy": "timestamp",
                "target_database": "dbt",
                "target_schema": "analytics",
                "unique_key": "id",
                "updated_at": "last_update",
            },
        )
        assertEqualNodes(expected, node)
        file_id = "snowplow://" + normalize("snapshots/nested/snap_1.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(self.parser.manifest.files[file_id].nodes, ["snapshot.snowplow.foo"])

    def test_multi_block(self):
        raw_1 = """
            {{
                config(unique_key="id", target_schema="analytics",
                       target_database="dbt", strategy="timestamp",
                       updated_at="last_update")
            }}
            select 1 as id, now() as last_update
        """
        raw_2 = """
            {{
                config(unique_key="id", target_schema="analytics",
                       target_database="dbt", strategy="timestamp",
                       updated_at="last_update")
            }}
            select 2 as id, now() as last_update
        """
        full_file = """
        {{% snapshot foo %}}{}{{% endsnapshot %}}
        {{% snapshot bar %}}{}{{% endsnapshot %}}
        """.format(
            raw_1, raw_2
        )
        block = self.file_block_for(full_file, "nested/snap_1.sql")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=2)
        nodes = sorted(self.parser.manifest.nodes.values(), key=lambda n: n.name)
        expect_foo = SnapshotNode(
            alias="foo",
            name="foo",
            database="dbt",
            schema="analytics",
            resource_type=NodeType.Snapshot,
            unique_id="snapshot.snowplow.foo",
            fqn=["snowplow", "nested", "snap_1", "foo"],
            package_name="snowplow",
            original_file_path=normalize("snapshots/nested/snap_1.sql"),
            config=SnapshotConfig(
                strategy="timestamp",
                updated_at="last_update",
                target_database="dbt",
                target_schema="analytics",
                unique_key="id",
                materialized="snapshot",
            ),
            path=normalize("nested/snap_1.sql"),
            language="sql",
            raw_code=raw_1,
            checksum=block.file.checksum,
            unrendered_config={
                "unique_key": "id",
                "target_schema": "analytics",
                "target_database": "dbt",
                "strategy": "timestamp",
                "updated_at": "last_update",
            },
            config_call_dict={
                "strategy": "timestamp",
                "target_database": "dbt",
                "target_schema": "analytics",
                "unique_key": "id",
                "updated_at": "last_update",
            },
        )
        expect_bar = SnapshotNode(
            alias="bar",
            name="bar",
            database="dbt",
            schema="analytics",
            resource_type=NodeType.Snapshot,
            unique_id="snapshot.snowplow.bar",
            fqn=["snowplow", "nested", "snap_1", "bar"],
            package_name="snowplow",
            original_file_path=normalize("snapshots/nested/snap_1.sql"),
            config=SnapshotConfig(
                strategy="timestamp",
                updated_at="last_update",
                target_database="dbt",
                target_schema="analytics",
                unique_key="id",
                materialized="snapshot",
            ),
            path=normalize("nested/snap_1.sql"),
            language="sql",
            raw_code=raw_2,
            checksum=block.file.checksum,
            unrendered_config={
                "unique_key": "id",
                "target_schema": "analytics",
                "target_database": "dbt",
                "strategy": "timestamp",
                "updated_at": "last_update",
            },
            config_call_dict={
                "strategy": "timestamp",
                "target_database": "dbt",
                "target_schema": "analytics",
                "unique_key": "id",
                "updated_at": "last_update",
            },
        )
        assertEqualNodes(nodes[0], expect_bar)
        assertEqualNodes(nodes[1], expect_foo)
        file_id = "snowplow://" + normalize("snapshots/nested/snap_1.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(
            sorted(self.parser.manifest.files[file_id].nodes),
            ["snapshot.snowplow.bar", "snapshot.snowplow.foo"],
        )


class MacroParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = MacroParser(project=self.snowplow_project_config, manifest=Manifest())

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "macros")

    def test_single_block(self):
        raw_code = "{% macro foo(a, b) %}a ~ b{% endmacro %}"
        block = self.file_block_for(raw_code, "macro.sql")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assertEqual(len(self.parser.manifest.macros), 1)
        macro = list(self.parser.manifest.macros.values())[0]
        expected = Macro(
            name="foo",
            resource_type=NodeType.Macro,
            unique_id="macro.snowplow.foo",
            package_name="snowplow",
            original_file_path=normalize("macros/macro.sql"),
            path=normalize("macros/macro.sql"),
            macro_sql=raw_code,
        )
        assertEqualNodes(macro, expected)
        file_id = "snowplow://" + normalize("macros/macro.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(self.parser.manifest.files[file_id].macros, ["macro.snowplow.foo"])

    def test_multiple_blocks(self):
        raw_code = (
            "{% macro foo(a, b) %}a ~ b{% endmacro %}\n{% macro bar(c, d) %}c + d{% endmacro %}"
        )
        block = self.file_block_for(raw_code, "macro.sql")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assertEqual(len(self.parser.manifest.macros), 2)
        macros = sorted(self.parser.manifest.macros.values(), key=lambda m: m.name)
        expected_bar = Macro(
            name="bar",
            resource_type=NodeType.Macro,
            unique_id="macro.snowplow.bar",
            package_name="snowplow",
            original_file_path=normalize("macros/macro.sql"),
            path=normalize("macros/macro.sql"),
            macro_sql="{% macro bar(c, d) %}c + d{% endmacro %}",
        )
        expected_foo = Macro(
            name="foo",
            resource_type=NodeType.Macro,
            unique_id="macro.snowplow.foo",
            package_name="snowplow",
            original_file_path=normalize("macros/macro.sql"),
            path=normalize("macros/macro.sql"),
            macro_sql="{% macro foo(a, b) %}a ~ b{% endmacro %}",
        )
        assertEqualNodes(macros[0], expected_bar)
        assertEqualNodes(macros[1], expected_foo)
        file_id = "snowplow://" + normalize("macros/macro.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(
            sorted(self.parser.manifest.files[file_id].macros),
            ["macro.snowplow.bar", "macro.snowplow.foo"],
        )


class SingularTestParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = SingularTestParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "tests")

    def test_basic(self):
        raw_code = 'select * from {{ ref("blah") }} limit 0'
        block = self.file_block_for(raw_code, "test_1.sql")
        self.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1)
        node = list(self.parser.manifest.nodes.values())[0]
        expected = SingularTestNode(
            alias="test_1",
            name="test_1",
            database="test",
            schema="dbt_test__audit",
            resource_type=NodeType.Test,
            unique_id="test.snowplow.test_1",
            fqn=["snowplow", "test_1"],
            package_name="snowplow",
            original_file_path=normalize("tests/test_1.sql"),
            refs=[RefArgs(name="blah")],
            config=TestConfig(severity="ERROR"),
            tags=[],
            path=normalize("test_1.sql"),
            language="sql",
            raw_code=raw_code,
            checksum=block.file.checksum,
            unrendered_config={},
        )
        assertEqualNodes(node, expected)
        file_id = "snowplow://" + normalize("tests/test_1.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(self.parser.manifest.files[file_id].nodes, ["test.snowplow.test_1"])


class GenericTestParserTest(BaseParserTest):
    # generic tests in the test-paths directory currently leverage the macro parser
    def setUp(self):
        super().setUp()
        self.parser = GenericTestParser(project=self.snowplow_project_config, manifest=Manifest())

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "tests/generic")

    def test_basic(self):
        raw_code = "{% test not_null(model, column_name) %}select * from {{ model }} where {{ column_name }} is null {% endtest %}"
        block = self.file_block_for(raw_code, "test_1.sql")
        self.parser.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        node = list(self.parser.manifest.macros.values())[0]
        expected = Macro(
            name="test_not_null",
            resource_type=NodeType.Macro,
            unique_id="macro.snowplow.test_not_null",
            package_name="snowplow",
            original_file_path=normalize("tests/generic/test_1.sql"),
            path=normalize("tests/generic/test_1.sql"),
            macro_sql=raw_code,
        )
        assertEqualNodes(node, expected)
        file_id = "snowplow://" + normalize("tests/generic/test_1.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(
            self.parser.manifest.files[file_id].macros, ["macro.snowplow.test_not_null"]
        )


class AnalysisParserTest(BaseParserTest):
    def setUp(self):
        super().setUp()
        self.parser = AnalysisParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "analyses")

    def test_basic(self):
        raw_code = "select 1 as id"
        block = self.file_block_for(raw_code, "nested/analysis_1.sql")
        self.manifest.files[block.file.file_id] = block.file
        self.parser.parse_file(block)
        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1)
        node = list(self.parser.manifest.nodes.values())[0]
        expected = AnalysisNode(
            alias="analysis_1",
            name="analysis_1",
            database="test",
            schema="analytics",
            resource_type=NodeType.Analysis,
            unique_id="analysis.snowplow.analysis_1",
            fqn=["snowplow", "analysis", "nested", "analysis_1"],
            package_name="snowplow",
            original_file_path=normalize("analyses/nested/analysis_1.sql"),
            depends_on=DependsOn(),
            config=NodeConfig(),
            path=normalize("analysis/nested/analysis_1.sql"),
            language="sql",
            raw_code=raw_code,
            checksum=block.file.checksum,
            unrendered_config={},
            relation_name=None,
        )
        assertEqualNodes(node, expected)
        file_id = "snowplow://" + normalize("analyses/nested/analysis_1.sql")
        self.assertIn(file_id, self.parser.manifest.files)
        self.assertEqual(
            self.parser.manifest.files[file_id].nodes, ["analysis.snowplow.analysis_1"]
        )
