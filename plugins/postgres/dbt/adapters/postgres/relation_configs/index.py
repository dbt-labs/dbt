from dataclasses import dataclass, field
from enum import Enum
from typing import Optional, Set, Dict, Union, List, Tuple, Any

from dbt.exceptions import DbtRuntimeError

from dbt.adapters.relation_configs import (
    RelationConfig,
    RelationConfigChangeAction,
    RelationConfigChange,
    RelationResults,
    ValidationRule,
)


# A `NodeConfig` instance can have multiple indexes, this is just one index
# e.g. {"columns": ["column_a", "column_b"], "unique": True, "type": "hash"}
Columns = List[str]
ModelNodeEntry = Dict[str, Union[Columns, bool, str]]


class PostgresIndexMethod(str, Enum):
    btree = "btree"
    hash = "hash"
    gist = "gist"
    spgist = "spgist"
    gin = "gin"
    brin = "brin"

    @classmethod
    def default(cls) -> "PostgresIndexMethod":
        return cls.btree


@dataclass(frozen=True)
class PostgresIndexConfig(RelationConfig):
    """
    This config fallows the specs found here:
    https://www.postgresql.org/docs/current/sql-createindex.html

    The following parameters are configurable by dbt:
    - name: the name of the index in the database, this isn't predictable since we apply a timestamp
    - unique: checks for duplicate values when the index is created and on data updates
    - method: the index method to be used
    - column_names: the columns in the index

    Applicable defaults for non-configurable parameters:
    - concurrently: `False`
    - nulls_distinct: `True`
    """

    name: Optional[str] = None
    column_names: Optional[Set[str]] = field(default_factory=set)
    unique: Optional[bool] = False
    method: Optional[PostgresIndexMethod] = PostgresIndexMethod.btree

    def validation_rules(self) -> Set[ValidationRule]:
        return {
            (
                self.column_names is not None,
                DbtRuntimeError("Indexes require at least one column, but none were provided"),
            ),
        }

    @classmethod
    def parse_model_node(cls, model_node_entry: ModelNodeEntry) -> dict:
        kwargs = {
            "unique": model_node_entry.get("unique"),
            "method": model_node_entry.get("type"),
        }

        if column_names := model_node_entry.get("columns", []):
            # TODO: include the QuotePolicy instead of defaulting to lower()
            kwargs.update({"column_names": set(column.lower() for column in column_names)})

        return kwargs

    @classmethod
    def parse_relation_results(cls, relation_results: RelationResults) -> dict:
        index = relation_results.get("base", {})
        index_config = {
            "name": index.get("name"),
            # we shouldn't have to adjust the values from the database for the QuotePolicy
            "column_names": set(index.get("column_names", "").split(",")),
            "unique": index.get("unique"),
            "method": index.get("method"),
        }
        return index_config

    def as_user_config(self):
        """
        Returns: a dictionary that can be passed into `get_create_index_sql()`
        """
        config = {
            "columns": list(self.column_names),
            "unique": self.unique,
            "type": self.method.value,
        }
        return config

    def __hash__(self) -> int:
        return hash(
            # don't include the name for hashing since we generate a time-specific name for indexes in Postgres
            (
                frozenset(self.column_names),
                self.unique,
                self.method,
            )
        )

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, PostgresIndexConfig):
            # don't include the name for equality since we generate a time-specific name for indexes in Postgres
            return all(
                {
                    self.column_names == other.column_names,
                    self.unique == other.unique,
                    self.method == other.method,
                }
            )
        return False


@dataclass(frozen=True)
class PostgresIndexChange(RelationConfigChange):
    """
    Example of an index change:
    {
        "action": "create",
        "context": {
            "name": "",  # we don't know the name since it gets created as a hash at runtime
            "columns": ["column_1", "column_3"],
            "type": "hash",
            "unique": True
        }
    },
    {
        "action": "drop",
        "context": {
            "name": "index_abc",  # we only need this to drop, but we need the rest to compare
            "columns": ["column_1"],
            "type": "btree",
            "unique": True
        }
    }
    """

    context: PostgresIndexConfig = None

    def requires_full_refresh(self) -> bool:
        return False

    def validation_rules(self) -> Set[Union[Tuple[bool, DbtRuntimeError], bool]]:
        return {
            (
                self.action
                in {RelationConfigChangeAction.create, RelationConfigChangeAction.drop},
                DbtRuntimeError(
                    "Invalid operation, only `drop` and `create` changes are supported for indexes."
                ),
            ),
            (
                not (self.action == RelationConfigChangeAction.drop and self.context.name is None),
                DbtRuntimeError("Invalid operation, attempting to drop an index with no name."),
            ),
            (
                not (
                    self.action == RelationConfigChangeAction.create
                    and self.context.column_names == set()
                ),
                DbtRuntimeError(
                    "Invalid operations, attempting to create an index with no columns."
                ),
            ),
        }
