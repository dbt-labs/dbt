import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional, Union

from dbt.artifacts.resources.base import GraphResource
from dbt.artifacts.resources.types import NodeType
from dbt.artifacts.resources.v1.components import (
    ColumnInfo,
    FreshnessThreshold,
    HasRelationMetadata,
    Quoting,
)
from dbt.artifacts.resources.v1.config import BaseConfig
from dbt_common.contracts.config.properties import AdditionalPropertiesAllowed
from dbt_common.contracts.util import Mergeable
from dbt_common.exceptions import CompilationError


@dataclass
class SourceConfig(BaseConfig):
    enabled: bool = True
    event_time: Any = None


@dataclass
class ExternalPartition(AdditionalPropertiesAllowed):
    name: str = ""
    description: str = ""
    data_type: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)

    def __post_init__(self):
        if self.name == "" or self.data_type == "":
            raise CompilationError("External partition columns must have names and data types")


@dataclass
class ExternalTable(AdditionalPropertiesAllowed, Mergeable):
    location: Optional[str] = None
    file_format: Optional[str] = None
    row_format: Optional[str] = None
    tbl_properties: Optional[str] = None
    partitions: Optional[Union[List[str], List[ExternalPartition]]] = None

    def __bool__(self):
        return self.location is not None


@dataclass
class ParsedSourceMandatory(GraphResource, HasRelationMetadata):
    source_name: str
    source_description: str
    loader: str
    identifier: str
    resource_type: Literal[NodeType.Source]


@dataclass
class SourceDefinition(ParsedSourceMandatory):
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
    vars: Dict[str, Any] = field(default_factory=dict)
    relation_name: Optional[str] = None
    created_at: float = field(default_factory=lambda: time.time())
    unrendered_database: Optional[str] = None
    unrendered_schema: Optional[str] = None
