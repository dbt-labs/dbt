from unittest import mock

from dbt.artifacts.resources import DependsOn, UnitTestConfig, UnitTestFormat
from dbt.contracts.graph.nodes import NodeType, UnitTestDefinition
from dbt.contracts.graph.unparsed import UnitTestOutputFixture
from dbt.exceptions import ParsingError
from dbt.parser import SchemaParser
from dbt.parser.unit_tests import UnitTestParser, process_models_for_unit_test
from dbt_common.events.event_manager_client import add_callback_to_manager
from dbt_common.events.types import SystemStdErr
from tests.unit.parser.test_parser import SchemaParserTest, assertEqualNodes
from tests.unit.utils import MockNode
from tests.utils import EventCatcher

UNIT_TEST_MODEL_NOT_FOUND_SOURCE = """
unit_tests:
    - name: test_my_model_doesnt_exist
      model: my_model_doesnt_exist
      description: "unit test description"
      given: []
      expect:
        rows:
          - {a: 1}
"""


UNIT_TEST_SOURCE = """
unit_tests:
    - name: test_my_model
      model: my_model
      description: "unit test description"
      given: []
      expect:
        rows:
          - {a: 1}
"""


UNIT_TEST_VERSIONED_MODEL_SOURCE = """
unit_tests:
    - name: test_my_model_versioned
      model: my_model_versioned.v1
      description: "unit test description"
      given: []
      expect:
        rows:
          - {a: 1}
"""


UNIT_TEST_CONFIG_SOURCE = """
unit_tests:
    - name: test_my_model
      model: my_model
      config:
        tags: "schema_tag"
        meta:
          meta_key: meta_value
          meta_jinja_key: '{{ 1 + 1 }}'
      description: "unit test description"
      given: []
      expect:
        rows:
          - {a: 1}
"""


UNIT_TEST_MULTIPLE_SOURCE = """
unit_tests:
    - name: test_my_model
      model: my_model
      description: "unit test description"
      given: []
      expect:
        rows:
          - {a: 1}
    - name: test_my_model2
      model: my_model
      description: "unit test description"
      given: []
      expect:
        rows:
          - {a: 1}
"""

UNIT_TEST_NONE_ROWS_SORT = """
unit_tests:
  - name: test_my_model_null_handling
    model: my_model
    description: "unit test description"
    given: []
    expect:
        rows:
        - {"id":  , "col1": "d"}
        - {"id":  , "col1": "e"}
        - {"id": 6, "col1": "f"}
"""

UNIT_TEST_NONE_ROWS_SORT_CSV = """
unit_tests:
  - name: test_my_model_null_handling
    model: my_model
    description: "unit test description"
    given: []
    expect:
        format: csv
        rows: |
          id,col1
          ,d
          ,e
          6,f
"""

UNIT_TEST_NONE_ROWS_SORT_SQL = """
unit_tests:
  - name: test_my_model_null_handling
    model: my_model
    description: "unit test description"
    given: []
    expect:
        format: sql
        rows: |
          select null
          select 1
"""

UNIT_TEST_NONE_ROWS_SORT_FAILS = """
unit_tests:
  - name: test_my_model_null_handling
    model: my_model
    description: "this unit test needs one non-None value row"
    given: []
    expect:
        rows:
        - {"id":  , "col1": "d"}
        - {"id":  , "col1": "e"}
"""

UNIT_TEST_DISABLED = """
unit_tests:
    - name: test_my_model_disabled
      model: my_model
      description: "this unit test is disabled"
      config:
        enabled: false
      given: []
      expect:
        rows:
          - {a: 1}
"""


class UnitTestParserTest(SchemaParserTest):
    def setUp(self):
        super().setUp()
        my_model_node = MockNode(
            package="snowplow",
            name="my_model",
            config=mock.MagicMock(enabled=True),
            schema="test_schema",
            refs=[],
            sources=[],
            patch_path=None,
        )
        self.manifest.nodes = {my_model_node.unique_id: my_model_node}
        self.parser = SchemaParser(
            project=self.snowplow_project_config,
            manifest=self.manifest,
            root_project=self.root_project_config,
        )

    def file_block_for(self, data, filename):
        return super().file_block_for(data, filename, "unit_tests")

    def test_basic(self):
        block = self.yaml_block_for(UNIT_TEST_SOURCE, "test_my_model.yml")

        UnitTestParser(self.parser, block).parse()

        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1, unit_tests=1)
        unit_test = list(self.parser.manifest.unit_tests.values())[0]
        # Resolution of the tested model now happens only in the post-parse
        # pass, not in parse() itself (dbt-core #11139).
        self.assertEqual(unit_test.depends_on.nodes, [])
        process_models_for_unit_test(self.parser.manifest, unit_test, {})
        expected = UnitTestDefinition(
            name="test_my_model",
            model="my_model",
            resource_type=NodeType.Unit,
            package_name="snowplow",
            path=block.path.relative_path,
            original_file_path=block.path.original_file_path,
            unique_id="unit_test.snowplow.my_model.test_my_model",
            given=[],
            expect=UnitTestOutputFixture(rows=[{"a": 1}]),
            description="unit test description",
            overrides=None,
            depends_on=DependsOn(nodes=["model.snowplow.my_model"]),
            fqn=["snowplow", "my_model", "test_my_model"],
            config=UnitTestConfig(),
            schema="test_schema",
        )
        expected.build_unit_test_checksum()
        assertEqualNodes(unit_test, expected)

    def test_unit_test_config(self):
        block = self.yaml_block_for(UNIT_TEST_CONFIG_SOURCE, "test_my_model.yml")
        self.root_project_config.unit_tests = {
            "snowplow": {"my_model": {"+tags": ["project_tag"]}}
        }

        UnitTestParser(self.parser, block).parse()

        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1, unit_tests=1)
        unit_test = self.parser.manifest.unit_tests["unit_test.snowplow.my_model.test_my_model"]
        self.assertEqual(sorted(unit_test.config.tags), sorted(["schema_tag", "project_tag"]))
        self.assertEqual(unit_test.config.meta, {"meta_key": "meta_value", "meta_jinja_key": "2"})

    def test_unit_test_disabled(self):
        block = self.yaml_block_for(UNIT_TEST_DISABLED, "test_my_model.yml")
        self.root_project_config.unit_tests = {
            "snowplow": {"my_model": {"+tags": ["project_tag"]}}
        }

        UnitTestParser(self.parser, block).parse()

        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1, unit_tests=0, disabled=1)
        unit_test_disabled_list = self.parser.manifest.disabled[
            "unit_test.snowplow.my_model.test_my_model_disabled"
        ]
        self.assertEqual(len(unit_test_disabled_list), 1)
        unit_test_disabled = unit_test_disabled_list[0]
        self.assertEqual(unit_test_disabled.config.enabled, False)

    def test_unit_test_versioned_model(self):
        block = self.yaml_block_for(UNIT_TEST_VERSIONED_MODEL_SOURCE, "test_my_model.yml")
        my_model_versioned_node = MockNode(
            package="snowplow",
            name="my_model_versioned",
            config=mock.MagicMock(enabled=True),
            refs=[],
            sources=[],
            patch_path=None,
            version=1,
        )
        self.manifest.nodes[my_model_versioned_node.unique_id] = my_model_versioned_node

        UnitTestParser(self.parser, block).parse()
        self.parser.manifest.files[block.file.file_id] = block.file

        self.assert_has_manifest_lengths(self.parser.manifest, nodes=2, unit_tests=1)
        unit_test = self.parser.manifest.unit_tests[
            "unit_test.snowplow.my_model_versioned.v1.test_my_model_versioned"
        ]
        self.assertEqual(unit_test.depends_on.nodes, [])
        models_to_versions = {
            "snowplow": {"my_model_versioned": [my_model_versioned_node.unique_id]}
        }
        process_models_for_unit_test(self.parser.manifest, unit_test, models_to_versions)
        self.assertEqual(len(unit_test.depends_on.nodes), 1)
        self.assertEqual(unit_test.depends_on.nodes[0], "model.snowplow.my_model_versioned.v1")

    def test_model_resolved_only_in_post_parse_pass(self):
        # Regression test for dbt-core #11139 (deeper fix, CORE-804).
        # UnitTestParser.parse() must never resolve the tested model itself --
        # resolution order must not depend on filesystem parse order relative
        # to the tested model's own YAML. depends_on stays empty coming out of
        # parse(), regardless of whether the tested model is versioned, and is
        # resolved exactly once, deterministically, in
        # process_models_for_unit_test.
        block = self.yaml_block_for(UNIT_TEST_SOURCE, "test_my_model.yml")
        UnitTestParser(self.parser, block).parse()

        manifest = self.parser.manifest
        manifest.files[block.file.file_id] = block.file
        unit_test = manifest.unit_tests["unit_test.snowplow.my_model.test_my_model"]
        self.assertEqual(unit_test.depends_on.nodes, [])

        # Simulate the model versioning patch happening after this unit test
        # was parsed: the unversioned node id is replaced by a versioned one,
        # and ref_lookup is rebuilt -- exactly as manifest.py does before
        # process_unit_tests() runs.
        del manifest.nodes["model.snowplow.my_model"]
        versioned_node = MockNode(
            package="snowplow",
            name="my_model",
            config=mock.MagicMock(enabled=True),
            schema="test_schema",
            refs=[],
            sources=[],
            patch_path=None,
            version=1,
            latest_version=1,
            is_latest_version=True,
        )
        manifest.nodes[versioned_node.unique_id] = versioned_node
        manifest.rebuild_ref_lookup()

        models_to_versions = {"snowplow": {"my_model": [versioned_node.unique_id]}}
        process_models_for_unit_test(manifest, unit_test, models_to_versions)

        assert "unit_test.snowplow.my_model.test_my_model_v1" in manifest.unit_tests
        versioned_ut = manifest.unit_tests["unit_test.snowplow.my_model.test_my_model_v1"]
        self.assertEqual(versioned_ut.depends_on.nodes[0], "model.snowplow.my_model.v1")

    def test_disabled_model_no_duplicate_file_entry(self):
        # Regression test: process_models_for_unit_test's disable-routing must
        # not append the unit test's unique_id to source_file.unit_tests a
        # second time -- parse()'s add_unit_test() call already put it there.
        block = self.yaml_block_for(UNIT_TEST_SOURCE, "test_my_model.yml")
        UnitTestParser(self.parser, block).parse()

        manifest = self.parser.manifest
        manifest.files[block.file.file_id] = block.file
        unit_test = manifest.unit_tests["unit_test.snowplow.my_model.test_my_model"]
        schema_file = manifest.files[block.file.file_id]
        self.assertEqual(schema_file.unit_tests.count(unit_test.unique_id), 1)

        # Make the tested model disabled and rebuild the lookups so
        # find_tested_model_node() resolves it via disabled_lookup.
        model_node = manifest.nodes.pop("model.snowplow.my_model")
        model_node.config.enabled = False
        manifest.add_disabled_nofile(model_node)
        manifest.rebuild_ref_lookup()
        manifest.rebuild_disabled_lookup()

        process_models_for_unit_test(manifest, unit_test, {})

        self.assertNotIn(unit_test.unique_id, manifest.unit_tests)
        self.assertIn(unit_test.unique_id, manifest.disabled)
        self.assertEqual(len(manifest.disabled[unit_test.unique_id]), 1)
        self.assertEqual(schema_file.unit_tests.count(unit_test.unique_id), 1)

    def test_missing_model_raises_parsing_error(self):
        # A unit test whose `model` never resolves must raise in
        # process_models_for_unit_test (covers the not-found path of the
        # re-resolution added for dbt-core #11139).
        block = self.yaml_block_for(UNIT_TEST_MODEL_NOT_FOUND_SOURCE, "test_missing.yml")
        UnitTestParser(self.parser, block).parse()

        manifest = self.parser.manifest
        unit_test = manifest.unit_tests[
            "unit_test.snowplow.my_model_doesnt_exist.test_my_model_doesnt_exist"
        ]
        with self.assertRaises(ParsingError):
            process_models_for_unit_test(manifest, unit_test, {})

    def test_multiple_unit_tests(self):
        block = self.yaml_block_for(UNIT_TEST_MULTIPLE_SOURCE, "test_my_model.yml")

        UnitTestParser(self.parser, block).parse()

        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1, unit_tests=2)
        for unit_test in list(self.parser.manifest.unit_tests.values()):
            self.assertEqual(unit_test.depends_on.nodes, [])
            process_models_for_unit_test(self.parser.manifest, unit_test, {})
            self.assertEqual(len(unit_test.depends_on.nodes), 1)
            self.assertEqual(unit_test.depends_on.nodes[0], "model.snowplow.my_model")

    def _assert_fixture_yml_reorders_to_expected_rows(
        self, unit_test_fixture_yml, fixture_expected_field_format, expected_rows
    ):
        block = self.yaml_block_for(unit_test_fixture_yml, "test_my_model.yml")

        UnitTestParser(self.parser, block).parse()

        self.assert_has_manifest_lengths(self.parser.manifest, nodes=1, unit_tests=1)
        unit_test = list(self.parser.manifest.unit_tests.values())[0]
        self.assertEqual(unit_test.depends_on.nodes, [])
        process_models_for_unit_test(self.parser.manifest, unit_test, {})
        expected = UnitTestDefinition(
            name="test_my_model_null_handling",
            model="my_model",
            resource_type=NodeType.Unit,
            package_name="snowplow",
            path=block.path.relative_path,
            original_file_path=block.path.original_file_path,
            unique_id="unit_test.snowplow.my_model.test_my_model_null_handling",
            given=[],
            expect=UnitTestOutputFixture(format=fixture_expected_field_format, rows=expected_rows),
            description="unit test description",
            overrides=None,
            depends_on=DependsOn(nodes=["model.snowplow.my_model"]),
            fqn=["snowplow", "my_model", "test_my_model_null_handling"],
            config=UnitTestConfig(),
            schema="test_schema",
        )
        expected.build_unit_test_checksum()
        assertEqualNodes(unit_test, expected)

    def test_expected_promote_non_none_row_dct(self):
        expected_rows = [
            {"id": 6, "col1": "f"},
            {"id": None, "col1": "e"},
            {"id": None, "col1": "d"},
        ]
        self._assert_fixture_yml_reorders_to_expected_rows(
            UNIT_TEST_NONE_ROWS_SORT, UnitTestFormat.Dict, expected_rows
        )

    def test_expected_promote_non_none_row_csv(self):
        expected_rows = [
            {"id": "6", "col1": "f"},
            {"id": None, "col1": "e"},
            {"id": None, "col1": "d"},
        ]
        self._assert_fixture_yml_reorders_to_expected_rows(
            UNIT_TEST_NONE_ROWS_SORT_CSV, UnitTestFormat.CSV, expected_rows
        )

    def test_expected_promote_non_none_row_sql(self):
        expected_rows = "select null\n" + "select 1"
        self._assert_fixture_yml_reorders_to_expected_rows(
            UNIT_TEST_NONE_ROWS_SORT_SQL, UnitTestFormat.SQL, expected_rows
        )

    def test_no_full_row_does_not_raise_exception(self):
        catcher = EventCatcher(SystemStdErr)
        add_callback_to_manager(catcher.catch)

        block = self.yaml_block_for(UNIT_TEST_NONE_ROWS_SORT_FAILS, "test_my_model.yml")
        UnitTestParser(self.parser, block).parse()

        assert len(catcher.caught_events) == 1
