import time
from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any, Dict, List, Optional, Union

from dbt.artifacts.resources.base import Docs, FileHash, GraphResource
from dbt.artifacts.resources.types import NodeType, TimePeriod
from dbt.artifacts.resources.v1.config import NodeConfig
from dbt_common.contracts.config.properties import AdditionalPropertiesMixin
from dbt_common.contracts.constraints import ColumnLevelConstraint
from dbt_common.contracts.util import Mergeable
from dbt_common.dataclass_schema import ExtensibleDbtClassMixin, dbtClassMixin
from dbt_semantic_interfaces.type_enums import TimeGranularity

NodeVersion = Union[str, float]


@dataclass
class MacroDependsOn(dbtClassMixin):
    macros: List[str] = field(default_factory=list)

    # 'in' on lists is O(n) so this is O(n^2) for # of macros
    def add_macro(self, value: str):
        if value not in self.macros:
            self.macros.append(value)


@dataclass
class DependsOn(MacroDependsOn):
    nodes: List[str] = field(default_factory=list)

    def add_node(self, value: str):
        if value not in self.nodes:
            self.nodes.append(value)


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


@dataclass
class ColumnInfo(AdditionalPropertiesMixin, ExtensibleDbtClassMixin):
    """Used in all ManifestNodes and SourceDefinition"""

    name: str
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    data_type: Optional[str] = None
    constraints: List[ColumnLevelConstraint] = field(default_factory=list)
    quote: Optional[bool] = None
    tags: List[str] = field(default_factory=list)
    _extra: Dict[str, Any] = field(default_factory=dict)
    granularity: Optional[TimeGranularity] = None


@dataclass
class InjectedCTE(dbtClassMixin):
    """Used in CompiledNodes as part of ephemeral model processing"""

    id: str
    sql: str


@dataclass
class Contract(dbtClassMixin):
    enforced: bool = False
    alias_types: bool = True
    checksum: Optional[str] = None


@dataclass
class Quoting(dbtClassMixin, Mergeable):
    database: Optional[bool] = None
    schema: Optional[bool] = None
    identifier: Optional[bool] = None
    column: Optional[bool] = None


@dataclass
class Time(dbtClassMixin, Mergeable):
    count: Optional[int] = None
    period: Optional[TimePeriod] = None

    def exceeded(self, actual_age: float) -> bool:
        if self.period is None or self.count is None:
            return False
        kwargs: Dict[str, int] = {self.period.plural(): self.count}
        difference = timedelta(**kwargs).total_seconds()
        return actual_age > difference

    def __bool__(self):
        return self.count is not None and self.period is not None


@dataclass
class FreshnessThreshold(dbtClassMixin, Mergeable):
    warn_after: Optional[Time] = field(default_factory=Time)
    error_after: Optional[Time] = field(default_factory=Time)
    filter: Optional[str] = None

    def status(self, age: float) -> "dbt.artifacts.schemas.results.FreshnessStatus":  # type: ignore # noqa F821
        from dbt.artifacts.schemas.results import FreshnessStatus

        if self.error_after and self.error_after.exceeded(age):
            return FreshnessStatus.Error
        elif self.warn_after and self.warn_after.exceeded(age):
            return FreshnessStatus.Warn
        else:
            return FreshnessStatus.Pass

    def __bool__(self):
        return bool(self.warn_after) or bool(self.error_after)


@dataclass
class HasRelationMetadata(dbtClassMixin):
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

    @property
    def quoting_dict(self) -> Dict[str, bool]:
        if hasattr(self, "quoting"):
            return self.quoting.to_dict(omit_none=True)
        else:
            return {}


@dataclass
class DeferRelation(HasRelationMetadata):
    alias: str
    relation_name: Optional[str]
    # The rest of these fields match RelationConfig protocol exactly
    resource_type: NodeType
    name: str
    description: str
    compiled_code: Optional[str]
    meta: Dict[str, Any]
    tags: List[str]
    config: Optional[NodeConfig]

    @property
    def identifier(self):
        return self.alias


@dataclass
class ParsedResourceMandatory(GraphResource, HasRelationMetadata):
    alias: str
    checksum: FileHash
    config: NodeConfig = field(default_factory=NodeConfig)

    @property
    def identifier(self):
        return self.alias


@dataclass
class ParsedResource(ParsedResourceMandatory):
    tags: List[str] = field(default_factory=list)
    description: str = field(default="")
    columns: Dict[str, ColumnInfo] = field(default_factory=dict)
    meta: Dict[str, Any] = field(default_factory=dict)
    group: Optional[str] = None
    docs: Docs = field(default_factory=Docs)
    patch_path: Optional[str] = None
    build_path: Optional[str] = None
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    created_at: float = field(default_factory=lambda: time.time())
    config_call_dict: Dict[str, Any] = field(default_factory=dict)
    unrendered_config_call_dict: Dict[str, Any] = field(default_factory=dict)
    relation_name: Optional[str] = None
    raw_code: str = ""
    vars: Dict[str, Any] = field(default_factory=dict)

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if context and context.get("artifact") and "config_call_dict" in dct:
            del dct["config_call_dict"]
        if context and context.get("artifact") and "unrendered_config_call_dict" in dct:
            del dct["unrendered_config_call_dict"]
        return dct


@dataclass
class CompiledResource(ParsedResource):
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

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
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
