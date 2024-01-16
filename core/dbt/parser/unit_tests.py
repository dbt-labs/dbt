from csv import DictReader
from pathlib import Path
from typing import List, Set, Dict, Any, Optional, Type, TypeVar
import os
from io import StringIO
import csv

from dbt_extractor import py_extract_from_source, ExtractionError  # type: ignore

from dbt import utils
from dbt.config import RuntimeConfig
from dbt.context.context_config import (
    ContextConfig,
    BaseContextConfigGenerator,
    ContextConfigGenerator,
    UnrenderedConfigGenerator,
)
from dbt.context.providers import generate_parse_exposure, get_rendered
from dbt.contracts.files import FileHash
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.model_config import UnitTestNodeConfig, ModelConfig, UnitTestConfig
from dbt.contracts.graph.nodes import (
    ModelNode,
    UnitTestNode,
    UnitTestDefinition,
    DependsOn,
    UnitTestSourceDefinition,
    UnpatchedUnitTestDefinition,
)
from dbt.contracts.graph.unparsed import (
    UnparsedUnitTest,
    UnitTestFormat,
    UnitTestNodeVersion,
    UnitTestPatch,
    NodeVersion,
)
from dbt.common.dataclass_schema import dbtClassMixin
from dbt.exceptions import (
    ParsingError,
    InvalidUnitTestGivenInput,
    DbtInternalError,
)
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

    def parse_unit_test_case(self, test_case: UnitTestDefinition, version: Optional[str] = None):
        # Create unit test node based on the node being tested
        unique_id = self.manifest.ref_lookup.get_unique_id(
            key=test_case.model, package=test_case.package_name, version=version
        )
        tested_node = self.manifest.ref_lookup.perform_lookup(unique_id, self.manifest)
        assert isinstance(tested_node, ModelNode)

        # Create UnitTestNode based on model being tested. Since selection has
        # already been done, we don't have to care about fields that are necessary
        # for selection.
        name = f"{test_case.model}__{test_case.name}"
        unit_test_node = UnitTestNode(
            name=name,
            resource_type=NodeType.Unit,
            package_name=test_case.package_name,
            depends_on=test_case.depends_on,
            path=get_pseudo_test_path(name, test_case.original_file_path),
            original_file_path=test_case.original_file_path,
            unique_id=test_case.unique_id,
            config=UnitTestNodeConfig(
                materialized="unit",
                expected_rows=test_case.expect.rows,  # type:ignore
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
                "raw_code": self._build_fixture_raw_code(given.rows, None),
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
                # TODO: i think this should be model_name.version
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


T = TypeVar("T", bound=dbtClassMixin)


class UnitTestParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, "unit_tests")
        self.schema_parser = schema_parser
        self.yaml = yaml

    def _target_from_dict(self, cls: Type[T], data: Dict[str, Any]) -> T:
        path = self.yaml.path.original_file_path
        try:
            cls.validate(data)
            return cls.from_dict(data)
        except (ValidationError, JSONValidationError) as exc:
            raise YamlParseDictError(path, self.key, data, exc)

    # This should create the UnparseUnitTest object.  Then it should be turned into and UnpatchedUnitTest
    def parse(self) -> ParseResult:
        for data in self.get_key_dicts():
            unit_test = self._target_from_dict(UnparsedUnitTest, data)
            self.add_unit_test_definition(unit_test)

        return ParseResult()

    def add_unit_test_definition(self, unit_test: UnparsedUnitTest) -> None:
        unit_test_case_unique_id = (
            f"{NodeType.Unit}.{self.project.project_name}.{unit_test.model}.{unit_test.name}"
        )
        unit_test_fqn = self._build_fqn(
            self.project.project_name,
            self.yaml.path.original_file_path,
            unit_test.model,
            unit_test.name,
        )
        # unit_test_config = self._build_unit_test_config(unit_test_fqn, unit_test.config)

        unit_test_definition = UnpatchedUnitTestDefinition(
            name=unit_test.name,
            package_name=self.project.project_name,
            path=self.yaml.path.relative_path,
            original_file_path=self.yaml.path.original_file_path,
            unique_id=unit_test_case_unique_id,
            resource_type=NodeType.Unit,
            fqn=unit_test_fqn,
            model=unit_test.model,
            given=unit_test.given,
            expect=unit_test.expect,
            versions=unit_test.versions,
            description=unit_test.description,
            overrides=unit_test.overrides,
            config=unit_test.config,
        )

        # Check that format and type of rows matches for each given input,
        # convert rows to a list of dictionaries, and add the unique_id of
        # the unit_test_definition to the fixture source_file for partial parsing.
        self._validate_and_normalize_given(unit_test_definition)
        self._validate_and_normalize_expect(unit_test_definition)

        # # for calculating state:modified
        # unit_test_definition.build_unit_test_checksum()
        self.manifest.add_unit_test(self.yaml.file, unit_test_definition)

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

    def _get_fixture(self, fixture_name: str, project_name: str):
        fixture_unique_id = f"{NodeType.Fixture}.{project_name}.{fixture_name}"
        if fixture_unique_id in self.manifest.fixtures:
            fixture = self.manifest.fixtures[fixture_unique_id]
            return fixture
        else:
            raise ParsingError(
                f"File not found for fixture '{fixture_name}' in unit tests in {self.yaml.path.original_file_path}"
            )

    def _validate_and_normalize_given(self, unit_test_definition):
        for ut_input in unit_test_definition.given:
            self._validate_and_normalize_rows(ut_input, unit_test_definition, "input")

    def _validate_and_normalize_expect(self, unit_test_definition):
        self._validate_and_normalize_rows(
            unit_test_definition.expect, unit_test_definition, "expected"
        )

    def _validate_and_normalize_rows(self, ut_fixture, unit_test_definition, fixture_type) -> None:
        if ut_fixture.format == UnitTestFormat.Dict:
            if ut_fixture.rows is None and ut_fixture.fixture is None:  # This is a seed
                ut_fixture.rows = self._load_rows_from_seed(ut_fixture.input)
            if not isinstance(ut_fixture.rows, list):
                raise ParsingError(
                    f"Unit test {unit_test_definition.name} has {fixture_type} rows "
                    f"which do not match format {ut_fixture.format}"
                )
        elif ut_fixture.format == UnitTestFormat.CSV:
            if not (isinstance(ut_fixture.rows, str) or isinstance(ut_fixture.fixture, str)):
                raise ParsingError(
                    f"Unit test {unit_test_definition.name} has {fixture_type} rows or fixtures "
                    f"which do not match format {ut_fixture.format}.  Expected string."
                )

            if ut_fixture.fixture:
                # find fixture file object and store unit_test_definition unique_id
                fixture = self._get_fixture(ut_fixture.fixture, self.project.project_name)
                fixture_source_file = self.manifest.files[fixture.file_id]
                fixture_source_file.unit_tests.append(unit_test_definition.unique_id)
                ut_fixture.rows = fixture.rows
            else:
                ut_fixture.rows = self._convert_csv_to_list_of_dicts(ut_fixture.rows)

    def _convert_csv_to_list_of_dicts(self, csv_string: str) -> List[Dict[str, Any]]:
        dummy_file = StringIO(csv_string)
        reader = csv.DictReader(dummy_file)
        rows = []
        for row in reader:
            rows.append(row)
        return rows

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


# unit tests are patched because we need to support model versions but we can't know
# what versions of a model exist until after we parse the schma files for the models
class UnitTestPatcher:
    def __init__(
        self,
        root_project: RuntimeConfig,
        manifest: Manifest,
    ) -> None:
        self.root_project = root_project
        self.manifest = manifest
        self.patches_used: Dict[str, Set[str]] = {}
        self.unit_tests: Dict[str, UnitTestDefinition] = {}

    # This method calls the 'parse_unit_test' method which takes
    # the UnpatchedUnitTestDefinitions in the manifest and combines them
    # with what we know about versioned models to generate appropriate
    # unit tests
    def construct_unit_tests(self) -> None:
        for unique_id, unpatched in self.manifest.unit_tests.items():
            if isinstance(unpatched, UnitTestDefinition):
                # In partial parsing, there will be UnitTestDefinition
                # which must be retained.
                self.unit_tests[unique_id] = unpatched
                continue
            # returns None if there is no patch
            patch = self.get_patch_for(unpatched)

            # returns unpatched if there is no patch
            patched = self.patch_unit_test(unpatched, patch)

            # Convert UnpatchedUnitTestDefinition to a list of UnitTestDefinition based on model versions
            version_list = self.get_unit_test_versions(
                model_name=patched.model, versions=patched.versions
            )
            parsed_unit_test = self.build_unit_test_definition(
                unit_test=patched, versions=version_list
            )
            self.unit_tests[parsed_unit_test.unique_id] = self.build_unit_test_definition(
                unit_test=patched, versions=version_list
            )

    def patch_unit_test(
        self,
        unpatched: UnpatchedUnitTestDefinition,
        patch: Optional[UnitTestPatch],
    ) -> UnpatchedUnitTestDefinition:

        # This skips patching if no patch exists because of the
        # performance overhead of converting to and from dicts
        if patch is None:
            return unpatched

        unit_test_dct = unpatched.to_dict(omit_none=True)
        patch_path: Optional[Path] = None

        if patch is not None:
            unit_test_dct.update(patch.to_patch_dict())
            patch_path = patch.path

        unit_test = UnparsedUnitTest.from_dict(unit_test_dct)
        return unpatched.replace(unit_test=unit_test, patch_path=patch_path)

    def _find_tested_model_node(
        self, unit_test: UnpatchedUnitTestDefinition, versions: List[NodeVersion]
    ) -> List[ModelNode]:  # TODO: also source node?
        package_name = unit_test.package_name
        # TODO: does this work when `define_id` is used in the yaml?
        model_name_split = unit_test.model.split()
        model_name = model_name_split[0]
        tested_nodes = []
        if not versions:
            tested_nodes = [
                self.manifest.ref_lookup.find(
                    key=model_name, package=package_name, version=None, manifest=self.manifest
                )
            ]
        for version in versions:
            tested_nodes.append(
                self.manifest.ref_lookup.find(
                    key=model_name, package=package_name, version=version, manifest=self.manifest
                )
            )
        if not tested_nodes:
            raise ParsingError(
                f"Unable to find model '{package_name}.{unit_test.model}' for unit tests in {unit_test.original_file_path}"
            )

        return tested_nodes

    def build_unit_test_definition(
        self, unit_test: UnpatchedUnitTestDefinition, versions: List[NodeVersion]
    ) -> UnitTestDefinition:

        config = self._generate_unit_test_config(
            target=unit_test,
            rendered=True,
        )

        unit_test_config = config.finalize_and_validate()

        if not isinstance(config, UnitTestConfig):
            raise DbtInternalError(
                f"Calculated a {type(config)} for a unit test, but expected a UnitTestConfig"
            )

        tested_model_nodes = self._find_tested_model_node(unit_test, versions=versions)
        tested_model_unique_ids = [node.unique_id for node in tested_model_nodes]
        schema = tested_model_nodes[0].schema
        unit_test_case_unique_id = (
            f"{NodeType.Unit}.{unit_test.package_name}.{unit_test.model}.{unit_test.name}"
        )
        # unit_test_model_name = f"{unit_test.model}.v{version}" if version else unit_test.model
        unit_test_model_name = unit_test.model
        unit_test_fqn = self._build_fqn(
            unit_test.package_name,
            unit_test.original_file_path,
            unit_test_model_name,
            unit_test.name,
        )

        parsed_unit_test = UnitTestDefinition(
            name=unit_test.name,
            model=unit_test_model_name,
            resource_type=NodeType.Unit,
            package_name=unit_test.package_name,
            path=unit_test.path,
            original_file_path=unit_test.original_file_path,
            unique_id=unit_test_case_unique_id,
            versions=versions,
            given=unit_test.given,
            expect=unit_test.expect,
            description=unit_test.description,
            overrides=unit_test.overrides,
            depends_on=DependsOn(nodes=tested_model_unique_ids),
            fqn=unit_test_fqn,
            config=unit_test_config,
            schema=schema,
        )

        # for calculating state:modified
        parsed_unit_test.build_unit_test_checksum()

        # relation name is added after instantiation because the adapter does
        # not provide the relation name for a UnpatchedSourceDefinition object
        return parsed_unit_test

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

    def get_unit_test_versions(
        self, model_name: str, versions: Optional[UnitTestNodeVersion]
    ) -> List[Optional[NodeVersion]]:
        version_list = []
        if versions is None:
            for node in self.manifest.nodes.values():
                # only model nodes have unit tests
                if isinstance(node, ModelNode) and node.is_versioned:
                    if node.name == model_name:
                        version_list.append(node.version)
        elif versions.exclude is not None:
            for node in self.manifest.nodes.values():
                # only model nodes have unit tests
                if isinstance(node, ModelNode) and node.is_versioned:
                    if node.name == model_name:
                        # no version has been specified and this version is not explicitly excluded
                        if node.version not in versions.exclude:
                            version_list.append(node.version)
        # versions were explicitly included
        else:
            for i in versions.include:  # type: ignore[union-attr]
                # todo: does this actually need reformatting?
                version_list.append(i)

        return version_list

    def get_patch_for(
        self,
        unpatched: UnpatchedUnitTestDefinition,
    ) -> Optional[UnitTestPatch]:
        if isinstance(unpatched, UnitTestDefinition):
            return None
        key = unpatched.name
        patch: Optional[UnitTestPatch] = self.manifest.unit_test_patches.get(key)
        if patch is None:
            return None
        if key not in self.patches_used:
            # mark the key as used
            self.patches_used[key] = set()
        return patch

    def _generate_unit_test_config(self, target: UnpatchedUnitTestDefinition, rendered: bool):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        precedence_configs.update(target.config)

        return generator.calculate_node_config(
            config_call_dict={},
            fqn=target.fqn,
            resource_type=NodeType.Unit,
            project_name=target.package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )
