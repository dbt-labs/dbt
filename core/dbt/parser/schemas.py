import datetime
import re
import time
from abc import ABCMeta, abstractmethod
from dataclasses import dataclass, field
from typing import (
    Any,
    Callable,
    Dict,
    Generic,
    Iterable,
    List,
    Optional,
    Tuple,
    Type,
    TypeVar,
)

from dbt.artifacts.resources import CustomGranularity, Docs, RefArgs, TimeSpine
from dbt.clients.checked_load import (
    checked_load,
    issue_deprecation_warnings_for_failures,
)
from dbt.clients.jinja_static import statically_parse_ref_or_source
from dbt.clients.yaml_helper import load_yaml_text
from dbt.config import RuntimeConfig
from dbt.context.configured import SchemaYamlVars, generate_schema_yml_context
from dbt.context.context_config import ContextConfig
from dbt.contracts.files import SchemaSourceFile, SourceFile
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import (
    FunctionNode,
    Macro,
    ModelNode,
    ParsedFunctionPatch,
    ParsedMacroPatch,
    ParsedNodePatch,
    ParsedSingularTestPatch,
    UnpatchedSourceDefinition,
)
from dbt.contracts.graph.unparsed import (
    HasColumnDocs,
    HasColumnTests,
    SourcePatch,
    UnparsedAnalysisUpdate,
    UnparsedFunctionUpdate,
    UnparsedMacroUpdate,
    UnparsedModelUpdate,
    UnparsedNodeUpdate,
    UnparsedSingularTestUpdate,
    UnparsedSourceDefinition,
)
from dbt.events.types import (
    InvalidMacroAnnotation,
    MacroNotFoundForPatch,
    NoNodeForYamlKey,
    UnsupportedConstraintMaterialization,
    ValidationWarning,
    WrongResourceSchemaFile,
)
from dbt.exceptions import (
    DbtInternalError,
    DuplicateMacroPatchNameError,
    DuplicatePatchPathError,
    DuplicateSourcePatchNameError,
    InvalidAccessTypeError,
    JSONValidationError,
    ParsingError,
    YamlLoadError,
    YamlParseDictError,
    YamlParseListError,
)
from dbt.flags import get_flags
from dbt.node_types import AccessType, NodeType
from dbt.parser.base import SimpleParser
from dbt.parser.common import (
    ParserRef,
    TargetBlock,
    TestBlock,
    VersionedTestBlock,
    YamlBlock,
    schema_file_keys_to_resource_types,
    trimmed,
)
from dbt.parser.schema_generic_tests import SchemaGenericTestParser
from dbt.parser.schema_renderer import SchemaYamlRenderer
from dbt.parser.search import FileBlock
from dbt.utils import coerce_dict_str
from dbt_common.contracts.constraints import ConstraintType, ModelLevelConstraint
from dbt_common.dataclass_schema import ValidationError, dbtClassMixin
from dbt_common.events import EventLevel
from dbt_common.events.base_types import EventGroupType
from dbt_common.events.functions import fire_event, fire_or_defer_event
from dbt_common.events.types import Note
from dbt_common.exceptions import DbtValidationError
from dbt_common.utils import deep_merge

# ===============================================================================
#  Schema Parser classes
#
# The SchemaParser is a subclass of the SimpleParser from base.py, as is
# the SchemaGenericTestParser. The schema sub-parsers are all subclasses of
# the YamlReader parsing class. Most of the action in creating SourceDefinition
# nodes actually happens in the SourcePatcher class, in sources.py, which is
# called as a late-stage parsing step in manifest.py.
#
# The "patch" parsers read yaml config and properties and apply them to
# nodes that were already created from sql files.
#
# The SchemaParser and SourcePatcher both use the SchemaGenericTestParser
# (in schema_generic_tests.py) to create generic test nodes.
#
#  YamlReader
#      MetricParser (metrics) [schema_yaml_readers.py]
#      ExposureParser (exposures) [schema_yaml_readers.py]
#      GroupParser  (groups) [schema_yaml_readers.py]
#      SourceParser (sources)
#      PatchParser
#          MacroPatchParser (macros)
#          NodePatchParser
#              ModelPatchParser (models)
#              AnalysisPatchParser (analyses)
#              TestablePatchParser (seeds, snapshots)
#
# ===============================================================================


def yaml_from_file(
    source_file: SchemaSourceFile, validate: bool = False
) -> Optional[Dict[str, Any]]:
    """If loading the yaml fails, raise an exception."""
    try:
        # source_file.contents can sometimes be None
        to_load = source_file.contents or ""

        if validate:
            contents, failures = checked_load(to_load)
            issue_deprecation_warnings_for_failures(
                failures=failures, file=source_file.path.original_file_path
            )
            if contents is not None:
                from dbt.jsonschemas.jsonschemas import (
                    jsonschema_validate,
                    resources_schema,
                )

                # Validate the yaml against the jsonschema to raise deprecation warnings
                # for invalid fields.
                jsonschema_validate(
                    schema=resources_schema(),
                    json=contents,
                    file_path=source_file.path.original_file_path,
                )
        else:
            contents = load_yaml_text(to_load, source_file.path)

        if contents is None:
            return contents

        if not isinstance(contents, dict):
            raise DbtValidationError(
                f"Contents of file '{source_file.original_file_path}' are not valid. Dictionary expected."
            )

        # When loaded_at_field is defined as None or null, it shows up in
        # the dict but when it is not defined, it does not show up in the dict
        # We need to capture this to be able to override source level settings later.
        for source in contents.get("sources") or []:
            if source.get("tables") is None:
                source["tables"] = []
            for table in source.get("tables", []):
                if "loaded_at_field" in table or (
                    "config" in table
                    and table["config"] is not None
                    and table["config"].get("loaded_at_field")
                ):
                    table["loaded_at_field_present"] = True

        return contents
    except DbtValidationError as e:
        raise YamlLoadError(
            project_name=source_file.project_name, path=source_file.path.relative_path, exc=e
        )


# This is the main schema file parser, but almost everything happens in the
# the schema sub-parsers.
class SchemaParser(SimpleParser[YamlBlock, ModelNode]):
    def __init__(
        self,
        project: RuntimeConfig,
        manifest: Manifest,
        root_project: RuntimeConfig,
    ) -> None:
        super().__init__(project, manifest, root_project)

        self.generic_test_parser = SchemaGenericTestParser(project, manifest, root_project)

        self.schema_yaml_vars = SchemaYamlVars()
        self.render_ctx = generate_schema_yml_context(
            self.root_project, self.project.project_name, self.schema_yaml_vars
        )

    # This is unnecessary, but mypy was requiring it. Clean up parser code so
    # we don't have to do this.
    def parse_from_dict(self, dct):
        pass

    @classmethod
    def get_compiled_path(cls, block: FileBlock) -> str:
        # should this raise an error?
        return block.path.relative_path

    @property
    def resource_type(self) -> NodeType:
        return NodeType.Test

    def parse_file(self, block: FileBlock, dct: Optional[Dict] = None) -> None:
        assert isinstance(block.file, SchemaSourceFile)

        # If partially parsing, dct should be from pp_dict, otherwise
        # dict_from_yaml
        if dct:
            # contains the FileBlock and the data (dictionary)
            yaml_block = YamlBlock.from_file_block(block, dct)
            parser: YamlReader

            # There are 9 different yaml lists which are parsed by different parsers:
            # Model, Seed, Snapshot, Source, Macro, Analysis, Exposure, Metric, Group

            # ModelPatchParser.parse()
            if "models" in dct:
                # the models are already in the manifest as nodes when we reach this code,
                # even if they are disabled in the schema file
                model_parse_result = ModelPatchParser(self, yaml_block, "models").parse()
                for versioned_test_block in model_parse_result.versioned_test_blocks:
                    self.generic_test_parser.parse_versioned_tests(versioned_test_block)

            # PatchParser.parse()
            if "seeds" in dct:
                seed_parse_result = TestablePatchParser(self, yaml_block, "seeds").parse()
                for test_block in seed_parse_result.test_blocks:
                    self.generic_test_parser.parse_tests(test_block)

            # PatchParser.parse()
            if "snapshots" in dct:
                self._add_yaml_snapshot_nodes_to_manifest(dct["snapshots"], block)
                snapshot_parse_result = TestablePatchParser(self, yaml_block, "snapshots").parse()
                for test_block in snapshot_parse_result.test_blocks:
                    self.generic_test_parser.parse_tests(test_block)

            # This parser uses SourceParser.parse() which doesn't return
            # any test blocks. Source tests are handled at a later point
            # in the process.
            if "sources" in dct:
                parser = SourceParser(self, yaml_block, "sources")
                parser.parse()

            # PatchParser.parse() (but never test_blocks)
            if "macros" in dct:
                parser = MacroPatchParser(self, yaml_block, "macros")
                parser.parse()

            if "data_tests" in dct:
                parser = SingularTestPatchParser(self, yaml_block, "data_tests")
                try:
                    parser.parse()
                except ParsingError as e:
                    fire_event(
                        Note(
                            msg=f"Unable to parse 'data_tests' section of file '{block.path.original_file_path}'\n{e}",
                        ),
                        EventLevel.WARN,
                    )

            # PatchParser.parse() (but never test_blocks)
            if "analyses" in dct:
                parser = AnalysisPatchParser(self, yaml_block, "analyses")
                parser.parse()

            # ExposureParser.parse()
            if "exposures" in dct:
                from dbt.parser.schema_yaml_readers import ExposureParser

                exp_parser = ExposureParser(self, yaml_block)
                exp_parser.parse()

            # FunctionPatchParser.parse()
            if "functions" in dct:
                function_parser = FunctionPatchParser(self, yaml_block, "functions")
                function_parser.parse()

            # MetricParser.parse()
            if "metrics" in dct:
                from dbt.parser.schema_yaml_readers import MetricParser

                metric_parser = MetricParser(self, yaml_block)
                metric_parser.parse()

            # GroupParser.parse()
            if "groups" in dct:
                from dbt.parser.schema_yaml_readers import GroupParser

                group_parser = GroupParser(self, yaml_block)
                group_parser.parse()

            if "semantic_models" in dct:
                from dbt.parser.schema_yaml_readers import SemanticModelParser

                semantic_model_parser = SemanticModelParser(self, yaml_block)
                semantic_model_parser.parse()

            if "unit_tests" in dct:
                from dbt.parser.unit_tests import UnitTestParser

                unit_test_parser = UnitTestParser(self, yaml_block)
                unit_test_parser.parse()

            if "saved_queries" in dct:
                from dbt.parser.schema_yaml_readers import SavedQueryParser

                saved_query_parser = SavedQueryParser(self, yaml_block)
                saved_query_parser.parse()

    def _add_yaml_snapshot_nodes_to_manifest(
        self, snapshots: List[Dict[str, Any]], block: FileBlock
    ) -> None:
        """We support the creation of simple snapshots in yaml, without an
        accompanying SQL definition. For such snapshots, the user must supply
        a 'relation' property to indicate the target of the snapshot. This
        function looks for such snapshots and adds a node to manifest for each
        one we find, since they were not added during SQL parsing."""

        rebuild_refs = False
        for snapshot in snapshots:
            if "relation" in snapshot:
                from dbt.parser import SnapshotParser

                if "name" not in snapshot:
                    raise ParsingError("A snapshot must define the 'name' property. ")

                # Reuse the logic of SnapshotParser as far as possible to create
                # a new node we can add to the manifest.
                parser = SnapshotParser(self.project, self.manifest, self.root_project)
                fqn = parser.get_fqn_prefix(block.path.relative_path)
                fqn.append(snapshot["name"])

                compiled_path = snapshot["name"] + ".sql"
                snapshot_node = parser._create_parsetime_node(
                    block,
                    compiled_path,
                    parser.initial_config(fqn),
                    fqn,
                    snapshot["name"],
                )

                # Parse the expected ref() or source() expression given by
                # 'relation' so that we know what we are snapshotting.
                source_or_ref = statically_parse_ref_or_source(snapshot["relation"])
                if isinstance(source_or_ref, RefArgs):
                    snapshot_node.refs.append(source_or_ref)
                else:
                    snapshot_node.sources.append(source_or_ref)

                # Implement the snapshot SQL as a simple select *
                snapshot_node.raw_code = "select * from {{ " + snapshot["relation"] + " }}"

                # Add our new node to the manifest, and note that ref lookup collections
                # will need to be rebuilt. This adds the node unique_id to the "snapshots"
                # list in the SchemaSourceFile.
                self.manifest.add_node(block.file, snapshot_node)
                rebuild_refs = True

        if rebuild_refs:
            self.manifest.rebuild_ref_lookup()


Parsed = TypeVar(
    "Parsed", UnpatchedSourceDefinition, ParsedNodePatch, ParsedMacroPatch, ParsedSingularTestPatch
)
NodeTarget = TypeVar(
    "NodeTarget",
    UnparsedNodeUpdate,
    UnparsedAnalysisUpdate,
    UnparsedModelUpdate,
    UnparsedFunctionUpdate,
)
NonSourceTarget = TypeVar(
    "NonSourceTarget",
    UnparsedNodeUpdate,
    UnparsedAnalysisUpdate,
    UnparsedMacroUpdate,
    UnparsedModelUpdate,
    UnparsedFunctionUpdate,
    UnparsedSingularTestUpdate,
)


@dataclass
class ParseResult:
    test_blocks: List[TestBlock] = field(default_factory=list)
    versioned_test_blocks: List[VersionedTestBlock] = field(default_factory=list)


# abstract base class (ABCMeta)
# Many subclasses: MetricParser, ExposureParser, GroupParser, SourceParser,
# PatchParser, SemanticModelParser, SavedQueryParser, UnitTestParser
class YamlReader(metaclass=ABCMeta):
    def __init__(self, schema_parser: SchemaParser, yaml: YamlBlock, key: str) -> None:
        self.schema_parser: SchemaParser = schema_parser
        # key: models, seeds, snapshots, sources, macros,
        # analyses, exposures, unit_tests
        self.key: str = key
        self.yaml: YamlBlock = yaml
        self.schema_yaml_vars: SchemaYamlVars = SchemaYamlVars()
        self.render_ctx = generate_schema_yml_context(
            self.schema_parser.root_project,
            self.schema_parser.project.project_name,
            self.schema_yaml_vars,
        )
        self.renderer: SchemaYamlRenderer = SchemaYamlRenderer(self.render_ctx, self.key)

    @property
    def manifest(self) -> Manifest:
        return self.schema_parser.manifest

    @property
    def project(self) -> RuntimeConfig:
        return self.schema_parser.project

    @property
    def default_database(self) -> str:
        return self.schema_parser.default_database

    @property
    def root_project(self) -> RuntimeConfig:
        return self.schema_parser.root_project

    # for the different schema subparsers ('models', 'source', etc)
    # get the list of dicts pointed to by the key in the yaml config,
    # ensure that the dicts have string keys
    def get_key_dicts(self) -> Iterable[Dict[str, Any]]:
        data = self.yaml.data.get(self.key, [])
        if not isinstance(data, list):
            raise ParsingError(
                "{} must be a list, got {} instead: ({})".format(
                    self.key, type(data), trimmed(str(data))
                )
            )
        path = self.yaml.path.original_file_path

        # for each dict in the data (which is a list of dicts)
        for entry in data:

            # check that entry is a dict and that all dict values
            # are strings
            if coerce_dict_str(entry) is None:
                raise YamlParseListError(path, self.key, data, "expected a dict with string keys")

            if "name" not in entry and "model" not in entry:
                raise ParsingError("Entry did not contain a name")

            unrendered_config = {}
            if "config" in entry:
                unrendered_config = entry["config"]

            unrendered_version_configs = {}
            if "versions" in entry:
                for version in entry["versions"]:
                    if "v" in version:
                        unrendered_version_configs[version["v"]] = version.get("config", {})

            # For sources
            unrendered_database = entry.get("database", None)
            unrendered_schema = entry.get("schema", None)

            # Render the data (except for tests, data_tests and descriptions).
            # See the SchemaYamlRenderer
            entry = self.render_entry(entry)

            schema_file = self.yaml.file
            assert isinstance(schema_file, SchemaSourceFile)

            if unrendered_config:
                schema_file.add_unrendered_config(unrendered_config, self.key, entry["name"])

            for version, unrendered_version_config in unrendered_version_configs.items():
                schema_file.add_unrendered_config(
                    unrendered_version_config, self.key, entry["name"], version
                )

            if unrendered_database:
                schema_file.add_unrendered_database(self.key, entry["name"], unrendered_database)
            if unrendered_schema:
                schema_file.add_unrendered_schema(self.key, entry["name"], unrendered_schema)

            if self.schema_yaml_vars.env_vars:
                self.schema_parser.manifest.env_vars.update(self.schema_yaml_vars.env_vars)
                for var in self.schema_yaml_vars.env_vars.keys():
                    schema_file.add_env_var(var, self.key, entry["name"])
                self.schema_yaml_vars.env_vars = {}

            yield entry

    def render_entry(self, dct):
        try:
            # This does a deep_map which will fail if there are circular references
            dct = self.renderer.render_data(dct)
        except ParsingError as exc:
            raise ParsingError(
                f"Failed to render {self.yaml.file.path.original_file_path} from "
                f"project {self.project.project_name}: {exc}"
            ) from exc
        return dct

    @abstractmethod
    def parse(self) -> Optional[ParseResult]:
        raise NotImplementedError("parse is abstract")


T = TypeVar("T", bound=dbtClassMixin)


# This parses the 'sources' keys in yaml files.
class SourceParser(YamlReader):
    def _target_from_dict(self, cls: Type[T], data: Dict[str, Any]) -> T:
        path = self.yaml.path.original_file_path
        try:
            cls.validate(data)
            return cls.from_dict(data)
        except (ValidationError, JSONValidationError) as exc:
            raise YamlParseDictError(path, self.key, data, exc)

    # This parse method takes the yaml dictionaries in 'sources' keys and uses them
    # to create UnparsedSourceDefinition objects. They are then turned
    # into UnpatchedSourceDefinition objects in 'add_source_definitions'
    # or SourcePatch objects in 'add_source_patch'
    def parse(self) -> ParseResult:
        # get a verified list of dicts for the key handled by this parser
        for data in self.get_key_dicts():
            data = self.project.credentials.translate_aliases(data, recurse=True)

            is_override = "overrides" in data
            if is_override:
                data["path"] = self.yaml.path.original_file_path
                patch = self._target_from_dict(SourcePatch, data)
                assert isinstance(self.yaml.file, SchemaSourceFile)
                source_file = self.yaml.file
                # source patches must be unique
                key = (patch.overrides, patch.name)
                if key in self.manifest.source_patches:
                    raise DuplicateSourcePatchNameError(patch, self.manifest.source_patches[key])
                self.manifest.source_patches[key] = patch
                source_file.source_patches.append(key)
            else:
                source = self._target_from_dict(UnparsedSourceDefinition, data)
                # Store unrendered_database and unrendered_schema for state:modified comparisons
                if isinstance(self.yaml.file, SchemaSourceFile):
                    source.unrendered_database = self.yaml.file.get_unrendered_database(
                        "sources", source.name
                    )
                    source.unrendered_schema = self.yaml.file.get_unrendered_schema(
                        "sources", source.name
                    )

                self.add_source_definitions(source)
        return ParseResult()

    def add_source_definitions(self, source: UnparsedSourceDefinition) -> None:
        package_name = self.project.project_name
        original_file_path = self.yaml.path.original_file_path
        fqn_path = self.yaml.path.relative_path
        for table in source.tables:
            unique_id = ".".join([NodeType.Source, package_name, source.name, table.name])

            # the FQN is project name / path elements /source_name /table_name
            fqn = self.schema_parser.get_fqn_prefix(fqn_path)
            fqn.extend([source.name, table.name])

            source_def = UnpatchedSourceDefinition(
                source=source,
                table=table,
                path=original_file_path,
                original_file_path=original_file_path,
                package_name=package_name,
                unique_id=unique_id,
                resource_type=NodeType.Source,
                fqn=fqn,
                name=f"{source.name}_{table.name}",
            )
            assert isinstance(self.yaml.file, SchemaSourceFile)
            source_file: SchemaSourceFile = self.yaml.file
            self.manifest.add_source(source_file, source_def)


# This class has two subclasses: NodePatchParser and MacroPatchParser
class PatchParser(YamlReader, Generic[NonSourceTarget, Parsed]):
    @abstractmethod
    def _target_type(self) -> Type[NonSourceTarget]:
        raise NotImplementedError("_target_type not implemented")

    @abstractmethod
    def get_block(self, node: NonSourceTarget) -> TargetBlock:
        raise NotImplementedError("get_block is abstract")

    @abstractmethod
    def parse_patch(self, block: TargetBlock[NonSourceTarget], refs: ParserRef) -> None:
        raise NotImplementedError("parse_patch is abstract")

    def parse(self) -> ParseResult:
        node: NonSourceTarget
        # This will always be empty if the node a macro or analysis
        test_blocks: List[TestBlock] = []
        # This will always be empty if the node is _not_ a model
        versioned_test_blocks: List[VersionedTestBlock] = []

        # get list of 'node' objects
        # UnparsedNodeUpdate (TestablePatchParser, models, seeds, snapshots)
        #      = HasColumnTests, HasTests
        # UnparsedAnalysisUpdate (UnparsedAnalysisParser, analyses)
        #      = HasColumnDocs, HasDocs
        # UnparsedMacroUpdate (MacroPatchParser, 'macros')
        #      = HasDocs
        # correspond to this parser's 'key'
        for node in self.get_unparsed_target():
            # node_block is a TargetBlock (Macro or Analysis)
            # or a TestBlock (all of the others)
            node_block = self.get_block(node)
            if isinstance(node_block, TestBlock):
                # TestablePatchParser = seeds, snapshots
                test_blocks.append(node_block)
            if isinstance(node_block, VersionedTestBlock):
                # models
                versioned_test_blocks.append(node_block)
            if isinstance(node, (HasColumnDocs, HasColumnTests)):
                # UnparsedNodeUpdate and UnparsedAnalysisUpdate
                refs: ParserRef = ParserRef.from_target(node)
            else:
                refs = ParserRef()

            # There's no unique_id on the node yet so cannot add to disabled dict
            self.parse_patch(node_block, refs)

        return ParseResult(test_blocks, versioned_test_blocks)

    def get_unparsed_target(self) -> Iterable[NonSourceTarget]:
        path = self.yaml.path.original_file_path

        # get verified list of dicts for the 'key' that this
        # parser handles
        key_dicts = self.get_key_dicts()
        for data in key_dicts:
            # add extra data to each dict. This updates the dicts
            # in the parser yaml
            data.update(
                {
                    "original_file_path": path,
                    "yaml_key": self.key,
                    "package_name": self.project.project_name,
                }
            )
            try:
                # target_type: UnparsedNodeUpdate, UnparsedAnalysisUpdate,
                # or UnparsedMacroUpdate, UnparsedFunctionUpdate
                self._target_type().validate(data)
                if self.key != "macros":
                    # macros don't have the 'config' key support yet
                    self.normalize_meta_attribute(data, path)
                    self.normalize_docs_attribute(data, path)
                    self.normalize_group_attribute(data, path)
                    self.normalize_contract_attribute(data, path)
                    self.normalize_access_attribute(data, path)
                # `tests` has been deprecated, convert to `data_tests` here if present
                self.validate_data_tests(data)
                node = self._target_type().from_dict(data)
            except (ValidationError, JSONValidationError) as exc:
                raise YamlParseDictError(path, self.key, data, exc)
            else:
                yield node

    # We want to raise an error if some attributes are in two places, and move them
    # from toplevel to config if necessary
    def normalize_attribute(self, data, path, attribute) -> None:
        if attribute in data:
            if "config" in data and attribute in data["config"]:
                raise ParsingError(
                    f"""
                    In {path}: found {attribute} dictionary in 'config' dictionary and as top-level key.
                    Remove the top-level key and define it under 'config' dictionary only.
                """.strip()
                )
            else:
                if "config" not in data:
                    data["config"] = {}
                data["config"][attribute] = data.pop(attribute)

    def normalize_meta_attribute(self, data, path) -> None:
        return self.normalize_attribute(data, path, "meta")

    def normalize_docs_attribute(self, data, path) -> None:
        return self.normalize_attribute(data, path, "docs")

    def normalize_group_attribute(self, data, path) -> None:
        return self.normalize_attribute(data, path, "group")

    def normalize_contract_attribute(self, data, path) -> None:
        return self.normalize_attribute(data, path, "contract")

    def normalize_access_attribute(self, data, path) -> None:
        return self.normalize_attribute(data, path, "access")

    @property
    def is_root_project(self) -> bool:
        if self.root_project.project_name == self.project.project_name:
            return True
        return False

    def validate_data_tests(self, data) -> None:
        # Rename 'tests' -> 'data_tests' at both model-level and column-level
        # Raise a validation error if the user has defined both names
        def validate_and_rename(data, is_root_project: bool) -> None:
            if data.get("tests"):
                if "tests" in data and "data_tests" in data:
                    raise ValidationError(
                        "Invalid test config: cannot have both 'tests' and 'data_tests' defined"
                    )
                data["data_tests"] = data.pop("tests")

        # model-level tests
        validate_and_rename(data, self.is_root_project)

        # column-level tests
        if data.get("columns"):
            for column in data["columns"]:
                validate_and_rename(column, self.is_root_project)

        # versioned models
        if data.get("versions"):
            for version in data["versions"]:
                validate_and_rename(version, self.is_root_project)
                if version.get("columns"):
                    for column in version["columns"]:
                        validate_and_rename(column, self.is_root_project)

    def patch_node_config(self, node, patch) -> None:
        if "access" in patch.config:
            if AccessType.is_valid(patch.config["access"]):
                patch.config["access"] = AccessType(patch.config["access"])
            else:
                raise InvalidAccessTypeError(
                    unique_id=node.unique_id,
                    field_value=patch.config["access"],
                )
        # Get the ContextConfig that's used in calculating the config
        # This must match the model resource_type that's being patched
        config = ContextConfig(
            self.schema_parser.root_project,
            node.fqn,
            node.resource_type,
            self.schema_parser.project.project_name,
        )
        # We need to re-apply the config_call_dict after the patch config
        config._config_call_dict = node.config_call_dict
        config._unrendered_config_call_dict = node.unrendered_config_call_dict
        self.schema_parser.update_parsed_node_config(
            node,
            config,
            patch_config_dict=patch.config,
            patch_file_id=patch.file_id,
        )


# Subclasses of NodePatchParser: TestablePatchParser, ModelPatchParser, AnalysisPatchParser, FunctionPatchParser
# so models, seeds, snapshots, analyses, functions
class NodePatchParser(PatchParser[NodeTarget, ParsedNodePatch], Generic[NodeTarget]):
    def _get_node_patch(self, block: TargetBlock[NodeTarget], refs: ParserRef) -> ParsedNodePatch:
        # We're not passing the ParsedNodePatch around anymore, so we
        # could possibly skip creating one. Leaving here for now for
        # code consistency.
        deprecation_date: Optional[datetime.datetime] = None
        time_spine: Optional[TimeSpine] = None
        semantic_model = None
        metrics = None
        derived_semantics = None
        agg_time_dimension = None
        primary_entity = None

        if isinstance(block.target, UnparsedModelUpdate):
            deprecation_date = block.target.deprecation_date
            time_spine = (
                TimeSpine(
                    standard_granularity_column=block.target.time_spine.standard_granularity_column,
                    custom_granularities=[
                        CustomGranularity(
                            name=custom_granularity.name,
                            column_name=custom_granularity.column_name,
                        )
                        for custom_granularity in block.target.time_spine.custom_granularities
                    ],
                )
                if block.target.time_spine
                else None
            )
            semantic_model = block.target.semantic_model
            metrics = block.target.metrics
            derived_semantics = block.target.derived_semantics
            agg_time_dimension = block.target.agg_time_dimension
            primary_entity = block.target.primary_entity
        return ParsedNodePatch(
            name=block.target.name,
            original_file_path=block.target.original_file_path,
            yaml_key=block.target.yaml_key,
            package_name=block.target.package_name,
            description=block.target.description,
            columns=refs.column_info,
            meta=block.target.meta,
            docs=block.target.docs,
            config=block.target.config,
            access=block.target.access,
            version=None,
            latest_version=None,
            constraints=block.target.constraints,
            deprecation_date=deprecation_date,
            time_spine=time_spine,
            semantic_model=semantic_model,
            metrics=metrics,
            derived_semantics=derived_semantics,
            agg_time_dimension=agg_time_dimension,
            primary_entity=primary_entity,
        )

    def parse_patch(self, block: TargetBlock[NodeTarget], refs: ParserRef) -> None:
        patch = self._get_node_patch(block, refs)

        assert isinstance(self.yaml.file, SchemaSourceFile)
        source_file: SchemaSourceFile = self.yaml.file

        # TODO: I'd like to refactor this out but the early return makes doing so a bit messy
        if patch.yaml_key in ["models", "seeds", "snapshots"]:
            unique_id = self.manifest.ref_lookup.get_unique_id(
                patch.name, self.project.project_name, None
            ) or self.manifest.ref_lookup.get_unique_id(patch.name, None, None)

            if unique_id:
                resource_type = NodeType(unique_id.split(".")[0])
                if resource_type.pluralize() != patch.yaml_key:
                    fire_or_defer_event(
                        WrongResourceSchemaFile(
                            patch_name=patch.name,
                            resource_type=resource_type,
                            plural_resource_type=resource_type.pluralize(),
                            yaml_key=patch.yaml_key,
                            file_path=patch.original_file_path,
                        ),
                        force_warn_or_error_handling=True,
                        event_group_type=EventGroupType.PARSE,
                    )
                    return
        elif patch.yaml_key == "functions":
            unique_id = self.manifest.function_lookup.get_unique_id(patch.name, None)
        elif patch.yaml_key == "analyses":
            unique_id = self.manifest.analysis_lookup.get_unique_id(patch.name, None, None)
        else:
            raise DbtInternalError(
                f"Unexpected yaml_key {patch.yaml_key} for patch in "
                f"file {source_file.path.original_file_path}"
            )
        # handle disabled nodes
        if unique_id is None:
            # Node might be disabled. Following call returns list of matching disabled nodes
            resource_type = schema_file_keys_to_resource_types[patch.yaml_key]
            found_nodes = self.manifest.disabled_lookup.find(
                patch.name, patch.package_name, resource_types=[resource_type]
            )
            if found_nodes:
                if len(found_nodes) > 1 and patch.config.get("enabled"):
                    # There are multiple disabled nodes for this model and the schema file wants to enable one.
                    # We have no way to know which one to enable.
                    resource_type = found_nodes[0].unique_id.split(".")[0]
                    msg = (
                        f"Found {len(found_nodes)} matching disabled nodes for "
                        f"{resource_type} '{patch.name}'. Multiple nodes for the same "
                        "unique id cannot be enabled in the schema file. They must be enabled "
                        "in `dbt_project.yml` or in the sql files."
                    )
                    raise ParsingError(msg)

                # all nodes in the disabled dict have the same unique_id so just grab the first one
                # to append with the unique id
                source_file.append_patch(patch.yaml_key, found_nodes[0].unique_id)
                for node in found_nodes:
                    node.patch_path = source_file.file_id
                    # re-calculate the node config with the patch config.  Always do this
                    # for the case when no config is set to ensure the default of true gets captured
                    if patch.config:
                        self.patch_node_config(node, patch)

                    self.patch_node_properties(node, patch)
            else:
                fire_or_defer_event(
                    NoNodeForYamlKey(
                        patch_name=patch.name,
                        yaml_key=patch.yaml_key,
                        file_path=source_file.path.original_file_path,
                    ),
                    force_warn_or_error_handling=True,
                    event_group_type=EventGroupType.PARSE,
                )
                return  # we only return early if no disabled early nodes are found. Why don't we return after patching the disabled nodes?

        if patch.yaml_key == "functions":
            node = self.manifest.functions.get(unique_id)  # type: ignore[arg-type]
        else:
            node = self.manifest.nodes.get(unique_id)  # type: ignore[arg-type]

        if node:
            # patches can't be overwritten
            if node.patch_path:
                package_name, existing_file_path = node.patch_path.split("://")
                raise DuplicatePatchPathError(patch, existing_file_path)

            source_file.append_patch(patch.yaml_key, node.unique_id)
            # re-calculate the node config with the patch config.  Always do this
            # for the case when no config is set to ensure the default of true gets captured
            if patch.config:
                self.patch_node_config(node, patch)

            self.patch_node_properties(node, patch)

    def patch_node_properties(self, node, patch: "ParsedNodePatch") -> None:
        """Given a ParsedNodePatch, add the new information to the node."""
        # explicitly pick out the parts to update so we don't inadvertently
        # step on the model name or anything
        # Note: config should already be updated
        node.patch_path = patch.file_id
        # update created_at so process_docs will run in partial parsing
        node.created_at = time.time()
        node.description = patch.description
        node.columns = patch.columns
        node.name = patch.name

        if not isinstance(node, ModelNode):
            for attr in ["latest_version", "access", "version", "constraints"]:
                if getattr(patch, attr):
                    fire_or_defer_event(
                        ValidationWarning(
                            field_name=attr,
                            resource_type=node.resource_type.value,
                            node_name=patch.name,
                        ),
                        force_warn_or_error_handling=True,
                        event_group_type=EventGroupType.PARSE,
                    )


# TestablePatchParser = seeds, snapshots
class TestablePatchParser(NodePatchParser[UnparsedNodeUpdate]):
    __test__ = False

    def get_block(self, node: UnparsedNodeUpdate) -> TestBlock:
        return TestBlock.from_yaml_block(self.yaml, node)

    def _target_type(self) -> Type[UnparsedNodeUpdate]:
        return UnparsedNodeUpdate


class ModelPatchParser(NodePatchParser[UnparsedModelUpdate]):
    def get_block(self, node: UnparsedModelUpdate) -> VersionedTestBlock:
        return VersionedTestBlock.from_yaml_block(self.yaml, node)

    def parse_patch(self, block: TargetBlock[UnparsedModelUpdate], refs: ParserRef) -> None:
        target = block.target
        if NodeType.Model.pluralize() != target.yaml_key:
            fire_or_defer_event(
                WrongResourceSchemaFile(
                    patch_name=target.name,
                    resource_type=NodeType.Model,
                    plural_resource_type=NodeType.Model.pluralize(),
                    yaml_key=target.yaml_key,
                    file_path=target.original_file_path,
                ),
                force_warn_or_error_handling=True,
                event_group_type=EventGroupType.PARSE,
            )
            return

        versions = target.versions
        if not versions:
            super().parse_patch(block, refs)
        else:
            assert isinstance(self.yaml.file, SchemaSourceFile)
            source_file: SchemaSourceFile = self.yaml.file
            latest_version = (
                target.latest_version if target.latest_version is not None else max(versions).v
            )
            for unparsed_version in versions:
                versioned_model_name = (
                    unparsed_version.defined_in or f"{block.name}_{unparsed_version.formatted_v}"
                )
                # ref lookup without version - version is not set yet
                versioned_model_unique_id = self.manifest.ref_lookup.get_unique_id(
                    versioned_model_name, target.package_name, None
                )

                versioned_model_node: Optional[ModelNode] = None
                add_node_nofile_fn: Callable

                # If this is the latest version, it's allowed to define itself in a model file name that doesn't have a suffix
                if versioned_model_unique_id is None and unparsed_version.v == latest_version:
                    versioned_model_unique_id = self.manifest.ref_lookup.get_unique_id(
                        block.name, target.package_name, None
                    )

                if versioned_model_unique_id is None:
                    # Node might be disabled. Following call returns list of matching disabled nodes
                    found_nodes = self.manifest.disabled_lookup.find(
                        versioned_model_name, None, resource_types=[NodeType.Model]
                    )
                    if found_nodes:
                        if len(found_nodes) > 1 and target.config.get("enabled"):
                            # There are multiple disabled nodes for this model and the schema file wants to enable one.
                            # We have no way to know which one to enable.
                            resource_type = found_nodes[0].unique_id.split(".")[0]
                            msg = (
                                f"Found {len(found_nodes)} matching disabled nodes for "
                                f"{resource_type} '{target.name}'. Multiple nodes for the same "
                                "unique id cannot be enabled in the schema file. They must be enabled "
                                "in `dbt_project.yml` or in the sql files."
                            )
                            raise ParsingError(msg)
                        # We know that there's only one node in the disabled list because
                        # otherwise we would have raised the error above
                        found_node = found_nodes[0]
                        self.manifest.disabled.pop(found_node.unique_id)
                        assert isinstance(found_node, ModelNode)
                        versioned_model_node = found_node
                        add_node_nofile_fn = self.manifest.add_disabled_nofile
                else:
                    found_node = self.manifest.nodes.pop(versioned_model_unique_id)
                    assert isinstance(found_node, ModelNode)
                    versioned_model_node = found_node
                    add_node_nofile_fn = self.manifest.add_node_nofile

                if versioned_model_node is None:
                    fire_or_defer_event(
                        NoNodeForYamlKey(
                            patch_name=versioned_model_name,
                            yaml_key=target.yaml_key,
                            file_path=source_file.path.original_file_path,
                        ),
                        force_warn_or_error_handling=True,
                        event_group_type=EventGroupType.PARSE,
                    )
                    continue

                # update versioned node unique_id
                versioned_model_node_unique_id_old = versioned_model_node.unique_id
                versioned_model_node.unique_id = (
                    f"model.{target.package_name}.{target.name}.{unparsed_version.formatted_v}"
                )
                # update source file.nodes with new unique_id
                model_source_file = self.manifest.files[versioned_model_node.file_id]
                assert isinstance(model_source_file, SourceFile)
                # because of incomplete test setup, check before removing
                if versioned_model_node_unique_id_old in model_source_file.nodes:
                    model_source_file.nodes.remove(versioned_model_node_unique_id_old)
                model_source_file.nodes.append(versioned_model_node.unique_id)

                # update versioned node fqn
                versioned_model_node.fqn[-1] = target.name
                versioned_model_node.fqn.append(unparsed_version.formatted_v)

                # add versioned node back to nodes/disabled
                add_node_nofile_fn(versioned_model_node)

                # flatten columns based on include/exclude
                version_refs: ParserRef = ParserRef.from_versioned_target(
                    block.target, unparsed_version.v
                )

                versioned_model_patch = ParsedNodePatch(
                    name=target.name,
                    original_file_path=target.original_file_path,
                    yaml_key=target.yaml_key,
                    package_name=target.package_name,
                    description=unparsed_version.description or target.description,
                    columns=version_refs.column_info,
                    meta=target.meta,
                    docs=unparsed_version.docs or target.docs,
                    config=deep_merge(target.config, unparsed_version.config),
                    access=unparsed_version.access or target.access,
                    version=unparsed_version.v,
                    latest_version=latest_version,
                    constraints=unparsed_version.constraints or target.constraints,
                    deprecation_date=unparsed_version.deprecation_date,
                )
                # Node patched before config because config patching depends on model name,
                # which may have been updated in the version patch
                # versioned_model_node.patch(versioned_model_patch)
                self.patch_node_properties(versioned_model_node, versioned_model_patch)

                # Includes alias recomputation
                self.patch_node_config(versioned_model_node, versioned_model_patch)

                # Need to reapply setting constraints and contract checksum here, because
                # they depend on node.contract.enabled, which wouldn't be set when
                # patch_node_properties was called if it wasn't set in the model file.
                self.patch_constraints(versioned_model_node, versioned_model_patch.constraints)
                versioned_model_node.build_contract_checksum()
                source_file.append_patch(
                    versioned_model_patch.yaml_key, versioned_model_node.unique_id
                )
            self.manifest.rebuild_ref_lookup()
            self.manifest.rebuild_disabled_lookup()

    def _target_type(self) -> Type[UnparsedModelUpdate]:
        return UnparsedModelUpdate

    def patch_node_properties(self, node, patch: "ParsedNodePatch") -> None:
        super().patch_node_properties(node, patch)

        # Remaining patch properties are only relevant to ModelNode objects
        if not isinstance(node, ModelNode):
            return

        node.version = patch.version
        node.latest_version = patch.latest_version
        node.deprecation_date = patch.deprecation_date
        if patch.access:
            if AccessType.is_valid(patch.access):
                node.access = AccessType(patch.access)
            else:
                raise InvalidAccessTypeError(
                    unique_id=node.unique_id,
                    field_value=patch.access,
                )
        # breaking out False and None here is wordy but extra clear
        semantic_model_enabled = patch.semantic_model is True or (
            patch.semantic_model is not False
            and patch.semantic_model is not None
            and patch.semantic_model.enabled is not False
        )
        if semantic_model_enabled:
            from dbt.parser.schema_yaml_readers import SemanticModelParser

            semantic_model_parser = SemanticModelParser(self.schema_parser, self.yaml)
            semantic_model_parser.parse_v2_semantic_model_from_dbt_model_patch(
                node=node,
                patch=patch,
            )

            from dbt.parser.schema_yaml_readers import MetricParser

            MetricParser(self.schema_parser, self.yaml).parse_v2_metrics_from_dbt_model_patch(
                patch
            )

        # These two will have to be reapplied after config is built for versioned models
        self.patch_constraints(node, patch.constraints)
        self.patch_time_spine(node, patch.time_spine)
        node.build_contract_checksum()

    def patch_constraints(self, node: ModelNode, constraints: List[Dict[str, Any]]) -> None:
        contract_config = node.config.get("contract")
        if contract_config.enforced is True:
            self._validate_constraint_prerequisites(node)

            if any(
                c for c in constraints if "type" not in c or not ConstraintType.is_valid(c["type"])
            ):
                raise ParsingError(
                    f"Invalid constraint type on model {node.name}: "
                    f"Type must be one of {[ct.value for ct in ConstraintType]}"
                )

        self._validate_pk_constraints(node, constraints)
        node.constraints = [ModelLevelConstraint.from_dict(c) for c in constraints]
        self._process_constraints_refs_and_sources(node)

    def _process_constraints_refs_and_sources(self, model_node: ModelNode) -> None:
        """
        Populate model_node.refs and model_node.sources based on foreign-key constraint references,
        whether defined at the model-level or column-level.
        """
        for constraint in model_node.all_constraints:
            if constraint.type == ConstraintType.foreign_key and constraint.to:
                try:
                    ref_or_source = statically_parse_ref_or_source(constraint.to)
                except ParsingError:
                    raise ParsingError(
                        f"Invalid 'ref' or 'source' syntax on foreign key constraint 'to' on model {model_node.name}: {constraint.to}."
                    )

                if isinstance(ref_or_source, RefArgs):
                    model_node.refs.append(ref_or_source)
                else:
                    model_node.sources.append(ref_or_source)

    def patch_time_spine(self, node: ModelNode, time_spine: Optional[TimeSpine]) -> None:
        node.time_spine = time_spine

    def _validate_pk_constraints(
        self, model_node: ModelNode, constraints: List[Dict[str, Any]]
    ) -> None:
        errors = []
        # check for primary key constraints defined at the column level
        pk_col: List[str] = []
        for col in model_node.columns.values():
            for constraint in col.constraints:
                if constraint.type == ConstraintType.primary_key:
                    pk_col.append(col.name)

        if len(pk_col) > 1:
            errors.append(
                f"Found {len(pk_col)} columns ({pk_col}) with primary key constraints defined. "
                "Primary keys for multiple columns must be defined as a model level constraint."
            )

        if len(pk_col) > 0 and (
            any(
                constraint.type == ConstraintType.primary_key
                for constraint in model_node.constraints
            )
            or any(constraint["type"] == ConstraintType.primary_key for constraint in constraints)
        ):
            errors.append(
                "Primary key constraints defined at the model level and the columns level. "
                "Primary keys can be defined at the model level or the column level, not both."
            )

        if errors:
            raise ParsingError(
                f"Primary key constraint error: ({model_node.original_file_path})\n"
                + "\n".join(errors)
            )

    def _validate_constraint_prerequisites(self, model_node: ModelNode) -> None:
        column_warn_unsupported = [
            constraint.warn_unsupported
            for column in model_node.columns.values()
            for constraint in column.constraints
        ]
        model_warn_unsupported = [
            constraint.warn_unsupported for constraint in model_node.constraints
        ]
        warn_unsupported = column_warn_unsupported + model_warn_unsupported

        # if any constraint has `warn_unsupported` as True then send the warning
        if any(warn_unsupported) and not model_node.materialization_enforces_constraints:
            fire_or_defer_event(
                UnsupportedConstraintMaterialization(materialized=model_node.config.materialized),
                node=model_node,
                force_warn_or_error_handling=True,
                event_group_type=EventGroupType.PARSE,
            )

        errors = []
        if not model_node.columns:
            errors.append(
                "Constraints must be defined in a `yml` schema configuration file like `schema.yml`."
            )

        if str(model_node.language) != "sql":
            errors.append(f"Language Error: Expected 'sql' but found '{model_node.language}'")

        if errors:
            raise ParsingError(
                f"Contract enforcement failed for: ({model_node.original_file_path})\n"
                + "\n".join(errors)
            )


class AnalysisPatchParser(NodePatchParser[UnparsedAnalysisUpdate]):
    def get_block(self, node: UnparsedAnalysisUpdate) -> TargetBlock:
        return TargetBlock.from_yaml_block(self.yaml, node)

    def _target_type(self) -> Type[UnparsedAnalysisUpdate]:
        return UnparsedAnalysisUpdate


class SingularTestPatchParser(PatchParser[UnparsedSingularTestUpdate, ParsedSingularTestPatch]):
    def get_block(self, node: UnparsedSingularTestUpdate) -> TargetBlock:
        return TargetBlock.from_yaml_block(self.yaml, node)

    def _target_type(self) -> Type[UnparsedSingularTestUpdate]:
        return UnparsedSingularTestUpdate

    def parse_patch(self, block: TargetBlock[UnparsedSingularTestUpdate], refs: ParserRef) -> None:
        patch = ParsedSingularTestPatch(
            name=block.target.name,
            description=block.target.description,
            meta=block.target.meta,
            docs=block.target.docs,
            config=block.target.config,
            original_file_path=block.target.original_file_path,
            yaml_key=block.target.yaml_key,
            package_name=block.target.package_name,
        )

        assert isinstance(self.yaml.file, SchemaSourceFile)
        source_file: SchemaSourceFile = self.yaml.file

        unique_id = self.manifest.singular_test_lookup.get_unique_id(
            block.name, block.target.package_name
        )
        if not unique_id:
            fire_or_defer_event(
                NoNodeForYamlKey(
                    patch_name=patch.name,
                    yaml_key=patch.yaml_key,
                    file_path=source_file.path.original_file_path,
                ),
                force_warn_or_error_handling=True,
                event_group_type=EventGroupType.PARSE,
            )
            return

        node = self.manifest.nodes.get(unique_id)
        assert node is not None

        source_file.append_patch(patch.yaml_key, unique_id)
        if patch.config:
            self.patch_node_config(node, patch)

        node.patch_path = patch.file_id
        node.description = patch.description
        node.created_at = time.time()


class FunctionPatchParser(NodePatchParser[UnparsedFunctionUpdate]):
    def get_block(self, node: UnparsedFunctionUpdate) -> TargetBlock:
        return TargetBlock.from_yaml_block(self.yaml, node)

    def _target_type(self) -> Type[UnparsedFunctionUpdate]:
        return UnparsedFunctionUpdate

    def patch_node_properties(self, node, patch: "ParsedNodePatch") -> None:
        super().patch_node_properties(node, patch)

        assert isinstance(patch, ParsedFunctionPatch)
        assert isinstance(node, FunctionNode)

        node.arguments = patch.arguments
        node.returns = patch.returns

        # Process overloads: absorb overload SQL files into the root node
        if patch.overloads:
            self._absorb_overloads(node, patch.overloads)

    def _absorb_overloads(self, root_node: "FunctionNode", overloads: list) -> None:
        """Look up overload SQL files, store their code on the root node,
        and remove them from the manifest so they don't appear as separate
        DAG nodes."""
        from dbt.artifacts.resources import FunctionOverload

        absorbed: list[FunctionOverload] = []
        for overload in overloads:
            overload_name = overload.defined_in

            # Prevent self-reference
            if overload_name == root_node.name:
                raise ParsingError(
                    f"Function '{root_node.name}' cannot list itself in overloads "
                    f"(defined_in: '{overload_name}')."
                )

            # Find the overload node in the manifest by name
            overload_unique_id = f"function.{root_node.package_name}.{overload_name}"
            overload_node = self.manifest.functions.get(overload_unique_id)

            if overload_node is None:
                # Check if this overload was already claimed by another root function
                claimed_by = next(
                    (
                        root_uid
                        for fid, root_uid in self.manifest.function_overload_owners.items()
                        if fid.endswith(f"/{overload_name}.sql")
                    ),
                    None,
                )
                if claimed_by:
                    raise ParsingError(
                        f"Function overload '{overload_name}' (defined_in) is already "
                        f"used by '{claimed_by}'. Each overload file can only belong "
                        f"to one root function."
                    )
                raise ParsingError(
                    f"Function overload '{overload_name}' (defined_in) not found. "
                    f"Expected a SQL file at functions/{overload_name}.sql"
                )

            absorbed.append(
                FunctionOverload(
                    defined_in=overload.defined_in,
                    arguments=overload.arguments,
                    returns=overload.returns,
                    description=overload.description,
                    raw_body=overload_node.raw_code,
                )
            )

            self._merge_overload_dependencies(root_node, overload_node)

            # Track the overload→root relationship for partial parsing
            self.manifest.function_overload_owners[overload_node.file_id] = root_node.unique_id

            # Clear the source file's function list so partial parsing knows
            # this file no longer owns a standalone function node.
            overload_file = self.manifest.files.get(overload_node.file_id)
            if overload_file is not None and hasattr(overload_file, "functions"):
                overload_file.functions = []

            # Remove the overload node from the manifest — it's now part of the root
            del self.manifest.functions[overload_unique_id]

        root_node.overloads = absorbed

    @staticmethod
    def _merge_overload_dependencies(
        root_node: "FunctionNode", overload_node: "FunctionNode"
    ) -> None:
        """Carry an overload body's ref()/source()/function() dependencies onto
        the root.

        An overload SQL body can reference other nodes, but the overload node is
        deleted during absorption, so those edges would be silently lost — the
        root's own body never mentions them. process_refs/process_sources/
        process_functions run later over the root and resolve these into
        depends_on. Dedup against the root's own dependencies so a shared
        reference doesn't get duplicated.
        """
        for ref in overload_node.refs:
            if ref not in root_node.refs:
                root_node.refs.append(ref)
        for source in overload_node.sources:
            if source not in root_node.sources:
                root_node.sources.append(source)
        for function in overload_node.functions:
            if function not in root_node.functions:
                root_node.functions.append(function)

    def _get_node_patch(self, block: TargetBlock[NodeTarget], refs: ParserRef) -> ParsedNodePatch:
        target = block.target
        assert isinstance(target, UnparsedFunctionUpdate)

        # Convert unparsed overloads to FunctionOverload objects (without body yet)
        from dbt.artifacts.resources import FunctionOverload

        overloads = [
            FunctionOverload(
                defined_in=o.defined_in,
                arguments=o.arguments,
                returns=o.returns,
                description=o.description,
            )
            for o in target.overloads
        ]

        return ParsedFunctionPatch(
            name=target.name,
            original_file_path=target.original_file_path,
            yaml_key=target.yaml_key,
            package_name=target.package_name,
            description=target.description,
            columns=refs.column_info,
            meta=target.meta,
            docs=target.docs,
            config=target.config,
            access=target.access,
            version=None,
            latest_version=None,
            constraints=target.constraints,
            deprecation_date=None,
            time_spine=None,
            arguments=target.arguments,
            returns=target.returns,
            overloads=overloads,
        )


class MacroPatchParser(PatchParser[UnparsedMacroUpdate, ParsedMacroPatch]):
    def get_block(self, node: UnparsedMacroUpdate) -> TargetBlock:
        return TargetBlock.from_yaml_block(self.yaml, node)

    def _target_type(self) -> Type[UnparsedMacroUpdate]:
        return UnparsedMacroUpdate

    def parse_patch(self, block: TargetBlock[UnparsedMacroUpdate], refs: ParserRef) -> None:
        patch = ParsedMacroPatch(
            name=block.target.name,
            original_file_path=block.target.original_file_path,
            yaml_key=block.target.yaml_key,
            package_name=block.target.package_name,
            arguments=block.target.arguments,
            description=block.target.description,
            meta=block.target.meta,
            docs=block.target.docs,
            config=block.target.config,
        )
        assert isinstance(self.yaml.file, SchemaSourceFile)
        source_file = self.yaml.file
        # macros are fully namespaced
        unique_id = f"macro.{patch.package_name}.{patch.name}"
        macro = self.manifest.macros.get(unique_id)
        if not macro:
            fire_or_defer_event(
                MacroNotFoundForPatch(patch_name=patch.name),
                force_warn_or_error_handling=True,
                event_group_type=EventGroupType.PARSE,
            )
            return
        if macro.patch_path:
            package_name, existing_file_path = macro.patch_path.split("://")
            raise DuplicateMacroPatchNameError(patch, existing_file_path)
        source_file.macro_patches[patch.name] = unique_id

        # former macro.patch code
        macro.patch_path = patch.file_id
        macro.description = patch.description
        macro.created_at = time.time()

        meta = {**(patch.meta or {}), **(patch.config.get("meta") or {})}
        docs = patch.config.get("docs") or patch.docs

        # config inherits from HasConfig which is a dict so we need to cast it to Docs
        if isinstance(docs, dict):
            docs = Docs(**docs)

        macro.meta = meta
        macro.docs = docs
        macro.config.meta = meta
        macro.config.docs = docs

        if getattr(get_flags(), "validate_macro_args", True):
            self._check_patch_arguments(macro, patch)
            macro.arguments = patch.arguments if patch.arguments else macro.arguments
        else:
            macro.arguments = patch.arguments

    def _check_patch_arguments(self, macro: Macro, patch: ParsedMacroPatch) -> None:
        if not patch.arguments:
            return

        for macro_arg, patch_arg in zip(macro.arguments, patch.arguments):
            if patch_arg.name != macro_arg.name:
                msg = f"Argument {patch_arg.name} in yaml for macro {macro.name} does not match the jinja definition."
                self._fire_macro_arg_warning(msg, macro)

        if len(patch.arguments) != len(macro.arguments):
            msg = f"The number of arguments in the yaml for macro {macro.name} does not match the jinja definition."
            self._fire_macro_arg_warning(msg, macro)

        for patch_arg in patch.arguments:
            arg_type = patch_arg.type
            if arg_type is not None and arg_type.strip() != "" and not is_valid_type(arg_type):
                msg = f"Argument {patch_arg.name} in the yaml for macro {macro.name} has an invalid type."
                self._fire_macro_arg_warning(msg, macro)

    def _fire_macro_arg_warning(self, msg: str, macro: Macro) -> None:
        fire_or_defer_event(
            InvalidMacroAnnotation(
                msg=msg, macro_unique_id=macro.unique_id, macro_file_path=macro.original_file_path
            ),
            force_warn_or_error_handling=True,
            event_group_type=EventGroupType.PARSE,
        )


# valid type names, along with the number of parameters they require
macro_types: Dict[str, int] = {
    "str": 0,
    "string": 0,
    "bool": 0,
    "int": 0,
    "integer": 0,
    "float": 0,
    "any": 0,
    "list": 1,
    "dict": 2,
    "optional": 1,
    "relation": 0,
    "column": 0,
}


def is_valid_type(buffer: str) -> bool:
    buffer = buffer.replace(" ", "").replace("\t", "")
    type_desc, remainder = match_type_desc(buffer)
    return type_desc is not None and remainder == ""


def match_type_desc(buffer: str) -> Tuple[Optional[str], str]:
    """A matching buffer is a type name followed by an argument list with
    the correct number of arguments."""
    type_name, remainder = match_type_name(buffer)
    if type_name is None:
        return None, buffer
    attr_list, remainder = match_arg_list(remainder, macro_types[type_name])
    if attr_list is None:
        return None, buffer
    return type_name + attr_list, remainder


alpha_pattern = re.compile(r"[a-z]+")


def match_type_name(buffer: str) -> Tuple[Optional[str], str]:
    """A matching buffer starts with one of the valid type names from macro_types"""
    match = alpha_pattern.match(buffer)
    if match is not None and buffer[: match.end(0)] in macro_types:
        return buffer[: match.end(0)], buffer[match.end(0) :]
    else:
        return None, buffer


def match_arg_list(buffer: str, arg_count: int) -> Tuple[Optional[str], str]:
    """A matching buffer must begin with '[', followed by exactly arg_count type
    specs, followed by ']'"""

    if arg_count == 0:
        return "", buffer

    if not buffer.startswith("["):
        return None, buffer

    remainder = buffer[1:]
    for i in range(arg_count):
        type_desc, remainder = match_type_desc(remainder)
        if type_desc is None:
            return None, buffer
        if i != arg_count - 1:
            if not remainder.startswith(","):
                return None, buffer
            remainder = remainder[1:]

    if not remainder.startswith("]"):
        return None, buffer
    else:
        return "", remainder[1:]
