from dataclasses import field, dataclass
from typing import Any, List, Optional, Dict, Union, Type

from dbt.artifacts.resources import (
    ExposureConfig,
    MetricConfig,
    SavedQueryConfig,
    SemanticModelConfig,
    NodeConfig,
    SeedConfig,
    TestConfig,
    SnapshotConfig,
)
from dbt_common.contracts.config.base import BaseConfig, MergeBehavior, CompareBehavior
from dbt_common.contracts.config.metadata import Metadata, ShowBehavior
from dbt_common.dataclass_schema import (
    dbtClassMixin,
)
from dbt.contracts.util import Replaceable, list_str
from dbt.node_types import NodeType


def metas(*metas: Metadata) -> Dict[str, Any]:
    existing: Dict[str, Any] = {}
    for m in metas:
        existing = m.meta(existing)
    return existing


def insensitive_patterns(*patterns: str):
    lowercased = []
    for pattern in patterns:
        lowercased.append("".join("[{}{}]".format(s.upper(), s.lower()) for s in pattern))
    return "^({})$".format("|".join(lowercased))


@dataclass
class Hook(dbtClassMixin, Replaceable):
    sql: str
    transaction: bool = True
    index: Optional[int] = None


@dataclass
class SourceConfig(BaseConfig):
    enabled: bool = True


@dataclass
class UnitTestNodeConfig(NodeConfig):
    expected_rows: List[Dict[str, Any]] = field(default_factory=list)


@dataclass
class EmptySnapshotConfig(NodeConfig):
    materialized: str = "snapshot"
    unique_key: Optional[str] = None  # override NodeConfig unique_key definition


@dataclass
class UnitTestConfig(BaseConfig):
    tags: Union[str, List[str]] = field(
        default_factory=list_str,
        metadata=metas(ShowBehavior.Hide, MergeBehavior.Append, CompareBehavior.Exclude),
    )
    meta: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )


RESOURCE_TYPES: Dict[NodeType, Type[BaseConfig]] = {
    NodeType.Metric: MetricConfig,
    NodeType.SemanticModel: SemanticModelConfig,
    NodeType.SavedQuery: SavedQueryConfig,
    NodeType.Exposure: ExposureConfig,
    NodeType.Source: SourceConfig,
    NodeType.Seed: SeedConfig,
    NodeType.Test: TestConfig,
    NodeType.Model: NodeConfig,
    NodeType.Snapshot: SnapshotConfig,
    NodeType.Unit: UnitTestConfig,
}


# base resource types are like resource types, except nothing has mandatory
# configs.
BASE_RESOURCE_TYPES: Dict[NodeType, Type[BaseConfig]] = RESOURCE_TYPES.copy()
BASE_RESOURCE_TYPES.update({NodeType.Snapshot: EmptySnapshotConfig})


def get_config_for(resource_type: NodeType, base=False) -> Type[BaseConfig]:
    if base:
        lookup = BASE_RESOURCE_TYPES
    else:
        lookup = RESOURCE_TYPES
    return lookup.get(resource_type, NodeConfig)
