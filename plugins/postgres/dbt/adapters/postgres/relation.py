from dataclasses import dataclass
from typing import Optional, Set

from dbt.adapters.base.relation import BaseRelation
from dbt.adapters.relation_configs import (
    RelationConfigChangeAction,
    RelationResults,
)
from dbt.context.providers import RuntimeConfigObject
from dbt.exceptions import DbtRuntimeError

from dbt.adapters.postgres.relation_configs import (
    PostgresIndexConfig,
    PostgresIndexChange,
    PostgresMaterializedViewConfig,
    PostgresMaterializedViewConfigChangeCollection,
)


@dataclass(frozen=True, eq=False, repr=False)
class PostgresRelation(BaseRelation):
    def __post_init__(self):
        # Check for length of Postgres table/view names.
        # Check self.type to exclude test relation identifiers
        if (
            self.identifier is not None
            and self.type is not None
            and len(self.identifier) > self.relation_max_name_length()
        ):
            raise DbtRuntimeError(
                f"Relation name '{self.identifier}' "
                f"is longer than {self.relation_max_name_length()} characters"
            )

    def relation_max_name_length(self):
        return 63

    def get_materialized_view_config_change_collection(
        self, relation_results: RelationResults, runtime_config: RuntimeConfigObject
    ) -> Optional[PostgresMaterializedViewConfigChangeCollection]:
        """
        Postgres-specific implementation of `BaseRelation.get_materialized_view_config_changes`.

        The only tracked changes for materialized views are indexes.
        """
        config_change_collection_dict = {}

        existing_materialized_view_config = PostgresMaterializedViewConfig.parse_relation_results(
            relation_results
        )
        existing_materialized_view = PostgresMaterializedViewConfig.from_dict(
            existing_materialized_view_config
        )

        new_materialized_view_config = PostgresMaterializedViewConfig.parse_model_node(
            runtime_config.model
        )
        new_materialized_view = PostgresMaterializedViewConfig.from_dict(
            new_materialized_view_config
        )

        if index_config_changes := self._get_index_config_changes(
            existing_materialized_view.indexes, new_materialized_view.indexes
        ):
            config_change_collection_dict.update({"indexes": index_config_changes})

        if config_change_collection_dict:
            return PostgresMaterializedViewConfigChangeCollection(**config_change_collection_dict)

    def _get_index_config_changes(
        self, existing_indexes: Set[PostgresIndexConfig], new_indexes: Set[PostgresIndexConfig]
    ) -> Set[PostgresIndexChange]:
        """
        Get the index updates that will occur as a result of a new run

        There are four scenarios:

        1. Indexes are equal -> don't return these
        2. Index is new -> create these
        3. Index is old -> drop these
        4. Indexes are not equal -> drop old, create new -> two actions

        Returns: a set of index updates in the form {"action": "drop/create", "context": <IndexConfig>}
        """
        drop_changes = set(
            PostgresIndexChange.from_dict(
                {"action": RelationConfigChangeAction.drop, "context": index}
            )
            for index in existing_indexes.difference(new_indexes)
        )
        create_changes = set(
            PostgresIndexChange.from_dict(
                {"action": RelationConfigChangeAction.create, "context": index}
            )
            for index in new_indexes.difference(existing_indexes)
        )
        return drop_changes.union(create_changes)
