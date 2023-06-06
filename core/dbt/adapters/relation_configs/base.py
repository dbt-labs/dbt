from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Union, Dict, Set, Tuple, Hashable, Any, Optional

import agate
from dbt.contracts.graph.nodes import ModelNode
from dbt.contracts.relation import RelationType
from dbt.dataclass_schema import StrEnum
from dbt.exceptions import DbtRuntimeError
from dbt.utils import filter_null_values


"""
This is what relation metadata from the database looks like. It's a dictionary because there will be
multiple grains of data for a single object. For example, a materialized view in Postgres has base level information,
like name. But it also can have multiple indexes, which needs to be a separate query. It might look like this:

{
    "base": agate.Row({"table_name": "table_abc", "query": "select * from table_def"})
    "indexes": agate.Table("rows": [
        agate.Row({"name": "index_a", "columns": ["column_a"], "type": "hash", "unique": False}),
        agate.Row({"name": "index_b", "columns": ["time_dim_a"], "type": "btree", "unique": False}),
    ])
}
"""
RelationResults = Dict[str, Union[agate.Row, agate.Table]]


# a more specific error is optional, but encouraged
ValidationCheck = bool
ValidationRule = Union[Tuple[ValidationCheck, DbtRuntimeError], ValidationCheck]


@dataclass(frozen=True)
class RelationConfigBase(Hashable, ABC):
    relation_type: Optional[RelationType] = None

    def __post_init__(self):
        self.run_validation_rules()

    def validation_rules(self) -> Set[ValidationRule]:
        """
        A set of validation rules to run against the object upon creation.

        A validation rule is a combination of a validation check (bool) and an optional error message.

        This defaults to no validation rules if not implemented. It's recommended to override this with values,
        but that may not always be necessary.

        Returns: a set of validation rules
        """
        return set()

    def run_validation_rules(self):
        for validation_rule in self.validation_rules():
            validation_check, error = self._parse_validation_rule(validation_rule)

            try:
                assert validation_check
            except AssertionError:
                raise error

        self.run_child_validation_rules()

    def run_child_validation_rules(self):
        for attr_value in vars(self).values():
            if isinstance(attr_value, RelationConfigBase):
                attr_value.run_validation_rules()
            if isinstance(attr_value, set):
                for member in attr_value:
                    if isinstance(member, RelationConfigBase):
                        member.run_validation_rules()

    def _parse_validation_rule(
        self, validation_rule: ValidationRule
    ) -> Tuple[ValidationCheck, DbtRuntimeError]:
        default_error = DbtRuntimeError(
            f"There was a validation error in preparing this relation: {self.relation_type}."
            "No additional context was provided by this adapter."
        )
        if isinstance(validation_rule, tuple):
            return validation_rule
        elif isinstance(validation_rule, bool):
            return validation_rule, default_error
        else:
            raise DbtRuntimeError(f"Invalid validation rule format: {validation_rule}")

    @classmethod
    def from_dict(cls, kwargs_dict) -> "RelationConfigBase":
        """
        This assumes the subclass of `RelationConfigBase` is flat, in the sense that no attribute is
        itself another subclass of `RelationConfigBase`. If that's not the case, this should be overriden
        to manually manage that complexity. This can be automated in the future with something like
        `mashumaro` or `pydantic`.

        Args:
            kwargs_dict: the dict representation of this instance

        Returns: the `RelationConfigBase` representation associated with the provided dict
        """
        return cls(**filter_null_values(kwargs_dict))

    @abstractmethod
    def __hash__(self) -> int:
        raise self._not_implemented_error()

    @abstractmethod
    def __eq__(self, other) -> bool:
        raise self._not_implemented_error()

    @classmethod
    def _not_implemented_error(cls):
        return NotImplementedError(
            f"The relation type {cls.relation_type} has not been configured for this adapter."
        )


@dataclass(frozen=True)
class RelationConfig(RelationConfigBase, ABC):
    @classmethod
    @abstractmethod
    def parse_model_node(cls, model_node: ModelNode) -> dict:
        """
        Translates the description of this relation using jinja context variables into a dict representation
        of `RelationConfig`. This is generally used in conjunction with `RelationConfig.from_dict()`

        Args:
            model_node: the `ModelNode` instance that's in the `RuntimeConfigObject` in the jinja context

        Returns: a raw dictionary of kwargs that can be used to create a `RelationConfig` instance
        """
        raise cls._not_implemented_error()

    @classmethod
    @abstractmethod
    def parse_relation_results(cls, relation_results: RelationResults) -> dict:
        """
        Translates the description of this relation using data from the database into a dict representation
        of `RelationConfig`. This is generally used in conjunction with `RelationConfig.from_dict()`

        Args:
            relation_results: a dictionary of results from a "describe" macro. See `RelationResults`

        Returns: a raw dictionary of kwargs that can be used to create a `RelationConfig` instance
        """
        raise cls._not_implemented_error()


class RelationConfigChangeAction(StrEnum):
    alter = "alter"
    create = "create"
    drop = "drop"


@dataclass(frozen=True)
class RelationConfigChange(RelationConfigBase, ABC):
    action: Optional[RelationConfigChangeAction] = None
    context: Hashable = (
        None  # this is usually a RelationConfig, e.g. IndexConfig, but shouldn't be limited
    )

    @abstractmethod
    def requires_full_refresh(self) -> bool:
        raise self._not_implemented_error()

    def __hash__(self) -> int:
        return hash((self.action, self.context))

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, RelationConfigChange):
            return all({self.action == other.action, self.context == other.context})
        return False


@dataclass(frozen=True)
class RelationConfigChangeCollection(ABC):
    """
    Relation configuration changes should be registered on this class as a group, by defining a new attribute
    of type Set[RelationConfigChange]. For example:

    class PostgresIndexConfigChange(RelationConfigChange):
        action = RelationConfigChangeAction.drop
        context = PostgresIndexConfig

        @property
        def requires_full_refresh(self) -> bool:
            return False

    class PostgresMaterializedViewAutoRefreshConfigChange(RelationConfigChange):
        # this doesn't exist in Postgres, but assume it does for this example

        action = RelationConfigChangeAction.alter
        context = PostgresMaterializedView.auto_refresh

        @property
        def requires_full_refresh(self) -> bool:
            return True

    class PostgresMaterializedViewConfigChanges(RelationConfigChanges):
        auto_refresh: Set[PostgresMaterializedViewAutoRefreshConfigChange]
        indexes: Set[PostgresIndexConfigChange]
    """

    relation_type: Optional[RelationType] = None

    def config_change_groups(self) -> Set[str]:
        config_change_groups = set()
        for attr_name, attr_value in vars(self).items():
            if isinstance(attr_value, set) and all(
                isinstance(member, RelationConfigChange) for member in attr_value
            ):
                config_change_groups.add(attr_name)
        return config_change_groups

    def requires_full_refresh(self) -> bool:
        individual_config_change_requires_full_refresh = {
            config_change.requires_full_refresh()
            for config_change_group in self.config_change_groups()
            for config_change in getattr(self, config_change_group)
        }
        return any(individual_config_change_requires_full_refresh)
