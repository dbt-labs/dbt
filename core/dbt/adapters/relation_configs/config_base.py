from dataclasses import dataclass
from typing import Any, Dict, Union
from typing_extensions import Self

import agate
from dbt.contracts.graph.nodes import ModelNode, ParsedNode
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


@dataclass(frozen=True)
class RelationConfigBase:
    @classmethod
    def from_dict(cls, kwargs_dict: Dict[str, Any]) -> Self:
        """
        This assumes the subclass of `RelationConfigBase` is flat, in the sense that no attribute is
        itself another subclass of `RelationConfigBase`. If that's not the case, this should be overriden
        to manually manage that complexity.

        Args:
            kwargs_dict: the dict representation of this instance

        Returns: the `RelationConfigBase` representation associated with the provided dict
        """
        return cls(**filter_null_values(kwargs_dict))  # type: ignore

    ###
    # Parser for internal nodes, from dbt
    ###

    @classmethod
    def from_node(cls, node: ParsedNode) -> Self:
        config_dict = cls.parse_node(node)
        return cls.from_dict(config_dict)

    @classmethod
    def from_model_node(cls, model_node: ModelNode) -> Self:
        # this method is being deprecated in favor of the more generic `from_node`
        return cls.from_node(model_node)

    @classmethod
    def parse_node(cls, node: ParsedNode) -> Dict[str, Any]:
        # this method was originally implemented as `parse_model_node`
        if hasattr(cls, "parse_model_node"):
            return cls.parse_model_node(node)
        return {}

    ###
    # Parser for database results, generally used with `SQLAdapter`
    ###

    @classmethod
    def from_relation_results(cls, relation_results: RelationResults) -> Self:
        config_dict = cls.parse_relation_results(relation_results)
        return cls.from_dict(config_dict)

    @classmethod
    def parse_relation_results(cls, relation_results: RelationResults) -> Dict[str, Any]:
        raise NotImplementedError(
            "`parse_relation_results` has not been implemented for this relation_type."
        )

    ###
    # Parser for api results, generally used with `BaseAdapter`
    ###

    @classmethod
    def from_api_results(cls, api_results: Any) -> Self:
        config_dict = cls.parse_api_results(api_results)
        return cls.from_dict(config_dict)

    @classmethod
    def parse_api_results(cls, api_results: Any) -> Dict[str, Any]:
        raise NotImplementedError(
            "`parse_api_results` has not been implemented for this relation_type."
        )
