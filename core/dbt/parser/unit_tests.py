import csv
import os
from copy import deepcopy
from csv import DictReader
from io import StringIO
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

from dbt import utils
from dbt.artifacts.resources import ModelConfig, UnitTestConfig, UnitTestFormat
from dbt.config import RuntimeConfig
from dbt.context.context_config import ContextConfig
from dbt.context.providers import generate_parse_exposure, get_rendered
from dbt.contracts.files import FileHash, SchemaSourceFile
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.model_config import UnitTestNodeConfig
from dbt.contracts.graph.nodes import (
    DependsOn,
    ModelNode,
    UnitTestDefinition,
    UnitTestNode,
    UnitTestSourceDefinition,
)
from dbt.contracts.graph.unparsed import UnparsedUnitTest
from dbt.exceptions import InvalidUnitTestGivenInput, ParsingError
from dbt.graph import UniqueId
from dbt.node_types import NodeType
from dbt.parser.schemas import (
    JSONValidationError,
    ParseResult,
    SchemaParser,
    ValidationError,
    YamlBlock,
    YamlParseDictError,
    YamlReader,
)
from dbt.utils import get_pseudo_test_path
from dbt_common.events.functions import fire_event
from dbt_common.events.types import SystemStdErr
from dbt_extractor import ExtractionError, py_extract_from_source  # type: ignore


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
                if not unit_test_case.config.enabled:
                    continue
                self.parse_unit_test_case(unit_test_case)
        return self.unit_test_manifest

    def parse_unit_test_case(self, test_case: UnitTestDefinition):
        # Create unit test node based on the node being tested
        # The tested_node has already been resolved and is in depends_on.nodes
        tested_node_unique_id = test_case.depends_on.nodes[0]
        tested_node = self.manifest.nodes[tested_node_unique_id]
        assert isinstance(tested_node, ModelNode)

        # Create UnitTestNode based on model being tested. Since selection has
        # already been done, we don't have to care about fields that are necessary
        # for selection.
        # Note: no depends_on, that's added later using input nodes
        name = test_case.name
        if tested_node.is_versioned:
            name = name + f"_v{tested_node.version}"
        expected_sql: Optional[str] = None
        if test_case.expect.format == UnitTestFormat.SQL:
            expected_rows: List[Dict[str, Any]] = []
            expected_sql = test_case.expect.rows  # type: ignore
        else:
            assert isinstance(test_case.expect.rows, List)
            expected_rows = deepcopy(test_case.expect.rows)

        assert isinstance(expected_rows, List)
        unit_test_node = UnitTestNode(
            name=name,
            resource_type=NodeType.Unit,
            package_name=test_case.package_name,
            path=get_pseudo_test_path(name, test_case.original_file_path),
            original_file_path=test_case.original_file_path,
            unique_id=test_case.unique_id,
            config=UnitTestNodeConfig(
                materialized="unit", expected_rows=expected_rows, expected_sql=expected_sql
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
            self.root_project,  # config
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
            original_input_node = self._get_original_input_node(
                given.input, tested_node, test_case.name
            )
            input_name = original_input_node.name
            common_fields = {
                "resource_type": NodeType.Model,
                # root directory for input and output fixtures
                "original_file_path": unit_test_node.original_file_path,
                "config": ModelConfig(materialized="ephemeral"),
                "database": original_input_node.database,
                "alias": original_input_node.identifier,
                "schema": original_input_node.schema,
                "fqn": original_input_node.fqn,
                "checksum": FileHash.empty(),
                "raw_code": self._build_fixture_raw_code(given.rows, None, given.format),
                "package_name": original_input_node.package_name,
                "unique_id": f"model.{original_input_node.package_name}.{input_name}",
                "name": input_name,
                "path": f"{input_name}.sql",
            }
            resource_type = original_input_node.resource_type

            if resource_type in (
                NodeType.Model,
                NodeType.Seed,
                NodeType.Snapshot,
            ):

                input_node = ModelNode(
                    **common_fields,
                    defer_relation=original_input_node.defer_relation,
                )
                if resource_type == NodeType.Model:
                    if original_input_node.version:
                        input_node.version = original_input_node.version
                    if original_input_node.latest_version:
                        input_node.latest_version = original_input_node.latest_version

            elif resource_type == NodeType.Source:
                # We are reusing the database/schema/identifier from the original source,
                # but that shouldn't matter since this acts as an ephemeral model which just
                # wraps a CTE around the unit test node.
                input_node = UnitTestSourceDefinition(
                    **common_fields,
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

    def _build_fixture_raw_code(self, rows, column_name_to_data_types, fixture_format) -> str:
        # We're not currently using column_name_to_data_types, but leaving here for
        # possible future use.
        if fixture_format == UnitTestFormat.SQL:
            return rows
        else:
            return ("{{{{ get_fixture_sql({rows}, {column_name_to_data_types}) }}}}").format(
                rows=rows, column_name_to_data_types=column_name_to_data_types
            )

    def _get_original_input_node(self, input: str, tested_node: ModelNode, test_case_name: str):
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

        if not original_input_node:
            msg = f"Unit test '{test_case_name}' had an input ({input}) which was not found in the manifest."
            raise ParsingError(msg)

        return original_input_node


class UnitTestParser(YamlReader):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock) -> None:
        super().__init__(schema_parser, yaml, "unit_tests")
        self.schema_parser = schema_parser
        self.yaml = yaml

    def parse(self) -> ParseResult:
        for data in self.get_key_dicts():
            unit_test: UnparsedUnitTest = self._get_unit_test(data)
            tested_model_node = find_tested_model_node(
                self.manifest, self.project.project_name, unit_test.model
            )
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
                depends_on=DependsOn(),
                fqn=unit_test_fqn,
                config=unit_test_config,
                versions=unit_test.versions,
            )

            if tested_model_node:
                unit_test_definition.depends_on.nodes.append(tested_model_node.unique_id)
                unit_test_definition.schema = tested_model_node.schema

            # Check that format and type of rows matches for each given input,
            # convert rows to a list of dictionaries, and add the unique_id of
            # the unit_test_definition to the fixture source_file for partial parsing.
            self._validate_and_normalize_given(unit_test_definition)
            self._validate_and_normalize_expect(unit_test_definition)

            # for calculating state:modified
            unit_test_definition.build_unit_test_checksum()
            assert isinstance(self.yaml.file, SchemaSourceFile)
            if unit_test_config.enabled:
                self.manifest.add_unit_test(self.yaml.file, unit_test_definition)
            else:
                self.manifest.add_disabled(self.yaml.file, unit_test_definition)

        return ParseResult()

    def _get_unit_test(self, data: Dict[str, Any]) -> UnparsedUnitTest:
        try:
            UnparsedUnitTest.validate(data)
            return UnparsedUnitTest.from_dict(data)
        except (ValidationError, JSONValidationError) as exc:
            raise YamlParseDictError(self.yaml.path, self.key, data, exc)

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
                csv_rows = self.get_fixture_file_rows(
                    ut_fixture.fixture, self.project.project_name, unit_test_definition.unique_id
                )
            else:
                csv_rows = self._convert_csv_to_list_of_dicts(ut_fixture.rows)

            # Empty values (e.g. ,,) in a csv fixture should default to null, not ""
            ut_fixture.rows = [
                {k: (None if v == "" else v) for k, v in row.items()} for row in csv_rows
            ]

        elif ut_fixture.format == UnitTestFormat.SQL:
            if not (isinstance(ut_fixture.rows, str) or isinstance(ut_fixture.fixture, str)):
                raise ParsingError(
                    f"Unit test {unit_test_definition.name} has {fixture_type} rows or fixtures "
                    f"which do not match format {ut_fixture.format}.  Expected string."
                )

            if ut_fixture.fixture:
                ut_fixture.rows = self.get_fixture_file_rows(
                    ut_fixture.fixture, self.project.project_name, unit_test_definition.unique_id
                )

        # sanitize order of input
        if ut_fixture.rows and (
            ut_fixture.format == UnitTestFormat.Dict or ut_fixture.format == UnitTestFormat.CSV
        ):
            self._promote_first_non_none_row(ut_fixture)

    def _promote_first_non_none_row(self, ut_fixture):
        """
        Promote the first row with no None values to the top of the ut_fixture.rows list.

        This function modifies the ut_fixture object in place.

        Needed for databases like Redshift which uses the first value in a column to determine
        the column type. If the first value is None, the type is assumed to be VARCHAR(1).
        This leads to obscure type mismatch errors centered on a unit test fixture's `expect`.
        See https://github.com/dbt-labs/dbt-redshift/issues/821 for more info.
        """
        non_none_row_index = None

        # Iterate through each row and its index
        for index, row in enumerate(ut_fixture.rows):
            # Check if all values in the row are not None
            if all(value is not None for value in row.values()):
                non_none_row_index = index
                break

        if non_none_row_index is None:
            fire_event(
                SystemStdErr(
                    bmsg="Unit Test fixtures benefit from having at least one row free of Null values to ensure consistent column types. Failure to meet this recommendation can result in type mismatch errors between unit test source models and `expected` fixtures."
                )
            )
        else:
            ut_fixture.rows[0], ut_fixture.rows[non_none_row_index] = (
                ut_fixture.rows[non_none_row_index],
                ut_fixture.rows[0],
            )

    def get_fixture_file_rows(self, fixture_name, project_name, utdef_unique_id):
        # find fixture file object and store unit_test_definition unique_id
        fixture = self._get_fixture(fixture_name, project_name)
        fixture_source_file = self.manifest.files[fixture.file_id]
        fixture_source_file.unit_tests.append(utdef_unique_id)
        return fixture.rows

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


def find_tested_model_node(
    manifest: Manifest, current_project: str, unit_test_model: str
) -> Optional[ModelNode]:
    model_name_split = unit_test_model.split()
    model_name = model_name_split[0]
    model_version = model_name_split[1] if len(model_name_split) == 2 else None

    tested_node = manifest.ref_lookup.find(model_name, current_project, model_version, manifest)
    return tested_node


# This is called by the ManifestLoader after other processing has been done,
# so that model versions are available.
def process_models_for_unit_test(
    manifest: Manifest, current_project: str, unit_test_def: UnitTestDefinition, models_to_versions
):
    # If the unit tests doesn't have a depends_on.nodes[0] then we weren't able to resolve
    # the model, either because versions hadn't been processed yet, or it's not a valid model name
    if not unit_test_def.depends_on.nodes:
        tested_node = find_tested_model_node(manifest, current_project, unit_test_def.model)
        if not tested_node:
            raise ParsingError(
                f"Unable to find model '{current_project}.{unit_test_def.model}' for "
                f"unit test '{unit_test_def.name}' in {unit_test_def.original_file_path}"
            )
        unit_test_def.depends_on.nodes.append(tested_node.unique_id)
        unit_test_def.schema = tested_node.schema

    # The UnitTestDefinition should only have one "depends_on" at this point,
    # the one that's found by the "model" field.
    target_model_id = unit_test_def.depends_on.nodes[0]
    if target_model_id not in manifest.nodes:
        if target_model_id in manifest.disabled:
            # The model is disabled, so we don't need to do anything (#10540)
            return
        else:
            # If we've reached here and the model is not disabled, throw an error
            raise ParsingError(
                f"Unit test '{unit_test_def.name}' references a model that does not exist: {target_model_id}"
            )

    target_model = manifest.nodes[target_model_id]
    assert isinstance(target_model, ModelNode)

    target_model_is_incremental = "macro.dbt.is_incremental" in target_model.depends_on.macros
    unit_test_def_has_incremental_override = unit_test_def.overrides and isinstance(
        unit_test_def.overrides.macros.get("is_incremental"), bool
    )

    if target_model_is_incremental and (not unit_test_def_has_incremental_override):
        raise ParsingError(
            f"Boolean override for 'is_incremental' must be provided for unit test '{unit_test_def.name}' in model '{target_model.name}'"
        )

    unit_test_def_incremental_override_true = (
        unit_test_def.overrides and unit_test_def.overrides.macros.get("is_incremental")
    )
    unit_test_def_has_this_input = "this" in [i.input for i in unit_test_def.given]

    if (
        target_model_is_incremental
        and unit_test_def_incremental_override_true
        and (not unit_test_def_has_this_input)
    ):
        raise ParsingError(
            f"Unit test '{unit_test_def.name}' for incremental model '{target_model.name}' must have a 'this' input"
        )

    # unit_test_versions = unit_test_def.versions
    # We're setting up unit tests for versioned models, so if
    # the model isn't versioned, we don't need to do anything
    if not target_model.is_versioned:
        if unit_test_def.versions and (
            unit_test_def.versions.include or unit_test_def.versions.exclude
        ):
            # If model is  not versioned, we should not have an include or exclude
            msg = (
                f"Unit test '{unit_test_def.name}' should not have a versions include or exclude "
                f"when referencing non-versioned model '{target_model.name}'"
            )
            raise ParsingError(msg)
        else:
            return
    versioned_models = []
    if (
        target_model.package_name in models_to_versions
        and target_model.name in models_to_versions[target_model.package_name]
    ):
        versioned_models = models_to_versions[target_model.package_name][target_model.name]

    versions_to_test = []
    if unit_test_def.versions is None:
        versions_to_test = versioned_models
    elif unit_test_def.versions.exclude:
        for model_unique_id in versioned_models:
            model = manifest.nodes[model_unique_id]
            assert isinstance(model, ModelNode)
            if model.version in unit_test_def.versions.exclude:
                continue
            else:
                versions_to_test.append(model.unique_id)
    elif unit_test_def.versions.include:
        for model_unique_id in versioned_models:
            model = manifest.nodes[model_unique_id]
            assert isinstance(model, ModelNode)
            if model.version in unit_test_def.versions.include:
                versions_to_test.append(model.unique_id)
            else:
                continue

    if not versions_to_test:
        msg = (
            f"Unit test '{unit_test_def.name}' referenced a version of '{target_model.name}' "
            "which was not found."
        )
        raise ParsingError(msg)
    else:
        # Create unit test definitions that match the model versions
        original_unit_test_def = manifest.unit_tests.pop(unit_test_def.unique_id)
        original_unit_test_dict = original_unit_test_def.to_dict()
        schema_file = manifest.files[original_unit_test_def.file_id]
        assert isinstance(schema_file, SchemaSourceFile)
        schema_file.unit_tests.remove(original_unit_test_def.unique_id)
        for versioned_model_unique_id in versions_to_test:
            versioned_model = manifest.nodes[versioned_model_unique_id]
            assert isinstance(versioned_model, ModelNode)
            versioned_unit_test_unique_id = f"{NodeType.Unit}.{unit_test_def.package_name}.{unit_test_def.model}.{unit_test_def.name}_v{versioned_model.version}"
            new_unit_test_def = UnitTestDefinition.from_dict(original_unit_test_dict)
            new_unit_test_def.unique_id = versioned_unit_test_unique_id
            new_unit_test_def.depends_on.nodes[0] = versioned_model_unique_id
            new_unit_test_def.version = versioned_model.version
            schema_file.unit_tests.append(versioned_unit_test_unique_id)
            # fqn?
            manifest.unit_tests[versioned_unit_test_unique_id] = new_unit_test_def
