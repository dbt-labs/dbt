import hashlib
import os
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import (
    Any,
    Dict,
    Iterator,
    List,
    Literal,
    Optional,
    Sequence,
    Tuple,
    Type,
    Union,
    get_args,
)

from mashumaro.types import SerializableType

from dbt.adapters.base import ConstraintSupport
from dbt.adapters.factory import get_adapter_constraint_support
from dbt.artifacts.resources import Analysis as AnalysisResource
from dbt.artifacts.resources import (
    BaseResource,
    ColumnInfo,
    CompiledResource,
    DependsOn,
    Docs,
)
from dbt.artifacts.resources import Documentation as DocumentationResource
from dbt.artifacts.resources import Exposure as ExposureResource
from dbt.artifacts.resources import FileHash
from dbt.artifacts.resources import GenericTest as GenericTestResource
from dbt.artifacts.resources import GraphResource
from dbt.artifacts.resources import Group as GroupResource
from dbt.artifacts.resources import HasRelationMetadata as HasRelationMetadataResource
from dbt.artifacts.resources import HookNode as HookNodeResource
from dbt.artifacts.resources import InjectedCTE
from dbt.artifacts.resources import Macro as MacroResource
from dbt.artifacts.resources import MacroArgument
from dbt.artifacts.resources import Metric as MetricResource
from dbt.artifacts.resources import MetricInputMeasure
from dbt.artifacts.resources import Model as ModelResource
from dbt.artifacts.resources import (
    ModelConfig,
    NodeConfig,
    NodeVersion,
    ParsedResource,
    ParsedResourceMandatory,
)
from dbt.artifacts.resources import Quoting as QuotingResource
from dbt.artifacts.resources import SavedQuery as SavedQueryResource
from dbt.artifacts.resources import Seed as SeedResource
from dbt.artifacts.resources import SemanticModel as SemanticModelResource
from dbt.artifacts.resources import SingularTest as SingularTestResource
from dbt.artifacts.resources import Snapshot as SnapshotResource
from dbt.artifacts.resources import SourceDefinition as SourceDefinitionResource
from dbt.artifacts.resources import SqlOperation as SqlOperationResource
from dbt.artifacts.resources import TimeSpine
from dbt.artifacts.resources import UnitTestDefinition as UnitTestDefinitionResource
from dbt.artifacts.resources.v1.model import ModelFreshness
from dbt.artifacts.schemas.batch_results import BatchResults
from dbt.clients.jinja_static import statically_extract_has_name_this
from dbt.contracts.graph.model_config import UnitTestNodeConfig
from dbt.contracts.graph.node_args import ModelNodeArgs
from dbt.contracts.graph.unparsed import (
    HasYamlMetadata,
    TestDef,
    UnitTestOverrides,
    UnparsedColumn,
    UnparsedSourceDefinition,
    UnparsedSourceTableDefinition,
)
from dbt.events.types import (
    SeedExceedsLimitAndPathChanged,
    SeedExceedsLimitChecksumChanged,
    SeedExceedsLimitSamePath,
    SeedIncreased,
    UnversionedBreakingChange,
)
from dbt.exceptions import ContractBreakingChangeError, ParsingError, ValidationError
from dbt.flags import get_flags
from dbt.node_types import (
    REFABLE_NODE_TYPES,
    VERSIONED_NODE_TYPES,
    AccessType,
    NodeType,
)
from dbt_common.clients.system import write_file
from dbt_common.contracts.constraints import (
    ColumnLevelConstraint,
    ConstraintType,
    ModelLevelConstraint,
)
from dbt_common.dataclass_schema import dbtClassMixin
from dbt_common.events.contextvars import set_log_contextvars
from dbt_common.events.functions import warn_or_error

# =====================================================================
# This contains the classes for all of the nodes and node-like objects
# in the manifest. In the "nodes" dictionary of the manifest we find
# all of the objects in the ManifestNode union below. In addition the
# manifest contains "macros", "sources", "metrics", "exposures", "docs",
# and "disabled" dictionaries.
#
# The SeedNode is a ManifestNode, but can't be compiled because it has
# no SQL.
#
# All objects defined in this file should have BaseNode as a parent
# class.
#
# The two objects which do not show up in the DAG are Macro and
# Documentation.
# =====================================================================


# ==================================================
# Various parent classes and node attribute classes
# ==================================================


@dataclass
class BaseNode(BaseResource):
    """All nodes or node-like objects in this file should have this as a base class"""

    # In an ideal world this would be a class property. However, chaining @classmethod and
    # @property was deprecated in python 3.11 and removed in 3.13. There are more
    # complicated ways of making a class property, however a class method suits our
    # purposes well enough
    @classmethod
    def resource_class(cls) -> Type[BaseResource]:
        """Should be overriden by any class inheriting BaseNode"""
        raise NotImplementedError

    @property
    def search_name(self):
        return self.name

    @property
    def file_id(self):
        return f"{self.package_name}://{self.original_file_path}"

    @property
    def is_refable(self):
        return self.resource_type in REFABLE_NODE_TYPES

    @property
    def should_store_failures(self):
        return False

    # will this node map to an object in the database?
    @property
    def is_relational(self):
        return self.resource_type in REFABLE_NODE_TYPES

    @property
    def is_versioned(self):
        return self.resource_type in VERSIONED_NODE_TYPES and self.version is not None

    @property
    def is_ephemeral(self):
        return self.config.materialized == "ephemeral"

    @property
    def is_ephemeral_model(self):
        return self.is_refable and self.is_ephemeral

    def get_materialization(self):
        return self.config.materialized

    @classmethod
    def from_resource(cls, resource_instance: BaseResource):
        assert isinstance(resource_instance, cls.resource_class())
        return cls.from_dict(resource_instance.to_dict())

    def to_resource(self):
        return self.resource_class().from_dict(self.to_dict())


@dataclass
class GraphNode(GraphResource, BaseNode):
    """Nodes in the DAG. Macro and Documentation don't have fqn."""

    def same_fqn(self, other) -> bool:
        return self.fqn == other.fqn


@dataclass
class HasRelationMetadata(HasRelationMetadataResource):
    @classmethod
    def __pre_deserialize__(cls, data):
        data = super().__pre_deserialize__(data)
        if "database" not in data:
            data["database"] = None
        return data

    @property
    def quoting_dict(self) -> Dict[str, bool]:
        if hasattr(self, "quoting"):
            return self.quoting.to_dict(omit_none=True)
        else:
            return {}


@dataclass
class ParsedNodeMandatory(ParsedResourceMandatory, GraphNode, HasRelationMetadata):
    pass


# This needs to be in all ManifestNodes and also in SourceDefinition,
# because of "source freshness". Should not be in artifacts, because we
# don't write out _event_status.
@dataclass
class NodeInfoMixin:
    _event_status: Dict[str, Any] = field(default_factory=dict)

    @property
    def node_info(self):
        node_info = {
            "node_path": getattr(self, "path", None),
            "node_name": getattr(self, "name", None),
            "unique_id": getattr(self, "unique_id", None),
            "resource_type": str(getattr(self, "resource_type", "")),
            "materialized": self.config.get("materialized"),
            "node_status": str(self._event_status.get("node_status")),
            "node_started_at": self._event_status.get("started_at"),
            "node_finished_at": self._event_status.get("finished_at"),
            "meta": getattr(self, "meta", {}),
            "node_relation": {
                "database": getattr(self, "database", None),
                "schema": getattr(self, "schema", None),
                "alias": getattr(self, "alias", None),
                "relation_name": getattr(self, "relation_name", None),
            },
        }
        return node_info

    def update_event_status(self, **kwargs):
        for k, v in kwargs.items():
            self._event_status[k] = v
        set_log_contextvars(node_info=self.node_info)

    def clear_event_status(self):
        self._event_status = dict()


@dataclass
class ParsedNode(ParsedResource, NodeInfoMixin, ParsedNodeMandatory, SerializableType):
    def get_target_write_path(
        self, target_path: str, subdirectory: str, split_suffix: Optional[str] = None
    ):
        # This is called for both the "compiled" subdirectory of "target" and the "run" subdirectory
        if os.path.basename(self.path) == os.path.basename(self.original_file_path):
            # One-to-one relationship of nodes to files.
            path = self.original_file_path
        elif os.path.dirname(self.path) == os.path.basename(self.original_file_path):
            parent_dirname = os.path.dirname(self.original_file_path)
            dirname = os.path.dirname(self.path).replace(".", "_")
            basename = os.path.basename(self.path)
            path = os.path.join(parent_dirname, dirname, basename)
        else:
            #  Many-to-one relationship of nodes to files.
            path = os.path.join(self.original_file_path, self.path)

        if split_suffix:
            pathlib_path = Path(path)
            path = str(
                pathlib_path.parent
                / pathlib_path.stem
                / (pathlib_path.stem + f"_{split_suffix}" + pathlib_path.suffix)
            )

        target_write_path = os.path.join(target_path, subdirectory, self.package_name, path)
        return target_write_path

    def write_node(self, project_root: str, compiled_path, compiled_code: str):
        if os.path.isabs(compiled_path):
            full_path = compiled_path
        else:
            full_path = os.path.join(project_root, compiled_path)
        write_file(full_path, compiled_code)

    def _serialize(self):
        return self.to_dict()

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if "_event_status" in dct:
            del dct["_event_status"]
        return dct

    @classmethod
    def _deserialize(cls, dct: Dict[str, int]):
        # The serialized ParsedNodes do not differ from each other
        # in fields that would allow 'from_dict' to distinguis
        # between them.
        resource_type = dct["resource_type"]
        if resource_type == "model":
            return ModelNode.from_dict(dct)
        elif resource_type == "analysis":
            return AnalysisNode.from_dict(dct)
        elif resource_type == "seed":
            return SeedNode.from_dict(dct)
        elif resource_type == "sql":
            return SqlNode.from_dict(dct)
        elif resource_type == "test":
            if "test_metadata" in dct:
                return GenericTestNode.from_dict(dct)
            else:
                return SingularTestNode.from_dict(dct)
        elif resource_type == "operation":
            return HookNode.from_dict(dct)
        elif resource_type == "seed":
            return SeedNode.from_dict(dct)
        elif resource_type == "snapshot":
            return SnapshotNode.from_dict(dct)
        else:
            return cls.from_dict(dct)

    def _persist_column_docs(self) -> bool:
        if hasattr(self.config, "persist_docs"):
            assert isinstance(self.config, NodeConfig)
            return bool(self.config.persist_docs.get("columns"))
        return False

    def _persist_relation_docs(self) -> bool:
        if hasattr(self.config, "persist_docs"):
            assert isinstance(self.config, NodeConfig)
            return bool(self.config.persist_docs.get("relation"))
        return False

    def same_persisted_description(self, other) -> bool:
        # the check on configs will handle the case where we have different
        # persist settings, so we only have to care about the cases where they
        # are the same..
        if self._persist_relation_docs():
            if self.description != other.description:
                return False

        if self._persist_column_docs():
            # assert other._persist_column_docs()
            column_descriptions = {k: v.description for k, v in self.columns.items()}
            other_column_descriptions = {k: v.description for k, v in other.columns.items()}
            if column_descriptions != other_column_descriptions:
                return False

        return True

    def same_body(self, other) -> bool:
        return self.raw_code == other.raw_code

    def same_database_representation(self, other) -> bool:
        # compare the config representation, not the node's config value. This
        # compares the configured value, rather than the ultimate value (so
        # generate_*_name and unset values derived from the target are
        # ignored)
        keys = ("database", "schema", "alias")
        for key in keys:
            mine = self.unrendered_config.get(key)
            others = other.unrendered_config.get(key)
            if mine != others:
                return False
        return True

    def same_config(self, old) -> bool:
        return self.config.same_contents(
            self.unrendered_config,
            old.unrendered_config,
        )

    def build_contract_checksum(self):
        pass

    def same_contract(self, old, adapter_type=None) -> bool:
        # This would only apply to seeds
        return True

    def same_contents(self, old, adapter_type) -> bool:
        if old is None:
            return False

        # Need to ensure that same_contract is called because it
        # could throw an error
        same_contract = self.same_contract(old, adapter_type)
        return (
            self.same_body(old)
            and self.same_config(old)
            and self.same_persisted_description(old)
            and self.same_fqn(old)
            and self.same_database_representation(old)
            and same_contract
            and True
        )

    @property
    def is_external_node(self):
        return False


@dataclass
class CompiledNode(CompiledResource, ParsedNode):
    """Contains attributes necessary for SQL files and nodes with refs, sources, etc,
    so all ManifestNodes except SeedNode."""

    @property
    def empty(self):
        return not self.raw_code.strip()

    def set_cte(self, cte_id: str, sql: str):
        """This is the equivalent of what self.extra_ctes[cte_id] = sql would
        do if extra_ctes were an OrderedDict
        """
        for cte in self.extra_ctes:
            # Because it's possible that multiple threads are compiling the
            # node at the same time, we don't want to overwrite already compiled
            # sql in the extra_ctes with empty sql.
            if cte.id == cte_id:
                break
        else:
            self.extra_ctes.append(InjectedCTE(id=cte_id, sql=sql))

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def depends_on_macros(self):
        return self.depends_on.macros


# ====================================
# CompiledNode subclasses
# ====================================


@dataclass
class AnalysisNode(AnalysisResource, CompiledNode):
    @classmethod
    def resource_class(cls) -> Type[AnalysisResource]:
        return AnalysisResource


@dataclass
class HookNode(HookNodeResource, CompiledNode):
    @classmethod
    def resource_class(cls) -> Type[HookNodeResource]:
        return HookNodeResource


@dataclass
class BatchContext(dbtClassMixin):
    id: str
    event_time_start: datetime
    event_time_end: datetime

    def __post_serialize__(self, data, context):
        # This is insane, but necessary, I apologize. Mashumaro handles the
        # dictification of this class via a compile time generated `to_dict`
        # method based off of the _typing_ of th class. By default `datetime`
        # types are converted to strings. We don't want that, we want them to
        # stay datetimes.
        # Note: This is safe because the `BatchContext` isn't part of the artifact
        # and thus doesn't get written out.
        new_data = super().__post_serialize__(data, context)
        new_data["event_time_start"] = self.event_time_start
        new_data["event_time_end"] = self.event_time_end
        return new_data


@dataclass
class ModelNode(ModelResource, CompiledNode):
    previous_batch_results: Optional[BatchResults] = None
    batch: Optional[BatchContext] = None
    _has_this: Optional[bool] = None

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if "_has_this" in dct:
            del dct["_has_this"]
        if "previous_batch_results" in dct:
            del dct["previous_batch_results"]
        return dct

    @classmethod
    def resource_class(cls) -> Type[ModelResource]:
        return ModelResource

    @classmethod
    def from_args(cls, args: ModelNodeArgs) -> "ModelNode":
        unique_id = args.unique_id

        # build unrendered config -- for usage in ParsedNode.same_contents
        unrendered_config = {}
        unrendered_config["alias"] = args.identifier
        unrendered_config["schema"] = args.schema
        if args.database:
            unrendered_config["database"] = args.database

        return cls(
            resource_type=NodeType.Model,
            name=args.name,
            package_name=args.package_name,
            unique_id=unique_id,
            fqn=args.fqn,
            version=args.version,
            latest_version=args.latest_version,
            relation_name=args.relation_name,
            database=args.database,
            schema=args.schema,
            alias=args.identifier,
            deprecation_date=args.deprecation_date,
            checksum=FileHash.from_contents(f"{unique_id},{args.generated_at}"),
            access=AccessType(args.access),
            original_file_path="",
            path="",
            unrendered_config=unrendered_config,
            depends_on=DependsOn(nodes=args.depends_on_nodes),
            config=ModelConfig(enabled=args.enabled),
        )

    @property
    def is_external_node(self) -> bool:
        return not self.original_file_path and not self.path

    @property
    def is_latest_version(self) -> bool:
        return self.version is not None and self.version == self.latest_version

    @property
    def is_past_deprecation_date(self) -> bool:
        return (
            self.deprecation_date is not None
            and self.deprecation_date < datetime.now().astimezone()
        )

    @property
    def search_name(self):
        if self.version is None:
            return self.name
        else:
            return f"{self.name}.v{self.version}"

    @property
    def materialization_enforces_constraints(self) -> bool:
        return self.config.materialized in ["table", "incremental"]

    @property
    def all_constraints(self) -> List[Union[ModelLevelConstraint, ColumnLevelConstraint]]:
        constraints: List[Union[ModelLevelConstraint, ColumnLevelConstraint]] = []
        for model_level_constraint in self.constraints:
            constraints.append(model_level_constraint)

        for column in self.columns.values():
            for column_level_constraint in column.constraints:
                constraints.append(column_level_constraint)

        return constraints

    @property
    def has_this(self) -> bool:
        if self._has_this is None:
            self._has_this = statically_extract_has_name_this(self.raw_code)
        return self._has_this

    def infer_primary_key(self, data_tests: List["GenericTestNode"]) -> List[str]:
        """
        Infers the columns that can be used as primary key of a model in the following order:
        1. Columns with primary key constraints
        2. Columns with unique and not_null data tests
        3. Columns with enabled unique or dbt_utils.unique_combination_of_columns data tests
        4. Columns with disabled unique or dbt_utils.unique_combination_of_columns data tests
        """
        for constraint in self.constraints:
            if constraint.type == ConstraintType.primary_key:
                return constraint.columns

        for column, column_info in self.columns.items():
            for column_constraint in column_info.constraints:
                if column_constraint.type == ConstraintType.primary_key:
                    return [column]

        columns_with_enabled_unique_tests = set()
        columns_with_disabled_unique_tests = set()
        columns_with_not_null_tests = set()
        for test in data_tests:
            columns: List[str] = []
            # extract columns from test kwargs, ensuring columns is a List[str] given tests can have custom (user or pacakge-defined) kwarg types
            if "column_name" in test.test_metadata.kwargs and isinstance(
                test.test_metadata.kwargs["column_name"], str
            ):
                columns = [test.test_metadata.kwargs["column_name"]]
            elif "combination_of_columns" in test.test_metadata.kwargs and isinstance(
                test.test_metadata.kwargs["combination_of_columns"], list
            ):
                columns = [
                    column
                    for column in test.test_metadata.kwargs["combination_of_columns"]
                    if isinstance(column, str)
                ]

            for column in columns:
                if test.test_metadata.name in ["unique", "unique_combination_of_columns"]:
                    if test.config.enabled:
                        columns_with_enabled_unique_tests.add(column)
                    else:
                        columns_with_disabled_unique_tests.add(column)
                elif test.test_metadata.name == "not_null":
                    columns_with_not_null_tests.add(column)

        columns_with_unique_and_not_null_tests = []
        for column in columns_with_not_null_tests:
            if (
                column in columns_with_enabled_unique_tests
                or column in columns_with_disabled_unique_tests
            ):
                columns_with_unique_and_not_null_tests.append(column)
        if columns_with_unique_and_not_null_tests:
            return columns_with_unique_and_not_null_tests

        if columns_with_enabled_unique_tests:
            return list(columns_with_enabled_unique_tests)

        if columns_with_disabled_unique_tests:
            return list(columns_with_disabled_unique_tests)

        return []

    def same_contents(self, old, adapter_type) -> bool:
        return super().same_contents(old, adapter_type) and self.same_ref_representation(old)

    def same_ref_representation(self, old) -> bool:
        return (
            # Changing the latest_version may break downstream unpinned refs
            self.latest_version == old.latest_version
            # Changes to access or deprecation_date may lead to ref-related parsing errors
            and self.access == old.access
            and self.deprecation_date == old.deprecation_date
        )

    def build_contract_checksum(self):
        # We don't need to construct the checksum if the model does not
        # have contract enforced, because it won't be used.
        # This needs to be executed after contract config is set

        # Avoid rebuilding the checksum if it has already been set.
        if self.contract.checksum is not None:
            return

        if self.contract.enforced is True:
            contract_state = ""
            # We need to sort the columns so that order doesn't matter
            # columns is a str: ColumnInfo dictionary
            sorted_columns = sorted(self.columns.values(), key=lambda col: col.name)
            for column in sorted_columns:
                contract_state += f"|{column.name}"
                contract_state += str(column.data_type)
                contract_state += str(column.constraints)
            if self.materialization_enforces_constraints:
                contract_state += self.config.materialized
                contract_state += str(self.constraints)
            data = contract_state.encode("utf-8")
            self.contract.checksum = hashlib.new("sha256", data).hexdigest()

    def same_contract_removed(self) -> bool:
        """
        self: the removed (deleted, renamed, or disabled) model node
        """
        # If the contract wasn't previously enforced, no contract change has occurred
        if self.contract.enforced is False:
            return True

        # Removed node is past its deprecation_date, so deletion does not constitute a contract change
        if self.is_past_deprecation_date:
            return True

        # Disabled, deleted, or renamed node with previously enforced contract.
        if not self.config.enabled:
            breaking_change = f"Contracted model '{self.unique_id}' was disabled."
        else:
            breaking_change = f"Contracted model '{self.unique_id}' was deleted or renamed."

        if self.version is None:
            warn_or_error(
                UnversionedBreakingChange(
                    breaking_changes=[breaking_change],
                    model_name=self.name,
                    model_file_path=self.original_file_path,
                ),
                node=self,
            )
            return False
        else:
            raise (
                ContractBreakingChangeError(
                    breaking_changes=[breaking_change],
                    node=self,
                )
            )

    def same_contract(self, old, adapter_type=None) -> bool:
        # If the contract wasn't previously enforced:
        if old.contract.enforced is False and self.contract.enforced is False:
            # No change -- same_contract: True
            return True
        if old.contract.enforced is False and self.contract.enforced is True:
            # Now it's enforced. This is a change, but not a breaking change -- same_contract: False
            return False

        # Otherwise: The contract was previously enforced, and we need to check for changes.
        # Happy path: The contract is still being enforced, and the checksums are identical.
        if self.contract.enforced is True and self.contract.checksum == old.contract.checksum:
            # No change -- same_contract: True
            return True

        # Otherwise: There has been a change.
        # We need to determine if it is a **breaking** change.
        # These are the categories of breaking changes:
        contract_enforced_disabled: bool = False
        columns_removed: List[str] = []
        column_type_changes: List[Dict[str, str]] = []
        enforced_column_constraint_removed: List[Dict[str, str]] = (
            []
        )  # column_name, constraint_type
        enforced_model_constraint_removed: List[Dict[str, Any]] = []  # constraint_type, columns
        materialization_changed: List[str] = []

        if old.contract.enforced is True and self.contract.enforced is False:
            # Breaking change: the contract was previously enforced, and it no longer is
            contract_enforced_disabled = True

        constraint_support = get_adapter_constraint_support(adapter_type)
        column_constraints_exist = False

        # Next, compare each column from the previous contract (old.columns)
        for old_key, old_value in sorted(old.columns.items()):
            # Has this column been removed?
            if old_key not in self.columns.keys():
                columns_removed.append(old_value.name)
            # Has this column's data type changed?
            elif old_value.data_type != self.columns[old_key].data_type:
                column_type_changes.append(
                    {
                        "column_name": str(old_value.name),
                        "previous_column_type": str(old_value.data_type),
                        "current_column_type": str(self.columns[old_key].data_type),
                    }
                )

            # track if there are any column level constraints for the materialization check late
            if old_value.constraints:
                column_constraints_exist = True

            # Have enforced columns level constraints changed?
            # Constraints are only enforced for table and incremental materializations.
            # We only really care if the old node was one of those materializations for breaking changes
            if (
                old_key in self.columns.keys()
                and old_value.constraints != self.columns[old_key].constraints
                and old.materialization_enforces_constraints
            ):
                for old_constraint in old_value.constraints:
                    if (
                        old_constraint not in self.columns[old_key].constraints
                        and constraint_support[old_constraint.type] == ConstraintSupport.ENFORCED
                    ):
                        enforced_column_constraint_removed.append(
                            {
                                "column_name": old_key,
                                "constraint_name": old_constraint.name,
                                "constraint_type": ConstraintType(old_constraint.type),
                            }
                        )

        # Now compare the model level constraints
        if old.constraints != self.constraints and old.materialization_enforces_constraints:
            for old_constraint in old.constraints:
                if (
                    old_constraint not in self.constraints
                    and constraint_support[old_constraint.type] == ConstraintSupport.ENFORCED
                ):
                    enforced_model_constraint_removed.append(
                        {
                            "constraint_name": old_constraint.name,
                            "constraint_type": ConstraintType(old_constraint.type),
                            "columns": old_constraint.columns,
                        }
                    )

        # Check for relevant materialization changes.
        if (
            old.materialization_enforces_constraints
            and not self.materialization_enforces_constraints
            and (old.constraints or column_constraints_exist)
        ):
            materialization_changed = [old.config.materialized, self.config.materialized]

        # If a column has been added, it will be missing in the old.columns, and present in self.columns
        # That's a change (caught by the different checksums), but not a breaking change

        # Did we find any changes that we consider breaking? If there's an enforced contract, that's
        # a warning unless the model is versioned, then it's an error.
        if (
            contract_enforced_disabled
            or columns_removed
            or column_type_changes
            or enforced_model_constraint_removed
            or enforced_column_constraint_removed
            or materialization_changed
        ):

            breaking_changes = []
            if contract_enforced_disabled:
                breaking_changes.append(
                    "Contract enforcement was removed: Previously, this model had an enforced contract. It is no longer configured to enforce its contract, and this is a breaking change."
                )
            if columns_removed:
                columns_removed_str = "\n    - ".join(columns_removed)
                breaking_changes.append(f"Columns were removed: \n    - {columns_removed_str}")
            if column_type_changes:
                column_type_changes_str = "\n    - ".join(
                    [
                        f"{c['column_name']} ({c['previous_column_type']} -> {c['current_column_type']})"
                        for c in column_type_changes
                    ]
                )
                breaking_changes.append(
                    f"Columns with data_type changes: \n    - {column_type_changes_str}"
                )
            if enforced_column_constraint_removed:
                column_constraint_changes_str = "\n    - ".join(
                    [
                        f"'{c['constraint_name'] if c['constraint_name'] is not None else c['constraint_type']}' constraint on column {c['column_name']}"
                        for c in enforced_column_constraint_removed
                    ]
                )
                breaking_changes.append(
                    f"Enforced column level constraints were removed: \n    - {column_constraint_changes_str}"
                )
            if enforced_model_constraint_removed:
                model_constraint_changes_str = "\n    - ".join(
                    [
                        f"'{c['constraint_name'] if c['constraint_name'] is not None else c['constraint_type']}' constraint on columns {c['columns']}"
                        for c in enforced_model_constraint_removed
                    ]
                )
                breaking_changes.append(
                    f"Enforced model level constraints were removed: \n    - {model_constraint_changes_str}"
                )
            if materialization_changed:
                materialization_changes_str = (
                    f"{materialization_changed[0]} -> {materialization_changed[1]}"
                )

                breaking_changes.append(
                    f"Materialization changed with enforced constraints: \n    - {materialization_changes_str}"
                )

            if self.version is None:
                warn_or_error(
                    UnversionedBreakingChange(
                        contract_enforced_disabled=contract_enforced_disabled,
                        columns_removed=columns_removed,
                        column_type_changes=column_type_changes,
                        enforced_column_constraint_removed=enforced_column_constraint_removed,
                        enforced_model_constraint_removed=enforced_model_constraint_removed,
                        breaking_changes=breaking_changes,
                        model_name=self.name,
                        model_file_path=self.original_file_path,
                    ),
                    node=self,
                )
            else:
                raise (
                    ContractBreakingChangeError(
                        breaking_changes=breaking_changes,
                        node=self,
                    )
                )

        # Otherwise, the contract has changed -- same_contract: False
        return False


@dataclass
class SqlNode(SqlOperationResource, CompiledNode):
    @classmethod
    def resource_class(cls) -> Type[SqlOperationResource]:
        return SqlOperationResource


# ====================================
# Seed node
# ====================================


@dataclass
class SeedNode(SeedResource, ParsedNode):  # No SQLDefaults!
    @classmethod
    def resource_class(cls) -> Type[SeedResource]:
        return SeedResource

    def same_seeds(self, other: "SeedNode") -> bool:
        # for seeds, we check the hashes. If the hashes are different types,
        # no match. If the hashes are both the same 'path', log a warning and
        # assume they are the same
        # if the current checksum is a path, we want to log a warning.
        result = self.checksum == other.checksum

        if self.checksum.name == "path":
            msg: str
            if other.checksum.name != "path":
                warn_or_error(
                    SeedIncreased(package_name=self.package_name, name=self.name), node=self
                )
            elif result:
                warn_or_error(
                    SeedExceedsLimitSamePath(package_name=self.package_name, name=self.name),
                    node=self,
                )
            elif not result:
                warn_or_error(
                    SeedExceedsLimitAndPathChanged(package_name=self.package_name, name=self.name),
                    node=self,
                )
            else:
                warn_or_error(
                    SeedExceedsLimitChecksumChanged(
                        package_name=self.package_name,
                        name=self.name,
                        checksum_name=other.checksum.name,
                    ),
                    node=self,
                )

        return result

    @property
    def empty(self):
        """Seeds are never empty"""
        return False

    def _disallow_implicit_dependencies(self):
        """Disallow seeds to take implicit upstream dependencies via pre/post hooks"""
        # Seeds are root nodes in the DAG. They cannot depend on other nodes.
        # However, it's possible to define pre- and post-hooks on seeds, and for those
        # hooks to include {{ ref(...) }}. This worked in previous versions, but it
        # was never officially documented or supported behavior. Let's raise an explicit error,
        # which will surface during parsing if the user has written code such that we attempt
        # to capture & record a ref/source/metric call on the SeedNode.
        # For more details: https://github.com/dbt-labs/dbt-core/issues/6806
        hooks = [f'- pre_hook: "{hook.sql}"' for hook in self.config.pre_hook] + [
            f'- post_hook: "{hook.sql}"' for hook in self.config.post_hook
        ]
        hook_list = "\n".join(hooks)
        message = f"""
Seeds cannot depend on other nodes. dbt detected a seed with a pre- or post-hook
that calls 'ref', 'source', or 'metric', either directly or indirectly via other macros.

Error raised for '{self.unique_id}', which has these hooks defined: \n{hook_list}
        """
        raise ParsingError(message)

    @property
    def refs(self):
        self._disallow_implicit_dependencies()

    @property
    def sources(self):
        self._disallow_implicit_dependencies()

    @property
    def metrics(self):
        self._disallow_implicit_dependencies()

    def same_body(self, other) -> bool:
        return self.same_seeds(other)

    @property
    def depends_on_nodes(self):
        return []

    @property
    def depends_on_macros(self) -> List[str]:
        return self.depends_on.macros

    @property
    def extra_ctes(self):
        return []

    @property
    def extra_ctes_injected(self):
        return False

    @property
    def language(self):
        return "sql"


#    @property
#    def compiled_code(self):
#        return None


# ====================================
# Singular Test node
# ====================================


class TestShouldStoreFailures:
    @property
    def should_store_failures(self):
        if self.config.store_failures:
            return self.config.store_failures
        return get_flags().STORE_FAILURES

    @property
    def is_relational(self):
        if self.should_store_failures:
            return True
        return False


@dataclass
class SingularTestNode(SingularTestResource, TestShouldStoreFailures, CompiledNode):
    @classmethod
    def resource_class(cls) -> Type[SingularTestResource]:
        return SingularTestResource

    @property
    def test_node_type(self):
        return "singular"


# ====================================
# Generic Test node
# ====================================


@dataclass
class GenericTestNode(GenericTestResource, TestShouldStoreFailures, CompiledNode):
    @classmethod
    def resource_class(cls) -> Type[GenericTestResource]:
        return GenericTestResource

    def same_contents(self, other, adapter_type: Optional[str]) -> bool:
        if other is None:
            return False

        return self.same_config(other) and self.same_fqn(other) and True

    @property
    def test_node_type(self):
        return "generic"


@dataclass
class UnitTestSourceDefinition(ModelNode):
    source_name: str = "undefined"
    quoting: QuotingResource = field(default_factory=QuotingResource)

    @property
    def search_name(self):
        return f"{self.source_name}.{self.name}"


@dataclass
class UnitTestNode(CompiledNode):
    resource_type: Literal[NodeType.Unit]
    tested_node_unique_id: Optional[str] = None
    this_input_node_unique_id: Optional[str] = None
    overrides: Optional[UnitTestOverrides] = None
    config: UnitTestNodeConfig = field(default_factory=UnitTestNodeConfig)


@dataclass
class UnitTestDefinition(NodeInfoMixin, GraphNode, UnitTestDefinitionResource):
    @classmethod
    def resource_class(cls) -> Type[UnitTestDefinitionResource]:
        return UnitTestDefinitionResource

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def tags(self) -> List[str]:
        tags = self.config.tags
        return [tags] if isinstance(tags, str) else tags

    @property
    def versioned_name(self) -> str:
        versioned_name = self.name
        if self.version is not None:
            versioned_name += f"_v{self.version}"
        return versioned_name

    def build_unit_test_checksum(self):
        # everything except 'description'
        data = f"{self.model}-{self.versions}-{self.given}-{self.expect}-{self.overrides}"

        # include underlying fixture data
        for input in self.given:
            if input.fixture:
                data += f"-{input.rows}"

        self.checksum = hashlib.new("sha256", data.encode("utf-8")).hexdigest()

    def same_contents(self, other: Optional["UnitTestDefinition"]) -> bool:
        if other is None:
            return False

        return self.checksum == other.checksum


@dataclass
class UnitTestFileFixture(BaseNode):
    resource_type: Literal[NodeType.Fixture]
    rows: Optional[Union[List[Dict[str, Any]], str]] = None


# ====================================
# Snapshot node
# ====================================


@dataclass
class SnapshotNode(SnapshotResource, CompiledNode):
    @classmethod
    def resource_class(cls) -> Type[SnapshotResource]:
        return SnapshotResource


# ====================================
# Macro
# ====================================


@dataclass
class Macro(MacroResource, BaseNode):
    @classmethod
    def resource_class(cls) -> Type[MacroResource]:
        return MacroResource

    def same_contents(self, other: Optional["Macro"]) -> bool:
        if other is None:
            return False
        # the only thing that makes one macro different from another with the
        # same name/package is its content
        return self.macro_sql == other.macro_sql

    @property
    def depends_on_macros(self):
        return self.depends_on.macros


# ====================================
# Documentation node
# ====================================


@dataclass
class Documentation(DocumentationResource, BaseNode):
    @classmethod
    def resource_class(cls) -> Type[DocumentationResource]:
        return DocumentationResource

    @property
    def search_name(self):
        return self.name

    def same_contents(self, other: Optional["Documentation"]) -> bool:
        if other is None:
            return False
        # the only thing that makes one doc different from another with the
        # same name/package is its content
        return self.block_contents == other.block_contents


# ====================================
# Source node
# ====================================


def normalize_test(testdef: TestDef) -> Dict[str, Any]:
    if isinstance(testdef, str):
        return {testdef: {}}
    else:
        return testdef


@dataclass
class UnpatchedSourceDefinition(BaseNode):
    source: UnparsedSourceDefinition
    table: UnparsedSourceTableDefinition
    fqn: List[str]
    resource_type: Literal[NodeType.Source]
    patch_path: Optional[str] = None

    def get_full_source_name(self):
        return f"{self.source.name}_{self.table.name}"

    def get_source_representation(self):
        return f'source("{self.source.name}", "{self.table.name}")'

    def validate_data_tests(self, is_root_project: bool):
        """
        sources parse tests differently than models, so we need to do some validation
        here where it's done in the PatchParser for other nodes
        """
        # source table-level tests
        if self.tests and self.data_tests:
            raise ValidationError(
                "Invalid test config: cannot have both 'tests' and 'data_tests' defined"
            )
        if self.tests:
            self.data_tests.extend(self.tests)
            self.tests.clear()

        # column-level tests
        for column in self.columns:
            if column.tests and column.data_tests:
                raise ValidationError(
                    "Invalid test config: cannot have both 'tests' and 'data_tests' defined"
                )
            if column.tests:
                column.data_tests.extend(column.tests)
                column.tests.clear()

    @property
    def quote_columns(self) -> Optional[bool]:
        result = None
        if self.source.quoting.column is not None:
            result = self.source.quoting.column
        if self.table.quoting.column is not None:
            result = self.table.quoting.column
        return result

    @property
    def columns(self) -> Sequence[UnparsedColumn]:
        return [] if self.table.columns is None else self.table.columns

    def get_tests(self) -> Iterator[Tuple[Dict[str, Any], Optional[UnparsedColumn]]]:
        for data_test in self.data_tests:
            yield normalize_test(data_test), None

        for column in self.columns:
            if column.data_tests is not None:
                for data_test in column.data_tests:
                    yield normalize_test(data_test), column

    @property
    def data_tests(self) -> List[TestDef]:
        if self.table.data_tests is None:
            return []
        else:
            return self.table.data_tests

    # deprecated
    @property
    def tests(self) -> List[TestDef]:
        if self.table.tests is None:
            return []
        else:
            return self.table.tests


@dataclass
class SourceDefinition(
    NodeInfoMixin,
    GraphNode,
    SourceDefinitionResource,
    HasRelationMetadata,
):
    @classmethod
    def resource_class(cls) -> Type[SourceDefinitionResource]:
        return SourceDefinitionResource

    def same_database_representation(self, other: "SourceDefinition") -> bool:

        # preserve legacy behaviour -- use potentially rendered database
        if get_flags().state_modified_compare_more_unrendered_values is False:
            same_database = self.database == other.database
            same_schema = self.schema == other.schema
        else:
            same_database = self.unrendered_database == other.unrendered_database
            same_schema = self.unrendered_schema == other.unrendered_schema

        return same_database and same_schema and self.identifier == other.identifier and True

    def same_quoting(self, other: "SourceDefinition") -> bool:
        return self.quoting == other.quoting

    def same_freshness(self, other: "SourceDefinition") -> bool:
        return (
            self.freshness == other.freshness
            and self.loaded_at_field == other.loaded_at_field
            and True
        )

    def same_external(self, other: "SourceDefinition") -> bool:
        return self.external == other.external

    def same_config(self, old: "SourceDefinition") -> bool:
        return self.config.same_contents(
            self.unrendered_config,
            old.unrendered_config,
        )

    def same_contents(self, old: Optional["SourceDefinition"]) -> bool:
        # existing when it didn't before is a change!
        if old is None:
            return True

        # config changes are changes (because the only config is "enforced", and
        # enabling a source is a change!)
        # changing the database/schema/identifier is a change
        # messing around with external stuff is a change (uh, right?)
        # quoting changes are changes
        # freshness changes are changes, I guess
        # metadata/tags changes are not "changes"
        # patching/description changes are not "changes"
        return (
            self.same_database_representation(old)
            and self.same_fqn(old)
            and self.same_config(old)
            and self.same_quoting(old)
            and self.same_freshness(old)
            and self.same_external(old)
            and True
        )

    def get_full_source_name(self):
        return f"{self.source_name}_{self.name}"

    def get_source_representation(self):
        return f'source("{self.source.name}", "{self.table.name}")'

    @property
    def is_refable(self):
        return False

    @property
    def is_ephemeral(self):
        return False

    @property
    def is_ephemeral_model(self):
        return False

    @property
    def depends_on_nodes(self):
        return []

    @property
    def depends_on(self):
        return DependsOn(macros=[], nodes=[])

    @property
    def refs(self):
        return []

    @property
    def sources(self):
        return []

    @property
    def has_freshness(self) -> bool:
        return bool(self.freshness)

    @property
    def search_name(self):
        return f"{self.source_name}.{self.name}"

    @property
    def group(self):
        return None


# ====================================
# Exposure node
# ====================================


@dataclass
class Exposure(NodeInfoMixin, GraphNode, ExposureResource):
    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def search_name(self):
        return self.name

    @classmethod
    def resource_class(cls) -> Type[ExposureResource]:
        return ExposureResource

    def same_depends_on(self, old: "Exposure") -> bool:
        return set(self.depends_on.nodes) == set(old.depends_on.nodes)

    def same_description(self, old: "Exposure") -> bool:
        return self.description == old.description

    def same_label(self, old: "Exposure") -> bool:
        return self.label == old.label

    def same_maturity(self, old: "Exposure") -> bool:
        return self.maturity == old.maturity

    def same_owner(self, old: "Exposure") -> bool:
        return self.owner == old.owner

    def same_exposure_type(self, old: "Exposure") -> bool:
        return self.type == old.type

    def same_url(self, old: "Exposure") -> bool:
        return self.url == old.url

    def same_config(self, old: "Exposure") -> bool:
        return self.config.same_contents(
            self.unrendered_config,
            old.unrendered_config,
        )

    def same_contents(self, old: Optional["Exposure"]) -> bool:
        # existing when it didn't before is a change!
        # metadata/tags changes are not "changes"
        if old is None:
            return True

        return (
            self.same_fqn(old)
            and self.same_exposure_type(old)
            and self.same_owner(old)
            and self.same_maturity(old)
            and self.same_url(old)
            and self.same_description(old)
            and self.same_label(old)
            and self.same_depends_on(old)
            and self.same_config(old)
            and True
        )

    @property
    def group(self):
        return None

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if "_event_status" in dct:
            del dct["_event_status"]
        return dct


# ====================================
# Metric node
# ====================================


@dataclass
class Metric(GraphNode, MetricResource):
    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def search_name(self):
        return self.name

    @classmethod
    def resource_class(cls) -> Type[MetricResource]:
        return MetricResource

    def same_description(self, old: "Metric") -> bool:
        return self.description == old.description

    def same_label(self, old: "Metric") -> bool:
        return self.label == old.label

    def same_config(self, old: "Metric") -> bool:
        return self.config.same_contents(
            self.unrendered_config,
            old.unrendered_config,
        )

    def same_filter(self, old: "Metric") -> bool:
        return True  # TODO

    def same_metadata(self, old: "Metric") -> bool:
        return True  # TODO

    def same_type(self, old: "Metric") -> bool:
        return self.type == old.type

    def same_type_params(self, old: "Metric") -> bool:
        return True  # TODO

    def same_contents(self, old: Optional["Metric"]) -> bool:
        # existing when it didn't before is a change!
        # metadata/tags changes are not "changes"
        if old is None:
            return True

        return (
            self.same_filter(old)
            and self.same_metadata(old)
            and self.same_type(old)
            and self.same_type_params(old)
            and self.same_description(old)
            and self.same_label(old)
            and self.same_config(old)
            and True
        )

    def add_input_measure(self, input_measure: MetricInputMeasure) -> None:
        for existing_input_measure in self.type_params.input_measures:
            if input_measure == existing_input_measure:
                return
        self.type_params.input_measures.append(input_measure)


# ====================================
# Group node
# ====================================


@dataclass
class Group(GroupResource, BaseNode):
    @classmethod
    def resource_class(cls) -> Type[GroupResource]:
        return GroupResource

    def to_logging_dict(self) -> Dict[str, Union[str, Dict[str, str]]]:
        return {
            "name": self.name,
            "package_name": self.package_name,
            "owner": {k: str(v) for k, v in self.owner.to_dict(omit_none=True).items()},
        }


# ====================================
# SemanticModel node
# ====================================


@dataclass
class SemanticModel(GraphNode, SemanticModelResource):
    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def depends_on_macros(self):
        return self.depends_on.macros

    @classmethod
    def resource_class(cls) -> Type[SemanticModelResource]:
        return SemanticModelResource

    def same_model(self, old: "SemanticModel") -> bool:
        return self.model == old.model

    def same_description(self, old: "SemanticModel") -> bool:
        return self.description == old.description

    def same_defaults(self, old: "SemanticModel") -> bool:
        return self.defaults == old.defaults

    def same_entities(self, old: "SemanticModel") -> bool:
        return self.entities == old.entities

    def same_dimensions(self, old: "SemanticModel") -> bool:
        return self.dimensions == old.dimensions

    def same_measures(self, old: "SemanticModel") -> bool:
        return self.measures == old.measures

    def same_config(self, old: "SemanticModel") -> bool:
        return self.config == old.config

    def same_primary_entity(self, old: "SemanticModel") -> bool:
        return self.primary_entity == old.primary_entity

    def same_group(self, old: "SemanticModel") -> bool:
        return self.group == old.group

    def same_contents(self, old: Optional["SemanticModel"]) -> bool:
        # existing when it didn't before is a change!
        # metadata/tags changes are not "changes"
        if old is None:
            return True

        return (
            self.same_model(old)
            and self.same_description(old)
            and self.same_defaults(old)
            and self.same_entities(old)
            and self.same_dimensions(old)
            and self.same_measures(old)
            and self.same_config(old)
            and self.same_primary_entity(old)
            and self.same_group(old)
            and True
        )


# ====================================
# SavedQuery
# ====================================


@dataclass
class SavedQuery(NodeInfoMixin, GraphNode, SavedQueryResource):
    @classmethod
    def resource_class(cls) -> Type[SavedQueryResource]:
        return SavedQueryResource

    def same_metrics(self, old: "SavedQuery") -> bool:
        return self.query_params.metrics == old.query_params.metrics

    def same_group_by(self, old: "SavedQuery") -> bool:
        return self.query_params.group_by == old.query_params.group_by

    def same_description(self, old: "SavedQuery") -> bool:
        return self.description == old.description

    def same_where(self, old: "SavedQuery") -> bool:
        return self.query_params.where == old.query_params.where

    def same_label(self, old: "SavedQuery") -> bool:
        return self.label == old.label

    def same_config(self, old: "SavedQuery") -> bool:
        return self.config == old.config

    def same_group(self, old: "SavedQuery") -> bool:
        return self.group == old.group

    def same_exports(self, old: "SavedQuery") -> bool:
        if len(self.exports) != len(old.exports):
            return False

        # exports should be in the same order, so we zip them for easy iteration
        for old_export, new_export in zip(old.exports, self.exports):
            if not (old_export.name == new_export.name):
                return False
            keys = ["export_as", "schema", "alias"]
            for key in keys:
                if old_export.unrendered_config.get(key) != new_export.unrendered_config.get(key):
                    return False

        return True

    def same_tags(self, old: "SavedQuery") -> bool:
        return self.tags == old.tags

    def same_contents(self, old: Optional["SavedQuery"]) -> bool:
        # existing when it didn't before is a change!
        # metadata/tags changes are not "changes"
        if old is None:
            return True

        return (
            self.same_metrics(old)
            and self.same_group_by(old)
            and self.same_description(old)
            and self.same_where(old)
            and self.same_label(old)
            and self.same_config(old)
            and self.same_group(old)
            and self.same_exports(old)
            and self.same_tags(old)
            and True
        )

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if "_event_status" in dct:
            del dct["_event_status"]
        return dct


# ====================================
# Patches
# ====================================


@dataclass
class ParsedPatch(HasYamlMetadata):
    name: str
    description: str
    meta: Dict[str, Any]
    docs: Docs
    config: Dict[str, Any]


# The parsed node update is only the 'patch', not the test. The test became a
# regular parsed node. Note that description and columns must be present, but
# may be empty.
@dataclass
class ParsedNodePatch(ParsedPatch):
    columns: Dict[str, ColumnInfo]
    access: Optional[str]
    version: Optional[NodeVersion]
    latest_version: Optional[NodeVersion]
    constraints: List[Dict[str, Any]]
    deprecation_date: Optional[datetime]
    time_spine: Optional[TimeSpine] = None
    freshness: Optional[ModelFreshness] = None


@dataclass
class ParsedMacroPatch(ParsedPatch):
    arguments: List[MacroArgument] = field(default_factory=list)


@dataclass
class ParsedSingularTestPatch(ParsedPatch):
    pass


# ====================================
# Node unions/categories
# ====================================


# ManifestNode without SeedNode, which doesn't have the
# SQL related attributes
ManifestSQLNode = Union[
    AnalysisNode,
    SingularTestNode,
    HookNode,
    ModelNode,
    SqlNode,
    GenericTestNode,
    SnapshotNode,
    UnitTestNode,
]

# All SQL nodes plus SeedNode (csv files)
ManifestNode = Union[
    ManifestSQLNode,
    SeedNode,
]

ResultNode = Union[
    ManifestNode,
    SourceDefinition,
    HookNode,
]

# All nodes that can be in the DAG
GraphMemberNode = Union[
    ResultNode,
    Exposure,
    Metric,
    SavedQuery,
    SemanticModel,
    UnitTestDefinition,
]

# All "nodes" (or node-like objects) in this file
Resource = Union[
    GraphMemberNode,
    Documentation,
    Macro,
    Group,
]

TestNode = Union[SingularTestNode, GenericTestNode]

SemanticManifestNode = Union[SavedQuery, SemanticModel, Metric]

RESOURCE_CLASS_TO_NODE_CLASS: Dict[Type[BaseResource], Type[BaseNode]] = {
    node_class.resource_class(): node_class
    for node_class in get_args(Resource)
    if node_class is not UnitTestNode
}
