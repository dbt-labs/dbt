from csv import DictReader
from pathlib import Path
from typing import List, Set, Dict, Any
import os

from dbt_extractor import py_extract_from_source, ExtractionError  # type: ignore

from dbt import utils
from dbt.config import RuntimeConfig
from dbt.context.context_config import ContextConfig
from dbt.context.providers import generate_parse_exposure, get_rendered
from dbt.contracts.files import FileHash
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.model_config import UnitTestNodeConfig, ModelConfig
from dbt.contracts.graph.nodes import (
    ModelNode,
    UnitTestNode,
    UnitTestDefinition,
    DependsOn,
    UnitTestConfig,
    UnitTestSourceDefinition,
)
from dbt.contracts.graph.unparsed import UnparsedUnitTest
from dbt.exceptions import ParsingError, InvalidUnitTestGivenInput
from dbt.graph import UniqueId
from dbt.node_types import NodeType
from dbt.parser.schemas import (
    SchemaParser,
    YamlBlock,
    ValidationError,
    JSONValidationError,
    YamlParseDictError,
    YamlReader,
    ParseResult,
)
from dbt.utils import get_pseudo_test_path


class UnitTestManifestLoader:
    def __init__(self, manifest, root_project, selected) -> None:
        self.manifest: Manifest = manifest
        self.root_project: RuntimeConfig = root_project
        # selected comes from the initial selection against a "regular" manifest
        self.selected: Set[UniqueId] = selected
        self.unit_test_manifest = Manifest(macros=manifest.macros)

    def load(self) -> Manifest:
        for unique_id in self.selected:
            if unique_id in self.manifest.unit_tests:
                unit_test_case: UnitTestDefinition = self.manifest.unit_tests[unique_id]
                self.parse_unit_test_case(unit_test_case)
        return self.unit_test_manifest

    def parse_unit_test_case(self, test_case: UnitTestDefinition):
        # Create unit test node based on the node being tested
        tested_node = self.manifest.ref_lookup.perform_lookup(
            f"model.{test_case.package_name}.{test_case.model}", self.manifest
        )
        assert isinstance(tested_node, ModelNode)

        # Create UnitTestNode based on model being tested. Since selection has
        # already been done, we don't have to care about fields that are necessary
        # for selection.
        # Note: no depends_on, that's added later using input nodes
        name = f"{test_case.model}__{test_case.name}"
        unit_test_node = UnitTestNode(
            name=name,
            resource_type=NodeType.Unit,
            package_name=test_case.package_name,
            path=get_pseudo_test_path(name, test_case.original_file_path),
            original_file_path=test_case.original_file_path,
            unique_id=test_case.unique_id,
            config=UnitTestNodeConfig(
                materialized="unit",
                expected_rows=test_case.expect.get_rows(
                    self.root_project.project_root, self.root_project.fixture_paths
                ),
            ),
            raw_code=tested_node.raw_code,
            database=tested_node.database,
            schema=tested_node.schema,
            alias=name,
            fqn=test_case.unique_id.split("."),
            checksum=FileHash.empty(),
            tested_node_unique_id=tested_node.unique_id,
            overrides=test_case.overrides,
        )

        ctx = generate_parse_exposure(
            unit_test_node,  # type: ignore
            self.root_project,
            self.manifest,
            test_case.package_name,
        )
        get_rendered(unit_test_node.raw_code, ctx, unit_test_node, capture_macros=True)
        # unit_test_node now has a populated refs/sources

        self.unit_test_manifest.nodes[unit_test_node.unique_id] = unit_test_node

        # Now create input_nodes for the test inputs
        """
        given:
          - input: ref('my_model_a')
            rows: []
          - input: ref('my_model_b')
            rows:
              - {id: 1, b: 2}
              - {id: 2, b: 2}
        """
        # Add the model "input" nodes, consisting of all referenced models in the unit test.
        # This creates an ephemeral model for every input in every test, so there may be multiple
        # input models substituting for the same input ref'd model. Note that since these are
        # always "ephemeral" they just wrap the tested_node SQL in additional CTEs. No actual table
        # or view is created.
        for given in test_case.given:
            # extract the original_input_node from the ref in the "input" key of the given list
            original_input_node = self._get_original_input_node(given.input, tested_node)

            project_root = self.root_project.project_root
            common_fields = {
                "resource_type": NodeType.Model,
                "package_name": test_case.package_name,
                "original_file_path": original_input_node.original_file_path,
                "config": ModelConfig(materialized="ephemeral"),
                "database": original_input_node.database,
                "alias": original_input_node.identifier,
                "schema": original_input_node.schema,
                "fqn": original_input_node.fqn,
                "checksum": FileHash.empty(),
                "raw_code": self._build_fixture_raw_code(
                    given.get_rows(project_root, self.root_project.fixture_paths), None
                ),
            }

            if original_input_node.resource_type in (
                NodeType.Model,
                NodeType.Seed,
                NodeType.Snapshot,
            ):
                input_name = f"{unit_test_node.name}__{original_input_node.name}"
                input_node = ModelNode(
                    **common_fields,
                    unique_id=f"model.{test_case.package_name}.{input_name}",
                    name=input_name,
                    path=original_input_node.path,
                )
            elif original_input_node.resource_type == NodeType.Source:
                # We are reusing the database/schema/identifier from the original source,
                # but that shouldn't matter since this acts as an ephemeral model which just
                # wraps a CTE around the unit test node.
                input_name = f"{unit_test_node.name}__{original_input_node.search_name}__{original_input_node.name}"
                input_node = UnitTestSourceDefinition(
                    **common_fields,
                    unique_id=f"model.{test_case.package_name}.{input_name}",
                    name=original_input_node.name,  # must be the same name for source lookup to work
                    path=input_name + ".sql",  # for writing out compiled_code
                    source_name=original_input_node.source_name,  # needed for source lookup
                )
                # Sources need to go in the sources dictionary in order to create the right lookup
                self.unit_test_manifest.sources[input_node.unique_id] = input_node  # type: ignore

            # Both ModelNode and UnitTestSourceDefinition need to go in nodes dictionary
            self.unit_test_manifest.nodes[input_node.unique_id] = input_node

            # Populate this_input_node_unique_id if input fixture represents node being tested
            if original_input_node == tested_node:
                unit_test_node.this_input_node_unique_id = input_node.unique_id

            # Add unique ids of input_nodes to depends_on
            unit_test_node.depends_on.nodes.append(input_node.unique_id)

    def _build_fixture_raw_code(self, rows, column_name_to_data_types) -> str:
        # We're not currently using column_name_to_data_types, but leaving here for
        # possible future use.
        return ("{{{{ get_fixture_sql({rows}, {column_name_to_data_types}) }}}}").format(
            rows=rows, column_name_to_data_types=column_name_to_data_types
        )

    def _get_original_input_node(self, input: str, tested_node: ModelNode):
        """
        Returns the original input node as defined in the project given an input reference
        and the node being tested.

        input: str representing how input node is referenced in tested model sql
          * examples:
            - "ref('my_model_a')"
            - "source('my_source_schema', 'my_source_name')"
            - "this"
        tested_node: ModelNode of representing node being tested
        """
        if input.strip() == "this":
            original_input_node = tested_node
        else:
            try:
                statically_parsed = py_extract_from_source(f"{{{{ {input} }}}}")
            except ExtractionError:
                raise InvalidUnitTestGivenInput(input=input)

            if statically_parsed["refs"]:
                ref = list(statically_parsed["refs"])[0]
                name = ref.get("name")
                package = ref.get("package")
                version = ref.get("version")
                # TODO: disabled lookup, versioned lookup, public models
                original_input_node = self.manifest.ref_lookup.find(
                    name, package, version, self.manifest
                )
            elif statically_parsed["sources"]:
                source = list(statically_parsed["sources"])[0]
                input_source_name, input_name = source
                original_input_node = self.manifest.source_lookup.find(
                    f"{input_source_name}.{input_name}",
                    None,
                    self.manifest,
                )
            else:
                raise InvalidUnitTestGivenInput(input=input)

        return original_input_node


class UnitTestParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, "unit_tests")
        self.schema_parser = schema_parser
        self.yaml = yaml

    def parse(self) -> ParseResult:
        for data in self.get_key_dicts():
            unit_test = self._get_unit_test(data)
            tested_model_node = self._find_tested_model_node(unit_test)
            unit_test_case_unique_id = (
                f"{NodeType.Unit}.{self.project.project_name}.{unit_test.model}.{unit_test.name}"
            )
            unit_test_fqn = self._build_fqn(
                self.project.project_name,
                self.yaml.path.original_file_path,
                unit_test.model,
                unit_test.name,
            )
            unit_test_config = self._build_unit_test_config(unit_test_fqn, unit_test.config)

            # Check that format and type of rows matches for each given input
            for input in unit_test.given:
                if input.rows is None and input.fixture is None:
                    input.rows = self._load_rows_from_seed(input.input)
                input.validate_fixture("input", unit_test.name)
            unit_test.expect.validate_fixture("expected", unit_test.name)

            unit_test_definition = UnitTestDefinition(
                name=unit_test.name,
                model=unit_test.model,
                resource_type=NodeType.Unit,
                package_name=self.project.project_name,
                path=self.yaml.path.relative_path,
                original_file_path=self.yaml.path.original_file_path,
                unique_id=unit_test_case_unique_id,
                given=unit_test.given,
                expect=unit_test.expect,
                description=unit_test.description,
                overrides=unit_test.overrides,
                depends_on=DependsOn(nodes=[tested_model_node.unique_id]),
                fqn=unit_test_fqn,
                config=unit_test_config,
                schema=tested_model_node.schema,
            )
            # For partial parsing, we add the unique_id of the unit test definition to the
            # fixture file records
            self._add_unit_test_to_fixture_files(unit_test_definition)
            # for calculating state:modified
            unit_test_definition.build_unit_test_checksum(
                self.schema_parser.project.project_root, self.schema_parser.project.fixture_paths
            )
            self.manifest.add_unit_test(self.yaml.file, unit_test_definition)

        return ParseResult()

    def _add_unit_test_to_fixture_files(self, unit_test_definition):
        for given in unit_test_definition.given:
            if given.fixture:
                # find fixture file object and store unit_test_definition unique_id
                fixture_source_file = self.get_fixture_source_file(
                    given.fixture, self.project.project_name
                )
                fixture_source_file.unit_tests.append(unit_test_definition.unique_id)
        if unit_test_definition.expect.fixture:
            # find fixture file object and store unit_test_definition unique_id
            fixture_source_file = self.get_fixture_source_file(
                unit_test_definition.expect.fixture,
                self.project.project_name,
            )
            fixture_source_file.unit_tests.append(unit_test_definition.unique_id)

    def get_fixture_source_file(self, fixture_name: str, project_name: str):
        fixture_unique_id = f"fixture.{project_name}.{fixture_name}"
        if fixture_unique_id in self.manifest.fixtures:
            fixture = self.manifest.fixtures[fixture_unique_id]
            fixture_source_file = self.manifest.files[fixture.file_id]
            return fixture_source_file
        else:
            raise ParsingError(
                f"File not found for fixture '{fixture_name}' in unit tests in {self.yaml.path.original_file_path}"
            )

    def _get_unit_test(self, data: Dict[str, Any]) -> UnparsedUnitTest:
        try:
            UnparsedUnitTest.validate(data)
            return UnparsedUnitTest.from_dict(data)
        except (ValidationError, JSONValidationError) as exc:
            raise YamlParseDictError(self.yaml.path, self.key, data, exc)

    def _find_tested_model_node(self, unit_test: UnparsedUnitTest) -> ModelNode:
        package_name = self.project.project_name
        model_name_split = unit_test.model.split()
        model_name = model_name_split[0]
        model_version = model_name_split[1] if len(model_name_split) == 2 else None

        tested_node = self.manifest.ref_lookup.find(
            model_name, package_name, model_version, self.manifest
        )
        if not tested_node:
            raise ParsingError(
                f"Unable to find model '{package_name}.{unit_test.model}' for unit tests in {self.yaml.path.original_file_path}"
            )

        return tested_node

    def _build_unit_test_config(
        self, unit_test_fqn: List[str], config_dict: Dict[str, Any]
    ) -> UnitTestConfig:
        config = ContextConfig(
            self.schema_parser.root_project,
            unit_test_fqn,
            NodeType.Unit,
            self.schema_parser.project.project_name,
        )
        unit_test_config_dict = config.build_config_dict(patch_config_dict=config_dict)
        unit_test_config_dict = self.render_entry(unit_test_config_dict)

        return UnitTestConfig.from_dict(unit_test_config_dict)

    def _build_fqn(self, package_name, original_file_path, model_name, test_name):
        # This code comes from "get_fqn" and "get_fqn_prefix" in the base parser.
        # We need to get the directories underneath the model-path.
        path = Path(original_file_path)
        relative_path = str(path.relative_to(*path.parts[:1]))
        no_ext = os.path.splitext(relative_path)[0]
        fqn = [package_name]
        fqn.extend(utils.split_path(no_ext)[:-1])
        fqn.append(model_name)
        fqn.append(test_name)
        return fqn

    def _load_rows_from_seed(self, ref_str: str) -> List[Dict[str, Any]]:
        """Read rows from seed file on disk if not specified in YAML config. If seed file doesn't exist, return empty list."""
        ref = py_extract_from_source("{{ " + ref_str + " }}")["refs"][0]

        rows: List[Dict[str, Any]] = []

        seed_name = ref["name"]
        package_name = ref.get("package", self.project.project_name)

        seed_node = self.manifest.ref_lookup.find(seed_name, package_name, None, self.manifest)

        if not seed_node or seed_node.resource_type != NodeType.Seed:
            # Seed not found in custom package specified
            if package_name != self.project.project_name:
                raise ParsingError(
                    f"Unable to find seed '{package_name}.{seed_name}' for unit tests in '{package_name}' package"
                )
            else:
                raise ParsingError(
                    f"Unable to find seed '{package_name}.{seed_name}' for unit tests in directories: {self.project.seed_paths}"
                )

        seed_path = Path(seed_node.root_path) / seed_node.original_file_path
        with open(seed_path, "r") as f:
            for row in DictReader(f):
                rows.append(row)

        return rows
