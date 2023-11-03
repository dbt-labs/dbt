from typing import Dict, Type

from dbt.contracts.graph.nodes import ParsedNode
from dbt.contracts.relation import RelationType
from dbt.dataclass_schema import StrEnum
from dbt.exceptions import DbtRuntimeError

from dbt.adapters.relation_configs.config_base import RelationConfigBase
from dbt.adapters.relation_configs._materialized_view import MaterializedViewRelationConfig


class RelationConfigFactory:
    """
    This provides a way to work with relation configs both in the adapter and in the jinja context.

    This factory comes with a default set of settings which can be overridden in BaseAdapter.

    Args:
        relation_types: an enum that contains all possible relation types for this adapter
            this is generally `RelationType`, but there are cases where an adapter may override
            `RelationType` to include more options or exclude options
        relation_configs: a map from a relation_type to a relation_config
            this is generally only overridden if `relation_types` is also overridden
    """

    def __init__(self, **kwargs):
        # the `StrEnum` class will generally be `RelationType`, however this allows for extending that Enum
        self.relation_types: Type[StrEnum] = kwargs.get("relation_types", RelationType)
        self.relation_configs: Dict[StrEnum, Type[RelationConfigBase]] = kwargs.get(
            "relation_configs",
            {
                RelationType.MaterializedView: MaterializedViewRelationConfig,
            },
        )

        try:
            for relation_type in self.relation_configs.keys():
                self.relation_types(relation_type)
        except ValueError:
            raise DbtRuntimeError(
                f"Received relation configs for {relation_type} "  # noqa
                f"but these relation types are not registered on this factory.\n"
                f"    registered relation types: {', '.join(self.relation_types)}\n"
            )

    def make_from_node(self, node: ParsedNode) -> RelationConfigBase:
        relation_type = self.relation_types(node.config.materialized)
        relation_config = self._relation_config(relation_type)
        return relation_config.from_node(node)

    def _relation_config(self, relation_type: StrEnum) -> Type[RelationConfigBase]:
        if relation := self.relation_configs.get(relation_type):
            return relation
        raise DbtRuntimeError(
            f"This factory does not have a relation config for this type.\n"
            f"    received: {relation_type}\n"
            f"    options: {', '.join(t for t in self.relation_configs.keys())}\n"
        )
