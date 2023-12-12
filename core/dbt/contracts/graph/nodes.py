import os
from datetime import datetime
import time
from dataclasses import dataclass, field
from enum import Enum
import hashlib

from mashumaro.types import SerializableType
from typing import Optional, Union, List, Dict, Any, Sequence, Tuple, Iterator, Literal

from dbt.dataclass_schema import dbtClassMixin, ExtensibleDbtClassMixin

from dbt.clients.system import write_file
from dbt.contracts.files import FileHash
from dbt.contracts.graph.saved_queries import Export, QueryParams
from dbt.contracts.graph.semantic_models import (
    Defaults,
    Dimension,
    Entity,
    Measure,
    SourceFileMetadata,
)
from dbt.contracts.graph.unparsed import (
    ConstantPropertyInput,
    Docs,
    ExposureType,
    ExternalTable,
    FreshnessThreshold,
    HasYamlMetadata,
    MacroArgument,
    MaturityType,
    Owner,
    Quoting,
    TestDef,
    NodeVersion,
    UnparsedSourceDefinition,
    UnparsedSourceTableDefinition,
    UnparsedColumn,
)
from dbt.contracts.graph.node_args import ModelNodeArgs
from dbt.contracts.graph.semantic_layer_common import WhereFilterIntersection
from dbt.contracts.util import Replaceable, AdditionalPropertiesMixin
from dbt.events.functions import warn_or_error
from dbt.exceptions import ParsingError, ContractBreakingChangeError
from dbt.events.types import (
    SeedIncreased,
    SeedExceedsLimitSamePath,
    SeedExceedsLimitAndPathChanged,
    SeedExceedsLimitChecksumChanged,
    UnversionedBreakingChange,
)
from dbt.events.contextvars import set_log_contextvars
from dbt.flags import get_flags
from dbt.node_types import ModelLanguage, NodeType, AccessType
from dbt_semantic_interfaces.references import (
    EntityReference,
    MeasureReference,
    LinkableElementReference,
    SemanticModelReference,
    TimeDimensionReference,
)
from dbt_semantic_interfaces.references import MetricReference as DSIMetricReference
from dbt_semantic_interfaces.type_enums import (
    ConversionCalculationType,
    MetricType,
    TimeGranularity,
)

from .model_config import (
    NodeConfig,
    ModelConfig,
    SeedConfig,
    TestConfig,
    SourceConfig,
    MetricConfig,
    ExposureConfig,
    EmptySnapshotConfig,
    SnapshotConfig,
    SemanticModelConfig,
    SavedQueryConfig,
)


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
class BaseNode(dbtClassMixin, Replaceable):
    """All nodes or node-like objects in this file should have this as a base class"""

    name: str
    resource_type: NodeType
    package_name: str
    path: str
    original_file_path: str
    unique_id: str

    @property
    def search_name(self):
        return self.name

    @property
    def file_id(self):
        return f"{self.package_name}://{self.original_file_path}"

    @property
    def is_refable(self):
        return self.resource_type in NodeType.refable()

    @property
    def should_store_failures(self):
        return False

    # will this node map to an object in the database?
    @property
    def is_relational(self):
        return self.resource_type in NodeType.refable()

    @property
    def is_versioned(self):
        return self.resource_type in NodeType.versioned() and self.version is not None

    @property
    def is_ephemeral(self):
        return self.config.materialized == "ephemeral"

    @property
    def is_ephemeral_model(self):
        return self.is_refable and self.is_ephemeral

    def get_materialization(self):
        return self.config.materialized


@dataclass
class GraphNode(BaseNode):
    """Nodes in the DAG. Macro and Documentation don't have fqn."""

    fqn: List[str]

    def same_fqn(self, other) -> bool:
        return self.fqn == other.fqn


@dataclass
class RefArgs(dbtClassMixin):
    name: str
    package: Optional[str] = None
    version: Optional[NodeVersion] = None

    @property
    def positional_args(self) -> List[str]:
        if self.package:
            return [self.package, self.name]
        else:
            return [self.name]

    @property
    def keyword_args(self) -> Dict[str, Optional[NodeVersion]]:
        if self.version:
            return {"version": self.version}
        else:
            return {}


class ConstraintType(str, Enum):
    check = "check"
    not_null = "not_null"
    unique = "unique"
    primary_key = "primary_key"
    foreign_key = "foreign_key"
    custom = "custom"

    @classmethod
    def is_valid(cls, item):
        try:
            cls(item)
        except ValueError:
            return False
        return True


@dataclass
class ColumnLevelConstraint(dbtClassMixin):
    type: ConstraintType
    name: Optional[str] = None
    # expression is a user-provided field that will depend on the constraint type.
    # It could be a predicate (check type), or a sequence sql keywords (e.g. unique type),
    # so the vague naming of 'expression' is intended to capture this range.
    expression: Optional[str] = None
    warn_unenforced: bool = (
        True  # Warn if constraint cannot be enforced by platform but will be in DDL
    )
    warn_unsupported: bool = (
        True  # Warn if constraint is not supported by the platform and won't be in DDL
    )


@dataclass
class ModelLevelConstraint(ColumnLevelConstraint):
    columns: List[str] = field(default_factory=list)


@dataclass
class ColumnInfo(AdditionalPropertiesMixin, ExtensibleDbtClassMixin, Replaceable):
    """Used in all ManifestNodes and SourceDefinition"""

    name: str
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    data_type: Optional[str] = None
    constraints: List[ColumnLevelConstraint] = field(default_factory=list)
    quote: Optional[bool] = None
    tags: List[str] = field(default_factory=list)
    _extra: Dict[str, Any] = field(default_factory=dict)


@dataclass
class Contract(dbtClassMixin, Replaceable):
    enforced: bool = False
    alias_types: bool = True
    checksum: Optional[str] = None


# Metrics, exposures,
@dataclass
class HasRelationMetadata(dbtClassMixin, Replaceable):
    database: Optional[str]
    schema: str

    # Can't set database to None like it ought to be
    # because it messes up the subclasses and default parameters
    # so hack it here
    @classmethod
    def __pre_deserialize__(cls, data):
        data = super().__pre_deserialize__(data)
        if "database" not in data:
            data["database"] = None
        return data


@dataclass
class MacroDependsOn(dbtClassMixin, Replaceable):
    """Used only in the Macro class"""

    macros: List[str] = field(default_factory=list)

    # 'in' on lists is O(n) so this is O(n^2) for # of macros
    def add_macro(self, value: str):
        if value not in self.macros:
            self.macros.append(value)


@dataclass
class DeferRelation(HasRelationMetadata):
    alias: str
    relation_name: Optional[str]

    @property
    def identifier(self):
        return self.alias


@dataclass
class DependsOn(MacroDependsOn):
    nodes: List[str] = field(default_factory=list)

    def add_node(self, value: str):
        if value not in self.nodes:
            self.nodes.append(value)


@dataclass
class ParsedNodeMandatory(GraphNode, HasRelationMetadata, Replaceable):
    alias: str
    checksum: FileHash
    config: NodeConfig = field(default_factory=NodeConfig)

    @property
    def identifier(self):
        return self.alias


# This needs to be in all ManifestNodes and also in SourceDefinition,
# because of "source freshness"
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
class ParsedNode(NodeInfoMixin, ParsedNodeMandatory, SerializableType):
    tags: List[str] = field(default_factory=list)
    description: str = field(default="")
    columns: Dict[str, ColumnInfo] = field(default_factory=dict)
    meta: Dict[str, Any] = field(default_factory=dict)
    group: Optional[str] = None
    docs: Docs = field(default_factory=Docs)
    patch_path: Optional[str] = None
    build_path: Optional[str] = None
    deferred: bool = False
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    created_at: float = field(default_factory=lambda: time.time())
    config_call_dict: Dict[str, Any] = field(default_factory=dict)
    relation_name: Optional[str] = None
    raw_code: str = ""

    def get_target_write_path(self, target_path: str, subdirectory: str):
        # This is called for both the "compiled" subdirectory of "target" and the "run" subdirectory
        if os.path.basename(self.path) == os.path.basename(self.original_file_path):
            # One-to-one relationship of nodes to files.
            path = self.original_file_path
        else:
            #  Many-to-one relationship of nodes to files.
            path = os.path.join(self.original_file_path, self.path)
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

    def __post_serialize__(self, dct):
        dct = super().__post_serialize__(dct)
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
        elif resource_type == "rpc":
            return RPCNode.from_dict(dct)
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
class InjectedCTE(dbtClassMixin, Replaceable):
    """Used in CompiledNodes as part of ephemeral model processing"""

    id: str
    sql: str


@dataclass
class CompiledNode(ParsedNode):
    """Contains attributes necessary for SQL files and nodes with refs, sources, etc,
    so all ManifestNodes except SeedNode."""

    language: str = "sql"
    refs: List[RefArgs] = field(default_factory=list)
    sources: List[List[str]] = field(default_factory=list)
    metrics: List[List[str]] = field(default_factory=list)
    depends_on: DependsOn = field(default_factory=DependsOn)
    compiled_path: Optional[str] = None
    compiled: bool = False
    compiled_code: Optional[str] = None
    extra_ctes_injected: bool = False
    extra_ctes: List[InjectedCTE] = field(default_factory=list)
    _pre_injected_sql: Optional[str] = None
    contract: Contract = field(default_factory=Contract)

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

    def __post_serialize__(self, dct):
        dct = super().__post_serialize__(dct)
        if "_pre_injected_sql" in dct:
            del dct["_pre_injected_sql"]
        # Remove compiled attributes
        if "compiled" in dct and dct["compiled"] is False:
            del dct["compiled"]
            del dct["extra_ctes_injected"]
            del dct["extra_ctes"]
            # "omit_none" means these might not be in the dictionary
            if "compiled_code" in dct:
                del dct["compiled_code"]
        return dct

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
class AnalysisNode(CompiledNode):
    resource_type: Literal[NodeType.Analysis]


@dataclass
class HookNode(CompiledNode):
    resource_type: Literal[NodeType.Operation]
    index: Optional[int] = None


@dataclass
class ModelNode(CompiledNode):
    resource_type: Literal[NodeType.Model]
    access: AccessType = AccessType.Protected
    yaml_config_dict: Dict[str, Any] = field(default_factory=dict)
    config: ModelConfig = field(default_factory=ModelConfig)
    constraints: List[ModelLevelConstraint] = field(default_factory=list)
    version: Optional[NodeVersion] = None
    latest_version: Optional[NodeVersion] = None
    deprecation_date: Optional[datetime] = None
    defer_relation: Optional[DeferRelation] = None

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
    def search_name(self):
        if self.version is None:
            return self.name
        else:
            return f"{self.name}.v{self.version}"

    @property
    def materialization_enforces_constraints(self) -> bool:
        return self.config.materialized in ["table", "incremental"]

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
        enforced_column_constraint_removed: List[
            Dict[str, str]
        ] = []  # column_name, constraint_type
        enforced_model_constraint_removed: List[Dict[str, Any]] = []  # constraint_type, columns
        materialization_changed: List[str] = []

        if old.contract.enforced is True and self.contract.enforced is False:
            # Breaking change: the contract was previously enforced, and it no longer is
            contract_enforced_disabled = True

        # TODO: this avoid the circular imports but isn't ideal
        from dbt.adapters.factory import get_adapter_constraint_support
        from dbt.adapters.base import ConstraintSupport

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


# TODO: rm?
@dataclass
class RPCNode(CompiledNode):
    resource_type: Literal[NodeType.RPCCall]


@dataclass
class SqlNode(CompiledNode):
    resource_type: Literal[NodeType.SqlOperation]


# ====================================
# Seed node
# ====================================


@dataclass
class SeedNode(ParsedNode):  # No SQLDefaults!
    resource_type: Literal[NodeType.Seed]
    config: SeedConfig = field(default_factory=SeedConfig)
    # seeds need the root_path because the contents are not loaded initially
    # and we need the root_path to load the seed later
    root_path: Optional[str] = None
    depends_on: MacroDependsOn = field(default_factory=MacroDependsOn)
    defer_relation: Optional[DeferRelation] = None

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
class SingularTestNode(TestShouldStoreFailures, CompiledNode):
    resource_type: Literal[NodeType.Test]
    # Was not able to make mypy happy and keep the code working. We need to
    # refactor the various configs.
    config: TestConfig = field(default_factory=TestConfig)  # type: ignore

    @property
    def test_node_type(self):
        return "singular"


# ====================================
# Generic Test node
# ====================================


@dataclass
class TestMetadata(dbtClassMixin, Replaceable):
    __test__ = False

    name: str
    # kwargs are the args that are left in the test builder after
    # removing configs. They are set from the test builder when
    # the test node is created.
    kwargs: Dict[str, Any] = field(default_factory=dict)
    namespace: Optional[str] = None


# This has to be separated out because it has no default and so
# has to be included as a superclass, not an attribute
@dataclass
class HasTestMetadata(dbtClassMixin):
    test_metadata: TestMetadata


@dataclass
class GenericTestNode(TestShouldStoreFailures, CompiledNode, HasTestMetadata):
    resource_type: Literal[NodeType.Test]
    column_name: Optional[str] = None
    file_key_name: Optional[str] = None
    # Was not able to make mypy happy and keep the code working. We need to
    # refactor the various configs.
    config: TestConfig = field(default_factory=TestConfig)  # type: ignore
    attached_node: Optional[str] = None

    def same_contents(self, other, adapter_type: Optional[str]) -> bool:
        if other is None:
            return False

        return self.same_config(other) and self.same_fqn(other) and True

    @property
    def test_node_type(self):
        return "generic"

    def write_node(self, target_path: str, subdirectory: str, payload: str):
        # If this test came from yaml frontmatter, it will try to write to a directory
        # with the same name as the model file, which won't work. So change the
        # directory to model file name + .yaml.
        file_path = self.original_file_path
        if file_path.endswith(".sql") or file_path.endswith(".py"):
            file_path = file_path + ".yaml"
        path = os.path.join(file_path, self.path)
        full_path = os.path.join(target_path, subdirectory, self.package_name, path)

        write_file(full_path, payload)
        return full_path


# ====================================
# Snapshot node
# ====================================


@dataclass
class IntermediateSnapshotNode(CompiledNode):
    # at an intermediate stage in parsing, where we've built something better
    # than an unparsed node for rendering in parse mode, it's pretty possible
    # that we won't have critical snapshot-related information that is only
    # defined in config blocks. To fix that, we have an intermediate type that
    # uses a regular node config, which the snapshot parser will then convert
    # into a full ParsedSnapshotNode after rendering. Note: it currently does
    # not work to set snapshot config in schema files because of the validation.
    resource_type: Literal[NodeType.Snapshot]
    config: EmptySnapshotConfig = field(default_factory=EmptySnapshotConfig)


@dataclass
class SnapshotNode(CompiledNode):
    resource_type: Literal[NodeType.Snapshot]
    config: SnapshotConfig
    defer_relation: Optional[DeferRelation] = None


# ====================================
# Macro
# ====================================


@dataclass
class Macro(BaseNode):
    macro_sql: str
    resource_type: Literal[NodeType.Macro]
    depends_on: MacroDependsOn = field(default_factory=MacroDependsOn)
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    docs: Docs = field(default_factory=Docs)
    patch_path: Optional[str] = None
    arguments: List[MacroArgument] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())
    supported_languages: Optional[List[ModelLanguage]] = None

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
class Documentation(BaseNode):
    block_contents: str
    resource_type: Literal[NodeType.Documentation]

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
        for test in self.tests:
            yield normalize_test(test), None

        for column in self.columns:
            if column.tests is not None:
                for test in column.tests:
                    yield normalize_test(test), column

    @property
    def tests(self) -> List[TestDef]:
        if self.table.tests is None:
            return []
        else:
            return self.table.tests


@dataclass
class ParsedSourceMandatory(GraphNode, HasRelationMetadata):
    source_name: str
    source_description: str
    loader: str
    identifier: str
    resource_type: Literal[NodeType.Source]


@dataclass
class SourceDefinition(NodeInfoMixin, ParsedSourceMandatory):
    quoting: Quoting = field(default_factory=Quoting)
    loaded_at_field: Optional[str] = None
    freshness: Optional[FreshnessThreshold] = None
    external: Optional[ExternalTable] = None
    description: str = ""
    columns: Dict[str, ColumnInfo] = field(default_factory=dict)
    meta: Dict[str, Any] = field(default_factory=dict)
    source_meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)
    config: SourceConfig = field(default_factory=SourceConfig)
    patch_path: Optional[str] = None
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    relation_name: Optional[str] = None
    created_at: float = field(default_factory=lambda: time.time())

    def __post_serialize__(self, dct):
        if "_event_status" in dct:
            del dct["_event_status"]
        return dct

    def same_database_representation(self, other: "SourceDefinition") -> bool:
        return (
            self.database == other.database
            and self.schema == other.schema
            and self.identifier == other.identifier
            and True
        )

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


# ====================================
# Exposure node
# ====================================


@dataclass
class Exposure(GraphNode):
    type: ExposureType
    owner: Owner
    resource_type: Literal[NodeType.Exposure]
    description: str = ""
    label: Optional[str] = None
    maturity: Optional[MaturityType] = None
    meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)
    config: ExposureConfig = field(default_factory=ExposureConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    url: Optional[str] = None
    depends_on: DependsOn = field(default_factory=DependsOn)
    refs: List[RefArgs] = field(default_factory=list)
    sources: List[List[str]] = field(default_factory=list)
    metrics: List[List[str]] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def search_name(self):
        return self.name

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


# ====================================
# Metric node
# ====================================


@dataclass
class MetricInputMeasure(dbtClassMixin):
    name: str
    filter: Optional[WhereFilterIntersection] = None
    alias: Optional[str] = None
    join_to_timespine: bool = False
    fill_nulls_with: Optional[int] = None

    def measure_reference(self) -> MeasureReference:
        return MeasureReference(element_name=self.name)

    def post_aggregation_measure_reference(self) -> MeasureReference:
        return MeasureReference(element_name=self.alias or self.name)


@dataclass
class MetricTimeWindow(dbtClassMixin):
    count: int
    granularity: TimeGranularity


@dataclass
class MetricInput(dbtClassMixin):
    name: str
    filter: Optional[WhereFilterIntersection] = None
    alias: Optional[str] = None
    offset_window: Optional[MetricTimeWindow] = None
    offset_to_grain: Optional[TimeGranularity] = None

    def as_reference(self) -> DSIMetricReference:
        return DSIMetricReference(element_name=self.name)

    def post_aggregation_reference(self) -> DSIMetricReference:
        return DSIMetricReference(element_name=self.alias or self.name)


@dataclass
class ConversionTypeParams(dbtClassMixin):
    base_measure: MetricInputMeasure
    conversion_measure: MetricInputMeasure
    entity: str
    calculation: ConversionCalculationType = ConversionCalculationType.CONVERSION_RATE
    window: Optional[MetricTimeWindow] = None
    constant_properties: Optional[List[ConstantPropertyInput]] = None


@dataclass
class MetricTypeParams(dbtClassMixin):
    measure: Optional[MetricInputMeasure] = None
    input_measures: List[MetricInputMeasure] = field(default_factory=list)
    numerator: Optional[MetricInput] = None
    denominator: Optional[MetricInput] = None
    expr: Optional[str] = None
    window: Optional[MetricTimeWindow] = None
    grain_to_date: Optional[TimeGranularity] = None
    metrics: Optional[List[MetricInput]] = None
    conversion_type_params: Optional[ConversionTypeParams] = None


@dataclass
class MetricReference(dbtClassMixin, Replaceable):
    sql: Optional[Union[str, int]] = None
    unique_id: Optional[str] = None


@dataclass
class Metric(GraphNode):
    name: str
    description: str
    label: str
    type: MetricType
    type_params: MetricTypeParams
    filter: Optional[WhereFilterIntersection] = None
    metadata: Optional[SourceFileMetadata] = None
    resource_type: Literal[NodeType.Metric]
    meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)
    config: MetricConfig = field(default_factory=MetricConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    sources: List[List[str]] = field(default_factory=list)
    depends_on: DependsOn = field(default_factory=DependsOn)
    refs: List[RefArgs] = field(default_factory=list)
    metrics: List[List[str]] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())
    group: Optional[str] = None

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def search_name(self):
        return self.name

    @property
    def input_measures(self) -> List[MetricInputMeasure]:
        return self.type_params.input_measures

    @property
    def measure_references(self) -> List[MeasureReference]:
        return [x.measure_reference() for x in self.input_measures]

    @property
    def input_metrics(self) -> List[MetricInput]:
        return self.type_params.metrics or []

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


# ====================================
# Group node
# ====================================


@dataclass
class Group(BaseNode):
    name: str
    owner: Owner
    resource_type: Literal[NodeType.Group]


# ====================================
# SemanticModel and related classes
# ====================================


@dataclass
class NodeRelation(dbtClassMixin):
    alias: str
    schema_name: str  # TODO: Could this be called simply "schema" so we could reuse StateRelation?
    database: Optional[str] = None
    relation_name: Optional[str] = None


@dataclass
class SemanticModel(GraphNode):
    model: str
    node_relation: Optional[NodeRelation]
    description: Optional[str] = None
    label: Optional[str] = None
    defaults: Optional[Defaults] = None
    entities: Sequence[Entity] = field(default_factory=list)
    measures: Sequence[Measure] = field(default_factory=list)
    dimensions: Sequence[Dimension] = field(default_factory=list)
    metadata: Optional[SourceFileMetadata] = None
    depends_on: DependsOn = field(default_factory=DependsOn)
    refs: List[RefArgs] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())
    config: SemanticModelConfig = field(default_factory=SemanticModelConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    primary_entity: Optional[str] = None
    group: Optional[str] = None

    @property
    def entity_references(self) -> List[LinkableElementReference]:
        return [entity.reference for entity in self.entities]

    @property
    def dimension_references(self) -> List[LinkableElementReference]:
        return [dimension.reference for dimension in self.dimensions]

    @property
    def measure_references(self) -> List[MeasureReference]:
        return [measure.reference for measure in self.measures]

    @property
    def has_validity_dimensions(self) -> bool:
        return any([dim.validity_params is not None for dim in self.dimensions])

    @property
    def validity_start_dimension(self) -> Optional[Dimension]:
        validity_start_dims = [
            dim for dim in self.dimensions if dim.validity_params and dim.validity_params.is_start
        ]
        if not validity_start_dims:
            return None
        return validity_start_dims[0]

    @property
    def validity_end_dimension(self) -> Optional[Dimension]:
        validity_end_dims = [
            dim for dim in self.dimensions if dim.validity_params and dim.validity_params.is_end
        ]
        if not validity_end_dims:
            return None
        return validity_end_dims[0]

    @property
    def partitions(self) -> List[Dimension]:  # noqa: D
        return [dim for dim in self.dimensions or [] if dim.is_partition]

    @property
    def partition(self) -> Optional[Dimension]:
        partitions = self.partitions
        if not partitions:
            return None
        return partitions[0]

    @property
    def reference(self) -> SemanticModelReference:
        return SemanticModelReference(semantic_model_name=self.name)

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

    @property
    def depends_on_macros(self):
        return self.depends_on.macros

    def checked_agg_time_dimension_for_measure(
        self, measure_reference: MeasureReference
    ) -> TimeDimensionReference:
        measure: Optional[Measure] = None
        for measure in self.measures:
            if measure.reference == measure_reference:
                measure = measure

        assert (
            measure is not None
        ), f"No measure with name ({measure_reference.element_name}) in semantic_model with name ({self.name})"

        default_agg_time_dimension = (
            self.defaults.agg_time_dimension if self.defaults is not None else None
        )

        agg_time_dimension_name = measure.agg_time_dimension or default_agg_time_dimension
        assert agg_time_dimension_name is not None, (
            f"Aggregation time dimension for measure {measure.name} on semantic model {self.name} is not set! "
            "To fix this either specify a default `agg_time_dimension` for the semantic model or define an "
            "`agg_time_dimension` on the measure directly."
        )
        return TimeDimensionReference(element_name=agg_time_dimension_name)

    @property
    def primary_entity_reference(self) -> Optional[EntityReference]:
        return (
            EntityReference(element_name=self.primary_entity)
            if self.primary_entity is not None
            else None
        )

    def same_model(self, old: "SemanticModel") -> bool:
        return self.model == old.same_model

    def same_node_relation(self, old: "SemanticModel") -> bool:
        return self.node_relation == old.node_relation

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
            and self.same_node_relation(old)
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
# SavedQuery and related classes
# ====================================


@dataclass
class SavedQueryMandatory(GraphNode):
    query_params: QueryParams
    exports: List[Export]


@dataclass
class SavedQuery(NodeInfoMixin, SavedQueryMandatory):
    description: Optional[str] = None
    label: Optional[str] = None
    metadata: Optional[SourceFileMetadata] = None
    config: SavedQueryConfig = field(default_factory=SavedQueryConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    group: Optional[str] = None
    depends_on: DependsOn = field(default_factory=DependsOn)
    created_at: float = field(default_factory=lambda: time.time())
    refs: List[RefArgs] = field(default_factory=list)

    @property
    def metrics(self) -> List[str]:
        return self.query_params.metrics

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes

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
        for (old_export, new_export) in zip(old.exports, self.exports):
            if not new_export.same_contents(old_export):
                return False

        return True

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
            and True
        )


# ====================================
# Patches
# ====================================


@dataclass
class ParsedPatch(HasYamlMetadata, Replaceable):
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


@dataclass
class ParsedMacroPatch(ParsedPatch):
    arguments: List[MacroArgument] = field(default_factory=list)


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
    RPCNode,
    SqlNode,
    GenericTestNode,
    SnapshotNode,
]

# All SQL nodes plus SeedNode (csv files)
ManifestNode = Union[
    ManifestSQLNode,
    SeedNode,
]

ResultNode = Union[
    ManifestNode,
    SourceDefinition,
]

# All nodes that can be in the DAG
GraphMemberNode = Union[
    ResultNode,
    Exposure,
    Metric,
    SavedQuery,
    SemanticModel,
]

# All "nodes" (or node-like objects) in this file
Resource = Union[
    GraphMemberNode,
    Documentation,
    Macro,
    Group,
]

TestNode = Union[
    SingularTestNode,
    GenericTestNode,
]
