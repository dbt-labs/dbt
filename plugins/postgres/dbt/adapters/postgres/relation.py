from dataclasses import dataclass
from typing import Optional, Set, FrozenSet

from dbt.adapters.base.relation import BaseRelation
from dbt.adapters.relation_configs import (
    RelationConfigChangeAction,
    RelationResults,
)
from dbt.context.providers import RuntimeConfigObject
from dbt.contracts.relation import RelationType
from dbt.exceptions import DbtRuntimeError

from dbt.adapters.postgres.relation_configs import (
    PostgresIndexConfig,
    PostgresIndexConfigChange,
    PostgresMaterializedViewConfig,
    PostgresMaterializedViewConfigChangeCollection,
    MAX_CHARACTERS_IN_IDENTIFIER,
)


@dataclass(frozen=True, eq=False, repr=False)
class PostgresRelation(BaseRelation):
    renameable_relations = frozenset(
        {
            RelationType.View,
            RelationType.Table,
            RelationType.MaterializedView,
        }
    )
    replaceable_relations = frozenset(
        {
            RelationType.View,
            RelationType.Table,
        }
    )

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
        return MAX_CHARACTERS_IN_IDENTIFIER

    def get_materialized_view_config_change_collection(
        self, relation_results: RelationResults, runtime_config: RuntimeConfigObject
    ) -> Optional[PostgresMaterializedViewConfigChangeCollection]:
        config_change_collection = PostgresMaterializedViewConfigChangeCollection()

        existing_materialized_view = PostgresMaterializedViewConfig.from_relation_results(
            relation_results
        )
        new_materialized_view = PostgresMaterializedViewConfig.from_node(runtime_config.model)

        config_change_collection.indexes = self._get_index_config_changes(
            existing_materialized_view.indexes, new_materialized_view.indexes
        )

        # we return `None` instead of an empty `PostgresMaterializedViewConfigChangeCollection` object
        # so that it's easier and more extensible to check in the materialization:
        # `core/../materializations/materialized_view.sql` :
        #     {% if configuration_changes is none %}
        if config_change_collection.has_changes:
            return config_change_collection

    def _get_index_config_changes(
        self,
        existing_indexes: FrozenSet[PostgresIndexConfig],
        new_indexes: FrozenSet[PostgresIndexConfig],
    ) -> Set[PostgresIndexConfigChange]:
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
            PostgresIndexConfigChange.from_dict(
                {"action": RelationConfigChangeAction.drop, "context": index}
            )
            for index in existing_indexes.difference(new_indexes)
        )
        create_changes = set(
            PostgresIndexConfigChange.from_dict(
                {"action": RelationConfigChangeAction.create, "context": index}
            )
            for index in new_indexes.difference(existing_indexes)
        )
        return set().union(drop_changes, create_changes)
