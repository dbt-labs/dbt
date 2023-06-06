from dataclasses import dataclass, field
from typing import Optional, Set, Any

from dbt.adapters.relation_configs import (
    MaterializedViewConfig,
    MaterializedViewConfigChangeCollection,
    RelationResults,
    ValidationRule,
)
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import DbtRuntimeError

from dbt.adapters.postgres.relation_configs.index import PostgresIndexConfig, PostgresIndexChange


@dataclass(frozen=True)
class PostgresMaterializedViewConfig(MaterializedViewConfig):
    """
    This config follows the specs found here:
    https://www.postgresql.org/docs/current/sql-creatematerializedview.html

    The following parameters are configurable by dbt:
    - table_name: name of the materialized view
    - query: the query that defines the view
    - indexes: the collection (set) of indexes on the materialized view

    Applicable defaults for non-configurable parameters:
    - method: `heap`
    - tablespace_name: `default_tablespace`
    - with_data: `True`
    """

    table_name: Optional[str] = None
    query: Optional[str] = None
    indexes: Set[PostgresIndexConfig] = field(default_factory=set)

    def validation_rules(self) -> Set[ValidationRule]:
        # index rules get run by default
        return {
            (
                self.table_name is None or len(self.table_name) <= 63,
                DbtRuntimeError(
                    f"The materialized view name is more than 63 characters: {self.table_name}"
                ),
            ),
        }

    @classmethod
    def from_dict(cls, kwargs_dict) -> "PostgresMaterializedViewConfig":
        config_dict = {
            "table_name": kwargs_dict.get("table_name"),
            "query": kwargs_dict.get("query"),
            "indexes": {
                PostgresIndexConfig.from_dict(index) for index in kwargs_dict.get("indexes", {})
            },
        }
        materialized_view: "PostgresMaterializedViewConfig" = super().from_dict(config_dict)  # type: ignore
        return materialized_view

    @classmethod
    def parse_model_node(cls, model_node: ModelNode) -> dict:
        """
        Postgres-specific implementation of `RelationConfig.from_model_node()` for materialized views
        """
        kwargs = {
            "table_name": model_node.identifier,
            "query": model_node.compiled_code,
        }

        # create index objects for each index found in the config
        if indexes := model_node.config.extra.get("indexes"):
            index_configs = [PostgresIndexConfig.parse_model_node(index) for index in indexes]
            kwargs.update({"indexes": index_configs})

        return kwargs

    @classmethod
    def parse_relation_results(cls, relation_results: RelationResults) -> dict:
        """
        Postgres-specific implementation of `RelationConfig.from_relation_results()` for materialized views
        """
        base_config = relation_results.get("base", {})
        kwargs = {
            "table_name": base_config.get("table_name"),
            "query": base_config.get("query"),
        }

        # create index objects for each index found in the config
        if indexes := relation_results.get("indexes"):
            index_configs = [
                PostgresIndexConfig.parse_relation_results({"base": index})
                for index in indexes.rows
            ]
            kwargs.update({"indexes": index_configs})

        return kwargs

    def __hash__(self) -> int:
        return hash(
            (
                self.table_name,
                self.query,
                self.indexes,
            )
        )

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, PostgresMaterializedViewConfig):
            return all(
                {
                    self.table_name == other.table_name,
                    self.query == other.query,
                    self.indexes == other.indexes,
                }
            )
        return False


@dataclass(frozen=True)
class PostgresMaterializedViewConfigChangeCollection(MaterializedViewConfigChangeCollection):
    indexes: Optional[Set[PostgresIndexChange]] = None
