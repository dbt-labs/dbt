from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional, Union

from metricflow_semantic_interfaces.type_enums.export_destination_type import (
    ExportDestinationType,
)

from dbt.artifacts.resources.base import GraphResource
from dbt.artifacts.resources.types import NodeType
from dbt.artifacts.resources.v1.components import DependsOn, RefArgs
from dbt.artifacts.resources.v1.config import list_str, metas
from dbt.artifacts.resources.v1.semantic_layer_components import (
    SourceFileMetadata,
    WhereFilterIntersection,
)
from dbt_common.contracts.config.base import BaseConfig, CompareBehavior, MergeBehavior
from dbt_common.contracts.config.metadata import ShowBehavior
from dbt_common.dataclass_schema import dbtClassMixin


@dataclass
class ExportConfig(dbtClassMixin):
    """Nested configuration attributes for exports."""

    export_as: ExportDestinationType
    schema_name: Optional[str] = None
    alias: Optional[str] = None
    database: Optional[str] = None


@dataclass
class Export(dbtClassMixin):
    """Configuration for writing query results to a table."""

    name: str
    config: ExportConfig
    unrendered_config: Dict[str, str] = field(default_factory=dict)


@dataclass
class QueryParams(dbtClassMixin):
    """The query parameters for the saved query"""

    metrics: List[str]
    group_by: List[str]
    where: Optional[WhereFilterIntersection]
    order_by: List[str] = field(default_factory=list)
    limit: Optional[int] = None


@dataclass
class SavedQueryCache(dbtClassMixin):
    enabled: bool = False


@dataclass
class SavedQueryConfig(BaseConfig):
    """Where config options for SavedQueries are stored.

    This class is much like many other node config classes. It's likely that
    this class will expand in the direction of what's in the `NodeAndTestConfig`
    class. It might make sense to clean the various *Config classes into one at
    some point.
    """

    enabled: bool = True
    group: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )
    meta: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )
    export_as: Optional[ExportDestinationType] = None
    schema: Optional[str] = None
    database: Optional[str] = None
    cache: SavedQueryCache = field(default_factory=SavedQueryCache)


@dataclass
class SavedQueryMandatory(GraphResource):
    query_params: QueryParams
    exports: List[Export]


@dataclass
class SavedQuery(SavedQueryMandatory):
    resource_type: Literal[NodeType.SavedQuery]
    description: Optional[str] = None
    label: Optional[str] = None
    metadata: Optional[SourceFileMetadata] = None
    config: SavedQueryConfig = field(default_factory=SavedQueryConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    group: Optional[str] = None
    depends_on: DependsOn = field(default_factory=DependsOn)
    created_at: float = field(default_factory=lambda: time.time())
    refs: List[RefArgs] = field(default_factory=list)
    tags: Union[str, List[str]] = field(
        default_factory=list_str,
        metadata=metas(ShowBehavior.Hide, MergeBehavior.Append, CompareBehavior.Exclude),
    )

    @property
    def metrics(self) -> List[str]:
        return self.query_params.metrics

    @property
    def depends_on_nodes(self):
        return self.depends_on.nodes
