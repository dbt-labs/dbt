from dataclasses import dataclass, field
from typing import Any, Dict, FrozenSet, List, Set

import agate
from dbt.adapters.relation_configs import (
    MaterializedViewRelationConfig,
    RelationResults,
    RelationConfigValidationMixin,
    RelationConfigValidationRule,
)
from dbt.contracts.graph.nodes import ParsedNode
from dbt.exceptions import DbtRuntimeError

from dbt.adapters.postgres.relation_configs.constants import MAX_CHARACTERS_IN_IDENTIFIER
from dbt.adapters.postgres.relation_configs.index import (
    PostgresIndexConfig,
    PostgresIndexConfigChange,
)


@dataclass(frozen=True, eq=True, unsafe_hash=True)
class PostgresMaterializedViewConfig(
    MaterializedViewRelationConfig, RelationConfigValidationMixin
):
    """
    This config follows the specs found here:
    https://www.postgresql.org/docs/current/sql-creatematerializedview.html

    The following parameters are configurable by dbt:
    - table_name: name of the materialized view
    - indexes: the collection (set) of indexes on the materialized view

    Applicable defaults for non-configurable parameters:
    - method: `heap`
    - tablespace_name: `default_tablespace`
    - with_data: `True`
    """

    table_name: str = ""
    indexes: FrozenSet[PostgresIndexConfig] = field(default_factory=frozenset)

    @property
    def validation_rules(self) -> Set[RelationConfigValidationRule]:
        # index rules get run by default with the mixin
        return {
            RelationConfigValidationRule(
                validation_check=self.table_name is None
                or len(self.table_name) <= MAX_CHARACTERS_IN_IDENTIFIER,
                validation_error=DbtRuntimeError(
                    f"The materialized view name is more than {MAX_CHARACTERS_IN_IDENTIFIER} "
                    f"characters: {self.table_name}"
                ),
            ),
        }

    @classmethod
    def from_dict(cls, config_dict: Dict[str, Any]) -> "PostgresMaterializedViewConfig":
        kwargs_dict = {
            "table_name": config_dict.get("table_name"),
            "indexes": frozenset(
                PostgresIndexConfig.from_dict(index) for index in config_dict.get("indexes", {})
            ),
        }
        materialized_view: "PostgresMaterializedViewConfig" = super().from_dict(kwargs_dict)  # type: ignore
        return materialized_view

    @classmethod
    def parse_node(cls, node: ParsedNode) -> Dict[str, Any]:
        indexes: List[dict] = node.config.extra.get("indexes", [])
        config_dict = {
            "table_name": node.identifier,
            "indexes": [PostgresIndexConfig.parse_node(index) for index in indexes],
        }
        return config_dict

    @classmethod
    def parse_relation_results(cls, relation_results: RelationResults) -> Dict[str, Any]:
        indexes: agate.Table = relation_results.get("indexes", agate.Table(rows={}))
        config_dict = {
            "indexes": [
                PostgresIndexConfig.parse_relation_results(index) for index in indexes.rows
            ],
        }
        return config_dict


@dataclass
class PostgresMaterializedViewConfigChangeCollection:
    indexes: Set[PostgresIndexConfigChange] = field(default_factory=set)

    @property
    def requires_full_refresh(self) -> bool:
        return any(index.requires_full_refresh for index in self.indexes)

    @property
    def has_changes(self) -> bool:
        return self.indexes != set()
