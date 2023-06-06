from abc import ABC
from dataclasses import dataclass

from dbt.adapters.relation_configs.base import (
    RelationConfig,
    RelationConfigChange,
    RelationConfigChangeCollection,
)
from dbt.contracts.relation import RelationType


@dataclass(frozen=True)
class MaterializedViewConfig(RelationConfig, ABC):
    relation_type = RelationType.MaterializedView


@dataclass(frozen=True)
class MaterializedViewConfigChange(RelationConfigChange, ABC):
    relation_type = RelationType.MaterializedView


@dataclass(frozen=True)
class MaterializedViewConfigChangeCollection(RelationConfigChangeCollection, ABC):
    relation_type = RelationType.MaterializedView
